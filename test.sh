#!/bin/bash
#
# test.sh - Build and run AgenticOS kernel tests
#
# This script builds the kernel with test features enabled and runs it in QEMU.
# Tests execute automatically during kernel boot and QEMU exits with appropriate
# status codes:
#   - Exit code 33 (0x10 << 1 | 1) = All tests passed
#   - Exit code 35 (0x11 << 1 | 1) = Test failure
#
# Usage: ./test.sh

# Build and run tests
echo "Building and running kernel tests..."

# Stage userland apps into host_share/ so test boots see the same artifacts
# as interactive boots. Failures here do not block tests (they use embedded
# fixtures), but we want the staged file present whenever possible.
HOST_SHARE_STAGE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
mkdir -p "$HOST_SHARE_STAGE"
if cargo build --release --manifest-path userland/Cargo.toml; then
    USER_HELLO="userland/target/x86_64-unknown-none/release/hello"
    if [ -f "$USER_HELLO" ]; then
        STAGED="$HOST_SHARE_STAGE/HELLO.ELF"
        TMP="$HOST_SHARE_STAGE/.HELLO.ELF.tmp.$$"
        cp "$USER_HELLO" "$TMP"
        mv -f "$TMP" "$STAGED"
        echo "Staged $STAGED ($(wc -c < "$STAGED" | tr -d ' ') bytes)"
    fi
else
    echo "Warning: userland build failed; continuing without HELLO.ELF"
fi

# Cargo build must be ran twice to make sure image file is built
cargo build --features test
cargo build --features test

# Run with QEMU configured for testing
BIOS_IMAGE="${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}"
HOST_SHARE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
mkdir -p "$HOST_SHARE"
echo "Running tests against: $BIOS_IMAGE"
echo "Host folder: $HOST_SHARE -> /host (read-only)"
qemu-system-x86_64 \
    -drive format=raw,file="$BIOS_IMAGE",if=ide,index=0 \
    -drive file=fat:ro:"$HOST_SHARE",if=ide,index=1,snapshot=on \
    -serial stdio \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -display none \
    -no-reboot

# Check exit code
EXIT_CODE=$?
if [ $EXIT_CODE -eq 33 ]; then  # 0x10 << 1 | 1 = 33
    echo "Tests passed!"
    exit 0
else
    echo "Tests failed! Exit code: $EXIT_CODE"
    exit 1
fi