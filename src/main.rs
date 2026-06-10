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
mod output;
mod paths;
mod telemetry;
mod trashbin;
mod util;

use std::time::Instant;

use clap::Parser;

use cli::{Cli, Command};
use error::Result;
use output::{envelope_err, envelope_ok, Outcome};

fn main() {
    let cli = Cli::parse();

    // report/miss are meta commands: they print directly and don't go through
    // the result envelope or generic telemetry (miss writes its own event).
    match &cli.command {
        Command::Report(a) => return commands::report::run(a),
        Command::Miss(a) => return commands::report::run_miss(a),
        _ => {}
    }

    let tool = cli.command.name();
    let start = Instant::now();
    let res = dispatch(&cli.command);
    let latency = start.elapsed().as_millis();

    match res {
        Ok(o) => {
            let env = envelope_ok(tool, &o);
            let s = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());
            println!("{s}");
            telemetry::log(&telemetry::Event {
                tool,
                ok: true,
                backend: o.backend.as_deref(),
                fallback_reason: o.fallback_reason.as_deref(),
                result_count: o.returned,
                bytes_out: s.len(),
                latency_ms: latency,
                truncated: o.truncated,
                error_code: None,
                args: cli.command.args_summary(),
            });
        }
        Err(e) => {
            let env = envelope_err(tool, e.as_json());
            let s = serde_json::to_string_pretty(&env).unwrap_or_else(|_| "{}".into());
            println!("{s}");
            telemetry::log(&telemetry::Event {
                tool,
                ok: false,
                backend: None,
                fallback_reason: None,
                result_count: 0,
                bytes_out: s.len(),
                latency_ms: latency,
                truncated: false,
                error_code: Some(&e.code),
                args: cli.command.args_summary(),
            });
            std::process::exit(1);
        }
    }
}

fn dispatch(cmd: &Command) -> Result<Outcome> {
    match cmd {
        Command::Locate(a) => commands::locate::run(a),
        Command::Read(a) => commands::read::run(a),
        Command::Edit(a) => commands::edit::run(a),
        Command::Move(a) => commands::fsops::run_move(a),
        Command::Remove(a) => commands::fsops::run_remove(a),
        Command::Restore(a) => commands::restore::run(a),
        Command::Report(_) | Command::Miss(_) => unreachable!("handled before dispatch"),
    }
}
