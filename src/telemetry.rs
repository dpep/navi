//! The feedback loop. Every command appends one event; `navi report`
//! aggregates them. This is how we learn whether navi is actually used, where
//! it falls back, where results get truncated, and where it misses entirely.
//!
//! Logging is best-effort: a telemetry failure must never fail the command.

use std::fs::{self, OpenOptions};
use std::io::Write;

use chrono::Utc;
use serde_json::{json, Value};

use crate::paths;

pub struct Event<'a> {
    pub tool: &'a str,
    pub ok: bool,
    pub backend: Option<&'a str>,
    pub fallback_reason: Option<&'a str>,
    pub result_count: usize,
    pub bytes_out: usize,
    pub latency_ms: u128,
    pub truncated: bool,
    pub error_code: Option<&'a str>,
    pub args: Value,
}

pub fn log(event: &Event) {
    let record = json!({
        "ts": Utc::now().to_rfc3339(),
        "tool": event.tool,
        "ok": event.ok,
        "backend": event.backend,
        "fallback_reason": event.fallback_reason,
        "result_count": event.result_count,
        "bytes_out": event.bytes_out,
        "latency_ms": event.latency_ms,
        "truncated": event.truncated,
        "error_code": event.error_code,
        "args": event.args,
    });
    append(&record);
}

/// A `navi miss` note — an explicit signal that a result was unhelpful, with
/// optional reference to which tool let the user/agent down.
pub fn log_miss(note: &str, ref_tool: Option<&str>) {
    let record = json!({
        "ts": Utc::now().to_rfc3339(),
        "tool": "miss",
        "ok": true,
        "note": note,
        "ref_tool": ref_tool,
    });
    append(&record);
}

fn append(record: &Value) {
    let path = paths::telemetry_log();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{record}");
    }
}
