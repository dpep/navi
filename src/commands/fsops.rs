//! `move` and `remove` — destructive ops without reference checks (deferred),
//! but a scope guard, a before-image journal, and trash-backed removal mean
//! `navi undo` can bring things back.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::cli::{MoveArgs, RemoveArgs};
use crate::error::{NaviError, Result};
use crate::journal;
use crate::output::Outcome;
use crate::trashbin;

/// Removals above this many paths require --force.
const SCOPE_THRESHOLD: usize = 20;

pub fn run_move(a: &MoveArgs) -> Result<Outcome> {
    let src = Path::new(&a.from);
    if !src.exists() {
        return Err(NaviError::new(
            "not_found",
            format!("source does not exist: {}", a.from),
        ));
    }
    let dst_exists = Path::new(&a.to).exists();
    if dst_exists && !a.force {
        return Err(NaviError::new(
            "destination_exists",
            "destination exists; pass --force to overwrite",
        )
        .with_details(json!({ "to": a.to })));
    }

    if a.dry_run {
        return Ok(Outcome::new(json!({
            "applied": false,
            "from": a.from,
            "to": a.to,
            "overwrite": dst_exists,
            "hint": "dry run: nothing moved",
        })));
    }

    let txn = journal::record("move", vec![json!({ "from": a.from, "to": a.to })]);
    fs::rename(&a.from, &a.to)
        .map_err(|e| NaviError::new("move_failed", format!("could not move: {e}")))?;
    Ok(Outcome::new(json!({
        "applied": true,
        "transaction_id": txn,
        "from": a.from,
        "to": a.to,
    })))
}

pub fn run_remove(a: &RemoveArgs) -> Result<Outcome> {
    if a.paths.len() > SCOPE_THRESHOLD && !a.force {
        return Err(NaviError::new(
            "scope_exceeded",
            "too many paths for one removal; pass --force if intentional",
        )
        .with_details(json!({ "count": a.paths.len(), "threshold": SCOPE_THRESHOLD })));
    }

    let action = if a.purge { "purge" } else { "trash" };
    let targets: Vec<_> = a
        .paths
        .iter()
        .map(|p| {
            let path = Path::new(p);
            let meta = fs::metadata(path).ok();
            json!({
                "path": p,
                "exists": path.exists(),
                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                "size": meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            })
        })
        .collect();

    if a.dry_run {
        let n = targets.len();
        return Ok(Outcome::new(json!({
            "applied": false,
            "action": action,
            "targets": targets,
            "hint": "dry run: nothing removed",
        }))
        .budget(n, 0, false));
    }

    // Mint the txn up front: it namespaces the managed-trash holding dir and
    // names the journal entry.
    let txn = journal::new_txn(&format!("remove:{}", a.paths.join("\u{0}")));
    let mut items = Vec::new();
    let mut removed = Vec::new();
    for p in &a.paths {
        let path = Path::new(p);
        if !path.exists() {
            items.push(json!({ "path": p, "kind": "missing" }));
            continue;
        }
        let kind = if path.is_dir() { "dir" } else { "file" };
        if a.purge {
            purge(path)?;
            items.push(json!({ "path": p, "kind": kind, "purged": true }));
        } else {
            let trashed = trashbin::send(path, &txn)?;
            items.push(json!({
                "path": p,
                "kind": kind,
                "trashed": trashed.map(|t| t.to_string_lossy().into_owned()),
            }));
        }
        removed.push(p.clone());
    }
    journal::write(&txn, "remove", items);

    let n = removed.len();
    Ok(Outcome::new(json!({
        "applied": true,
        "action": action,
        "transaction_id": txn,
        "removed": removed,
        "restorable": !a.purge,
    }))
    .budget(n, 0, false))
}

fn purge(path: &Path) -> Result<()> {
    let r = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    r.map_err(|e| {
        NaviError::new(
            "remove_failed",
            format!("could not remove {}: {e}", path.display()),
        )
    })
}
