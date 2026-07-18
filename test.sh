#!/bin/bash
#
# test.sh - Build and run AgenticOS kernel tests
#
# This script builds the kernel with test features enabled and runs it in QEMU.
# Tests execute automatically during kernel boot and QEMU exits with appropriate
# status codes:
#   - Exit code 33 (0x10 << 1 | 1) = All tests passed
#   - Exit code 35 (0x11 << 1 | 1) = Test failure (or no tests matched filter)
#
# Usage:
#   ./test.sh                       # run all tests
#   ./test.sh arc                   # run only the `arc` module
#   ./test.sh arc heap              # run `arc` and `heap` modules
#   ./test.sh 'arc::test_weak*'     # glob within a module
#   ./test.sh '*scroll*'            # substring across module::fn
#   ./test.sh -l                    # list available modules and exit
#   ./test.sh --skip-userland       # skip the userland prebuild
#
# Filter syntax: comma-separated patterns, matched against `<module>` or
# `<module>::<fn>`. Each pattern supports `*` as a leading and/or trailing
# wildcard. Patterns are passed to QEMU via `-fw_cfg` and read at boot — no
# rebuild required when the filter changes.

set -u

usage() {
    cat <<'EOF'
Usage: ./test.sh [PATTERN ...] [--skip-userland] [--rebuild-userland] [-l|--list] [-h|--help]

Run kernel tests in QEMU. With no PATTERN, runs the entire suite.

Filter patterns match against `<module>` or `<module>::<fn>` and support `*`
at the start and/or end:
  ./test.sh arc                # the `arc` module
  ./test.sh arc heap           # arc + heap
  ./test.sh 'arc::test_weak*'  # glob within a module
  ./test.sh '*scroll*'         # substring anywhere

Flags:
  --skip-userland     Skip building optional userland apps and hello-cpp
                      (mandatory committed compiler-compat, network, and
                      BusyBox fixtures are still staged). Wins over
                      --rebuild-userland if both are passed.
  --rebuild-userland  Force rebuild of prebuilt-managed userland apps (zsh).
                      Default copies the committed userland/prebuilt/ELF into
                      host_share/. Equivalent: REBUILD_USERLAND=1 env.
  -l, --list          Print available modules and exit (no build/QEMU).
  -h, --help          Show this help.

Environment:
  AGENTICOS_TEST_MEMORY  QEMU RAM for tests (default: 256M; use 128M for
                         reclamation stress runs).
  AGENTICOS_TEST_NETWORK Set to off for an explicit no-NIC boot smoke.
  AGENTICOS_QEMU_BIN     Exact qemu-system-x86_64 binary to launch.
  AGENTICOS_COMPOSITOR   Boot policy: legacy (default), retained, gpu, or auto.
  AGENTICOS_GPU_STRICT   Set to 1 to make unavailable GPU mode fail loudly.
  AGENTICOS_THEME        Frame theme: classic, aero, or auto (default auto).
EOF
}

# Argument parsing
PATTERNS=()
SKIP_USERLAND=0
LIST_ONLY=0
REBUILD_USERLAND_FLAG=0

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        -l|--list) LIST_ONLY=1; shift ;;
        --skip-userland) SKIP_USERLAND=1; shift ;;
        --rebuild-userland) REBUILD_USERLAND_FLAG=1; shift ;;
        --) shift; while [ $# -gt 0 ]; do PATTERNS+=("$1"); shift; done ;;
        -*) echo "Unknown flag: $1" >&2; usage >&2; exit 2 ;;
        *) PATTERNS+=("$1"); shift ;;
    esac
done

# Translate CLI flag into the env contract prebuilt-lib.sh consumes.
if [ "$REBUILD_USERLAND_FLAG" = "1" ]; then
    REBUILD_USERLAND=1
fi
REBUILD_USERLAND="${REBUILD_USERLAND:-0}"
export REBUILD_USERLAND

if [ "$LIST_ONLY" -eq 1 ]; then
    echo "Available test modules (from src/tests/mod.rs MODULES registry):"
    awk '/^static MODULES:/,/^\];$/' src/tests/mod.rs \
        | grep -oE '\("[a-z_]+"' \
        | tr -d '("'
    exit 0
fi

# Build comma-separated filter
FILTER=""
if [ ${#PATTERNS[@]} -gt 0 ]; then
    FILTER=$(IFS=','; echo "${PATTERNS[*]}")
    echo "Test filter: $FILTER"
fi

echo "Building and running kernel tests..."

HOST_SHARE_STAGE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
mkdir -p "$HOST_SHARE_STAGE"

REPO_ROOT="$(pwd)"
export REPO_ROOT HOST_SHARE_STAGE
# shellcheck source=userland/stage-lib.sh
. "$REPO_ROOT/userland/stage-lib.sh"
# Stage the read-only zsh configuration source tree. The kernel imports it
# into its managed runtime /etc after mounting the host share.
stage_zsh_config || exit 1
# Test fixtures remain mandatory even with --skip-userland; optional apps and
# prebuilt-managed interactive programs retain soft-fail staging semantics.
stage_userland test "$SKIP_USERLAND" || {
    echo "Userland fixture staging failed" >&2
    exit 1
}

# Cargo build must be ran twice to make sure the bootloader image is built
# from the freshly-compiled kernel binary (the second pass invokes the
# bootloader-linker build script).
#
# `--release` matches build.sh: the dev profile produces a much larger kernel
# binary, which the BIOS-stage bootloader can fail to load silently in some
# configurations. Tests run faster against an optimized kernel anyway.
cargo build --release --features test
cargo build --release --features test

# Run with QEMU configured for testing
BIOS_IMAGE="${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}"
HOST_SHARE="${AGENTICOS_HOST_SHARE:-$(pwd)/host_share}"
mkdir -p "$HOST_SHARE"
echo "Running tests against: $BIOS_IMAGE"
echo "Host folder: $HOST_SHARE -> /host (read-only)"

# Build QEMU args. When a filter is set, deliver it via fw_cfg — the kernel
# reads `opt/agenticos/test_filter` at boot. Commas inside the filter must be
# escaped as `,,` per QEMU option-parser rules; our filter syntax already uses
# `,` as the pattern separator so we double them here.
DATA_IMAGE="${AGENTICOS_DATA_IMAGE:-target/bootloader/data.img}"
echo "Data disk: $DATA_IMAGE -> /data (writable, snapshot for tests)"
QEMU_ARGS=(
    -drive "format=raw,file=$BIOS_IMAGE,if=ide,index=0"
    -drive "file=fat:ro:$HOST_SHARE,if=ide,index=1,snapshot=on"
    -drive "format=raw,file=$DATA_IMAGE,if=ide,index=2,snapshot=on"
    -serial stdio
    -device "isa-debug-exit,iobase=0xf4,iosize=0x04"
    -display none
    -no-reboot
    -m "${AGENTICOS_TEST_MEMORY:-256M}"
)
if [ "${AGENTICOS_TEST_NETWORK:-on}" = "off" ]; then
    QEMU_ARGS+=(-nic none)
    echo "Test networking disabled (AGENTICOS_TEST_NETWORK=off)"
else
    QEMU_ARGS+=(
        -netdev "user,id=agenticos-net,restrict=on,guestfwd=tcp:10.0.2.100:8080-cmd:$(pwd)/tools/net-test-echo.py,guestfwd=tcp:10.0.2.101:8081-cmd:$(pwd)/tools/net-test-http.py"
        -device "virtio-net-pci,disable-legacy=on,netdev=agenticos-net,mac=02:41:47:4e:54:01"
    )
fi
if [ -n "$FILTER" ]; then
    ESCAPED_FILTER=${FILTER//,/,,}
    QEMU_ARGS+=(-fw_cfg "name=opt/agenticos/test_filter,string=$ESCAPED_FILTER")
fi

COMPOSITOR_REQUEST="${AGENTICOS_COMPOSITOR:-legacy}"
GPU_STRICT="${AGENTICOS_GPU_STRICT:-0}"
THEME_REQUEST="${AGENTICOS_THEME:-auto}"
case "$COMPOSITOR_REQUEST" in legacy|retained|gpu|auto) ;; *) echo "Invalid AGENTICOS_COMPOSITOR: $COMPOSITOR_REQUEST" >&2; exit 2 ;; esac
case "$GPU_STRICT" in 0|1) ;; *) echo "AGENTICOS_GPU_STRICT must be 0 or 1" >&2; exit 2 ;; esac
case "$THEME_REQUEST" in classic|aero|auto) ;; *) echo "Invalid AGENTICOS_THEME: $THEME_REQUEST" >&2; exit 2 ;; esac
QEMU_ARGS+=(-fw_cfg "name=opt/agenticos/compositor,string=$COMPOSITOR_REQUEST")
QEMU_ARGS+=(-fw_cfg "name=opt/agenticos/gpu_strict,string=$GPU_STRICT")
QEMU_ARGS+=(-fw_cfg "name=opt/agenticos/theme,string=$THEME_REQUEST")

QEMU_BIN="${AGENTICOS_QEMU_BIN:-$(command -v qemu-system-x86_64 || true)}"
if [ -z "$QEMU_BIN" ] || [ ! -x "$QEMU_BIN" ]; then
    echo "QEMU binary is missing or not executable: ${QEMU_BIN:-<unset>}" >&2
    exit 1
fi
"$QEMU_BIN" "${QEMU_ARGS[@]}"

# Check exit code
EXIT_CODE=$?
if [ $EXIT_CODE -eq 33 ]; then  # 0x10 << 1 | 1 = 33
    echo "Tests passed!"
    exit 0
else
    echo "Tests failed! Exit code: $EXIT_CODE"
    exit 1
fi
