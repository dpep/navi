//! navi — semantic filesystem operations for AI coding agents.
//!
//! `main` is thin: parse args, dispatch to a command, wrap the result in the
//! shared envelope, and record one telemetry event. All logic lives in the
//! `commands` modules; all I/O-free helpers in the sibling modules.

mod backend;
mod cli;
mod commands;
mod error;
mod journal;
mod mcp;
mod output;
mod paths;
mod telemetry;
mod trashbin;
mod util;

use std::time::Instant;

use clap::Parser;
use serde_json::Value;

use cli::{Cli, Command};
use error::Result;
use output::{envelope_err, envelope_ok, Outcome};

fn main() {
    let cli = Cli::parse();

    // Meta commands print their own shape and skip the result envelope: report/
    // miss aggregate or record the feedback loop; mcp serves it over stdio.
    match &cli.command {
        Command::Report(a) => return commands::report::run(a),
        Command::Miss(a) => return commands::report::run_miss(a),
        Command::Mcp => return mcp::run(),
        Command::Install => return commands::install::run(),
        _ => {}
    }

    let (env, ok) = execute(&cli.command);
    let s = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());
    println!("{s}");
    if !ok {
        std::process::exit(1);
    }
}

/// Run a result-bearing command: dispatch it, wrap the outcome in the shared
/// envelope, and record one telemetry event. Returns the envelope plus whether
/// it succeeded. Shared by the CLI and the MCP transport so both feed the same
/// feedback loop; only the framing around the envelope differs per transport.
pub fn execute(cmd: &Command) -> (Value, bool) {
    let tool = cmd.name();
    let start = Instant::now();
    let res = dispatch(cmd);
    let latency = start.elapsed().as_millis();

    match res {
        Ok(o) => {
            let env = envelope_ok(tool, &o);
            telemetry::log(&telemetry::Event {
                tool,
                ok: true,
                backend: o.backend.as_deref(),
                fallback_reason: o.fallback_reason.as_deref(),
                result_count: o.returned,
                bytes_out: serde_json::to_string(&env).map_or(0, |s| s.len()),
                latency_ms: latency,
                truncated: o.truncated,
                error_code: None,
                args: cmd.args_summary(),
            });
            (env, true)
        }
        Err(e) => {
            let env = envelope_err(tool, e.as_json());
            telemetry::log(&telemetry::Event {
                tool,
                ok: false,
                backend: None,
                fallback_reason: None,
                result_count: 0,
                bytes_out: serde_json::to_string(&env).map_or(0, |s| s.len()),
                latency_ms: latency,
                truncated: false,
                error_code: Some(&e.code),
                args: cmd.args_summary(),
            });
            (env, false)
        }
    }
}

fn dispatch(cmd: &Command) -> Result<Outcome> {
    match cmd {
        Command::Locate(a) => commands::locate::run(a),
        Command::Read(a) => commands::read::run(a),
        Command::Edit(a) => commands::edit::run(a),
        Command::Info(a) => commands::info::run(a),
        Command::Move(a) => commands::fsops::run_move(a),
        Command::Remove(a) => commands::fsops::run_remove(a),
        Command::Undo(a) => commands::undo::run(a),
        Command::Report(_) | Command::Miss(_) | Command::Mcp | Command::Install => {
            unreachable!("handled before dispatch")
        }
    }
}
