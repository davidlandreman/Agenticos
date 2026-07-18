#!/bin/bash
# Run the hardware-backed VirGL qualification tests. Unlike ordinary kernel
# tests this deliberately opens the qualified GL frontend and requires a real
# virtio-vga-gl device.

set -euo pipefail

if [ -z "${AGENTICOS_QEMU_BIN:-}" ]; then
    echo "AGENTICOS_QEMU_BIN must name the qualified VirGL QEMU binary" >&2
    exit 2
fi

export AGENTICOS_TEST_VIRGL=1
export AGENTICOS_TEST_VIRGL_SCANOUT=1
export AGENTICOS_TEST_NETWORK=off
export AGENTICOS_COMPOSITOR=legacy
export AGENTICOS_GPU_STRICT=0
export COCOA_GL_TRACE=1

tmp_dir=$(mktemp -d .context/virgl-scanout.XXXXXX)
log_file="$tmp_dir/guest.log"

cleanup() {
    status=$?
    if [ "$status" -ne 0 ] && [ -f "$log_file" ]; then
        echo "VirGL qualification log ($log_file):" >&2
        rg -n 'VIRGL_SCANOUT_READY|\[cocoa-gl\].*(scanout|borrow|texture blit)|virtio_gpu|virgl|ERROR|panic' "$log_file" \
            | tail -160 >&2 || true
    fi
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

./test.sh --skip-userland virgl_integration >"$log_file" 2>&1 &
test_pid=$!

ready=0
for _ in $(seq 1 600); do
    if rg -q 'VIRGL_SCANOUT_READY' "$log_file"; then
        ready=1
        break
    fi
    if ! kill -0 "$test_pid" 2>/dev/null; then
        break
    fi
    sleep 0.1
done

if [ "$ready" -ne 1 ]; then
    wait "$test_pid" || true
    sed -n '1,240p' "$log_file"
    echo "VirGL scanout fixture did not become ready" >&2
    exit 1
fi

presented=0
for _ in $(seq 1 100); do
    if rg -q '\[cocoa-gl\] render start.*scanout=1' "$log_file" \
        && rg -q '\[cocoa-gl\] before bind borrow.*tex=[1-9][0-9]*' "$log_file" \
        && rg -q '\[cocoa-gl\] after texture blit.*err=0x0' "$log_file"; then
        presented=1
        break
    fi
    sleep 0.05
done
if [ "$presented" -ne 1 ]; then
    echo "Cocoa presenter did not borrow and blit the VirGL scanout texture" >&2
    exit 1
fi
echo "Host Cocoa presenter borrowed and blitted the VirGL scanout texture without GL errors"

wait "$test_pid"
sed -n '/VIRGL_SCANOUT_READY/,$p' "$log_file"
