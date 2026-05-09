#!/usr/bin/env bash
#
# .conductor/run.sh — invoked by conductor.build when the user clicks Run.
# Builds AgenticOS and launches QEMU using only this workspace's artifacts.
# `runScriptMode: nonconcurrent` in conductor.json keeps a single workspace
# from leaking QEMU processes; different workspaces still run in parallel.

set -euo pipefail

# Conductor sets CONDUCTOR_WORKSPACE_PATH; honor it but tolerate manual runs.
cd "${CONDUCTOR_WORKSPACE_PATH:-$(git rev-parse --show-toplevel)}"

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
echo " reserved ports : ${port_lo}-${port_hi} (currently unused; future GDB)"
echo "=================================================================="

# Delegate to build.sh, which handles the two-pass cargo build and launches
# QEMU. AGENTICOS_BIOS_IMAGE flows through automatically.
exec ./build.sh "$@"
