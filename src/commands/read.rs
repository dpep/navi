//! `read` — intent-aware reads. The token-saving modes are `outline`
//! (signatures only) and `symbol` (one definition span); `full`/`range` are the
//! literal reads. Every result carries a content hash to anchor a later edit.

use std::fs;

use serde_json::{json, Value};

use crate::backend::{self, Backends};
use crate::cli::{ReadArgs, ReadMode};
use crate::error::{NaviError, Result};
use crate::output::Outcome;
use crate::util;

pub fn run(a: &ReadArgs) -> Result<Outcome> {
    let bytes = fs::read(&a.path)
        .map_err(|e| NaviError::new("not_found", format!("cannot read {}: {e}", a.path)))?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let lines: Vec<&str> = content.lines().collect();
    let hash = util::content_hash(&bytes);
    let lang = util::lang_from_path(&a.path);

    match a.mode {
        ReadMode::Full => Ok(read_range(
            &a.path,
            &lines,
            1,
            lines.len(),
            a.limit,
            &hash,
            lang,
            "full",
        )),
        ReadMode::Range => {
            let s = a
                .range
                .as_deref()
                .ok_or_else(|| NaviError::new("invalid_args", "--range required for mode=range"))?;
            let (start, end) = util::parse_range(s, lines.len())?;
            Ok(read_range(
                &a.path, &lines, start, end, a.limit, &hash, lang, "range",
            ))
        }
        ReadMode::Outline => Ok(read_outline(&a.path, &lines, a.limit, &hash, lang)),
        ReadMode::Symbol => read_symbol(a, &lines, &hash, lang),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_range(
    path: &str,
    lines: &[&str],
    start: usize,
    end: usize,
    limit: usize,
    hash: &str,
    lang: Option<&str>,
    mode: &str,
) -> Outcome {
    let total = lines.len();
    let start = start.max(1);
    let mut end = end.min(total);
    let mut truncated = false;
    if total == 0 {
        return file_slice(path, &[], 0, 0, total, hash, lang, mode, false);
    }
    if end >= start && end - start + 1 > limit {
        end = start + limit - 1;
        truncated = true;
    }
    let slice = if start <= total {
        &lines[start - 1..end]
    } else {
        &[][..]
    };
    file_slice(path, slice, start, end, total, hash, lang, mode, truncated)
}

#[allow(clippy::too_many_arguments)]
fn file_slice(
    path: &str,
    slice: &[&str],
    start: usize,
    end: usize,
    total: usize,
    hash: &str,
    lang: Option<&str>,
    mode: &str,
    truncated: bool,
) -> Outcome {
    let returned = slice.len();
    Outcome::new(json!({
        "path": path,
        "lang": lang,
        "total_lines": total,
        "content_hash": hash,
        "mode": mode,
        "start_line": start,
        "end_line": end,
        "text": slice.join("\n"),
    }))
    .budget(returned, total.saturating_sub(returned), truncated)
}

fn read_outline(
    path: &str,
    lines: &[&str],
    limit: usize,
    hash: &str,
    lang: Option<&str>,
) -> Outcome {
    let mut outline = Vec::new();
    let mut seen = 0usize;
    for (i, l) in lines.iter().enumerate() {
        if let Some(kind) = definition_kind(l) {
            seen += 1;
            if outline.len() < limit {
                outline.push(json!({ "line": i + 1, "kind": kind, "text": util::snippet(l) }));
            }
        }
    }
    let returned = outline.len();
    Outcome::new(json!({
        "path": path,
        "lang": lang,
        "total_lines": lines.len(),
        "content_hash": hash,
        "mode": "outline",
        "outline": outline,
    }))
    .budget(returned, seen.saturating_sub(returned), seen > returned)
}

fn read_symbol(a: &ReadArgs, lines: &[&str], hash: &str, lang: Option<&str>) -> Result<Outcome> {
    let sym = a
        .symbol
        .as_deref()
        .ok_or_else(|| NaviError::new("invalid_args", "--symbol required for mode=symbol"))?;
    if lines.is_empty() {
        return Err(NaviError::new("symbol_not_found", "file is empty"));
    }
    let backends = Backends::detect();
    let (start_idx, via) = find_symbol_def(&a.path, sym, lines, &backends).ok_or_else(|| {
        NaviError::new(
            "symbol_not_found",
            format!("no definition of '{sym}' in {}", a.path),
        )
    })?;
    let end_idx = extract_block(lines, start_idx, a.limit);
    let span = end_idx - start_idx + 1;
    Ok(Outcome::new(json!({
        "path": a.path,
        "lang": lang,
        "total_lines": lines.len(),
        "content_hash": hash,
        "mode": "symbol",
        "symbol": sym,
        "found_via": via,
        "start_line": start_idx + 1,
        "end_line": end_idx + 1,
        "text": lines[start_idx..=end_idx].join("\n"),
    }))
    .budget(span, 0, span >= a.limit))
}

/// Locate the line a symbol is defined on. Prefers rq; falls back to a
/// definition-heuristic scan of the file.
fn find_symbol_def(
    path: &str,
    sym: &str,
    lines: &[&str],
    b: &Backends,
) -> Option<(usize, &'static str)> {
    if b.rq {
        let args = ["--ndjson", "--no-record", "--limit", "20", sym];
        if let Ok(out) = backend::run("rq", &args) {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    if v["name"].as_str() == Some(sym)
                        && same_file(v["file"].as_str().unwrap_or(""), path)
                    {
                        if let Some(l) = v["line"].as_u64() {
                            let idx = (l as usize).saturating_sub(1);
                            if idx < lines.len() {
                                return Some((idx, "rq"));
                            }
                        }
                    }
                }
            }
        }
    }
    for (i, l) in lines.iter().enumerate() {
        if is_definition(l) && l.contains(sym) {
            return Some((i, "scan"));
        }
    }
    None
}

fn same_file(a: &str, b: &str) -> bool {
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    a == b || a.ends_with(b) || b.ends_with(a) || base(a) == base(b)
}

/// Find the end of a definition block: brace-balanced for C-family code,
/// indentation-based otherwise. Capped at `limit` lines.
fn extract_block(lines: &[&str], start: usize, limit: usize) -> usize {
    let max_end = (start + limit - 1).min(lines.len() - 1);
    let brace_in_range = (start..=max_end).any(|i| lines[i].contains('{'));

    if brace_in_range {
        let mut depth = 0i32;
        let mut opened = false;
        for (offset, line) in lines[start..=max_end].iter().enumerate() {
            for ch in line.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth <= 0 {
                return start + offset;
            }
        }
        return max_end;
    }

    let base = indent(lines[start]);
    for (offset, line) in lines[start + 1..=max_end].iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if indent(line) <= base {
            return start + offset;
        }
    }
    max_end
}

fn indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn is_definition(line: &str) -> bool {
    definition_kind(line).is_some()
}

/// Heuristic: if this line introduces a definition, return the keyword that
/// marks it (`fn`, `class`, `def`, …) — used as the outline entry's `kind`.
/// Looks among the first few tokens. Intentionally language-agnostic.
fn definition_kind(line: &str) -> Option<&'static str> {
    const KW: &[&str] = &[
        "fn",
        "func",
        "function",
        "def",
        "class",
        "struct",
        "impl",
        "trait",
        "enum",
        "interface",
        "module",
        "package",
        "type",
        "const",
        "static",
        "public",
        "private",
        "protected",
        "export",
    ];
    let t = line.trim_start();
    if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
        return None;
    }
    for (idx, word) in t.split(|c: char| c.is_whitespace() || c == '(').enumerate() {
        if idx >= 3 {
            break;
        }
        let w = word.trim_matches(|c: char| !c.is_alphanumeric());
        if let Some(kw) = KW.iter().find(|k| **k == w) {
            return Some(kw);
        }
    }
    None
}
