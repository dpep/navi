//! Backend detection. navi prefers purpose-built tools (`rq` for symbols, `rg`
//! for content, `fd` for filenames) and falls back to POSIX `grep`/`find` when
//! they're absent, so it works on a bare machine but gets sharper where the
//! good tools exist.

use std::process::{Command, Output};

use crate::error::{NaviError, Result};

#[derive(Debug, Clone, Copy)]
pub struct Backends {
    pub rq: bool,
    pub rg: bool,
    /// The resolved `fd` binary name (`fd`, or `fdfind` on Debian/Ubuntu), or
    /// `None` if neither is installed.
    pub fd: Option<&'static str>,
    pub grep: bool,
    pub find: bool,
}

impl Backends {
    pub fn detect() -> Self {
        Backends {
            rq: have("rq"),
            rg: have("rg"),
            fd: have_any(&["fd", "fdfind"]),
            grep: have("grep"),
            find: have("find"),
        }
    }
}

fn have(bin: &str) -> bool {
    which::which(bin).is_ok()
}

/// The first of `names` found on PATH, kept as a stable name for spawning.
fn have_any(names: &[&'static str]) -> Option<&'static str> {
    names.iter().copied().find(|n| have(n))
}

/// Run a tool and return its raw output. A nonzero exit is NOT an error here —
/// `rg`/`grep` exit 1 on "no matches", which callers treat as an empty result.
/// Only a failure to spawn (missing binary, OS error) is an error.
pub fn run(bin: &str, args: &[&str]) -> Result<Output> {
    Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| NaviError::new("backend_spawn_failed", format!("could not run {bin}: {e}")))
}
