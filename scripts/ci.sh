#!/usr/bin/env bash
# Vérifications d'intégration : à lancer avant chaque commit et dans la CI.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

if command -v cargo-deny >/dev/null 2>&1; then
    cargo deny check
else
    echo "cargo-deny absent, vérification des licences ignorée (cargo install cargo-deny --locked)" >&2
fi
