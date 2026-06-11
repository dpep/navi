//! `edit` — safe, transactional write. Preview by default; anchors must be
//! unique; an optional base hash rejects edits against a changed file. Applied
//! edits journal a before-image first.

use std::fs;

use serde_json::json;
use similar::TextDiff;

use crate::cli::EditArgs;
use crate::error::{NaviError, Result};
use crate::journal;
use crate::output::Outcome;
use crate::util;

pub fn run(a: &EditArgs) -> Result<Outcome> {
    // An absent target is a create: write a brand-new file from --content.
    if !std::path::Path::new(&a.path).exists() {
        return create_file(a);
    }

    let bytes = fs::read(&a.path)
        .map_err(|e| NaviError::new("not_found", format!("cannot read {}: {e}", a.path)))?;
    let old = String::from_utf8_lossy(&bytes).into_owned();
    let hash = util::content_hash(&bytes);

    if let Some(expected) = &a.base_hash {
        if expected != &hash {
            return Err(NaviError::new(
                "stale_base",
                "file changed since it was read; re-read and retry",
            )
            .with_details(json!({ "expected": expected, "actual": hash })));
        }
    }

    let new = compute_new(a, &old)?;
    let diff = unified_diff(&a.path, &old, &new);

    if !a.confirm {
        return Ok(Outcome::new(json!({
            "applied": false,
            "diff": diff,
            "content_hash": hash,
            "hint": "re-run with --confirm to apply",
        })));
    }

    let txn = journal::record("edit", vec![journal::file_item(&a.path, Some(old))]);
    fs::write(&a.path, &new)?;
    Ok(Outcome::new(json!({
        "applied": true,
        "transaction_id": txn,
        "diff": diff,
        "content_hash": util::content_hash(new.as_bytes()),
    })))
}

/// Create a new file from `--content`. Anchor/range/base-hash all assume prior
/// content, so they're rejected here. Journaled as a `create` so `undo` can
/// undo it by deleting the file.
fn create_file(a: &EditArgs) -> Result<Outcome> {
    let content = a.content.as_deref().ok_or_else(|| {
        NaviError::new(
            "invalid_args",
            format!("{} does not exist; pass --content to create it", a.path),
        )
    })?;
    if a.anchor.is_some() || a.range.is_some() || a.base_hash.is_some() {
        return Err(NaviError::new(
            "invalid_args",
            "creating a new file takes only --content",
        ));
    }

    let diff = unified_diff(&a.path, "", content);
    if !a.confirm {
        return Ok(Outcome::new(json!({
            "applied": false,
            "created": true,
            "diff": diff,
            "hint": "re-run with --confirm to create",
        })));
    }

    if let Some(parent) = std::path::Path::new(&a.path).parent() {
        fs::create_dir_all(parent)?;
    }
    let txn = journal::record("create", vec![json!({ "path": a.path })]);
    fs::write(&a.path, content)?;
    Ok(Outcome::new(json!({
        "applied": true,
        "created": true,
        "transaction_id": txn,
        "diff": diff,
        "content_hash": util::content_hash(content.as_bytes()),
    })))
}

fn compute_new(a: &EditArgs, old: &str) -> Result<String> {
    let anchored = a.anchor.is_some();
    let ranged = a.range.is_some();

    if anchored && !ranged {
        let anchor = a.anchor.as_ref().unwrap();
        let replace = a
            .replace
            .as_ref()
            .ok_or_else(|| NaviError::new("invalid_args", "--anchor requires --replace"))?;
        let count = old.matches(anchor.as_str()).count();
        if count == 0 {
            return Err(NaviError::new("anchor_not_found", "anchor text not found")
                .with_details(json!({ "anchor": anchor })));
        }
        if count > 1 {
            return Err(NaviError::new(
                "anchor_ambiguous",
                "anchor matches multiple locations; make it unique",
            )
            .with_details(json!({ "anchor": anchor, "count": count })));
        }
        Ok(old.replacen(anchor.as_str(), replace, 1))
    } else if ranged && !anchored {
        let content = a
            .content
            .as_ref()
            .ok_or_else(|| NaviError::new("invalid_args", "--range requires --content"))?;
        let lines: Vec<&str> = old.lines().collect();
        let total = lines.len();
        let (s, e) = util::parse_range(a.range.as_ref().unwrap(), total)?;
        if e > total {
            return Err(NaviError::new("invalid_args", "range out of bounds")
                .with_details(json!({ "total_lines": total })));
        }
        let mut out: Vec<&str> = Vec::new();
        out.extend_from_slice(&lines[0..s - 1]);
        out.extend(content.lines());
        out.extend_from_slice(&lines[e..]);
        let mut joined = out.join("\n");
        if old.ends_with('\n') {
            joined.push('\n');
        }
        Ok(joined)
    } else {
        Err(NaviError::new(
            "invalid_args",
            "provide --anchor with --replace, OR --range with --content",
        ))
    }
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}
