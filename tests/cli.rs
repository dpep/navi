//! End-to-end tests over the real binary and real backends. Each test isolates
//! navi's state via a temp NAVI_DATA_DIR so telemetry/journal don't leak.

use std::path::Path;

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::tempdir;

fn run(data: &Path, args: &[&str]) -> Value {
    let out = String::from_utf8(raw(data, args)).unwrap();
    serde_json::from_str(&out).unwrap_or_else(|_| panic!("non-JSON stdout: {out}"))
}

/// Raw stdout — for the meta commands (`report`, `miss`) that don't emit the
/// result envelope. Isolates both navi state and the trash under `data`, so
/// removals never touch the real `~/.Trash`.
fn raw(data: &Path, args: &[&str]) -> Vec<u8> {
    Command::cargo_bin("navi")
        .unwrap()
        .env("NAVI_DATA_DIR", data)
        .env("NAVI_TRASH_DIR", data.join("trash"))
        .args(args)
        .output()
        .unwrap()
        .stdout
}

fn txn_of(v: &Value) -> &str {
    v["result"]["transaction_id"]
        .as_str()
        .expect("transaction_id in result")
}

/// Drive `navi mcp`: feed each request as a JSON-RPC line on stdin, close it,
/// and parse the line-delimited responses. Isolates state like `raw`.
fn mcp(data: &Path, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let out = Command::cargo_bin("navi")
        .unwrap()
        .env("NAVI_DATA_DIR", data)
        .env("NAVI_TRASH_DIR", data.join("trash"))
        .arg("mcp")
        .write_stdin(input)
        .output()
        .unwrap()
        .stdout;
    String::from_utf8(out)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|_| panic!("non-JSON line: {l}")))
        .collect()
}

/// The response envelope a `tools/call` carries as its text content.
fn tool_envelope(resp: &Value) -> Value {
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(text).expect("envelope JSON")
}

#[test]
fn install_without_claude_prints_the_manual_command() {
    // PATH stripped to an empty dir so `claude` is absent: install must fail
    // gracefully and surface the manual registration command.
    let empty = tempdir().unwrap();
    let out = Command::cargo_bin("navi")
        .unwrap()
        .env("PATH", empty.path())
        .arg("install")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("claude mcp add --scope user navi -- navi mcp"));
}

#[test]
fn locate_text_finds_the_match() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    std::fs::write(work.path().join("f.txt"), "alpha\nbeta needle\ngamma\n").unwrap();

    let v = run(
        data.path(),
        &["locate", "needle", work.path().to_str().unwrap()],
    );
    assert_eq!(v["ok"], true);
    let hits = v["result"]["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["line"], 2);
}

#[test]
fn read_range_returns_the_slice_with_a_hash() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("f.txt");
    std::fs::write(&f, "one\ntwo\nthree\nfour\n").unwrap();

    let v = run(
        data.path(),
        &[
            "read",
            f.to_str().unwrap(),
            "--mode",
            "range",
            "--range",
            "2:3",
        ],
    );
    assert_eq!(v["result"]["text"], "two\nthree");
    assert_eq!(v["result"]["start_line"], 2);
    assert_eq!(v["result"]["end_line"], 3);
    assert!(v["result"]["content_hash"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
}

#[test]
fn read_outline_lists_definitions() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("m.rs");
    std::fs::write(&f, "fn a() {}\nlet x = 1;\nfn b() {}\n").unwrap();

    let v = run(
        data.path(),
        &["read", f.to_str().unwrap(), "--mode", "outline"],
    );
    let outline = v["result"]["outline"].as_array().unwrap();
    assert_eq!(outline.len(), 2);
    assert_eq!(outline[0]["line"], 1);
    assert_eq!(outline[1]["line"], 3);
}

#[test]
fn edit_previews_without_writing_then_applies_on_confirm() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("c.txt");
    std::fs::write(&f, "x = 1\n").unwrap();
    let path = f.to_str().unwrap();

    let preview = run(
        data.path(),
        &["edit", path, "--anchor", "x = 1", "--replace", "x = 2"],
    );
    assert_eq!(preview["result"]["applied"], false);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "x = 1\n");

    let applied = run(
        data.path(),
        &[
            "edit",
            path,
            "--anchor",
            "x = 1",
            "--replace",
            "x = 2",
            "--confirm",
        ],
    );
    assert_eq!(applied["result"]["applied"], true);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "x = 2\n");
}

#[test]
fn edit_rejects_an_ambiguous_anchor() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("d.txt");
    std::fs::write(&f, "dup\ndup\n").unwrap();

    let v = run(
        data.path(),
        &[
            "edit",
            f.to_str().unwrap(),
            "--anchor",
            "dup",
            "--replace",
            "x",
        ],
    );
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "anchor_ambiguous");
    assert_eq!(v["error"]["details"]["count"], 2);
}

#[test]
fn edit_rejects_a_stale_base_hash() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("e.txt");
    std::fs::write(&f, "v = 1\n").unwrap();

    let v = run(
        data.path(),
        &[
            "edit",
            f.to_str().unwrap(),
            "--anchor",
            "v = 1",
            "--replace",
            "v = 2",
            "--base-hash",
            "sha256:not-the-real-hash",
        ],
    );
    assert_eq!(v["error"]["code"], "stale_base");
}

#[test]
fn remove_scope_guard_blocks_large_removals() {
    let data = tempdir().unwrap();
    let mut args: Vec<String> = vec!["remove".into()];
    for i in 0..21 {
        args.push(format!("/tmp/navi-nonexistent-{i}"));
    }
    let out = Command::cargo_bin("navi")
        .unwrap()
        .env("NAVI_DATA_DIR", data.path())
        .args(&args)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "scope_exceeded");
}

#[test]
fn move_previews_then_renames_on_confirm() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let from = work.path().join("a.txt");
    let to = work.path().join("b.txt");
    std::fs::write(&from, "hi\n").unwrap();
    let (fp, tp) = (from.to_str().unwrap(), to.to_str().unwrap());

    let preview = run(data.path(), &["move", fp, tp]);
    assert_eq!(preview["result"]["applied"], false);
    assert!(from.exists() && !to.exists());

    let applied = run(data.path(), &["move", fp, tp, "--confirm"]);
    assert_eq!(applied["result"]["applied"], true);
    assert!(!from.exists() && to.exists());
}

#[test]
fn move_refuses_to_clobber_without_force() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let from = work.path().join("a.txt");
    let to = work.path().join("b.txt");
    std::fs::write(&from, "a\n").unwrap();
    std::fs::write(&to, "b\n").unwrap();

    let v = run(
        data.path(),
        &[
            "move",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
            "--confirm",
        ],
    );
    assert_eq!(v["error"]["code"], "destination_exists");
    assert_eq!(std::fs::read_to_string(&to).unwrap(), "b\n");
}

#[test]
fn locate_by_filename() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    std::fs::write(work.path().join("widget_loader.rs"), "fn x() {}\n").unwrap();
    std::fs::write(work.path().join("other.rs"), "fn y() {}\n").unwrap();

    let v = run(
        data.path(),
        &[
            "locate",
            "widget",
            work.path().to_str().unwrap(),
            "--match",
            "file",
        ],
    );
    let hits = v["result"]["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"]
        .as_str()
        .unwrap()
        .ends_with("widget_loader.rs"));
}

#[test]
fn locate_limit_reports_elided_in_budget() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    std::fs::write(work.path().join("f.txt"), "hit\nhit\nhit\nhit\n").unwrap();

    let v = run(
        data.path(),
        &[
            "locate",
            "hit",
            work.path().to_str().unwrap(),
            "--limit",
            "2",
        ],
    );
    assert_eq!(v["result"]["hits"].as_array().unwrap().len(), 2);
    assert_eq!(v["budget"]["returned"], 2);
    assert_eq!(v["budget"]["elided"], 2);
    assert_eq!(v["budget"]["truncated"], true);
}

#[test]
fn read_full_truncates_at_limit() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("big.txt");
    std::fs::write(&f, "1\n2\n3\n4\n5\n").unwrap();

    let v = run(data.path(), &["read", f.to_str().unwrap(), "--limit", "2"]);
    assert_eq!(v["result"]["end_line"], 2);
    assert_eq!(v["result"]["total_lines"], 5);
    assert_eq!(v["budget"]["truncated"], true);
}

#[test]
fn read_symbol_extracts_the_definition_block() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("m.rs");
    std::fs::write(
        &f,
        "fn before() {}\nfn navi_widget() {\n    let z = 1;\n}\nfn after() {}\n",
    )
    .unwrap();

    let v = run(
        data.path(),
        &[
            "read",
            f.to_str().unwrap(),
            "--mode",
            "symbol",
            "--symbol",
            "navi_widget",
        ],
    );
    assert_eq!(v["ok"], true);
    let text = v["result"]["text"].as_str().unwrap();
    assert!(text.starts_with("fn navi_widget()"));
    assert!(text.contains("let z = 1;"));
    assert!(text.trim_end().ends_with('}'));
    assert!(!text.contains("fn after"));
}

#[test]
fn edit_range_replaces_lines() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("r.txt");
    std::fs::write(&f, "a\nb\nc\n").unwrap();

    let v = run(
        data.path(),
        &[
            "edit",
            f.to_str().unwrap(),
            "--range",
            "2:2",
            "--content",
            "B",
            "--confirm",
        ],
    );
    assert_eq!(v["result"]["applied"], true);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "a\nB\nc\n");
}

#[test]
fn remove_confirm_trashes_and_reports() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("gone.txt");
    std::fs::write(&f, "bye\n").unwrap();

    let v = run(data.path(), &["remove", f.to_str().unwrap(), "--confirm"]);
    assert_eq!(v["result"]["applied"], true);
    assert_eq!(v["result"]["action"], "trash");
    assert_eq!(v["result"]["restorable"], true);
    assert_eq!(v["result"]["removed"].as_array().unwrap().len(), 1);
    assert!(!f.exists());
}

#[test]
fn remove_then_restore_brings_a_file_back() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("recover.txt");
    std::fs::write(&f, "important\n").unwrap();

    let removed = run(data.path(), &["remove", f.to_str().unwrap(), "--confirm"]);
    assert!(!f.exists());

    let restored = run(data.path(), &["restore", txn_of(&removed)]);
    assert_eq!(restored["result"]["op"], "remove");
    assert_eq!(restored["result"]["restored"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "important\n");
}

#[test]
fn remove_then_restore_brings_a_directory_back() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let dir = work.path().join("pkg");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mod.rs"), "fn x() {}\n").unwrap();

    let removed = run(data.path(), &["remove", dir.to_str().unwrap(), "--confirm"]);
    assert!(!dir.exists());

    run(data.path(), &["restore", txn_of(&removed)]);
    assert!(dir.join("mod.rs").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("mod.rs")).unwrap(),
        "fn x() {}\n"
    );
}

#[test]
fn purge_permanently_deletes_and_is_not_restorable() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("doomed.txt");
    std::fs::write(&f, "gone for good\n").unwrap();

    let removed = run(
        data.path(),
        &["remove", f.to_str().unwrap(), "--purge", "--confirm"],
    );
    assert_eq!(removed["result"]["action"], "purge");
    assert_eq!(removed["result"]["restorable"], false);
    assert!(!f.exists());

    let restored = run(data.path(), &["restore", txn_of(&removed)]);
    assert!(restored["result"]["restored"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(restored["result"]["skipped"].as_array().unwrap().len(), 1);
    assert!(!f.exists());
}

#[test]
fn edit_then_restore_reverts_content() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("cfg.txt");
    std::fs::write(&f, "v = 1\n").unwrap();

    let edited = run(
        data.path(),
        &[
            "edit",
            f.to_str().unwrap(),
            "--anchor",
            "v = 1",
            "--replace",
            "v = 2",
            "--confirm",
        ],
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "v = 2\n");

    run(data.path(), &["restore", txn_of(&edited)]);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "v = 1\n");
}

#[test]
fn move_then_restore_renames_back() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let from = work.path().join("a.txt");
    let to = work.path().join("b.txt");
    std::fs::write(&from, "hi\n").unwrap();

    let moved = run(
        data.path(),
        &[
            "move",
            from.to_str().unwrap(),
            to.to_str().unwrap(),
            "--confirm",
        ],
    );
    assert!(!from.exists() && to.exists());

    run(data.path(), &["restore", txn_of(&moved)]);
    assert!(from.exists() && !to.exists());
}

#[test]
fn restore_rejects_unknown_transaction() {
    let data = tempdir().unwrap();
    let v = run(data.path(), &["restore", "deadbeef"]);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "unknown_txn");
}

#[test]
fn mcp_initialize_and_lists_the_tools() {
    let data = tempdir().unwrap();
    let resps = mcp(
        data.path(),
        &[
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ],
    );

    // The notification produces no response: two requests, two responses.
    assert_eq!(resps.len(), 2);
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "navi");
    assert_eq!(resps[0]["result"]["protocolVersion"], "2025-06-18");

    let names: Vec<&str> = resps[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["locate", "read", "edit", "move", "remove", "restore"]
    );
}

#[test]
fn mcp_tools_call_runs_a_command() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    std::fs::write(work.path().join("f.txt"), "alpha\nbeta needle\n").unwrap();

    let resps = mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "locate",
                            "arguments": {"query": "needle", "paths": [work.path().to_str().unwrap()]}}})],
    );

    assert_eq!(resps[0]["result"]["isError"], false);
    let env = tool_envelope(&resps[0]);
    assert_eq!(env["ok"], true);
    assert_eq!(env["result"]["hits"][0]["line"], 2);
}

#[test]
fn mcp_edit_then_restore_round_trips() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("cfg.txt");
    std::fs::write(&f, "v = 1\n").unwrap();
    let path = f.to_str().unwrap();

    let edited = mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "edit",
                            "arguments": {"path": path, "anchor": "v = 1", "replace": "v = 2", "confirm": true}}})],
    );
    let txn = tool_envelope(&edited[0])["result"]["transaction_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "v = 2\n");

    mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "restore", "arguments": {"txn": txn}}})],
    );
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "v = 1\n");
}

#[test]
fn mcp_surfaces_a_navi_error_as_is_error() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    let f = work.path().join("d.txt");
    std::fs::write(&f, "dup\ndup\n").unwrap();

    let resps = mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "edit",
                            "arguments": {"path": f.to_str().unwrap(), "anchor": "dup", "replace": "x"}}})],
    );

    // A navi failure is a successful call whose envelope carries the error.
    assert_eq!(resps[0]["result"]["isError"], true);
    let env = tool_envelope(&resps[0]);
    assert_eq!(env["ok"], false);
    assert_eq!(env["error"]["code"], "anchor_ambiguous");
}

#[test]
fn mcp_rejects_an_unknown_tool() {
    let data = tempdir().unwrap();
    let resps = mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                 "params": {"name": "frobnicate", "arguments": {}}})],
    );
    assert_eq!(resps[0]["id"], 7);
    assert_eq!(resps[0]["error"]["code"], -32602);
}

#[test]
fn mcp_call_records_telemetry() {
    let work = tempdir().unwrap();
    let data = tempdir().unwrap();
    std::fs::write(work.path().join("f.txt"), "needle\n").unwrap();

    mcp(
        data.path(),
        &[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                 "params": {"name": "locate",
                            "arguments": {"query": "needle", "paths": [work.path().to_str().unwrap()]}}})],
    );

    // The MCP path feeds the same feedback loop as the CLI.
    let report: Value = serde_json::from_slice(&raw(data.path(), &["report", "--json"])).unwrap();
    assert!(report["by_tool"]["locate"].as_u64().unwrap() >= 1);
}

#[test]
fn miss_and_report_round_trip() {
    let data = tempdir().unwrap();
    let work = tempdir().unwrap();
    std::fs::write(work.path().join("f.txt"), "needle\n").unwrap();

    // Generate one real event, then a miss.
    run(
        data.path(),
        &["locate", "needle", work.path().to_str().unwrap()],
    );
    let recorded: Value = serde_json::from_slice(&raw(
        data.path(),
        &["miss", "unhelpful", "--tool", "locate"],
    ))
    .unwrap();
    assert_eq!(recorded["recorded"], true);

    let report: Value = serde_json::from_slice(&raw(data.path(), &["report", "--json"])).unwrap();
    assert!(report["by_tool"]["locate"].as_u64().unwrap() >= 1);
    assert_eq!(report["misses"].as_array().unwrap().len(), 1);
}
