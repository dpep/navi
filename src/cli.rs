//! Command-line surface. Thin: clap parses into these structs, `main`
//! dispatches, the `commands` modules hold the logic.

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(
    name = "navi",
    version,
    about = "Semantic filesystem operations for AI coding agents",
    long_about = "navi exposes intent-level filesystem operations (locate / read / edit / move / remove) \
with structured JSON output, wrapping rq/ripgrep/grep/find where available."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Find code by content, symbol, or filename (ranked, with snippets).
    Locate(LocateArgs),
    /// Read a file by full / outline / range / symbol — token-aware.
    Read(ReadArgs),
    /// Edit a file with a previewed, anchor-unique, hash-guarded change.
    Edit(EditArgs),
    /// Move or rename a file (previewed unless --confirm).
    Move(MoveArgs),
    /// Remove files to the trash (previewed, scope-guarded, restorable).
    Remove(RemoveArgs),
    /// Reverse a journaled edit / move / remove by transaction id.
    Restore(RestoreArgs),
    /// Summarize the telemetry log: usage, fallbacks, misses, latency.
    Report(ReportArgs),
    /// Record that a result was unhelpful — feeds `navi report`.
    Miss(MissArgs),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Locate(_) => "locate",
            Command::Read(_) => "read",
            Command::Edit(_) => "edit",
            Command::Move(_) => "move",
            Command::Remove(_) => "remove",
            Command::Restore(_) => "restore",
            Command::Report(_) => "report",
            Command::Miss(_) => "miss",
        }
    }

    /// A compact, non-sensitive summary of inputs for telemetry.
    pub fn args_summary(&self) -> Value {
        match self {
            Command::Locate(a) => json!({"match": format!("{:?}", a.match_kind), "lang": a.lang, "limit": a.limit}),
            Command::Read(a) => json!({"mode": format!("{:?}", a.mode), "limit": a.limit}),
            Command::Edit(a) => json!({"form": if a.anchor.is_some() {"anchor"} else {"range"}, "confirm": a.confirm}),
            Command::Move(a) => json!({"confirm": a.confirm, "force": a.force}),
            Command::Remove(a) => json!({"count": a.paths.len(), "confirm": a.confirm, "force": a.force, "purge": a.purge}),
            Command::Restore(_) => Value::Null,
            _ => Value::Null,
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum MatchKind {
    /// Content search (rg → grep).
    Text,
    /// Symbol/definition search (rq → text fallback).
    Symbol,
    /// Filename search (rg --files → find).
    File,
}

#[derive(Args)]
pub struct LocateArgs {
    /// What to find.
    pub query: String,
    /// Directories to search (default: current directory).
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    /// What kind of match.
    #[arg(long = "match", value_enum, default_value_t = MatchKind::Text)]
    pub match_kind: MatchKind,
    /// Restrict to a language (e.g. rust, go, python).
    #[arg(long)]
    pub lang: Option<String>,
    /// Restrict symbol kinds (rq): class, module, method, function.
    #[arg(long)]
    pub kind: Option<String>,
    /// Treat query as a literal string, not a regex.
    #[arg(long)]
    pub fixed: bool,
    /// Max results.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ReadMode {
    /// The whole file (up to --limit lines).
    Full,
    /// Signatures only — a navigable skeleton.
    Outline,
    /// A line range, A:B.
    Range,
    /// A single symbol's definition span.
    Symbol,
}

#[derive(Args)]
pub struct ReadArgs {
    /// File to read.
    pub path: String,
    /// How to read it.
    #[arg(long = "mode", value_enum, default_value_t = ReadMode::Full)]
    pub mode: ReadMode,
    /// Line range for mode=range, e.g. 40:80.
    #[arg(long)]
    pub range: Option<String>,
    /// Symbol name for mode=symbol.
    #[arg(long)]
    pub symbol: Option<String>,
    /// Max lines / outline entries returned.
    #[arg(long, default_value_t = 400)]
    pub limit: usize,
}

#[derive(Args)]
pub struct EditArgs {
    /// File to edit.
    pub path: String,
    /// Unique anchor text to replace (use with --replace).
    #[arg(long)]
    pub anchor: Option<String>,
    /// Replacement for the anchor.
    #[arg(long)]
    pub replace: Option<String>,
    /// Line range A:B to replace (use with --content).
    #[arg(long)]
    pub range: Option<String>,
    /// Replacement content for the range.
    #[arg(long)]
    pub content: Option<String>,
    /// Expected current content hash; edit is rejected if the file changed.
    #[arg(long = "base-hash")]
    pub base_hash: Option<String>,
    /// Apply the edit. Without this, navi only previews the diff.
    #[arg(long)]
    pub confirm: bool,
}

#[derive(Args)]
pub struct MoveArgs {
    /// Source path.
    pub from: String,
    /// Destination path.
    pub to: String,
    /// Apply the move. Without this, navi only previews.
    #[arg(long)]
    pub confirm: bool,
    /// Overwrite the destination if it exists.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct RemoveArgs {
    /// Paths to remove.
    #[arg(required = true)]
    pub paths: Vec<String>,
    /// Apply the removal. Without this, navi only previews.
    #[arg(long)]
    pub confirm: bool,
    /// Bypass the scope guard for large removals.
    #[arg(long)]
    pub force: bool,
    /// Permanently delete instead of moving to the trash (not restorable).
    #[arg(long)]
    pub purge: bool,
}

#[derive(Args)]
pub struct RestoreArgs {
    /// Transaction id reported by a prior edit / move / remove.
    pub txn: String,
}

#[derive(Args)]
pub struct ReportArgs {
    /// Emit the aggregation as JSON instead of a human summary.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MissArgs {
    /// What went wrong / what you expected.
    pub note: String,
    /// Which tool produced the unhelpful result.
    #[arg(long = "tool")]
    pub ref_tool: Option<String>,
}
