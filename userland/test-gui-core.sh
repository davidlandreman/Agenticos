#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TOOLCHAIN=$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$SCRIPT_DIR/../rust-toolchain.toml")
TEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/agenticos-gui-core.XXXXXX")
trap 'rm -rf "$TEST_DIR"' EXIT INT TERM

cd "$TEST_DIR"
RUSTUP_TOOLCHAIN="$TOOLCHAIN" cargo test \
    --manifest-path "$SCRIPT_DIR/libs/gui-core/Cargo.toml" \
    -p gui-core
