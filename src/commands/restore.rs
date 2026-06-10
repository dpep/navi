//! `restore` — reverse a journaled transaction. One command covers all three
//! mutations: edit (rewrite before-content), move (rename back), remove (move
//! the item out of the trash to where it was).

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::cli::RestoreArgs;
use crate::error::{NaviError, Result};
use crate::journal;
use crate::output::Outcome;
use crate::trashbin;

pub fn run(a: &RestoreArgs) -> Result<Outcome> {
    let entry = journal::load(&a.txn)?;
    let op = entry["op"].as_str().unwrap_or("");
    let items = entry["items"].as_array().cloned().unwrap_or_default();

    let (restored, skipped) = match op {
        "edit" => restore_edit(&items)?,
        "move" => restore_move(&items)?,
        "remove" => restore_remove(&items)?,
        other => {
            return Err(NaviError::new(
                "unrestorable",
                format!("cannot restore op '{other}'"),
            ))
        }
    };

    let n = restored.len();
    Ok(Outcome::new(json!({
        "transaction_id": a.txn,
        "op": op,
        "restored": restored,
        "skipped": skipped,
    }))
    .budget(n, skipped.len(), false))
}

fn restore_edit(items: &[Value]) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut restored = Vec::new();
    for it in items {
        let path = str_field(it, "path")?;
        let before = it["before"].as_str().ok_or_else(|| {
            NaviError::new(
                "unrestorable",
                "edit before-image was not captured (file too large)",
            )
            .with_details(json!({ "path": path }))
        })?;
        fs::write(path, before)?;
        restored.push(json!({ "path": path }));
    }
    Ok((restored, Vec::new()))
}

fn restore_move(items: &[Value]) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut restored = Vec::new();
    for it in items {
        let from = str_field(it, "from")?;
        let to = str_field(it, "to")?;
        if !Path::new(to).exists() {
            return Err(NaviError::new(
                "unrestorable",
                "moved file is no longer at its destination",
            )
            .with_details(json!({ "to": to })));
        }
        trashbin::move_path(Path::new(to), Path::new(from))?;
        restored.push(json!({ "path": from }));
    }
    Ok((restored, Vec::new()))
}

fn restore_remove(items: &[Value]) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut restored = Vec::new();
    let mut skipped = Vec::new();
    for it in items {
        let path = match it["path"].as_str() {
            Some(p) => p,
            None => continue,
        };
        if it["purged"].as_bool() == Some(true) {
            skipped.push(json!({ "path": path, "reason": "purged" }));
            continue;
        }
        match it["trashed"].as_str() {
            Some(trashed) => {
                if let Some(parent) = Path::new(path).parent() {
                    let _ = fs::create_dir_all(parent);
                }
                trashbin::move_path(Path::new(trashed), Path::new(path))?;
                restored.push(json!({ "path": path }));
            }
            None => skipped.push(json!({ "path": path, "reason": "trash location unknown" })),
        }
    }
    Ok((restored, skipped))
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| NaviError::new("journal_corrupt", format!("journal item missing '{key}'")))
}
