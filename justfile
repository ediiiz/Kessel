# Kessel task runner. `just --list` shows every recipe.
#
# The server embeds apps/web/build at compile time, so every recipe that produces a binary builds
# the web UI first.

set shell := ["bash", "-euo", "pipefail", "-c"]

# One build cache for the checkout and all of its git worktrees, kept under budget by
# scripts/cache-guard.sh (limits: KESSEL_CACHE_MAX_GB, KESSEL_MIN_FREE_GB, KESSEL_BUN_CACHE_MAX_GB).
cache_dir := parent_directory(`git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || echo "$PWD/.git"`) / ".cache"
export CARGO_TARGET_DIR := env("CARGO_TARGET_DIR", cache_dir / "target")
export CARGO_INCREMENTAL := env("CARGO_INCREMENTAL", "0")

default:
    @just --list

# Everything CI runs.
ci: fmt-check clippy test web-lint web-check web-test web-build

# Fast local loop: Rust formatting, lints and tests.
check: fmt-check clippy test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

clippy: _cache-guard
    cargo clippy --workspace --all-targets --all-features -- -D warnings

test: _cache-guard
    cargo test --workspace --all-features

# Python tests for the capture/scrub scripts.
test-py:
    pip3 install --quiet --target scripts/.pylibs -r scripts/requirements.txt
    PYTHONPATH="$PWD/scripts/.pylibs:$PWD/scripts:${PYTHONPATH:-}" python3 -m pytest scripts/tests -q

web-install: _cache-guard
    bun install

web-lint: web-install
    bun run --filter '*' lint

web-check: web-install
    bun run --filter '*' check

web-test: web-install
    bun run --filter web test

web-build: web-install
    bun run --filter web build

# Debug build of the daemon, with the TLS-PSK transport.
build: web-build _cache-guard
    cargo build -p server --features tls-psk

# Release build of the daemon, with the TLS-PSK transport.
release: web-build _cache-guard
    cargo build -p server --release --features tls-psk

# Run the daemon with state in ./.dev (never your real data directory).
run: web-build _cache-guard
    mkdir -p .dev
    KESSEL_CONFIG_FILE=.dev/config.json KESSEL_PROFILE_DIR=.dev/profiles KESSEL_TOKEN_FILE=.dev/kessel.token \
        cargo run -p server --features tls-psk

# Run a simulated appliance on 127.0.0.1:8443 (onboard it with the same profile ZIP).
sim profile="crates/hc-profile/tests/fixtures/dishwasher-tls.zip": _cache-guard
    cargo run -p kessel-sim -- --profile {{profile}}

# Browser tests: the daemon against the simulator and an in-process MQTT broker, all on loopback.
# The binaries are copied out first because other builds share the target directory.
e2e *args: web-build _cache-guard
    cargo build -p server --features tls-psk
    cargo build -p kessel-sim
    mkdir -p e2e/.bin
    cp "$CARGO_TARGET_DIR/debug/kessel" "$CARGO_TARGET_DIR/debug/kessel-sim" e2e/.bin/
    cd e2e && bunx playwright install chromium
    cd e2e && KESSEL_BIN="$PWD/.bin/kessel" KESSEL_SIM_BIN="$PWD/.bin/kessel-sim" bunx playwright test {{args}}

# Regenerate the TypeScript API bindings from the Rust types.
types: _cache-guard
    cargo run -p server --example export-types

# Tests against real appliances, one at a time. These can WRITE to your appliances.
live: _cache-guard
    cargo test --workspace --all-features -- --ignored --test-threads=1 --nocapture

# Report duplicate dependency versions (informational).
deps-dupes:
    cargo tree --workspace --all-features --duplicates --edges normal

# Show how big the build caches are, and the limits they are held to.
cache:
    @scripts/cache-guard.sh --report

# Drop the Rust build cache now (the next build starts from scratch).
cache-prune:
    scripts/cache-guard.sh --force

[private]
_cache-guard:
    @scripts/cache-guard.sh

clean:
    cargo clean
    rm -rf apps/web/build node_modules apps/web/node_modules packages/api-types/node_modules .dev
