//! `locate` — unified search. One intent ("find the thing"), three match
//! modes, each preferring a purpose-built backend and falling back to POSIX.

use serde_json::{json, Value};

use crate::backend::{self, Backends};
use crate::cli::{LocateArgs, MatchKind};
use crate::error::{NaviError, Result};
use crate::output::Outcome;
use crate::util;

pub fn run(a: &LocateArgs) -> Result<Outcome> {
    let backends = Backends::detect();
    let paths: Vec<String> = if a.paths.is_empty() {
        vec![".".to_string()]
    } else {
        a.paths.clone()
    };
    match a.match_kind {
        MatchKind::Text => locate_text(a, &paths, &backends),
        MatchKind::Symbol => locate_symbol(a, &paths, &backends),
        MatchKind::File => locate_file(a, &paths, &backends),
    }
}

fn finish(hits: Vec<Value>, total: usize, backend: &str, fallback: Option<String>) -> Outcome {
    let returned = hits.len();
    let elided = total.saturating_sub(returned);
    let mut o = Outcome::new(json!({ "hits": hits }))
        .backend(backend)
        .budget(returned, elided, elided > 0);
    if let Some(f) = fallback {
        o = o.fallback(f);
    }
    o
}

fn locate_text(a: &LocateArgs, paths: &[String], b: &Backends) -> Result<Outcome> {
    if b.rg {
        let mut args: Vec<String> = vec!["--json".into()];
        if a.fixed {
            args.push("-F".into());
        }
        if let Some(g) = a.lang.as_deref().and_then(util::lang_glob) {
            args.push("-g".into());
            args.push(g);
        }
        args.push("-e".into());
        args.push(a.query.clone());
        args.extend(paths.iter().cloned());

        let out = backend::run("rg", &as_refs(&args))?;
        let (hits, total) = parse_rg(&out.stdout, a.limit);
        if hits.is_empty() && out.status.code() == Some(2) {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(NaviError::new("search_failed", err.trim().to_string()));
        }
        return Ok(finish(hits, total, "ripgrep", None));
    }
    if b.grep {
        let mut args: Vec<String> = vec!["-rIn".into()];
        if a.fixed {
            args.push("-F".into());
        }
        if let Some(g) = a.lang.as_deref().and_then(util::lang_glob) {
            args.push(format!("--include={g}"));
        }
        args.push("-e".into());
        args.push(a.query.clone());
        args.extend(paths.iter().cloned());

        let out = backend::run("grep", &as_refs(&args))?;
        let (hits, total) = parse_grep(&out.stdout, a.limit);
        return Ok(finish(
            hits,
            total,
            "grep",
            Some("rg unavailable; used grep".into()),
        ));
    }
    Err(NaviError::new(
        "no_backend",
        "no content-search backend found (need rg or grep)",
    ))
}

fn locate_symbol(a: &LocateArgs, paths: &[String], b: &Backends) -> Result<Outcome> {
    if b.rq {
        let mut args: Vec<String> = vec![
            "--ndjson".into(),
            "--no-record".into(),
            "--limit".into(),
            a.limit.to_string(),
        ];
        if let Some(k) = &a.kind {
            args.push("--kind".into());
            args.push(k.clone());
        }
        for p in paths {
            if p != "." {
                args.push("-p".into());
                args.push(p.clone());
            }
        }
        args.push(a.query.clone());

        let out = backend::run("rq", &as_refs(&args))?;
        let (hits, total) = parse_rq(&out.stdout, a.limit);
        return Ok(finish(hits, total, "rq", None));
    }
    // Fallback: rq absent → search for the symbol name as literal text.
    let as_text = LocateArgs {
        query: a.query.clone(),
        paths: a.paths.clone(),
        match_kind: MatchKind::Text,
        lang: a.lang.clone(),
        kind: None,
        fixed: true,
        limit: a.limit,
    };
    let o = locate_text(&as_text, paths, b)?;
    Ok(o.fallback("rq unavailable; literal text search for symbol name"))
}

fn locate_file(a: &LocateArgs, paths: &[String], b: &Backends) -> Result<Outcome> {
    let needle = a.query.to_lowercase();
    if b.fd {
        // fd matches the filename natively, so no post-filter is needed.
        // --fixed-strings keeps it a literal substring match, consistent with
        // the `find -iname '*q*'` fallback rather than fd's default regex.
        let mut args: Vec<String> = vec!["--type".into(), "f".into(), "--fixed-strings".into()];
        if let Some(ext) = a.lang.as_deref().and_then(util::lang_ext) {
            args.push("-e".into());
            args.push(ext.into());
        }
        args.push(a.query.clone());
        args.extend(paths.iter().cloned());

        let out = backend::run("fd", &as_refs(&args))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let (mut hits, mut total) = (Vec::new(), 0usize);
        for line in text.lines() {
            total += 1;
            if hits.len() < a.limit {
                hits.push(json!({ "path": line, "kind": "file" }));
            }
        }
        return Ok(finish(hits, total, "fd", None));
    }
    if b.rg {
        let mut args: Vec<String> = vec!["--files".into()];
        if let Some(g) = a.lang.as_deref().and_then(util::lang_glob) {
            args.push("-g".into());
            args.push(g);
        }
        args.extend(paths.iter().cloned());

        let out = backend::run("rg", &as_refs(&args))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let (mut hits, mut total) = (Vec::new(), 0usize);
        for line in text.lines() {
            if line.to_lowercase().contains(&needle) {
                total += 1;
                if hits.len() < a.limit {
                    hits.push(json!({ "path": line, "kind": "file" }));
                }
            }
        }
        return Ok(finish(
            hits,
            total,
            "ripgrep",
            Some("fd unavailable; used rg --files".into()),
        ));
    }
    if b.find {
        let mut args: Vec<String> = paths.to_vec();
        args.push("-type".into());
        args.push("f".into());
        args.push("-iname".into());
        args.push(format!("*{}*", a.query));

        let out = backend::run("find", &as_refs(&args))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let (mut hits, mut total) = (Vec::new(), 0usize);
        for line in text.lines() {
            total += 1;
            if hits.len() < a.limit {
                hits.push(json!({ "path": line, "kind": "file" }));
            }
        }
        return Ok(finish(
            hits,
            total,
            "find",
            Some("fd/rg unavailable; used find".into()),
        ));
    }
    Err(NaviError::new(
        "no_backend",
        "no filename-search backend found (need fd, rg, or find)",
    ))
}

fn parse_rg(stdout: &[u8], limit: usize) -> (Vec<Value>, usize) {
    let text = String::from_utf8_lossy(stdout);
    let (mut hits, mut total) = (Vec::new(), 0usize);
    for line in text.lines() {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v["type"] != "match" {
            continue;
        }
        total += 1;
        if hits.len() >= limit {
            continue;
        }
        let d = &v["data"];
        hits.push(json!({
            "path": d["path"]["text"].as_str().unwrap_or(""),
            "line": d["line_number"].as_u64().unwrap_or(0),
            "kind": "text",
            "snippet": util::snippet(d["lines"]["text"].as_str().unwrap_or("")),
        }));
    }
    (hits, total)
}

fn parse_grep(stdout: &[u8], limit: usize) -> (Vec<Value>, usize) {
    let text = String::from_utf8_lossy(stdout);
    let (mut hits, mut total) = (Vec::new(), 0usize);
    for line in text.lines() {
        let mut it = line.splitn(3, ':');
        if let (Some(p), Some(l), Some(c)) = (it.next(), it.next(), it.next()) {
            if let Ok(n) = l.parse::<u64>() {
                total += 1;
                if hits.len() >= limit {
                    continue;
                }
                hits.push(json!({
                    "path": p, "line": n, "kind": "text", "snippet": util::snippet(c),
                }));
            }
        }
    }
    (hits, total)
}

fn parse_rq(stdout: &[u8], limit: usize) -> (Vec<Value>, usize) {
    let text = String::from_utf8_lossy(stdout);
    let (mut hits, mut total) = (Vec::new(), 0usize);
    for line in text.lines() {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        total += 1;
        if hits.len() >= limit {
            continue;
        }
        hits.push(json!({
            "path": v["file"].as_str().unwrap_or(""),
            "line": v["line"].as_u64().unwrap_or(0),
            "kind": v["kind"].as_str().unwrap_or("symbol"),
            "symbol": v["name"].as_str().unwrap_or(""),
            "parent": v["parent"].as_str(),
            "snippet": util::snippet(v["signature"].as_str().unwrap_or("")),
            "score": v["score"].as_f64(),
        }));
    }
    (hits, total)
}

fn as_refs(args: &[String]) -> Vec<&str> {
    args.iter().map(|s| s.as_str()).collect()
}
