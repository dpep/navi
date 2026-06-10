//! Before-image journal for every mutation (edit/move/remove). Cheap audit
//! trail today; the substrate for a future `navi restore`. Capped per-file so
//! a journal entry never balloons on large files.

use std::fs::{self, OpenOptions};
use std::io::Write;

use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::paths;

/// Files larger than this are journaled by path only, not by content.
const MAX_JOURNALED_BYTES: usize = 256 * 1024;

/// Write a journal entry, returning a short transaction id derived from its
/// content (deterministic for a given op+payload+timestamp). `items` is one
/// JSON object per affected path: {path, kind, before?}.
pub fn record(op: &str, items: Vec<Value>) -> String {
    let ts = Utc::now().to_rfc3339();
    let body = json!({ "op": op, "ts": ts, "items": items });
    let txn = txn_id(&body.to_string());

    let dir = paths::journal_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dir.join(format!("{txn}.json")))
    {
        let _ = writeln!(f, "{body}");
    }
    txn
}

/// Capture a file's current content as a journal item, eliding oversized files.
pub fn file_item(path: &str, content: Option<String>) -> Value {
    match content {
        Some(c) if c.len() <= MAX_JOURNALED_BYTES => {
            json!({ "path": path, "kind": "file", "before": c })
        }
        Some(c) => json!({ "path": path, "kind": "file", "before_elided": true, "bytes": c.len() }),
        None => json!({ "path": path, "kind": "missing" }),
    }
}

pub fn txn_id(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}
