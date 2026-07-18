# `src/net/` — IPv4 networking

This subsystem owns the single-interface, polling-driven IPv4 stack. smoltcp
0.12 is an internal protocol engine; no smoltcp type should escape into the
FD table, syscall ABI, or hardware drivers.

## Scope and layout

- `mod.rs` — global stack, initialization, one-pass polling, worker cadence,
  read-only configuration/counter snapshots.
- `stack.rs` — smoltcp `Interface`/`SocketSet`, kernel-owned DHCPv4 socket,
  dynamic address/default-route installation.
- `resolver_config.rs` — renders DHCP DNS state and atomically publishes the
  kernel-managed `/etc/resolv.conf` after releasing the network lock.
- `socket.rs` — bounded TCP/UDP/ICMP registry, ephemeral ports, readiness,
  options, and deferred final-close handling.
- `abi.rs` — AgenticOS-owned IPv4 sockaddr representation.
- `../drivers/virtio/net.rs` — modern VirtIO NIC, DMA pools, and smoltcp
  Ethernet `Device`; hardware knowledge stays there.
- `../userland/network_syscalls.rs` — Linux x86-64 socket ABI and usercopy.

Delivered scope is Ethernet/ARP/IPv4/DHCPv4/ICMP/UDP/TCP, one modern
VirtIO-net interface, MTU 1500, and DHCP-backed musl name resolution. IPv6,
TLS, interface mutation, fragmentation, offloads, multiqueue, and NIC IRQs are
not implemented.

## Load-bearing rules

- `NETWORK` is the only stack lock. Never acquire the process table while it
  is held, never acquire it inside an FD-table closure, and never yield while
  it is held. Every `NETWORK` and deferred-close-queue critical section must
  hold an `InterruptGuard`: kernel threads are timer-preemptible, while the
  syscall path runs with interrupts masked, so an unguarded spinlock can
  deadlock the local CPU if its holder is preempted by ring 3. The network
  worker's post-poll process-table wake follows the same rule, after dropping
  `NETWORK`.
- Stage IDs, sockaddrs, iovecs, and payloads in bounded kernel memory before
  entering the stack. Stage received bytes before copying back to ring 3.
- `SocketHandle` is an `Arc`-shared open-file description. `O_NONBLOCK` is
  shared by dup/fork; `FD_CLOEXEC` remains descriptor-local. Final drop queues
  a socket ID for deferred close instead of taking `NETWORK` under the process
  lock.
- Blocking syscalls use `WaitingForNetwork` and restart the syscall. Preserve
  the first absolute timeout deadline across restarts and wake conservatively;
  the re-fired operation must re-check its exact socket.
- DHCP owns the interface address and route. Userland must not synthesize
  `ifconfig`, `route`, or `udhcpc` behavior.
- DHCP resolver publication must happen after dropping `NETWORK`; `/etc` is a
  kernel-owned runtime namespace and userland mutation attempts fail.
- Production boot never waits for DHCP. Bounded synchronous wait helpers are
  test-only.
- Entropy initialization precedes network initialization. Both smoltcp's
  random seed and the first ephemeral port come from the trusted random broker;
  network startup fails closed if it cannot obtain them.

## Polling and QEMU contract

The `net-rx-tx` worker follows smoltcp deadlines, capped at one 100 Hz PIT tick
while user sockets exist and ten ticks while idle. Syscalls may perform one
bounded poll before testing readiness. No network code changes PIC/IDT masks.

`build.sh` supplies modern `virtio-net-pci` and QEMU user-mode NAT by default;
`AGENTICOS_NETWORK=off ./build.sh` boots with `-nic none`. `test.sh` uses
`restrict=on` and repository-owned guest-forwarded TCP endpoints, so it cannot
reach the host LAN or Internet. The test guest normally leases `10.0.2.15`;
restricted QEMU may omit the default router option.

## Validation

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland network
./test.sh --skip-userland network_userland
./test.sh --skip-userland
```

`network` covers queue ownership and bounded registry/DHCP behavior.
`network_userland` runs the committed static-musl fixture and BusyBox
`ping`/`nc`/HTTP-only `wget` hostname paths against QEMU-local services.
Every booted wait must have a PIT-tick deadline; missing committed fixtures
are failures.
