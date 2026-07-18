#!/usr/bin/env bash
#
# .conductor/archive.sh — runs before conductor.build removes this workspace.
# Best-effort: kill any QEMU process this workspace launched. Do NOT delete
# target/ — Conductor removes the worktree directory itself, and rebuilds
# elsewhere are unaffected.

# Best-effort semantics: do not abort on first failure.
set -uo pipefail

workspace_path="${CONDUCTOR_WORKSPACE_PATH:-}"
workspace_name="${CONDUCTOR_WORKSPACE_NAME:-<unset>}"

if [[ -n "$workspace_path" ]]; then
    # Match QEMU processes whose disk image lives inside this workspace; this
    # avoids killing QEMUs spawned by other parallel workspaces.
    pkill -f "qemu-system-x86_64.*${workspace_path}" 2>/dev/null || true
fi

if [[ -n "${CONDUCTOR_WORKSPACE_NAME:-}" ]]; then
    workspace_rpc_socket="/tmp/agenticos-rpc-${CONDUCTOR_WORKSPACE_NAME}.sock"
    workspace_clipboard_socket="/tmp/agenticos-clipboard-${CONDUCTOR_WORKSPACE_NAME}.sock"
    rm -f "$workspace_rpc_socket" "$workspace_clipboard_socket"
fi

echo "archived: $workspace_name"
exit 0
