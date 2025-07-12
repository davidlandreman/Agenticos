#!/bin/bash

# Default values
CLEAN=false
RUN_QEMU=true
HELP=false

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -c|--clean)
            CLEAN=true
            shift
            ;;
        -n|--no-qemu)
            RUN_QEMU=false
            shift
            ;;
        -h|--help)
            HELP=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            HELP=true
            shift
            ;;
    esac
done

# Show help if requested
if [ "$HELP" = true ]; then
    echo "Usage: $0 [OPTIONS]"
    echo ""
    echo "Build AgenticOS kernel and create bootloader images"
    echo ""
    echo "Options:"
    echo "  -c, --clean     Clean build artifacts before building"
    echo "  -n, --no-qemu   Build only, don't run QEMU"
    echo "  -h, --help      Show this help message"
    echo ""
    echo "Default: Build kernel, create images, and run in QEMU"
    exit 0
fi

# Clean if requested
if [ "$CLEAN" = true ]; then
    echo "🧹 Cleaning build artifacts..."
    cargo clean
    rm -rf target/bootloader
fi

# First build pass - compile the kernel
echo "🔨 Building kernel (pass 1/2)..."
cargo build
if [ $? -ne 0 ]; then
    echo "❌ Build failed!"
    exit 1
fi

# Second build pass - create disk images
echo "💾 Creating disk images (pass 2/2)..."
cargo build
if [ $? -ne 0 ]; then
    echo "❌ Image creation failed!"
    exit 1
fi

echo "✅ Build complete!"

# Run in QEMU if requested
if [ "$RUN_QEMU" = true ]; then
    echo "🚀 Launching QEMU..."
    qemu-system-x86_64 -drive format=raw,file=target/bootloader/bios.img -serial stdio -no-reboot -no-shutdown
fi