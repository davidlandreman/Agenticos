#!/usr/bin/env bash

set -euo pipefail

source_image="${1:-${AGENTICOS_DATA_IMAGE:-target/bootloader/data-ext2.img}}"
if [[ ! -f "$source_image" ]]; then
    echo "ext2 image not found: $source_image" >&2
    exit 1
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/agenticos-ext2-interop.XXXXXX")"
test_image="$work_dir/data-ext2.img"
cleanup() {
    rm -f "$test_image"
    rmdir "$work_dir" 2>/dev/null || true
}
trap cleanup EXIT
cp "$source_image" "$test_image"

echo "Booting filesystem tests against a disposable non-snapshot image"
AGENTICOS_DATA_IMAGE="$test_image" \
AGENTICOS_TEST_DATA_SNAPSHOT=off \
AGENTICOS_FORCE_DIRTY_MOUNT=1 \
./test.sh filesystem --skip-userland

echo "Validating guest mutations with the host e2fsck implementation"
AGENTICOS_DATA_IMAGE="$test_image" scripts/fsck-data.sh
