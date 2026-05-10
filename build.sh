#!/bin/bash

# Default values
CLEAN=false
RUN_QEMU=true
HELP=false
DEBUG=false

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
        -d|--debug)
            DEBUG=true
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
    echo "  -d, --debug     Build in debug mode (larger kernel, slower boot)"
    echo "  -n, --no-qemu   Build only, don't run QEMU"
    echo "  -h, --help      Show this help message"
    echo ""
    echo "Default: Build in release mode, create images, and run in QEMU"
    exit 0
fi

# Clean if requested
if [ "$CLEAN" = true ]; then
    echo "🧹 Cleaning build artifacts..."
    cargo clean
    rm -rf target/bootloader
    cargo clean --manifest-path userland/Cargo.toml 2>/dev/null || true
fi

# Stage userland apps into host_share/ so the guest can `run /HOST/<NAME>.ELF`.
# Done before the kernel build so a stale staged file never lingers when the
# userland build fails. Failure here is a warning — kernel tests use embedded
# fixtures, so they don't strictly need host_share/HELLO.ELF, and we still
# want the kernel build to proceed for iteration.
echo "🛠  Building userland (release)..."
HOST_SHARE_STAGE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
mkdir -p "$HOST_SHARE_STAGE"

# U4: stage minimal /etc/{passwd,group} so musl's getpwuid_r returns the
# root entry zsh needs for $HOME/$USER/$SHELL. The kernel-side path
# rewriter in src/userland/path.rs::apply_fs_rewrite maps
# /etc/passwd -> /host/etc/passwd; FAT's case-insensitive subdir walker
# resolves that to host_share/ETC/PASSWD. Single-line root entry is the
# musl-getpwuid_r minimum (seven colon-delimited fields).
mkdir -p "$HOST_SHARE_STAGE/ETC"
printf 'root:x:0:0::/root:/bin/zsh\n' > "$HOST_SHARE_STAGE/ETC/PASSWD"
printf 'root:x:0:\n'                  > "$HOST_SHARE_STAGE/ETC/GROUP"
if cargo build --release --manifest-path userland/Cargo.toml; then
    USER_HELLO="userland/target/x86_64-unknown-none/release/hello"
    if [ -f "$USER_HELLO" ]; then
        STAGED="$HOST_SHARE_STAGE/HELLO.ELF"
        TMP="$HOST_SHARE_STAGE/.HELLO.ELF.tmp.$$"
        cp "$USER_HELLO" "$TMP"
        mv -f "$TMP" "$STAGED"
        SIZE=$(wc -c < "$STAGED" | tr -d ' ')
        echo "📦 Staged $STAGED ($SIZE bytes)"
    else
        echo "⚠️  Userland build succeeded but $USER_HELLO not found; skipping stage"
    fi
else
    echo "⚠️  Userland build failed; continuing without HELLO.ELF (kernel tests use embedded fixtures)"
fi

# C++ userland app — built with the host's musl-based static C++ cross
# compiler if available. Mirrors the rust userland's soft-fail pattern: a
# missing toolchain warns + skips so the kernel build still proceeds.
# Install hint for macOS / Homebrew: `brew install x86_64-linux-musl-cross`
# or build via musl-cross-make.
echo "🛠  Building C++ userland (HELLOCPP)..."
MUSL_GXX="${MUSL_GXX:-x86_64-linux-musl-g++}"
if command -v "$MUSL_GXX" >/dev/null 2>&1; then
    if make -C userland/apps/hello-cpp MUSL_GXX="$MUSL_GXX"; then
        CPP_BIN="userland/apps/hello-cpp/build/hello-cpp"
        if [ -f "$CPP_BIN" ]; then
            # Verify ET_EXEC. Some toolchains default to PIE even with
            # -no-pie; the loader rejects ET_DYN, so we'd rather fail
            # here with a clear message than at run-time inside the guest.
            # macOS doesn't ship a host `readelf`; derive it from the
            # cross-toolchain (e.g. x86_64-linux-musl-g++ → x86_64-linux-musl-readelf)
            # and fall back to host `readelf` for Linux build hosts.
            MUSL_READELF="${MUSL_GXX%g++}readelf"
            command -v "$MUSL_READELF" >/dev/null 2>&1 || MUSL_READELF=readelf
            ET_TYPE=$("$MUSL_READELF" -h "$CPP_BIN" 2>/dev/null | awk '/Type:/ { print $2 }')
            if [ "$ET_TYPE" != "EXEC" ]; then
                echo "❌ $CPP_BIN is $ET_TYPE, expected EXEC. Toolchain likely defaults to PIE."
                echo "   Try: $MUSL_GXX -static -no-pie -fno-pie ..."
                exit 1
            fi
            STAGED="$HOST_SHARE_STAGE/HELLOCPP.ELF"
            TMP="$HOST_SHARE_STAGE/.HELLOCPP.ELF.tmp.$$"
            cp "$CPP_BIN" "$TMP"
            mv -f "$TMP" "$STAGED"
            SIZE=$(wc -c < "$STAGED" | tr -d ' ')
            echo "📦 Staged $STAGED ($SIZE bytes)"
        else
            echo "⚠️  C++ build succeeded but $CPP_BIN not found; skipping stage"
        fi
    else
        echo "❌ C++ userland build failed."
        exit 1
    fi
else
    echo "⚠️  $MUSL_GXX not found on PATH — skipping HELLOCPP.ELF."
    echo "   Install hint (macOS): brew install x86_64-linux-musl-cross"
    echo "   Override the binary name: MUSL_GXX=<path-to-musl-g++> ./build.sh"
fi

# zsh — first real userland shell. Built statically against musl + a
# vendored ncurses-widec by userland/apps/zsh/Makefile. Same probe +
# readelf hard-fail pattern as HELLOCPP. The Makefile fetches sources
# from upstream on first build and caches them under
# userland/apps/zsh/build/tarballs/, so subsequent builds are offline.
echo "🛠  Building zsh userland (ZSH)..."
MUSL_CC="${MUSL_CC:-x86_64-linux-musl-gcc}"
if command -v "$MUSL_CC" >/dev/null 2>&1; then
    if make -C userland/apps/zsh MUSL_CC="$MUSL_CC"; then
        ZSH_BIN="userland/apps/zsh/build/zsh"
        if [ -f "$ZSH_BIN" ]; then
            MUSL_READELF="${MUSL_CC%gcc}readelf"
            command -v "$MUSL_READELF" >/dev/null 2>&1 || MUSL_READELF=readelf
            ET_TYPE=$("$MUSL_READELF" -h "$ZSH_BIN" 2>/dev/null | awk '/Type:/ { print $2 }')
            if [ "$ET_TYPE" != "EXEC" ]; then
                echo "❌ $ZSH_BIN is $ET_TYPE, expected EXEC. Toolchain likely defaults to PIE."
                echo "   Try: $MUSL_CC -static -no-pie -fno-pie ..."
                exit 1
            fi
            STAGED="$HOST_SHARE_STAGE/ZSH.ELF"
            TMP="$HOST_SHARE_STAGE/.ZSH.ELF.tmp.$$"
            cp "$ZSH_BIN" "$TMP"
            mv -f "$TMP" "$STAGED"
            SIZE=$(wc -c < "$STAGED" | tr -d ' ')
            echo "📦 Staged $STAGED ($SIZE bytes)"
        else
            echo "⚠️  zsh build succeeded but $ZSH_BIN not found; skipping stage"
        fi
    else
        echo "❌ zsh userland build failed."
        exit 1
    fi
else
    echo "⚠️  $MUSL_CC not found on PATH — skipping ZSH.ELF."
    echo "   Install hint (macOS): brew install x86_64-linux-musl-cross"
    echo "   Override the binary name: MUSL_CC=<path-to-musl-gcc> ./build.sh"
fi

# Determine build flags
BUILD_FLAGS="--release"
if [ "$DEBUG" = true ]; then
    BUILD_FLAGS=""
    echo "🐛 Building in DEBUG mode"
else
    echo "📦 Building in RELEASE mode"
fi

# First build pass - compile the kernel
echo "🔨 Building kernel (pass 1/2)..."
cargo build $BUILD_FLAGS
if [ $? -ne 0 ]; then
    echo "❌ Build failed!"
    exit 1
fi

# Second build pass - create disk images
echo "💾 Creating disk images (pass 2/2)..."
cargo build $BUILD_FLAGS
if [ $? -ne 0 ]; then
    echo "❌ Image creation failed!"
    exit 1
fi

echo "✅ Build complete!"

# Run in QEMU if requested
if [ "$RUN_QEMU" = true ]; then
    BIOS_IMAGE="${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}"
    HOST_SHARE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
    RPC_SOCK="${AGENTICOS_RPC_SOCK:-/tmp/agenticos-rpc.sock}"
    mkdir -p "$HOST_SHARE"
    # Stale socket from a previous run will block QEMU's listener.
    rm -f "$RPC_SOCK"
    echo "🚀 Launching QEMU with image: $BIOS_IMAGE"
    echo "📂 Mounting host folder: $HOST_SHARE -> /host (read-only)"
    echo "🔌 MCP RPC chardev socket: $RPC_SOCK (chmod 0600 once QEMU creates it)"
    # Restrict the socket to the launching user as soon as QEMU creates it.
    # Backgrounded so it races QEMU startup; if the socket isn't there yet,
    # chmod will fail silently — that's fine, we retry until QEMU is up.
    (
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            if [ -S "$RPC_SOCK" ]; then
                chmod 0600 "$RPC_SOCK" && exit 0
            fi
            sleep 0.2
        done
    ) &
    qemu-system-x86_64 \
        -drive format=raw,file="$BIOS_IMAGE",if=ide,index=0 \
        -drive file=fat:ro:"$HOST_SHARE",if=ide,index=1,snapshot=on \
        -serial stdio \
        -chardev socket,id=rpc,path="$RPC_SOCK",server=on,wait=off \
        -serial chardev:rpc \
        -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -device virtio-tablet-pci \
        -m 128M
fi