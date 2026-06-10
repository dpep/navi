//! MCP stdio transport: exposes navi's result-bearing commands as native MCP
//! tools, so an agent can call `locate`/`read`/`edit`/`move`/`remove`/`restore`
//! as tools instead of shelling out and re-parsing text.
//!
//! The command logic is transport-agnostic. This module only speaks JSON-RPC
//! 2.0 over line-delimited stdio (the MCP stdio framing — one message per line,
//! no embedded newlines), translates a `tools/call` into a `Command`, and runs
//! it through the same `crate::execute` path the CLI uses — so every MCP call
//! still feeds telemetry. The tool output is the exact response envelope.

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::cli::Command;

/// Echoed back to clients that don't pin a version. We mirror the client's
/// requested `protocolVersion` when it sends one (see `initialize_result`).
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Read JSON-RPC messages line by line from stdin and answer on stdout until
/// the stream closes. Requests get a response; notifications (no `id`) don't.
pub fn run() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

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
    }
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
        "capabilities": { "tools": {} },
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

    let cmd = build_command(name, args).map_err(|m| (-32602, m))?;
    let (env, ok) = crate::execute(&cmd);
    let text = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());

    Ok(json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": !ok,
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
        "move" => Command::Move(de(args)?),
        "remove" => Command::Remove(de(args)?),
        "restore" => Command::Restore(de(args)?),
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
                    "match": { "type": "string", "enum": ["text", "symbol", "file"], "default": "text", "description": "Match kind: content, symbol/definition, or filename." },
                    "lang": { "type": "string", "description": "Restrict to a language (e.g. rust, go, python)." },
                    "kind": { "type": "string", "description": "Restrict symbol kinds (rq): class, module, method, function." },
                    "fixed": { "type": "boolean", "default": false, "description": "Treat query as a literal string, not a regex." },
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
                    "range": { "type": "string", "description": "Line range for mode=range, e.g. 40:80." },
                    "symbol": { "type": "string", "description": "Symbol name for mode=symbol." },
                    "limit": { "type": "integer", "default": 400, "description": "Max lines / outline entries returned." }
                }
            }
        },
        {
            "name": "edit",
            "description": "Edit a file with a previewed, anchor-unique, hash-guarded change. Previews unless confirm=true.",
            "inputSchema": {
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string", "description": "File to edit." },
                    "anchor": { "type": "string", "description": "Unique anchor text to replace (use with replace)." },
                    "replace": { "type": "string", "description": "Replacement for the anchor." },
                    "range": { "type": "string", "description": "Line range A:B to replace (use with content)." },
                    "content": { "type": "string", "description": "Replacement content for the range." },
                    "base_hash": { "type": "string", "description": "Expected current content hash from a prior read; edit is rejected if the file changed." },
                    "confirm": { "type": "boolean", "default": false, "description": "Apply the edit. Without this, navi only previews the diff." }
                }
            }
        },
        {
            "name": "move",
            "description": "Move or rename a file. Previews unless confirm=true; refuses to clobber without force.",
            "inputSchema": {
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string", "description": "Source path." },
                    "to": { "type": "string", "description": "Destination path." },
                    "confirm": { "type": "boolean", "default": false, "description": "Apply the move. Without this, navi only previews." },
                    "force": { "type": "boolean", "default": false, "description": "Overwrite the destination if it exists." }
                }
            }
        },
        {
            "name": "remove",
            "description": "Remove files to the trash (restorable). Previews unless confirm=true; scope-guarded.",
            "inputSchema": {
                "type": "object",
                "required": ["paths"],
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Paths to remove." },
                    "confirm": { "type": "boolean", "default": false, "description": "Apply the removal. Without this, navi only previews." },
                    "force": { "type": "boolean", "default": false, "description": "Bypass the scope guard for large removals." },
                    "purge": { "type": "boolean", "default": false, "description": "Permanently delete instead of trashing (not restorable)." }
                }
            }
        },
        {
            "name": "restore",
            "description": "Reverse a journaled edit / move / remove by transaction id.",
            "inputSchema": {
                "type": "object",
                "required": ["txn"],
                "properties": {
                    "txn": { "type": "string", "description": "Transaction id reported by a prior edit / move / remove." }
                }
            }
        }
    ])
}
