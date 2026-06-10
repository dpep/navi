//! `report` and `miss` — the feedback loop's read side. `report` aggregates the
//! telemetry log into a usefulness picture; `miss` appends an explicit "this was
//! unhelpful" signal.

use std::collections::BTreeMap;
use std::fs;

use serde_json::{json, Value};

use crate::cli::{MissArgs, ReportArgs};
use crate::paths;
use crate::telemetry;

pub fn run_miss(a: &MissArgs) {
    telemetry::log_miss(&a.note, a.ref_tool.as_deref());
    let env = json!({
        "tool": "miss",
        "ok": true,
        "recorded": true,
        "note": a.note,
        "ref_tool": a.ref_tool,
    });
    println!("{}", serde_json::to_string_pretty(&env).unwrap_or_default());
}

pub fn run(a: &ReportArgs) {
    let path = paths::telemetry_log();
    // Read the rotated generation first, then the active log, so events stay in
    // chronological order across a rotation.
    let mut content = fs::read_to_string(paths::telemetry_log_rotated()).unwrap_or_default();
    content.push_str(&fs::read_to_string(&path).unwrap_or_default());
    if content.is_empty() {
        println!("no telemetry recorded yet ({})", path.display());
        return;
    }

    let events: Vec<Value> = content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let mut total = 0u64;
    let mut by_tool: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_backend: BTreeMap<String, u64> = BTreeMap::new();
    let mut fallback_reasons: BTreeMap<String, u64> = BTreeMap::new();
    let mut error_codes: BTreeMap<String, u64> = BTreeMap::new();
    let mut truncated = 0u64;
    let mut fallbacks = 0u64;
    let mut errors = 0u64;
    let mut latencies: Vec<u64> = Vec::new();
    let mut misses: Vec<Value> = Vec::new();

    for e in &events {
        let tool = e["tool"].as_str().unwrap_or("?");
        if tool == "miss" {
            misses.push(json!({
                "ts": e["ts"], "note": e["note"], "ref_tool": e["ref_tool"],
            }));
            continue;
        }
        total += 1;
        *by_tool.entry(tool.to_string()).or_insert(0) += 1;
        if let Some(b) = e["backend"].as_str() {
            *by_backend.entry(b.to_string()).or_insert(0) += 1;
        }
        if let Some(r) = e["fallback_reason"].as_str() {
            fallbacks += 1;
            *fallback_reasons.entry(r.to_string()).or_insert(0) += 1;
        }
        if e["truncated"].as_bool().unwrap_or(false) {
            truncated += 1;
        }
        if !e["ok"].as_bool().unwrap_or(true) {
            errors += 1;
            if let Some(c) = e["error_code"].as_str() {
                *error_codes.entry(c.to_string()).or_insert(0) += 1;
            }
        }
        if let Some(l) = e["latency_ms"].as_u64() {
            latencies.push(l);
        }
    }

    let agg = json!({
        "events": total,
        "by_tool": by_tool,
        "by_backend": by_backend,
        "fallbacks": { "count": fallbacks, "reasons": fallback_reasons },
        "truncated": truncated,
        "errors": { "count": errors, "codes": error_codes },
        "latency_ms": { "p50": percentile(&mut latencies, 50), "p95": percentile(&mut latencies, 95) },
        "misses": misses,
    });

    if a.json {
        println!("{}", serde_json::to_string_pretty(&agg).unwrap_or_default());
    } else {
        print_human(&agg);
    }
}

fn percentile(values: &mut [u64], p: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let idx = (p * (values.len() - 1) + 50) / 100;
    Some(values[idx])
}

fn print_human(agg: &Value) {
    println!("navi usage report");
    println!("  events: {}", agg["events"]);
    print_counts("  by tool", &agg["by_tool"]);
    print_counts("  by backend", &agg["by_backend"]);
    println!(
        "  fallbacks: {} | truncated: {} | errors: {}",
        agg["fallbacks"]["count"], agg["truncated"], agg["errors"]["count"]
    );
    print_counts("  fallback reasons", &agg["fallbacks"]["reasons"]);
    print_counts("  error codes", &agg["errors"]["codes"]);
    println!(
        "  latency ms: p50={} p95={}",
        fmt_opt(&agg["latency_ms"]["p50"]),
        fmt_opt(&agg["latency_ms"]["p95"])
    );
    let misses = agg["misses"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("  misses: {misses}");
    if let Some(arr) = agg["misses"].as_array() {
        for m in arr.iter().rev().take(5) {
            println!("    - [{}] {}", trim_q(&m["ref_tool"]), trim_q(&m["note"]));
        }
    }
}

fn print_counts(label: &str, obj: &Value) {
    if let Some(map) = obj.as_object() {
        if map.is_empty() {
            return;
        }
        let parts: Vec<String> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
        println!("{label}: {}", parts.join(" "));
    }
}

fn fmt_opt(v: &Value) -> String {
    if v.is_null() {
        "-".to_string()
    } else {
        v.to_string()
    }
}

fn trim_q(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}
