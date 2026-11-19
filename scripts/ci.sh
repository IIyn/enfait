#!/usr/bin/env bash
# Integration checks: run before every commit and in CI.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check
else
    echo "cargo-deny missing, licence check skipped (cargo install cargo-deny --locked)" >&2
fi
