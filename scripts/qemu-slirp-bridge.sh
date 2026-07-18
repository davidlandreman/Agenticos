#!/bin/bash
# Slirp bridge for QEMU binaries built without the `user` netdev backend
# (notably the pinned macOS VirGL bottle). Source this file, then call
# `agenticos_start_slirp_bridge <unix-socket-path>`; on success the bridge
# process id is in AGENTICOS_SLIRP_BRIDGE_PID and the caller attaches its
# guest NIC with `-netdev stream,server=off,addr.type=unix,addr.path=<path>`.
#
# The bridge is a machine-less helper QEMU (any build with slirp, e.g. stock
# Homebrew) that joins `-netdev user` NAT and a unix stream listener on one
# hub, forwarding Ethernet frames between them. The guest sees the ordinary
# slirp network: 10.0.2.0/24, gateway/host alias 10.0.2.2, DNS 10.0.2.3.

agenticos_qemu_has_netdev() {
    "$1" -netdev help 2>/dev/null | grep -qx "$2"
}

# Print the first QEMU binary that can serve as the bridge: an explicit
# AGENTICOS_QEMU_NET_HELPER_BIN wins, then PATH, then the conventional
# Homebrew opt locations. The helper needs both `user` (slirp NAT) and
# `stream` (unix listener) backends.
agenticos_find_slirp_helper() {
    local candidate
    for candidate in \
        "${AGENTICOS_QEMU_NET_HELPER_BIN:-}" \
        "$(command -v qemu-system-x86_64 || true)" \
        /opt/homebrew/opt/qemu/bin/qemu-system-x86_64 \
        /usr/local/opt/qemu/bin/qemu-system-x86_64; do
        [ -n "$candidate" ] && [ -x "$candidate" ] || continue
        if agenticos_qemu_has_netdev "$candidate" user \
            && agenticos_qemu_has_netdev "$candidate" stream; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

agenticos_start_slirp_bridge() {
    local sock=$1
    AGENTICOS_SLIRP_BRIDGE_BIN=$(agenticos_find_slirp_helper) || return 1
    rm -f "$sock"
    "$AGENTICOS_SLIRP_BRIDGE_BIN" -machine none -nodefaults -display none \
        -netdev user,id=usernet \
        -netdev "stream,id=guestlink,server=on,addr.type=unix,addr.path=$sock" \
        -netdev hubport,id=port-user,hubid=0,netdev=usernet \
        -netdev hubport,id=port-guest,hubid=0,netdev=guestlink \
        >"$sock.log" 2>&1 &
    AGENTICOS_SLIRP_BRIDGE_PID=$!
    local _i
    for _i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25; do
        if ! kill -0 "$AGENTICOS_SLIRP_BRIDGE_PID" 2>/dev/null; then
            break
        fi
        if [ -S "$sock" ]; then
            return 0
        fi
        sleep 0.2
    done
    agenticos_stop_slirp_bridge
    return 1
}

agenticos_stop_slirp_bridge() {
    if [ -n "${AGENTICOS_SLIRP_BRIDGE_PID:-}" ] && kill -0 "$AGENTICOS_SLIRP_BRIDGE_PID" 2>/dev/null; then
        kill "$AGENTICOS_SLIRP_BRIDGE_PID" 2>/dev/null
        wait "$AGENTICOS_SLIRP_BRIDGE_PID" 2>/dev/null
    fi
    AGENTICOS_SLIRP_BRIDGE_PID=""
}
