# CLAUDE.md

Guidance for Claude Code when working in this repo.

## What navi is

navi is a CLI of semantic filesystem operations for AI coding agents. It exposes intent-level commands — `locate`, `read`, `edit`, `move`, `remove` — with structured JSON I/O, instead of making an agent run `ls`/`find`/`grep`/`cat` loops and re-parse text. It wraps purpose-built tools (`rq` for symbols, `ripgrep` for content) where present and falls back to POSIX `grep`/`find` so it works on a bare machine.

This is an MVP. The point right now is to get the surface in front of real agent usage and let the telemetry tell us where it helps and where it falls short. Prefer shipping a thin, observable slice over a complete one.

## Architecture

The crate is deliberately thin-at-the-edges:

- `src/main.rs` — parse args, dispatch to a command, wrap the result in the shared envelope, record one telemetry event. No logic.
- `src/cli.rs` — clap structs/enums only. The whole CLI surface lives here.
- `src/commands/` — one module per command; this is where the logic is. `locate`, `read`, `edit`, `fsops` (move + remove), `restore`, `report` (report + miss).
- Sibling modules are I/O-light helpers: `backend` (tool detection + spawning), `output` (the envelope), `telemetry` (the feedback log), `journal` (before-images + restore substrate), `trashbin` (OS/managed trash + the move primitive restore uses), `paths` (state locations), `util` (hashing, lang maps, range parsing), `error` (structured errors).

Keep `main.rs` and `cli.rs` boring. New behavior goes in a `commands/` module plus a helper module if it's reusable.

## The response envelope (contract)

Every result-bearing command (`locate`/`read`/`edit`/`move`/`remove`) prints ONE JSON object with this exact outer shape, success or failure:

```json
{ "tool": "...", "ok": true|false, "backend": "rq|ripgrep|grep|find|null",
  "fallback_reason": null|"...", "budget": {"returned":N,"elided":M,"truncated":bool},
  "result": { ... }|null, "error": null|{"code","message","details"} }
```

Agents branch on `ok` and `error.code`, not on message text. `budget` always tells them what they did NOT see — never silently truncate without reflecting it there. `report` and `miss` are meta commands and intentionally print their own shapes, not this envelope.

## Backend strategy

Detection is per-invocation via `which` (`backend::Backends::detect`). Preference order:

- content search (`locate --match text`): `rg` → `grep`
- symbol search (`locate --match symbol`): `rq` → literal text search of the name
- filename search (`locate --match file`): `rg --files` → `find`
- `read`/`edit`/`move`/`remove`: pure std::fs, no external tool

When a preferred backend is missing and navi falls back, it MUST set `fallback_reason` (surfaced in the envelope and telemetry) so the gap is measurable. A nonzero exit from `rg`/`grep` means "no matches", NOT an error — only a spawn failure or `rg` exit code 2 is a real error.

Note: `rq` searches a prebuilt index (`rq --index <dir>`). An un-indexed repo yields empty symbol results — a known onboarding gap, not a bug.

## Safety model

Mutations are preview-first and reversible-ish:

- `edit`/`move`/`remove` preview by default; nothing touches disk without `--confirm`.
- `edit` anchors must match exactly once — 0 or >1 is an error (`anchor_not_found` / `anchor_ambiguous`). Never guess a location.
- `edit --base-hash <h>` rejects the write if the file changed since it was read (`stale_base`). The hash comes from a prior `read`.
- `remove` refuses more than `SCOPE_THRESHOLD` (20) paths without `--force` (`scope_exceeded`).
- `move` refuses to clobber an existing destination without `--force` (`destination_exists`).
- `remove` sends to the trash (recoverable) by default; `--purge` permanently deletes and is journaled as not-restorable.
- Every applied mutation writes a journal entry (`paths::journal_dir()`) and reports a `transaction_id`. `navi restore <txn>` reverses it: edit → rewrite before-content, move → rename back, remove → move the item out of the trash. The journal entry shape per op is documented at the top of `journal.rs`.

Trash mechanism (`trashbin`): default is the OS trash via the `trash` crate — on macOS forced to `DeleteMethod::NsFileManager` because the crate's default Finder/AppleScript path times out headless and needs automation permission. We capture where the item landed so restore is navi's own move and doesn't need macOS's absent restore API:

- managed mode (`NAVI_TRASH_DIR` set): we move the item into our own holding dir and know the exact destination. Deterministic and hermetic; tests always set it so they never touch the real `~/.Trash`.
- cross-volume: the OS trash lands a file in its own volume's `.Trashes`, which our `~/.Trash` capture can't see. We detect this up front (an `st_dev` compare against home) and route those removals into a managed holding dir under the data dir instead, so they stay restorable — no per-volume `.Trashes` diffing.
- same-volume OS trash: a before/after diff of `~/.Trash`, filtered by name resemblance (`pick_trashed`/`resembles`) so a file that races into the trash concurrently is never mistaken for ours. A missed capture is recoverable (journaled `null`); a wrong one wouldn't be — so we bias to None.

Reference checks (refuse/warn when removing or moving a still-imported file) are intentionally OUT of this MVP.

## Telemetry / feedback loop (the reason this MVP exists)

Every result command appends one event to `paths::telemetry_log()` (`{NAVI_DATA_DIR or platform data dir}/navi/telemetry.jsonl`): tool, ok, backend, fallback_reason, result_count, bytes_out, latency_ms, truncated, error_code, args summary. Logging is best-effort and must never fail a command.

- `navi miss "<note>" [--tool <t>]` records an explicit "this was unhelpful" signal.
- `navi report [--json]` aggregates the log: usage by tool/backend, fallback frequency + reasons, truncation rate, error codes, latency p50/p95, and recent misses.

When extending navi, preserve this loop. If you add a capability, make sure its successes AND its shortfalls show up in `report`. That's how we'll know what to build next.

## Build / test / run

```
cargo build              # debug binary at target/debug/navi
cargo test               # hermetic e2e suite in tests/cli.rs
cargo build --release    # stripped, LTO'd binary
```

`tests/cli.rs` drives the real binary against temp files with an isolated `NAVI_DATA_DIR`, so telemetry/journal never leak between tests or onto the dev machine. Set `NAVI_DATA_DIR` to redirect all state — always do this in tests and scratch runs.

## Adding a command

1. Add a variant + `Args` struct to `cli.rs`, and a name/args_summary arm.
2. Add a `commands/<name>.rs` with `pub fn run(a: &Args) -> Result<Outcome>` (or a meta command that prints directly, like `report`).
3. Wire it in `main::dispatch`.
4. Return an `Outcome` with an honest `budget`; raise `NaviError` with a stable `code` for failures.
5. Add hermetic tests to `tests/cli.rs`.

## Roadmap / known gaps

- `rq` requires a prebuilt index; onboarding should index or detect-and-prompt.
- Reference-aware `move`/`remove`.
- MCP transport: a `navi mcp` stdio subcommand exposing the same commands as native tools, so agents don't shell out. The command logic is transport-agnostic by design.
