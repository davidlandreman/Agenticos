#!/usr/bin/env bash
#
# .conductor/setup.sh — runs once at workspace creation under conductor.build.
# Job: bootstrap the toolchain, verify QEMU is available, and seed personal
# Claude Code permissions. Must stay fast (no `cargo build`).

set -euo pipefail

echo "=================================================================="
echo " AgenticOS — conductor workspace setup"
echo "------------------------------------------------------------------"
echo " workspace name : ${CONDUCTOR_WORKSPACE_NAME:-<unset>}"
echo " workspace path : ${CONDUCTOR_WORKSPACE_PATH:-<unset>}"
echo " root checkout  : ${CONDUCTOR_ROOT_PATH:-<unset>}"
echo " default branch : ${CONDUCTOR_DEFAULT_BRANCH:-<unset>}"
echo " port block     : ${CONDUCTOR_PORT:-<unset>}–$(( ${CONDUCTOR_PORT:-0} + 9 ))"
echo "=================================================================="

# Ensure the pinned toolchain (rust-toolchain.toml) and components are present.
# rustup is idempotent for component installs of the same versions, so this is
# safe to run concurrently across parallel workspaces.
echo "→ Installing/refreshing rust toolchain pinned in rust-toolchain.toml"
rustup show

# QEMU is required for ./build.sh and ./test.sh.
if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
    echo "✗ qemu-system-x86_64 is not on PATH." >&2
    echo "  Install it before running ./build.sh." >&2
    echo "  macOS: brew install qemu" >&2
    exit 1
fi
echo "✓ qemu-system-x86_64: $(command -v qemu-system-x86_64)"

# .claude/settings.local.json is gitignored (personal permissions). Seed it
# from the main checkout if available; otherwise create an empty allowlist
# so the agent has somewhere to add per-workspace approvals.
local_settings=".claude/settings.local.json"
if [[ ! -f "$local_settings" ]]; then
    mkdir -p .claude
    if [[ -n "${CONDUCTOR_ROOT_PATH:-}" && -f "$CONDUCTOR_ROOT_PATH/$local_settings" ]]; then
        cp "$CONDUCTOR_ROOT_PATH/$local_settings" "$local_settings"
        echo "✓ Copied $local_settings from root checkout"
    else
        cat >"$local_settings" <<'JSON'
{
  "permissions": {
    "allow": [],
    "deny": []
  }
}
JSON
        echo "✓ Created empty $local_settings (no main-checkout copy found)"
    fi
fi

echo "✓ Setup complete. Run \`./build.sh\` (or click Run in Conductor) to boot AgenticOS."
