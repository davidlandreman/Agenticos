# Shared shell helpers for staging prebuilt-managed userland ELFs.
#
# Sourced by build.sh, test.sh, and userland/refresh-prebuilt.sh. Each
# stage_<app> function encapsulates the decision tree:
#
#   1. Decide whether to rebuild (env/flag or missing prebuilt).
#   2. If rebuilding: invoke the upstream build, verify, refresh the
#      committed prebuilt AND the host_share staging file atomically.
#   3. If not rebuilding (or rebuild failed): fall back to copying the
#      committed prebuilt into host_share.
#   4. If neither rebuild nor prebuilt is available: warn and continue
#      (kernel tests use embedded fixtures, so a missing optional ELF
#      is non-fatal for the build).
#
# HELLO.ELF (Rust) and HELLOCPP.ELF (C++) are *not* managed here. They
# build from source every run because they have no upstream tarball
# fetch and their toolchains are either already required (Rust) or
# their build time is trivial (hello-cpp).
#
# Required environment, set by the caller before sourcing:
#   HOST_SHARE_STAGE  — absolute path to host_share/ staging dir
#   REPO_ROOT         — absolute path to repo root
#   REBUILD_USERLAND  — "1" to force rebuild of every prebuilt app
#
# Per-app overrides:
#   REBUILD_ZSH       — "1" to force rebuild of just zsh
#   REBUILD_BUSYBOX   — "1" to force rebuild of just busybox
#
# The caller MAY pass `--rebuild-userland` on its own CLI and translate
# that into `REBUILD_USERLAND=1` before sourcing this file.

# Atomic copy: write to tmp then mv -f. Same pattern the scripts already
# use for host_share staging — avoids partial files if the script is
# killed mid-copy.
_prebuilt_atomic_copy() {
    local src=$1 dst=$2
    local tmp="${dst%/*}/.${dst##*/}.tmp.$$"
    cp "$src" "$tmp" && mv -f "$tmp" "$dst"
}

# Stage ZSH.ELF. Returns 0 on success (file present in host_share), 1
# if neither rebuild nor prebuilt produced a staged file.
stage_zsh() {
    local prebuilt="$REPO_ROOT/userland/prebuilt/ZSH.ELF"
    local staged="$HOST_SHARE_STAGE/ZSH.ELF"
    local src_build="$REPO_ROOT/userland/apps/zsh/build/zsh"

    local want_rebuild=0
    if [ "${REBUILD_USERLAND:-0}" = "1" ] || [ "${REBUILD_ZSH:-0}" = "1" ]; then
        want_rebuild=1
    elif [ ! -f "$prebuilt" ]; then
        want_rebuild=1
        echo "ℹ️  userland/prebuilt/ZSH.ELF not found — will attempt rebuild."
    fi

    if [ "$want_rebuild" = "1" ]; then
        local musl_cc="${MUSL_CC:-x86_64-linux-musl-gcc}"
        if command -v "$musl_cc" >/dev/null 2>&1; then
            echo "🛠  Building zsh userland (ZSH)..."
            if make -C "$REPO_ROOT/userland/apps/zsh" MUSL_CC="$musl_cc"; then
                if [ -f "$src_build" ]; then
                    local musl_readelf="${musl_cc%gcc}readelf"
                    command -v "$musl_readelf" >/dev/null 2>&1 || musl_readelf=readelf
                    local et_type
                    et_type=$("$musl_readelf" -h "$src_build" 2>/dev/null | awk '/Type:/ { print $2 }')
                    if [ "$et_type" != "EXEC" ]; then
                        echo "❌ $src_build is $et_type, expected EXEC. Toolchain likely defaults to PIE."
                        echo "   Try: $musl_cc -static -no-pie -fno-pie ..."
                        return 1
                    fi
                    _prebuilt_atomic_copy "$src_build" "$prebuilt"
                    _prebuilt_atomic_copy "$src_build" "$staged"
                    local size
                    size=$(wc -c < "$staged" | tr -d ' ')
                    echo "📦 Refreshed userland/prebuilt/ZSH.ELF and staged $staged ($size bytes)"
                    return 0
                fi
                echo "⚠️  zsh build succeeded but $src_build not found."
            else
                echo "⚠️  zsh build failed."
            fi
            # Build attempted but did not produce a usable binary. Fall
            # through to the prebuilt fallback so the caller still gets
            # *some* ZSH.ELF in host_share if one exists.
        else
            echo "ℹ️  $musl_cc not found — cannot rebuild zsh."
            echo "   Install hint (macOS): brew install x86_64-linux-musl-cross"
        fi
    fi

    if [ -f "$prebuilt" ]; then
        _prebuilt_atomic_copy "$prebuilt" "$staged"
        local size
        size=$(wc -c < "$staged" | tr -d ' ')
        echo "📦 Staged $staged from userland/prebuilt/ ($size bytes)"
        return 0
    fi

    echo "⚠️  ZSH.ELF unavailable — no prebuilt and no successful rebuild."
    echo "   Kernel tests with embedded fixtures still pass; the interactive"
    echo "   zsh shell command will not work in this boot."
    return 1
}

# Stage BB.ELF (BusyBox multicall coreutils). Returns 0 on success, 1
# if neither rebuild nor prebuilt produced a staged file. Mirrors
# stage_zsh exactly — see that function for the rebuild-vs-copy
# decision tree.
stage_busybox() {
    local prebuilt="$REPO_ROOT/userland/prebuilt/BB.ELF"
    local staged="$HOST_SHARE_STAGE/BB.ELF"
    local src_build="$REPO_ROOT/userland/apps/busybox/build/busybox"

    local want_rebuild=0
    if [ "${REBUILD_USERLAND:-0}" = "1" ] || [ "${REBUILD_BUSYBOX:-0}" = "1" ]; then
        want_rebuild=1
    elif [ ! -f "$prebuilt" ]; then
        want_rebuild=1
        echo "ℹ️  userland/prebuilt/BB.ELF not found — will attempt rebuild."
    fi

    if [ "$want_rebuild" = "1" ]; then
        local musl_cc="${MUSL_CC:-x86_64-linux-musl-gcc}"
        if command -v "$musl_cc" >/dev/null 2>&1; then
            echo "🛠  Building busybox userland (BB)..."
            if make -C "$REPO_ROOT/userland/apps/busybox" MUSL_CC="$musl_cc"; then
                if [ -f "$src_build" ]; then
                    local musl_readelf="${musl_cc%gcc}readelf"
                    command -v "$musl_readelf" >/dev/null 2>&1 || musl_readelf=readelf
                    local et_type
                    et_type=$("$musl_readelf" -h "$src_build" 2>/dev/null | awk '/Type:/ { print $2 }')
                    if [ "$et_type" != "EXEC" ]; then
                        echo "❌ $src_build is $et_type, expected EXEC. Toolchain likely defaults to PIE."
                        echo "   Try: $musl_cc -static -no-pie -fno-pie ..."
                        return 1
                    fi
                    _prebuilt_atomic_copy "$src_build" "$prebuilt"
                    _prebuilt_atomic_copy "$src_build" "$staged"
                    local size
                    size=$(wc -c < "$staged" | tr -d ' ')
                    echo "📦 Refreshed userland/prebuilt/BB.ELF and staged $staged ($size bytes)"
                    return 0
                fi
                echo "⚠️  busybox build succeeded but $src_build not found."
            else
                echo "⚠️  busybox build failed."
            fi
            # Fall through to prebuilt fallback so the caller still gets
            # a usable BB.ELF in host_share if one exists.
        else
            echo "ℹ️  $musl_cc not found — cannot rebuild busybox."
            echo "   Install hint (macOS): brew install x86_64-linux-musl-cross"
        fi
    fi

    if [ -f "$prebuilt" ]; then
        _prebuilt_atomic_copy "$prebuilt" "$staged"
        local size
        size=$(wc -c < "$staged" | tr -d ' ')
        echo "📦 Staged $staged from userland/prebuilt/ ($size bytes)"
        return 0
    fi

    echo "⚠️  BB.ELF unavailable — no prebuilt and no successful rebuild."
    echo "   Kernel tests with embedded fixtures still pass; the /bin/<applet>"
    echo "   namespace will return -ENOENT for stat/access without BB.ELF staged."
    return 1
}
