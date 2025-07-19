#!/bin/bash

# Build and run tests
echo "Building and running kernel tests..."
cargo build --features test

# Run with QEMU configured for testing
qemu-system-x86_64 \
    -drive format=raw,file=target/bootloader/bios.img \
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