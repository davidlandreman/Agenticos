#!/bin/bash

# Default values
CLEAN=false
RUN_QEMU=true
HELP=false
DEBUG=false
REBUILD_USERLAND_FLAG=0

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
        --rebuild-userland)
            REBUILD_USERLAND_FLAG=1
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
    echo "  -c, --clean             Clean build artifacts before building"
    echo "  -d, --debug             Build in debug mode (larger kernel, slower boot)"
    echo "  -n, --no-qemu           Build only, don't run QEMU"
    echo "      --rebuild-userland  Force rebuild of prebuilt-managed userland apps"
    echo "                          (default: copy from userland/prebuilt/ when present)"
    echo "                          Equivalent: REBUILD_USERLAND=1 env. Per-app:"
    echo "                          REBUILD_ZSH=1."
    echo "                          QEMU RAM defaults to 2G; override with"
    echo "                          AGENTICOS_QEMU_MEMORY (for example 4G)."
    echo "                          Rendering: AGENTICOS_COMPOSITOR=legacy|retained|gpu|auto"
    echo "                          AGENTICOS_GPU_STRICT=1 refuses GPU fallback."
    echo "                          AGENTICOS_THEME=classic|aero|auto (default auto)."
    echo "                          AGENTICOS_QEMU_2D=on forces plain VirtIO 2D scanout;"
    echo "                          auto (default) avoids its black Cocoa window on macOS."
    echo "                          AGENTICOS_QEMU_BIN selects one exact QEMU binary;"
    echo "                          AGENTICOS_QEMU_GL=es|core selects Cocoa GL mode."
    echo "                          macOS window: cocoa zoom-to-fit is on by default so"
    echo "                          the 1280x720 guest can be resized/enlarged on Retina."
    echo "                          AGENTICOS_QEMU_ZOOM=off disables it; AGENTICOS_QEMU_FULLSCREEN=1"
    echo "                          starts maximized to the display. AGENTICOS_QEMU_SCALE=N"
    echo "                          enlarges the window to N times its native size on open"
    echo "                          (default 4; needs Accessibility permission for the terminal)."
    echo "                          Networking defaults on; set"
    echo "                          AGENTICOS_NETWORK=off to pass -nic none."
    echo "  -h, --help              Show this help message"
    echo ""
    echo "Default: Build in release mode, create images, and run in QEMU"
    exit 0
fi

# Translate the CLI flag into the env contract that prebuilt-lib.sh
# consumes. Honor an existing REBUILD_USERLAND=1 from the caller too.
if [ "$REBUILD_USERLAND_FLAG" = "1" ]; then
    REBUILD_USERLAND=1
fi
REBUILD_USERLAND="${REBUILD_USERLAND:-0}"
export REBUILD_USERLAND

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

# Stage the read-only zsh configuration source tree. The kernel imports it
# into its managed runtime /etc after mounting the host share.
REPO_ROOT="$(pwd)"
export REPO_ROOT HOST_SHARE_STAGE
# shellcheck source=userland/stage-lib.sh
. "$REPO_ROOT/userland/stage-lib.sh"
stage_zsh_config || exit 1
stage_userland build 0 || true

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
    QEMU_BIN="${AGENTICOS_QEMU_BIN:-$(command -v qemu-system-x86_64 || true)}"
    if [ -z "$QEMU_BIN" ] || [ ! -x "$QEMU_BIN" ]; then
        echo "❌ QEMU binary is missing or not executable: ${QEMU_BIN:-<unset>}" >&2
        exit 1
    fi
    # shellcheck source=scripts/qemu-compositor.sh
    . "$(pwd)/scripts/qemu-compositor.sh"
    if ! agenticos_configure_qemu "$QEMU_BIN"; then
        exit 1
    fi
    echo "🚀 Launching QEMU with image: $BIOS_IMAGE"
    echo "🖥  QEMU binary: $QEMU_BIN"
    "$QEMU_BIN" --version | head -n 1
    echo "🎨 Requested compositor: ${AGENTICOS_COMPOSITOR:-legacy} (strict=${AGENTICOS_GPU_STRICT:-0})"
    echo "🪟 Requested theme: ${AGENTICOS_THEME:-auto}"
    echo "📂 Mounting host folder: $HOST_SHARE -> /host (read-only)"
    echo "🔌 MCP RPC chardev socket: $RPC_SOCK (chmod 0600 once QEMU creates it)"
    QEMU_MEMORY="${AGENTICOS_QEMU_MEMORY:-2G}"
    echo "🧠 QEMU memory: $QEMU_MEMORY (override with AGENTICOS_QEMU_MEMORY)"
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
    DATA_IMAGE="${AGENTICOS_DATA_IMAGE:-target/bootloader/data.img}"
    echo "💽 Persistent data disk: $DATA_IMAGE"
    if [ "${AGENTICOS_NETWORK:-on}" = "off" ]; then
        NETWORK_ARGS=(-nic none)
        echo "🌐 Networking disabled (AGENTICOS_NETWORK=off)"
    else
        NETWORK_ARGS=(
            -netdev "user,id=agenticos-net"
            -device "virtio-net-pci,disable-legacy=on,netdev=agenticos-net,mac=02:41:47:4e:54:01"
        )
        echo "🌐 QEMU user networking enabled"
    fi
    QEMU_ARGS=(
        -drive "format=raw,file=$BIOS_IMAGE,if=ide,index=0"
        -drive "file=fat:ro:$HOST_SHARE,if=ide,index=1,snapshot=on"
        -drive "format=raw,file=$DATA_IMAGE,if=ide,index=2"
        -serial stdio
        -chardev "socket,id=rpc,path=$RPC_SOCK,server=on,wait=off"
        -serial chardev:rpc
        -no-reboot -no-shutdown
        -device "isa-debug-exit,iobase=0xf4,iosize=0x04"
        -device virtio-tablet-pci
        -m "$QEMU_MEMORY"
    )
    QEMU_ARGS+=("${NETWORK_ARGS[@]}")
    # On macOS the cocoa backend has no initial-scale flag, so open the window
    # then enlarge it to AGENTICOS_QEMU_SCALE (default 4x) via a backgrounded
    # AppleScript helper. zoom-to-fit=on (set by qemu-compositor.sh) scales the
    # guest image to fill the resized window. Skipped when zoom is off, when the
    # scale is <=1, or off macOS. Best-effort: needs Accessibility permission.
    QEMU_SCALE="${AGENTICOS_QEMU_SCALE:-4}"
    if [ "$(uname -s)" = "Darwin" ] && [ "${AGENTICOS_QEMU_ZOOM:-on}" != off ]; then
        echo "🔍 Scaling QEMU window to ${QEMU_SCALE}x (override with AGENTICOS_QEMU_SCALE)"
        "$(pwd)/scripts/qemu-window-scale.sh" "$(basename "$QEMU_BIN")" "$QEMU_SCALE" &
    fi
    "$QEMU_BIN" "${QEMU_ARGS[@]}" "${AGENTICOS_QEMU_RENDER_ARGS[@]}" "${AGENTICOS_QEMU_FW_CFG_ARGS[@]}"
fi
