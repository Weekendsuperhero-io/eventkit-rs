#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   ./ci-check.sh        # check only (same as CI)
#   ./ci-check.sh --fix  # auto-fix formatting + clippy, then check

FIX=false
if [[ "${1:-}" == "--fix" ]]; then
    FIX=true
fi

if $FIX; then
    echo "==> Fixing formatting..."
    cargo fmt --all

    echo "==> Fixing clippy warnings..."
    cargo clippy --all-targets --all-features --fix --allow-dirty
else
    echo "==> Checking formatting..."
    cargo fmt --all -- --check

    echo "==> Running clippy..."
    cargo clippy --all-targets --all-features -- -D warnings
fi

echo "==> Building..."
cargo build --all-features

echo "==> Running tests..."
cargo test --all-features

echo "==> All checks passed."
