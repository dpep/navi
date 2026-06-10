//! `install` — register navi's MCP server with Claude Code at user scope, so
//! the result commands show up as native MCP tools in every project. A thin
//! convenience wrapper around `claude mcp add` so setup is one command after a
//! `brew install`. Prints its own message; not part of the result envelope.

use std::process::Command;

const SERVER: &str = "navi";

/// The exact registration command, also surfaced verbatim when `claude` is
/// missing so the user can run it by hand. `--` stops `claude mcp add` from
/// parsing `navi mcp` as its own flags.
const MANUAL: &str = "claude mcp add --scope user navi -- navi mcp";

pub fn run() {
    let args = ["mcp", "add", "--scope", "user", SERVER, "--", "navi", "mcp"];

    match Command::new("claude").args(args).status() {
        Ok(s) if s.success() => {
            println!("navi: registered MCP server '{SERVER}' at user scope.");
            println!("Reconnect MCP (or restart Claude Code) to pick it up.");
        }
        Ok(_) => {
            // claude ran but declined — most often it's already registered.
            eprintln!(
                "navi: `claude mcp add` did not succeed — it may already be registered.\n\
                 Check:  claude mcp get {SERVER}\n\
                 Re-add: claude mcp remove {SERVER} -s user && navi install"
            );
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!(
                "navi: couldn't run `claude` — is Claude Code installed and on PATH?\n\
                 Register manually with:\n  {MANUAL}"
            );
            std::process::exit(1);
        }
    }
}
