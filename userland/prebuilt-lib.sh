# Deprecated compatibility entry point. Manifest-driven staging lives in
# `stage-lib.sh`; keep this filename so older local automation still sources a
# valid library while it migrates.

if [ -z "${REPO_ROOT:-}" ]; then
    echo "prebuilt-lib.sh requires REPO_ROOT" >&2
    return 1
fi

# shellcheck source=userland/stage-lib.sh
. "$REPO_ROOT/userland/stage-lib.sh"
