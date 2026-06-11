//! `info` — repo orientation. One call answers "what is this project and how do
//! I build/test it", so a fresh agent doesn't burn turns probing manifests and
//! `git` by hand. Pure filesystem plus a couple of read-only `git` queries; no
//! search backend. Detection is shallow (root-level manifests only) and honest:
//! the commands are the ecosystem's canonical ones, not proof they're wired up.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::backend::{self, Backends};
use crate::cli::InfoArgs;
use crate::error::Result;
use crate::output::Outcome;

pub fn run(a: &InfoArgs) -> Result<Outcome> {
    let dir = a.path.as_deref().unwrap_or(".");
    let (vcs, git_root, branch) = git_facts(dir);
    let root = git_root.unwrap_or_else(|| {
        fs::canonicalize(dir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| dir.to_string())
    });

    let projects = detect_projects(Path::new(dir));
    let mut languages: Vec<String> = Vec::new();
    for p in &projects {
        if let Some(k) = p["kind"].as_str() {
            if !languages.iter().any(|l| l == k) {
                languages.push(k.to_string());
            }
        }
    }

    let b = Backends::detect();
    Ok(Outcome::new(json!({
        "root": root,
        "vcs": vcs,
        "branch": branch,
        "languages": languages,
        "projects": projects,
        "scripts": read_scripts(Path::new(dir)),
        "make_targets": read_make_targets(Path::new(dir)),
        "backends": {
            "rq": b.rq,
            "rg": b.rg,
            "fd": b.fd.is_some(),
            "grep": b.grep,
            "find": b.find,
        },
    })))
}

/// VCS facts via read-only `git`. Returns (vcs, repo root, branch). All None
/// when git is absent or `dir` isn't a working tree; an empty branch (detached
/// HEAD) collapses to None.
fn git_facts(dir: &str) -> (Option<&'static str>, Option<String>, Option<String>) {
    let Some(root) = git_out(dir, &["rev-parse", "--show-toplevel"]) else {
        return (None, None, None);
    };
    let branch = git_out(dir, &["branch", "--show-current"]);
    (Some("git"), Some(root), branch)
}

fn git_out(dir: &str, args: &[&str]) -> Option<String> {
    let mut full = vec!["-C", dir];
    full.extend_from_slice(args);
    let out = backend::run("git", &full).ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// One project entry per recognized root-level manifest. Commands are the
/// ecosystem's canonical build/test/lint invocations.
fn detect_projects(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    let has = |name: &str| dir.join(name).is_file();

    if has("Cargo.toml") {
        out.push(project(
            "rust",
            "Cargo.toml",
            Some("cargo build"),
            Some("cargo test"),
            Some("cargo clippy"),
        ));
    }
    if has("go.mod") {
        out.push(project(
            "go",
            "go.mod",
            Some("go build ./..."),
            Some("go test ./..."),
            Some("go vet ./..."),
        ));
    }
    if has("package.json") {
        let pm = node_pm(dir);
        let run = |s: &str| {
            if pm == "npm" {
                format!("npm run {s}")
            } else {
                format!("{pm} {s}")
            }
        };
        let test = if pm == "npm" {
            "npm test".to_string()
        } else {
            format!("{pm} test")
        };
        out.push(project(
            "node",
            "package.json",
            Some(&run("build")),
            Some(&test),
            Some(&run("lint")),
        ));
    }
    if has("Gemfile") {
        out.push(project(
            "ruby",
            "Gemfile",
            None,
            Some("bundle exec rspec"),
            Some("bundle exec rubocop"),
        ));
    }
    if has("pyproject.toml") {
        out.push(project(
            "python",
            "pyproject.toml",
            None,
            Some("pytest"),
            Some("ruff check"),
        ));
    } else if has("setup.py") || has("requirements.txt") {
        let m = if has("setup.py") {
            "setup.py"
        } else {
            "requirements.txt"
        };
        out.push(project("python", m, None, Some("pytest"), None));
    }
    if has("CMakeLists.txt") {
        out.push(project(
            "cpp",
            "CMakeLists.txt",
            Some("cmake --build build"),
            Some("ctest"),
            None,
        ));
    }
    out
}

fn project(
    kind: &str,
    manifest: &str,
    build: Option<&str>,
    test: Option<&str>,
    lint: Option<&str>,
) -> Value {
    json!({ "kind": kind, "manifest": manifest, "build": build, "test": test, "lint": lint })
}

/// The Node package manager, inferred from the lockfile (defaults to npm).
fn node_pm(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if dir.join("yarn.lock").is_file() {
        "yarn"
    } else if dir.join("bun.lockb").is_file() {
        "bun"
    } else {
        "npm"
    }
}

/// The `scripts` block from package.json verbatim, or null.
fn read_scripts(dir: &Path) -> Value {
    fs::read_to_string(dir.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("scripts").cloned())
        .unwrap_or(Value::Null)
}

/// Top-level Makefile targets (column-0 `name:` rules), capped, or null. Skips
/// comments, recipe lines, `.PHONY`-style dot rules, and variable assignments.
fn read_make_targets(dir: &Path) -> Value {
    let Ok(txt) = fs::read_to_string(dir.join("Makefile")) else {
        return Value::Null;
    };
    let mut targets: Vec<String> = Vec::new();
    for line in txt.lines() {
        if line.starts_with([' ', '\t', '#', '.']) {
            continue;
        }
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.trim_start().starts_with('=') {
            continue; // VAR := ...
        }
        let name = name.trim();
        let ok = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || "-_./".contains(c));
        if ok && !targets.iter().any(|t| t == name) {
            targets.push(name.to_string());
        }
        if targets.len() >= 30 {
            break;
        }
    }
    if targets.is_empty() {
        Value::Null
    } else {
        json!(targets)
    }
}
