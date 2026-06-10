# navi

Semantic filesystem operations for AI coding agents. Instead of `ls`/`find`/`grep`/`cat` loops with text output an agent has to re-parse, navi exposes intent-level commands with structured JSON: `locate`, `read`, `edit`, `move`, `remove`.

It wraps the best tool available — [`rq`](https://github.com/dpep/iriq) for symbols, [`ripgrep`](https://github.com/BurntSushi/ripgrep) for content — and falls back to POSIX `grep`/`find`, so it works anywhere and gets sharper where good tools exist.

Status: MVP. Built to be tested against real agent usage, with a built-in feedback loop (`navi report`) to show where it helps and where it falls short.

## Why

- Structured JSON in, structured JSON out — no text re-parsing.
- One ranked, snippet-bearing result instead of N exploratory round-trips.
- Token-aware reads: `--mode outline` turns a big file into a signature skeleton; `--mode symbol` returns one definition.
- Safe writes: every mutation previews a diff first, anchors must be unique, and an optional base hash rejects edits against a changed file.
- Every call carries a `budget` block so the agent knows what it did NOT see.

## Build

```
cargo build --release
# binary at target/release/navi
```

Requires Rust. Optional but recommended on PATH: `rq`, `rg`. `grep`/`find` are the fallbacks.

## Commands

Find content, a symbol, or a filename (ranked, with snippets):

```
navi locate "validateToken" src --match symbol --limit 5
navi locate "TODO" . --match text --lang rust
navi locate widget src --match file
```

Read by intent — full, outline, range, or one symbol:

```
navi read src/auth/token.rs --mode outline
navi read src/auth/token.rs --mode symbol --symbol refresh
navi read src/auth/token.rs --mode range --range 40:80
```

Edit safely — preview a diff, then confirm. Anchors must match exactly once:

```
navi edit src/config.rs --anchor "const TTL = 3600" --replace "const TTL = 7200"
navi edit src/config.rs --anchor "const TTL = 3600" --replace "const TTL = 7200" --confirm
navi edit src/config.rs --range 12:14 --content "new lines" --base-hash sha256:... --confirm
```

Move and remove — previewed, guarded, and reversible. `remove` sends to the trash by default; `restore` reverses any edit/move/remove by its transaction id:

```
navi move old/path.rs new/path.rs --confirm
navi remove tmp/scratch.rs --confirm        # → trash, restorable
navi remove tmp/scratch.rs --purge --confirm # permanent, not restorable
navi restore 8e3392fecabd                    # txn id from any edit/move/remove
```

## Output shape

Every result command prints one envelope:

```json
{
  "tool": "locate",
  "ok": true,
  "backend": "ripgrep",
  "fallback_reason": null,
  "budget": { "returned": 3, "elided": 5, "truncated": true },
  "result": { "hits": [ { "path": "...", "line": 42, "kind": "text", "snippet": "..." } ] },
  "error": null
}
```

Failures keep the same shape with `ok: false` and a structured `error` (`code`, `message`, `details`) — branch on `error.code`, not the message.

## Safety

- `edit`/`move`/`remove` do nothing without `--confirm` — they preview first.
- `edit` anchors must be unique; ambiguous anchors are rejected.
- `edit --base-hash` rejects writes if the file changed since you read it.
- `remove` blocks removals of more than 20 paths without `--force`.
- `move` won't overwrite an existing destination without `--force`.
- `remove` sends to the OS trash (recoverable); `--purge` permanently deletes.
- Every applied mutation journals a before-image and reports a transaction id; `navi restore <txn>` reverses it (rewrite for edit, rename back for move, un-trash for remove).

## Feedback loop

navi logs one event per call (tool, backend, fallbacks, truncation, errors, latency) to `telemetry.jsonl` under its data dir (override with `NAVI_DATA_DIR`; set `NAVI_TRASH_DIR` to redirect removals into a managed holding dir instead of the OS trash — used by tests and sandboxes).

```
navi miss "symbol search missed the obvious definition" --tool locate
navi report          # human summary
navi report --json   # machine aggregation
```

`report` surfaces usage by tool/backend, fallback frequency, truncation rate, error codes, latency, and recent misses — the signal for what to iterate on.

## Roadmap

- Index onboarding for `rq` (symbol search needs a prebuilt index).
- Reference-aware `move`/`remove`.
- `navi mcp` — MCP stdio transport so agents call these as native tools instead of shelling out.
