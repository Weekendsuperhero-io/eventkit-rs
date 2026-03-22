#!/usr/bin/env bash
set -euo pipefail

# Build a universal (arm64 + x86_64) macOS binary.
#
# Usage:
#   ./build-universal.sh           # release build
#   ./build-universal.sh --debug   # debug build

PROFILE="release"
PROFILE_DIR="release"
if [[ "${1:-}" == "--debug" ]]; then
    PROFILE="dev"
    PROFILE_DIR="debug"
fi

RELEASE_FLAG=""
if [[ "$PROFILE" == "release" ]]; then
    RELEASE_FLAG="--release"
fi

echo "==> Building arm64..."
cargo build $RELEASE_FLAG --target aarch64-apple-darwin

echo "==> Building x86_64..."
cargo build $RELEASE_FLAG --target x86_64-apple-darwin

ARM="target/aarch64-apple-darwin/$PROFILE_DIR/eventkit"
X86="target/x86_64-apple-darwin/$PROFILE_DIR/eventkit"
UNIVERSAL="target/eventkit-universal"

echo "==> Creating universal binary..."
lipo -create -output "$UNIVERSAL" "$ARM" "$X86"

file "$UNIVERSAL"
echo "==> Built: $UNIVERSAL"
