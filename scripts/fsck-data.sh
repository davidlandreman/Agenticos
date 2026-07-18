#!/usr/bin/env bash

set -euo pipefail

repair=0
image="${AGENTICOS_DATA_IMAGE:-target/bootloader/data-ext2.img}"
if [[ "${1:-}" == "--repair" ]]; then
    repair=1
    shift
fi
if [[ $# -gt 1 ]]; then
    echo "usage: $0 [--repair] [image]" >&2
    exit 2
fi
if [[ $# -eq 1 ]]; then
    image="$1"
fi
if [[ ! -f "$image" ]]; then
    echo "ext2 image not found: $image" >&2
    exit 1
fi

image_dir="$(cd "$(dirname "$image")" && pwd -P)"
image="$image_dir/$(basename "$image")"
while IFS= read -r process; do
    case "$process" in
        *qemu-system-x86_64*"$image"*)
            echo "refusing to check a data image attached to a running QEMU: $image" >&2
            exit 1
            ;;
    esac
done < <(ps ax -o command=)

find_e2fsck() {
    local candidate
    if [[ -n "${AGENTICOS_E2FSCK:-}" && -x "$AGENTICOS_E2FSCK" ]]; then
        echo "$AGENTICOS_E2FSCK"
        return
    fi
    if command -v e2fsck >/dev/null 2>&1; then
        command -v e2fsck
        return
    fi
    for candidate in \
        /opt/homebrew/opt/e2fsprogs/sbin/e2fsck \
        /usr/local/opt/e2fsprogs/sbin/e2fsck; do
        if [[ -x "$candidate" ]]; then
            echo "$candidate"
            return
        fi
    done
    echo "e2fsck not found; install e2fsprogs (macOS: brew install e2fsprogs)" >&2
    exit 1
}

e2fsck="$(find_e2fsck)"
if [[ $repair -eq 1 ]]; then
    echo "Running interactive repair check on $image"
    exec "$e2fsck" -f "$image"
fi
echo "Running read-only ext2 check on $image"
exec "$e2fsck" -fn "$image"
