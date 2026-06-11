//! Before-image journal for every mutation (edit/create/move/remove). The
//! substrate `navi restore` reverses from. Each entry is named by its
//! transaction id and records exactly what's needed to undo the op:
//!   edit   → [{ path, before }]                 (rewrite before-content)
//!   create → [{ path }]                          (delete the created file)
//!   move   → [{ from, to }]                     (rename to → from)
//!   remove → [{ path, kind, trashed | purged }] (move trashed → path)

use std::fs::{self, OpenOptions};
use std::io::Write;

use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::{NaviError, Result};
use crate::paths;

/// Files larger than this are journaled by path only, not by content.
const MAX_JOURNALED_BYTES: usize = 256 * 1024;

/// Mint a unique transaction id from a seed plus the current time.
pub fn new_txn(seed: &str) -> String {
    txn_id(&format!("{seed}{}", Utc::now().to_rfc3339()))
}

/// Write a journal entry under the given transaction id.
pub fn write(txn: &str, op: &str, items: Vec<Value>) {
    let body = json!({ "txn": txn, "op": op, "ts": Utc::now().to_rfc3339(), "items": items });
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
}

/// Convenience for ops whose items are known up front: mint a txn, write, return it.
pub fn record(op: &str, items: Vec<Value>) -> String {
    let txn = new_txn(op);
    write(&txn, op, items);
    txn
}

/// Load a journal entry for restore.
pub fn load(txn: &str) -> Result<Value> {
    let path = paths::journal_dir().join(format!("{txn}.json"));
    let s = fs::read_to_string(&path)
        .map_err(|_| NaviError::new("unknown_txn", format!("no journal entry for {txn}")))?;
    serde_json::from_str(&s).map_err(|e| NaviError::new("journal_corrupt", e.to_string()))
}

/// Capture a file's current content as a journal item, eliding oversized files.
pub fn file_item(path: &str, content: Option<String>) -> Value {
    match content {
        Some(c) if c.len() <= MAX_JOURNALED_BYTES => {
            json!({ "path": path, "before": c })
        }
        Some(c) => json!({ "path": path, "before_elided": true, "bytes": c.len() }),
        None => json!({ "path": path, "kind": "missing" }),
    }
}

pub fn txn_id(payload: &str) -> String {
    let digest = Sha256::digest(payload.as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}
