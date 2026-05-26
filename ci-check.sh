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

echo "==> Running tests (nextest)..."
# nextest gives parallel test execution and per-test timing.
# Config lives in .config/nextest.toml; the `live-eventkit` group runs serial
# so we don't fight over EKEventStore consent.
if ! cargo nextest --version >/dev/null 2>&1; then
    echo "cargo-nextest is required. Install with: cargo install cargo-nextest --locked"
    exit 1
fi
cargo nextest run --all-features --profile "${NEXTEST_PROFILE:-default}"

echo "==> Building docs (warnings = errors)..."
# Mirrors the docs job CI runs — catches broken intra-doc links and
# malformed doc-comments before they hit a PR.
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --quiet

echo "==> All checks passed."
