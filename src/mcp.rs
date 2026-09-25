//! MCP stdio transport: exposes navi's result-bearing commands as native MCP
//! tools, so an agent can call `locate`/`read`/`edit`/`move`/`remove`/`undo`
//! (plus `miss`, the feedback signal) instead of shelling out and re-parsing
//! text. It also serves the undo history as a read-only MCP resource.
//!
//! The command logic is transport-agnostic. This module only speaks JSON-RPC
//! 2.0 over line-delimited stdio (the MCP stdio framing — one message per line,
//! no embedded newlines), translates a `tools/call` into a `Command`, and runs
//! it through the same `crate::execute` path the CLI uses — so every MCP call
//! still feeds telemetry. The tool output is the exact response envelope.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::cli::Command;

/// Echoed back to clients that don't pin a version. We mirror the client's
/// requested `protocolVersion` when it sends one (see `initialize_result`).
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The single resource navi serves: the list of reversible transactions.
const UNDO_HISTORY_URI: &str = "navi://undo-history";

/// Read JSON-RPC messages line by line from stdin and answer on stdout until
/// the stream closes. Requests get a response; notifications (no `id`) don't.
pub fn run() {
    let launched_from = launch_path().and_then(stamp_of);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    // The client is still holding the tool list of the build we replaced, and
    // it cannot know to re-ask: from where it sits, nothing happened.
    if std::env::var_os(RESTARTED).is_some() {
        write_msg(
            &mut out,
            &json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
        );
    }

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(req) => {
                if let Some(resp) = handle(&req) {
                    write_msg(&mut out, &resp);
                }
            }
            Err(_) => write_msg(&mut out, &error(Value::Null, -32700, "parse error")),
        }
        // **Here, and nowhere else.** This is the one thread that takes bytes
        // off stdin, so the safe moment to become another program is the one
        // where it holds none: after answering for what it read, before
        // reading again. Anything that arrives during the swap stays in the
        // pipe and the new build reads it.
        if let Some(newer) = superseded(&launched_from) {
            become_new_build(&newer);
        }
    }
}

/// Set across the `exec` so the replacement knows it replaced a build, which it
/// has no other way to tell — everything else about the two invocations is
/// identical.
const RESTARTED: &str = "NAVI_MCP_RESTARTED";

/// A binary's identity on disk.
///
/// **The inode is the load-bearing field.** An installer stages a new file and
/// renames it over the path, which always gives a new inode — but not always a
/// new mtime, because a clone on APFS carries the original's timestamps across.
/// Size and mtime sit beside it for a rewrite in place, which is not how
/// anything installs but is how a build script might.
#[derive(PartialEq)]
struct Stamp {
    path: PathBuf,
    inode: u64,
    len: u64,
    modified: SystemTime,
}

fn stamp_of(path: PathBuf) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(&path).ok()?;
    Some(Stamp {
        path,
        inode: meta.ino(),
        len: meta.len(),
        modified: meta.modified().ok()?,
    })
}

/// The path to watch: the name this process was launched *as*, rather than the
/// file that name resolves to right now.
///
/// `current_exe()` is the obvious source and is wrong on Linux, where it reads
/// `/proc/self/exe` and so resolves through the symlink. navi installs through
/// the tap, and `brew upgrade` moves the *link* — a new Cellar directory, the
/// old file left alone — so nothing the resolved path watched would move.
/// macOS reports the path as invoked. Reading argv[0] makes the two agree.
///
/// `stamp_of` still follows the link, which is the point: a rebuild and a
/// relink become one event, because either way the name comes to rest on a
/// different inode than the one being served.
fn launch_path() -> Option<PathBuf> {
    let argv0 = PathBuf::from(std::env::args_os().next()?);
    if argv0.as_os_str().is_empty() {
        return None;
    }
    if argv0.components().count() > 1 {
        return match argv0.is_absolute() {
            true => Some(argv0),
            false => Some(std::env::current_dir().ok()?.join(argv0)),
        };
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&argv0))
        .find(|candidate| candidate.is_file())
        .or_else(|| std::env::current_exe().ok())
}

/// The binary to become, if the one on disk is no longer the one running.
///
/// Both stamps must be readable. A path that has *become* unreadable is not an
/// upgrade — exec'ing it would fail on every request from then on — and a path
/// that was never readable cannot tell us anything changed.
fn superseded(launched_from: &Option<Stamp>) -> Option<PathBuf> {
    let was = launched_from.as_ref()?;
    let now = stamp_of(was.path.clone())?;
    match *was == now {
        true => None,
        false => Some(now.path),
    }
}

/// Replace this process with the build at `path`, keeping the pid and the file
/// descriptors so the client's pipes survive and it never learns the program on
/// the other end changed. Returns only on failure: a successful `exec` does not
/// come back, and a failed one leaves the old build serving, which is what it
/// was already doing.
fn become_new_build(path: &Path) {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new(path)
        .args(std::env::args_os().skip(1))
        .env(RESTARTED, "1")
        .exec();
}

/// Route one parsed message. Returns `None` for notifications, which the spec
/// says must not be answered.
fn handle(req: &Value) -> Option<Value> {
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Notifications carry no `id` and get no reply (e.g. notifications/initialized).
    let id = req.get("id").cloned()?;

    Some(match method {
        "initialize" => success(id, initialize_result(req)),
        "tools/list" => success(id, json!({ "tools": tool_specs() })),
        "tools/call" => match call_tool(req) {
            Ok(result) => success(id, result),
            Err((code, msg)) => error(id, code, &msg),
        },
        "resources/list" => success(id, json!({ "resources": resource_specs() })),
        "resources/read" => match read_resource(req) {
            Ok(result) => success(id, result),
            Err((code, msg)) => error(id, code, &msg),
        },
        "ping" => success(id, json!({})),
        _ => error(id, -32601, "method not found"),
    })
}

fn initialize_result(req: &Value) -> Value {
    let version = req
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        // `listChanged` is declared because it is true: this server can restart
        // into a build with a different tool list, and the notification it then
        // sends is only one a client may act on if the handshake said so.
        "capabilities": { "tools": { "listChanged": true }, "resources": {} },
        "serverInfo": { "name": "navi", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// Build the command, run it through the shared `execute` path, and return the
/// response envelope as the tool's text content. A navi-level failure is still
/// a successful tool call whose envelope carries the structured error, flagged
/// with `isError` so the agent can branch without parsing the text.
fn call_tool(req: &Value) -> std::result::Result<Value, (i64, String)> {
    let name = req
        .pointer("/params/name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let args = req
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // `miss` is a feedback signal, not a result-bearing command — it records
    // directly and never runs through execute/dispatch (which would panic on a
    // meta command, and whose stdout println would corrupt the JSON-RPC stream).
    if name == "miss" {
        return miss_tool(&args);
    }

    let cmd = build_command(name, args).map_err(|m| (-32602, m))?;
    let (env, ok) = crate::execute(&cmd);
    let text = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());

    Ok(json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": !ok,
    }))
}

/// Record a `miss` and return its acknowledgement. Mirrors the CLI's `miss`
/// shape without printing (stdout is the JSON-RPC channel here).
fn miss_tool(args: &Value) -> std::result::Result<Value, (i64, String)> {
    let note = args
        .get("note")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing 'note'".to_string()))?;
    let tool = args.get("tool").and_then(Value::as_str);
    crate::telemetry::log_miss(note, tool);
    let env =
        json!({ "tool": "miss", "ok": true, "recorded": true, "note": note, "ref_tool": tool });
    let text = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());
    Ok(json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": false,
    }))
}

/// The resource catalogue returned by `resources/list`.
fn resource_specs() -> Value {
    json!([
        {
            "uri": UNDO_HISTORY_URI,
            "name": "Undo history",
            "description": "Reversible navi transactions, newest first. Undo one with the undo tool and its txn.",
            "mimeType": "application/json",
        }
    ])
}

/// Serve a resource by URI. Only the undo history is published.
fn read_resource(req: &Value) -> std::result::Result<Value, (i64, String)> {
    let uri = req
        .pointer("/params/uri")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing resource uri".to_string()))?;
    if uri != UNDO_HISTORY_URI {
        return Err((-32602, format!("unknown resource: {uri}")));
    }
    let body = json!({ "transactions": crate::journal::history(50) });
    let text = serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".into());
    Ok(json!({
        "contents": [ {
            "uri": uri,
            "mimeType": "application/json",
            "text": text,
        } ],
    }))
}

/// Map a tool name + arguments object into a `Command`. Arguments deserialize
/// straight into the same `Args` structs the CLI parses, so defaults and field
/// names stay in one place.
fn build_command(name: &str, args: Value) -> std::result::Result<Command, String> {
    fn de<T: DeserializeOwned>(v: Value) -> std::result::Result<T, String> {
        serde_json::from_value(v).map_err(|e| format!("invalid arguments: {e}"))
    }
    Ok(match name {
        "locate" => Command::Locate(de(args)?),
        "read" => Command::Read(de(args)?),
        "edit" => Command::Edit(de(args)?),
        "info" => Command::Info(de(args)?),
        "move" => Command::Move(de(args)?),
        "remove" => Command::Remove(de(args)?),
        "undo" => Command::Undo(de(args)?),
        other => return Err(format!("unknown tool: {other}")),
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn write_msg(out: &mut impl Write, msg: &Value) {
    let s = serde_json::to_string(msg).unwrap_or_else(|_| "{}".into());
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}

/// The tool catalogue returned by `tools/list`. Schemas are hand-written to
/// mirror the clap surface in `cli.rs`; keep them in sync when args change.
/// Every tool returns the standard response envelope as its text content.
fn tool_specs() -> Value {
    json!([
        {
            "name": "locate",
            "description": "Find code by content, symbol, or filename. Returns ranked hits with snippets.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "What to find." },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Directories to search (default: current directory)." },
                    "match": { "type": "string", "enum": ["text", "symbol", "file", "references"], "default": "text", "description": "Match kind: content, symbol/definition, filename, or references (whole-word usages of an identifier; textual, not semantic)." },
                    "lang": { "type": "string", "description": "Restrict to a language (e.g. rust, go, python)." },
                    "kind": { "type": "string", "description": "Restrict symbol kinds (rq): class, module, method, function." },
                    "fixed": { "type": "boolean", "default": false, "description": "Treat query as a literal string, not a regex." },
                    "ignore_case": { "type": "boolean", "default": false, "description": "Match case-insensitively (content search)." },
                    "files_with_matches": { "type": "boolean", "default": false, "description": "Return only the paths of files containing a match, not each line (content search)." },
                    "limit": { "type": "integer", "default": 50, "description": "Max results." }
                }
            }
        },
        {
            "name": "read",
            "description": "Read a file by full / outline / range / symbol. Token-aware; returns a content hash for guarded edits.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File to read." },
                    "mode": { "type": "string", "enum": ["full", "outline", "range", "symbol"], "default": "full", "description": "How to read it." },
                    "range": { "type": "string", "description": "Line range for mode=range: A:B, A: (to end), :B (from start), or A." },
                    "symbol": { "type": "string", "description": "Symbol name for mode=symbol." },
                    "limit": { "type": "integer", "default": 400, "description": "Max lines / outline entries returned." }
                }
            }
        },
        {
            "name": "info",
            "description": "Orient in a repo: root, vcs/branch, languages, per-ecosystem build/test/lint commands, package.json scripts, Makefile targets, and which navi search backends are available here.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory to inspect (default: current directory)." }
                }
            }
        },
        {
            "name": "edit",
            "description": "Edit a file with an anchor-unique, hash-guarded change, or create it from content when the path doesn't exist. Set dry_run to preview the diff instead of applying.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File to edit, or to create (when it doesn't exist) from content." },
                    "anchor": { "type": "string", "description": "Unique anchor text to replace (use with replace)." },
                    "replace": { "type": "string", "description": "Replacement for the anchor." },
                    "range": { "type": "string", "description": "Line range to replace (use with content): A:B, A:, :B, or A." },
                    "content": { "type": "string", "description": "Replacement content for the range, or the body of a new file." },
                    "base_hash": { "type": "string", "description": "Expected current content hash from a prior read; edit is rejected if the file changed." },
                    "dry_run": { "type": "boolean", "default": false, "description": "Preview the diff without applying it." }
                }
            }
        },
        {
            "name": "move",
            "description": "Move or rename a file. Set dry_run to preview; refuses to clobber an existing destination without force.",
            "inputSchema": {
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string", "description": "Source path." },
                    "to": { "type": "string", "description": "Destination path." },
                    "dry_run": { "type": "boolean", "default": false, "description": "Preview the move without applying it." },
                    "force": { "type": "boolean", "default": false, "description": "Overwrite the destination if it exists." }
                }
            }
        },
        {
            "name": "remove",
            "description": "Remove files to the trash (restorable via undo). Set dry_run to preview; scope-guarded.",
            "inputSchema": {
                "type": "object",
                "required": ["paths"],
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths to remove." },
                    "dry_run": { "type": "boolean", "default": false, "description": "Preview the removal without applying it." },
                    "force": { "type": "boolean", "default": false, "description": "Bypass the scope guard for large removals." },
                    "purge": { "type": "boolean", "default": false, "description": "Permanently delete instead of trashing (not restorable)." }
                }
            }
        },
        {
            "name": "undo",
            "description": "Undo a journaled edit / move / remove by transaction id.",
            "inputSchema": {
                "type": "object",
                "required": ["txn"],
                "properties": {
                    "txn": { "type": "string", "description": "Transaction id reported by a prior edit / move / remove." }
                }
            }
        },
        {
            "name": "miss",
            "description": "Record that a navi result was unhelpful — the explicit feedback signal that feeds `navi report`. Use it when a tool returned the wrong thing, missed something, or fell short.",
            "inputSchema": {
                "type": "object",
                "required": ["note"],
                "properties": {
                    "note": { "type": "string", "description": "What went wrong or what you expected instead." },
                    "tool": { "type": "string", "description": "Which navi tool produced the unhelpful result (optional)." }
                }
            }
        }
    ])
}
