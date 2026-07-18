#!/bin/bash
#
# refresh-prebuilt.sh - Force-rebuild every prebuilt-managed userland app
# and refresh the committed binaries under userland/prebuilt/. Zsh's build
# also refreshes the committed function subset under userland/zsh-config/.
#
# Run this AFTER changing the source, Makefile, or build flags of a
# prebuilt-managed app (currently: zsh, and any future Linux ports that
# fetch upstream tarballs). Then `git add` + commit the updated binaries
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

# host_share staging happens as a side effect — refresh always updates
# both the committed prebuilt and the live staging file so the very
# next build.sh / test.sh run picks up the new binary without an extra
# rebuild.
HOST_SHARE_STAGE="${AGENTICOS_HOST_SHARE:-$REPO_ROOT/host_share}"
mkdir -p "$HOST_SHARE_STAGE"

# Force every stage_* function to take the rebuild branch.
REBUILD_USERLAND=1
export REPO_ROOT HOST_SHARE_STAGE REBUILD_USERLAND

# shellcheck source=userland/prebuilt-lib.sh
. "$REPO_ROOT/userland/prebuilt-lib.sh"

echo "🔄 Refreshing prebuilt userland ELFs..."

# Each stage_* call is a hard failure point — refresh is the explicit
# "I want fresh binaries" workflow, so any failure should stop and
# surface immediately rather than silently committing stale bits.
stage_zsh     || { echo "❌ stage_zsh failed.";     exit 1; }
stage_busybox || { echo "❌ stage_busybox failed."; exit 1; }

# Future prebuilt-managed apps go here:
# stage_bash || { echo "❌ stage_bash failed."; exit 1; }

echo ""
echo "✅ Refresh complete. Changes under userland/prebuilt/ and zsh-config/functions/:"
echo ""
git -C "$REPO_ROOT" status --short userland/prebuilt/ userland/zsh-config/functions/
echo ""
echo "Commit the updated binaries alongside any source/Makefile changes:"
echo "  git add userland/prebuilt/ userland/apps/<app>/ userland/zsh-config/"
echo "  git commit -m \"userland(<app>): <change>; refresh prebuilt\""
