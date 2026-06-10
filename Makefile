# navi — build / install / test helpers.
#
#   make            - same as `make help`
#   make build      - dev build      → ./target/debug/navi
#   make release    - optimized build → ./target/release/navi
#   make install    - cargo install --path . (into ~/.cargo/bin)
#   make uninstall  - cargo uninstall navi
#   make test       - cargo test
#   make dogfood    - run navi on its own source (Q=<query>); reproducible
#   make mcp        - run the MCP server over stdio (for manual testing)
#   make lint       - cargo fmt --check && cargo clippy (warnings = errors)
#   make fmt        - cargo fmt
#   make clean      - cargo clean
#
# Note: this machine's cargo came via Homebrew's keg-only rustup and may not be
# on PATH. Either add it or run, e.g.:
#   make build CARGO=/opt/homebrew/opt/rustup/bin/cargo

CARGO ?= cargo
BIN   := navi

.DEFAULT_GOAL := help
.PHONY: help build release install uninstall test dogfood mcp lint fmt clean

help:
	@echo "navi targets:"
	@echo "  make build      dev build      → target/debug/$(BIN)"
	@echo "  make release    optimized build → target/release/$(BIN)"
	@echo "  make install    cargo install --path . (→ ~/.cargo/bin)"
	@echo "  make uninstall  cargo uninstall $(BIN)"
	@echo "  make test       cargo test"
	@echo "  make dogfood    run navi on its own source (Q=<query>, ARGS=<flags>)"
	@echo "  make mcp        run the MCP server over stdio"
	@echo "  make lint       cargo fmt --check && cargo clippy"
	@echo "  make fmt        cargo fmt"
	@echo "  make clean      cargo clean"

build:
	$(CARGO) build

release:
	$(CARGO) build --release

install:
	$(CARGO) install --path .

uninstall:
	$(CARGO) uninstall $(BIN)

test:
	$(CARGO) test

# Dogfood navi on its own source. Reproducible and side-effect free: telemetry
# and journal go to a throwaway dir under target/ (never your real state dir).
#   make dogfood Q=Outcome
#   make dogfood Q=run ARGS="--match symbol --limit 5"
Q            ?= Outcome
ARGS         ?=
DOGFOOD_DATA := $(CURDIR)/target/dogfood-data
dogfood: build
	@NAVI_DATA_DIR="$(DOGFOOD_DATA)" ./target/debug/$(BIN) locate "$(Q)" src $(ARGS)

# Serve the commands as MCP tools over stdio. Reads JSON-RPC on stdin; useful
# for a manual smoke test (e.g. pipe in an `initialize` + `tools/list`).
mcp: build
	@./target/debug/$(BIN) mcp

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

clean:
	$(CARGO) clean
