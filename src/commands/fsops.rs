//! `move` and `remove` — destructive ops without reference checks (deferred),
//! but with preview, a scope guard, and a before-image journal.

use std::fs;
use std::path::Path;

use serde_json::json;

use crate::cli::{MoveArgs, RemoveArgs};
use crate::error::{NaviError, Result};
use crate::journal;
use crate::output::Outcome;

/// Removals above this many paths require --force.
const SCOPE_THRESHOLD: usize = 20;

pub fn run_move(a: &MoveArgs) -> Result<Outcome> {
    let src = Path::new(&a.from);
    if !src.exists() {
        return Err(NaviError::new("not_found", format!("source does not exist: {}", a.from)));
    }
    let dst_exists = Path::new(&a.to).exists();
    if dst_exists && !a.force {
        return Err(NaviError::new("destination_exists", "destination exists; pass --force to overwrite")
            .with_details(json!({ "to": a.to })));
    }

    if !a.confirm {
        return Ok(Outcome::new(json!({
            "applied": false,
            "from": a.from,
            "to": a.to,
            "overwrite": dst_exists,
            "hint": "re-run with --confirm to apply",
        })));
    }

    journal::record("move", vec![json!({ "from": a.from, "to": a.to })]);
    fs::rename(&a.from, &a.to)
        .map_err(|e| NaviError::new("move_failed", format!("could not move: {e}")))?;
    Ok(Outcome::new(json!({ "applied": true, "from": a.from, "to": a.to })))
}

pub fn run_remove(a: &RemoveArgs) -> Result<Outcome> {
    if a.paths.len() > SCOPE_THRESHOLD && !a.force {
        return Err(NaviError::new(
            "scope_exceeded",
            "too many paths for one removal; pass --force if intentional",
        )
        .with_details(json!({ "count": a.paths.len(), "threshold": SCOPE_THRESHOLD })));
    }

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

    if !a.confirm {
        let n = targets.len();
        return Ok(Outcome::new(json!({
            "applied": false,
            "targets": targets,
            "hint": "re-run with --confirm to apply",
        }))
        .budget(n, 0, false));
    }

    let mut items = Vec::new();
    let mut removed = Vec::new();
    for p in &a.paths {
        let path = Path::new(p);
        if !path.exists() {
            items.push(journal::file_item(p, None));
            continue;
        }
        if path.is_dir() {
            items.push(json!({ "path": p, "kind": "dir" }));
            fs::remove_dir_all(path)
                .map_err(|e| NaviError::new("remove_failed", format!("could not remove {p}: {e}")))?;
        } else {
            items.push(journal::file_item(p, fs::read_to_string(path).ok()));
            fs::remove_file(path)
                .map_err(|e| NaviError::new("remove_failed", format!("could not remove {p}: {e}")))?;
        }
        removed.push(p.clone());
    }
    journal::record("remove", items);
    let n = removed.len();
    Ok(Outcome::new(json!({ "applied": true, "removed": removed })).budget(n, 0, false))
}
