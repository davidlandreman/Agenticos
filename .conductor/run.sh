#!/usr/bin/env bash
#
# .conductor/run.sh — invoked by conductor.build when the user clicks Run.
# Builds AgenticOS and launches QEMU using only this workspace's artifacts.
# `runScriptMode: nonconcurrent` in conductor.json keeps a single workspace
# from leaking QEMU processes; different workspaces still run in parallel.

set -euo pipefail

# Conductor sets CONDUCTOR_WORKSPACE_PATH; honor it but tolerate manual runs.
cd "${CONDUCTOR_WORKSPACE_PATH:-$(git rev-parse --show-toplevel)}"

# Exercise the qualified VirGL compositor with Futurism window chrome. The
# pinned macOS QEMU bottle has no user-network backend; build.sh detects that
# and bridges the guest NIC to a stock QEMU's slirp over a unix stream socket,
# so networking stays on by default. Explicit caller values still override all
# of these defaults.
qemu_virgl_prefix="${AGENTICOS_QEMU_VIRGL_PREFIX:-$(brew --cellar qemu)/1.0.27}"
export AGENTICOS_COMPOSITOR="${AGENTICOS_COMPOSITOR:-gpu}"
export AGENTICOS_GPU_STRICT="${AGENTICOS_GPU_STRICT:-1}"
export AGENTICOS_QEMU_BIN="${AGENTICOS_QEMU_BIN:-$qemu_virgl_prefix/bin/qemu-system-x86_64}"
export AGENTICOS_QEMU_GL="${AGENTICOS_QEMU_GL:-es}"
export AGENTICOS_THEME="${AGENTICOS_THEME:-futurism}"
export AGENTICOS_NETWORK="${AGENTICOS_NETWORK:-on}"

# build.sh's manual-run default is machine-global. Give each Conductor
# workspace its own RPC socket so parallel QEMUs cannot unlink or replace one
# another's endpoint.
if [[ -z "${AGENTICOS_RPC_SOCK:-}" && -n "${CONDUCTOR_WORKSPACE_NAME:-}" ]]; then
    export AGENTICOS_RPC_SOCK="/tmp/agenticos-rpc-${CONDUCTOR_WORKSPACE_NAME}.sock"
fi
if [[ -z "${AGENTICOS_CLIPBOARD_SOCK:-}" && -n "${CONDUCTOR_WORKSPACE_NAME:-}" ]]; then
    export AGENTICOS_CLIPBOARD_SOCK="/tmp/agenticos-clipboard-${CONDUCTOR_WORKSPACE_NAME}.sock"
fi
if [[ -z "${AGENTICOS_SLIRP_SOCK:-}" && -n "${CONDUCTOR_WORKSPACE_NAME:-}" ]]; then
    export AGENTICOS_SLIRP_SOCK="/tmp/agenticos-slirp-${CONDUCTOR_WORKSPACE_NAME}.sock"
fi

# Per-workspace override hook. Dropping a .conductor/run.local.sh in a
# workspace lets you experiment with QEMU flags (e.g. -gdb, -d int)
# without dirtying git. The file is gitignored.
if [[ -x ".conductor/run.local.sh" ]]; then
    echo "→ Delegating to .conductor/run.local.sh override"
    exec ./.conductor/run.local.sh "$@"
fi

bios_image="${AGENTICOS_BIOS_IMAGE:-target/bootloader/bios.img}"
port_lo="${CONDUCTOR_PORT:-0}"
port_hi=$(( port_lo + 9 ))

echo "=================================================================="
echo " AgenticOS — running workspace ${CONDUCTOR_WORKSPACE_NAME:-<local>}"
echo "------------------------------------------------------------------"
echo " image path     : $bios_image"
echo " compositor     : $AGENTICOS_COMPOSITOR"
echo " gpu strict     : $AGENTICOS_GPU_STRICT"
echo " qemu binary    : $AGENTICOS_QEMU_BIN"
echo " window theme   : $AGENTICOS_THEME"
echo " network        : $AGENTICOS_NETWORK"
echo " reserved ports : ${port_lo}-${port_hi} (currently unused; future GDB)"
echo "=================================================================="

# Delegate to build.sh, which handles the two-pass cargo build and launches
# QEMU. AGENTICOS_BIOS_IMAGE flows through automatically.
exec ./build.sh "$@"
