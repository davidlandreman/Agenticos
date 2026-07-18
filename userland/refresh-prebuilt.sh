#!/bin/bash
#
# refresh-prebuilt.sh - Force-rebuild every prebuilt-managed userland app
# and refresh the committed binaries under userland/prebuilt/. Zsh's build
# also refreshes the committed function subset under userland/zsh-config/.
#
# Run this AFTER changing the source, Makefile, or build flags of a
# prebuilt-managed app (currently zsh and BusyBox) plus the committed test
# fixtures. Then `git add` + commit the updated binaries
# alongside the source-side change so the repo stays consistent.
#
# Unlike build.sh and test.sh — which soft-fail when the musl toolchain
# is missing so kernel iteration is never blocked — this script hard-
# fails on any build problem. The whole point of running it is to
# produce a fresh binary.
#
# Usage:
#   ./userland/refresh-prebuilt.sh
#
# Required: x86_64-linux-musl-gcc on PATH (override with MUSL_CC).

set -euo pipefail

# Resolve repo root regardless of where the script is invoked from.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# The shared library expects a staging location even though refresh itself
# only updates committed prebuilts; the next build/test stages those outputs.
HOST_SHARE_STAGE="${AGENTICOS_HOST_SHARE:-$REPO_ROOT/host_share}"
mkdir -p "$HOST_SHARE_STAGE"

# shellcheck source=userland/stage-lib.sh
. "$REPO_ROOT/userland/stage-lib.sh"

echo "🔄 Refreshing prebuilt userland ELFs..."

refresh_manifest_prebuilts

echo ""
echo "✅ Refresh complete. Changes under userland/prebuilt/ and zsh-config/functions/:"
echo ""
git -C "$REPO_ROOT" status --short userland/prebuilt/ userland/zsh-config/functions/
echo ""
echo "Commit the updated binaries alongside any source/Makefile changes:"
echo "  git add userland/prebuilt/ userland/apps/<app>/ userland/zsh-config/"
echo "  git commit -m \"userland(<app>): <change>; refresh prebuilt\""
