---
title: "feat: Basic IPv4 network stack"
status: complete
created: 2026-07-17
plan_type: feat
depth: deep
related_docs:
  - CLAUDE.md
  - src/drivers/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - docs/ARCHITECTURE.md
  - docs/IMPLEMENTATION_PLAN.md
  - docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md
  - https://docs.oasis-open.org/virtio/virtio/v1.2/virtio-v1.2.html
  - https://docs.rs/smoltcp/0.12.0/smoltcp/
  - https://www.qemu.org/docs/master/system/devices/net.html
---

# feat: Basic IPv4 network stack

## Summary

Add a small but complete IPv4 networking vertical slice to AgenticOS:

```text
QEMU user-mode network
        |
modern virtio-net PCI device
        |
Ethernet device adapter
        |
smoltcp: Ethernet + ARP + IPv4 + ICMP + UDP + TCP + DHCPv4
        |
kernel socket registry
        |
Linux x86-64 socket syscalls + per-process FDs
        |
static-musl userland (network fixture, then selected BusyBox applets)
```

The first release is IPv4-only, single-interface, and polling-driven. It must
obtain a DHCP lease from QEMU, exchange ICMP/UDP/TCP traffic, and expose enough
of the Linux socket ABI for a committed static-musl integration fixture and
numeric-address BusyBox `ping`, `nc`, and HTTP `wget` flows. It deliberately
uses the existing single-CPU scheduler instead of adding PCI interrupt routing
or an APIC as part of the same change.

The protocol implementation should use a pinned, `no_std` smoltcp dependency
rather than introducing a second home-grown TCP state machine. AgenticOS owns
the hardware driver, resource policy, blocking semantics, Linux ABI, and tests;
smoltcp owns packet parsing, checksums, ARP, and transport state machines.

## Current-state evidence

- There is no `src/net/` module and `docs/ARCHITECTURE.md` explicitly records
  that no network stack exists.
- `Cargo.toml` has no network dependency. The kernel is `no_std` with `alloc`
  available after heap initialization.
- `src/drivers/pci.rs` already enumerates PCI functions and exposes BAR,
  bus-mastering, and VirtIO input discovery.
- `src/drivers/virtio/common.rs` already handles modern VirtIO PCI
  capabilities, feature negotiation, queue setup, notification, and used-ring
  polling. Its queue API assumes one anonymous buffer per descriptor and does
  not retain caller-owned buffer metadata, which is insufficient for a
  reusable RX/TX packet pool.
- The existing VirtIO tablet is polled. There is no generic PCI INTx/MSI/MSI-X
  registration surface; the IDT and PIC masks only cover PIT, keyboard, and
  mouse IRQs.
- `build.sh` and `test.sh` do not select a NIC model explicitly. QEMU may add a
  default NIC, but QEMU documents that the default model may change. The stack
  needs an explicit modern `virtio-net-pci` contract.
- The scheduler has a 100 Hz monotonic tick and can run a dedicated kernel
  worker. Ring-3 blocking syscalls already save the syscall frame, mark a
  `Ring3BlockReason`, yield, and re-fire the syscall when woken.
- `FdTable` supports clone-on-`dup`/`fork` slots for files, directories, and
  pipes. It has no socket slot.
- Linux syscall dispatch has no socket syscall numbers. `read`, `write`,
  `fcntl`, `close`, and `poll` must also learn socket behavior.
- BusyBox networking applets are intentionally disabled and the prior BusyBox
  plan names `src/net/` as their prerequisite.
- The pinned compiler is Rust 1.90 nightly. smoltcp 0.12 documents Rust 1.80 as
  its minimum, while the current smoltcp 0.13 line requires a newer compiler.

## Goals

- Detect and initialize one modern VirtIO network device in QEMU.
- Send and receive bounded Ethernet frames without DMA into movable, freed, or
  physically discontinuous memory.
- Support Ethernet, ARP, IPv4, automatic ICMP echo replies, ICMP sockets, UDP,
  TCP, and DHCPv4 on one interface with MTU 1500.
- Acquire and renew a QEMU DHCP lease and install its address, default route,
  and DNS-server metadata.
- Give each socket a bounded receive/transmit allocation and a stable
  kernel-side identity shared correctly by `dup` and `fork`.
- Implement blocking and nonblocking Linux socket behavior without holding the
  process-table lock, network lock, or user pointers across a yield.
- Support outbound TCP, TCP listen/accept with a deliberately bounded backlog,
  connected and unconnected UDP, and ICMP echo traffic.
- Make socket readiness visible through `poll`/`ppoll`; make `read`/`write`
  operate on connected sockets.
- Boot and pass a committed static-musl network fixture against a hermetic
  QEMU TCP echo endpoint.
- Re-enable a small, documented BusyBox networking set after the syscall
  fixture passes.
- Keep `./test.sh` deterministic and prevent its network backend from reaching
  the host LAN or public Internet.

## Non-goals

- IPv6, SLAAC, NDP, IP forwarding, routing between interfaces, VLANs, Wi-Fi,
  or more than one NIC.
- TLS, certificate storage, HTTPS, SSH, or an AgenticOS application protocol.
- A full resolver/NSS implementation. The first user-visible commands use
  numeric IPv4 addresses; DHCP-provided DNS metadata is retained for a later
  resolver phase.
- General PCI IRQ routing, IOAPIC, MSI, MSI-X, or interrupt-driven network I/O.
- TCP performance tuning, zero-copy user I/O, checksum/GSO/TSO offload,
  multiqueue VirtIO, merged RX buffers, jumbo frames, or packet forwarding.
- Full Linux ancillary data, every socket option, `epoll`, `select`, netlink,
  `ioctl`-based interface configuration, or POSIX asynchronous I/O.
- Privilege separation. Current userland reports UID 0; raw ICMP sockets are
  permitted under that existing model.
- Building a custom TCP implementation. That is separable research work and
  not required to make networking usable.

## Key decisions

### Pin a minimal `no_std` smoltcp 0.12 feature set

Add an exact `=0.12.0` dependency with default features disabled. Enable only
the features required for `alloc`, Ethernet, IPv4, DHCPv4, raw/ICMP, UDP, and
TCP sockets. Do not enable `std`, host raw-socket/TUN/TAP adapters, logging, the
async facade, IPv6, DNS, or fragmentation by accident.

The exact dependency declaration is verified first with `cargo check` under
the pinned nightly and custom target. If a transitive feature enables `std`,
the unit stops there and corrects the feature set; the kernel must not work
around it with conditional `std` imports.

smoltcp is an internal engine, not the public AgenticOS API. Keep its types
behind `src/net/` so a future upgrade or replacement does not leak into
`FdSlot`, the syscall dispatcher, or device drivers.

### Use modern VirtIO net with conservative feature negotiation

Add discovery for modern VirtIO network device ID `0x1041` and configure QEMU
with an explicit `virtio-net-pci,disable-legacy=on` device and stable locally
administered test MAC. Negotiate only:

- `VIRTIO_F_VERSION_1` (required), and
- `VIRTIO_NET_F_MAC` when offered.

Do not negotiate merged RX buffers, checksum offload, segmentation offload,
multiqueue, or a control queue. The driver therefore owns ordinary Ethernet
frames no larger than 1514 bytes and uses the negotiated non-merged VirtIO net
header layout.

Refactor `VirtioDevice::init_simple` into an explicit required/accepted feature
negotiation API, leaving `init_simple` as a compatibility wrapper for the
tablet. Reject missing required bits and set `FAILED` on partial initialization
failure.

### Make DMA ownership explicit before adding the NIC

The current queue stores only a translated address and descriptor length.
Networking needs stable RX buffers that remain submitted for arbitrary time,
and TX buffers that cannot be reused until the device returns their descriptor.

Extend `Virtqueue` with APIs that submit a physical address/length plus a
caller token, return the token on completion, and never silently substitute a
virtual address when translation fails. The NIC owns aligned, page-contained
DMA storage for:

- the descriptor/available/used rings,
- a bounded RX pool, and
- a bounded TX pool containing the VirtIO header plus Ethernet frame.

Every DMA object must be page-aligned and fit within its mapped page(s), or be
backed by a helper that proves each physical segment. Do not assume a heap
allocation spanning pages is physically contiguous. The initial NIC can use
one combined header+frame buffer per descriptor, so general scatter/gather is
not required for v1; the queue API must still track descriptor ownership and
completion correctly.

Use acquire/release fences at the ring publication/consumption boundaries.
Bounds-check device-written used IDs and lengths before indexing pools or
exposing frame bytes. A malformed completion drops the frame and records an
error; it must not panic in a polling or future interrupt context.

### Poll from one kernel worker

`net::init()` runs after heap and scheduler initialization. If a supported NIC
is present, it creates the stack and spawns a `net-rx-tx` kernel process. The
worker:

1. polls RX completions,
2. advances the smoltcp interface and sockets using the PIT-derived monotonic
   timestamp,
3. processes DHCP configuration changes,
4. reclaims TX completions and transmits queued frames,
5. snapshots which socket states changed,
6. drops the network lock,
7. wakes matching ring-3 waiters, and
8. sleeps until smoltcp's next deadline, capped at one scheduler tick while
   sockets are active and a slower idle cadence while no sockets exist.

Syscalls may call the same bounded `poll_once()` before observing readiness so
an immediately available result does not wait for the next worker slice. Only
the worker loops; syscall-side polls perform one pass.

The first version does not touch the PIC masks or IDT for the NIC. IRQ support
can replace the worker's one-tick receive latency later without changing the
device or socket interfaces.

### Keep one lock boundary and never yield while locked

Use one `spin::Mutex<Option<NetworkStack>>` for the device, smoltcp interface,
socket set, DHCP socket, and AgenticOS socket registry. The critical sections
are bounded and there is only one CPU.

Lock rules:

- Never acquire `PROCESS_TABLE` while holding the network lock.
- Never acquire the network lock inside a `with_fd_table*` closure.
- Copy socket IDs, FD flags, sockaddr values, and user payloads into bounded
  kernel staging storage first; release the process-table lock; then enter the
  network lock.
- Copy received bytes into a kernel staging buffer while holding the network
  lock; release it before writing to user memory.
- Drop the network lock before `block_current_ring3_and_yield`.
- The worker gathers wake decisions under the network lock, then drops it
  before modifying ring-3 queues.

These rules avoid a process-table/network inversion and guarantee that no
spinlock remains held across a context switch.

### Represent sockets as shared open-file descriptions

Add `FdSlot::Socket { handle: Arc<SocketHandle>, cloexec: bool }`.
`SocketHandle` contains a stable monotonically allocated socket ID; mutable
status flags such as `O_NONBLOCK` live in the registry entry so `dup` and
`fork` share them. `FD_CLOEXEC` remains per descriptor, matching the current FD
table model.

Dropping the final `Arc<SocketHandle>` schedules registry cleanup. Cleanup must
not directly take the network lock from inside a process-table-locked FD-table
mutation. Use a small deferred-close ID queue drained by the network worker and
by explicit post-`close`/post-`dup2` hooks, mirroring the existing pipe wake
discipline.

Each registry entry owns bounded buffers:

- TCP: 16 KiB RX + 16 KiB TX.
- UDP: a bounded packet buffer sized for several MTU packets in each direction.
- ICMP: a smaller bounded packet buffer sufficient for echo requests/replies.

Cap the number of live sockets at the existing FD-table scale. Allocation
failure returns `-ENFILE`/`-ENOBUFS`, never an unbounded heap growth attempt.

### Implement a deliberately finite Linux socket ABI

Add Linux x86-64 numbers and handlers for:

- `socket`, `bind`, `connect`, `listen`, `accept`, `accept4`;
- `getsockname`, `getpeername`, `shutdown`;
- `sendto`, `recvfrom`;
- limited `sendmsg`/`recvmsg` with one or more validated iovecs and no ancillary
  output in v1;
- `setsockopt`, `getsockopt` for the compatibility subset below.

Supported families/types/protocols:

- `AF_INET` only;
- `SOCK_STREAM`/TCP;
- `SOCK_DGRAM`/UDP;
- root-only-by-current-policy `SOCK_RAW`/ICMP;
- `SOCK_NONBLOCK` and `SOCK_CLOEXEC` creation flags.

Support the options common libc/BusyBox paths probe: `SO_ERROR`, `SO_TYPE`,
`SO_REUSEADDR`, `SO_RCVTIMEO`, `SO_SNDTIMEO`, `IP_TTL`, and `TCP_NODELAY`.
Apply options smoltcp exposes; store and report compatible values for harmless
hints; return `-ENOPROTOOPT` for unsupported options. Never return success for
an option whose semantics would affect correctness but are not implemented.

TCP listen uses a v1 backlog cap of one. When the backing smoltcp listener
becomes established, `accept` transfers that socket to the accepted registry
entry and installs a fresh listening socket on the original entry before
returning. Larger requested backlogs are clamped and documented.

Update the errno set for normal socket results (`EAFNOSUPPORT`,
`EPROTONOSUPPORT`, `ENOTCONN`, `EISCONN`, `EINPROGRESS`, `EALREADY`,
`ECONNREFUSED`, `EADDRINUSE`, `EADDRNOTAVAIL`, `ETIMEDOUT`, `ENOBUFS`,
`EMSGSIZE`, `EDESTADDRREQ`, `ENOPROTOOPT`, `ENETDOWN`, and `ENETUNREACH`).
Unknown families and protocols fail predictably rather than falling through to
`ENOSYS`.

### Integrate socket I/O and blocking with existing syscall restart

Teach `read`/`write` to use connected TCP/UDP sockets and `close`, `dup`,
`dup2`, `fcntl`, `poll`, and `ppoll` to recognize socket slots.

Add `Ring3BlockReason::WaitingForNetwork { deadline_tick: Option<u64> }`.
Blocking socket operations follow the existing pipe/wait4 pattern:

1. validate and stage arguments,
2. poll once and attempt the operation,
3. return success or a final error if available,
4. return `EAGAIN`/`EINPROGRESS` for nonblocking descriptors,
5. otherwise save the syscall frame with RIP rewound, mark the process blocked,
   and yield,
6. wake conservatively on network state change or deadline,
7. re-fire the syscall and re-check the exact socket.

`poll` computes real socket readiness (`POLLIN`, `POLLOUT`, `POLLERR`,
`POLLHUP`) and blocks only when there are no ready entries and the timeout is
nonzero. Preserve the existing behavior for non-socket FDs in this release so
networking does not silently rewrite terminal and pipe semantics. A network
event may wake all network waiters; each re-fired syscall filters its own
socket, which is acceptable at the current process cap.

The original absolute deadline must survive syscall restart. Add a small
per-process `NetworkWaitState` keyed by syscall number and operation identity;
the first attempt computes the absolute tick, event wakes preserve it, and the
deadline wake marks it expired. The re-fired handler observes expiration,
returns the operation-specific timeout result, and clears the state. Successful
completion, close, signal interruption, or a different syscall also clears
stale state. Recomputing `now + timeout` on every restart is forbidden because
it can turn a finite timeout into an infinite wait.

Closing a socket wakes waiters so they return `EBADF`/`ENOTCONN`; DHCP lease
loss wakes all socket waiters and causes appropriate network-unreachable
errors. Signals continue to use the existing syscall-boundary delivery path.

### Keep DHCP kernel-owned and DNS deferred

Create one internal DHCPv4 socket at stack initialization. On
`Configured`, atomically install the interface address and default route and
retain the offered DNS server list in a read-only `NetworkConfig` snapshot. On
`Deconfigured`, remove the dynamic address/route and wake blocked operations.

Log link state and configuration changes once at info level; packet-by-packet
logs stay at trace. Never print packet payloads by default.

Do not expose `udhcpc`, `ifconfig`, or `route` in v1: users cannot yet safely
mutate kernel-owned interface state, and the corresponding ioctls/netlink
surface is out of scope. Do not synthesize `/etc/resolv.conf` until a DNS phase
defines ownership and update semantics.

### Make QEMU behavior explicit and tests hermetic

Both launch scripts select the NIC explicitly.

- Interactive `build.sh`: modern virtio-net plus QEMU user networking, with
  outbound NAT enabled. Add `AGENTICOS_NETWORK=off` as an opt-out that passes
  `-nic none`.
- `test.sh`: modern virtio-net plus `-netdev user,restrict=on`, which retains
  QEMU's local DHCP/router services but prevents access to the host network or
  public Internet.

For the TCP integration test, add a QEMU `guestfwd` endpoint at a reserved
guest address that launches a tiny repository script acting as a framed echo
server. Do not depend on a public host, DNS, wall-clock timing, a privileged
port, or a persistent host listener. Give every wait a PIT-tick deadline so a
driver failure exits the test with an assertion instead of hanging QEMU.

## Proposed code layout

```text
src/
  drivers/
    pci.rs                         VirtIO net discovery
    virtio/
      common.rs                    feature negotiation + DMA queue ownership
      net.rs                       VirtIO net RX/TX pools and smoltcp Device
      mod.rs
  net/
    CLAUDE.md                      subsystem invariants and validation flow
    mod.rs                         init, global access, worker entry
    config.rs                      DHCP-derived immutable snapshot
    stack.rs                       Interface, SocketSet, DHCP, poll/deadlines
    socket.rs                      AgenticOS registry and readiness API
    abi.rs                         sockaddr/constants/wire conversion helpers
  userland/
    fdtable.rs                     FdSlot::Socket
    lifecycle.rs                   WaitingForNetwork + wake helper
    syscalls.rs                    socket handlers and FD integration
    abi.rs                         syscall numbers + errno + dispatch
  tests/
    network.rs                     pure/loopback/driver tests
    network_userland.rs            committed ELF integration test
userland/
  apps/network-test/               source + refresh Makefile
  prebuilt/network/NETTEST.ELF     mandatory static-musl fixture
tools/
  net-test-echo.py                 hermetic framed TCP echo command
```

If the smoltcp `Device` borrow model makes it cleaner, the adapter may live in
`src/net/device.rs` while `src/drivers/virtio/net.rs` exposes an AgenticOS
`EthernetDevice` trait. Keep hardware register/queue knowledge in `drivers/`
and protocol/socket knowledge in `net/` regardless of the exact file split.

## Implementation sequence

### U1 — dependency and compile-time boundary

**Files:** `Cargo.toml`, `Cargo.lock`, `src/main.rs`, new `src/net/mod.rs`.

1. Add exact smoltcp 0.12 with the minimal features above.
2. Add an empty `net` module and compile a type-only smoke path proving the
   selected `Interface`, `SocketSet`, DHCP, ICMP, UDP, and TCP APIs exist.
3. Run `cargo check` and inspect the resolved feature graph for `std`, host
   adapters, IPv6, and unintended logging dependencies.

**Exit:** the unchanged kernel compiles with the dependency and no new runtime
behavior.

### U2 — reusable VirtIO queue and DMA contract

**Files:** `src/drivers/virtio/common.rs`, `src/drivers/virtio/input.rs`,
`src/tests/virtio.rs` or `src/tests/network.rs`.

1. Add explicit required/accepted feature negotiation.
2. Replace buffer-slice submission with tokenized descriptor ownership and
   checked completion.
3. Make address-translation failure explicit.
4. Introduce page-safe ring storage and the DMA-buffer helper used by net.
5. Adapt the tablet without changing its observable behavior.
6. Test free-list wraparound, queue-full behavior, used-index wraparound,
   invalid completion IDs/lengths, and token return.

**Exit:** tablet still works interactively; queue tests pass; the queue never
uses a virtual address as a DMA fallback.

### U3 — VirtIO network device

**Files:** `src/drivers/pci.rs`, new `src/drivers/virtio/net.rs`,
`src/drivers/virtio/mod.rs`, `build.sh`, `test.sh`.

1. Add modern VirtIO net discovery and explicit QEMU NIC args.
2. Negotiate conservative features and read the MAC with a configuration-
   generation-safe loop.
3. Set up RX queue 0 and TX queue 1, pre-fill RX, and set `DRIVER_OK` only after
   both queues are ready.
4. Implement bounded RX dequeue/re-submit and TX allocate/submit/reclaim.
5. Implement the smoltcp Ethernet `Device` token adapter with MTU 1500.
6. Add driver counters for RX/TX frames, drops, malformed completions, and
   pool exhaustion.

**Exit:** a boot log shows the fixed MAC and link; raw frames can cross the
VirtIO queues; absence of the NIC is a nonfatal `Network unavailable` state.

### U4 — protocol stack, worker, and DHCP

**Files:** new `src/net/{config,stack,mod}.rs`, `src/kernel.rs`,
`src/process/*` only if a worker helper is genuinely missing.

1. Construct the interface at `0.0.0.0/0` with the device MAC.
2. Add the internal DHCP socket and apply configure/deconfigure events.
3. Spawn `net-rx-tx` and implement deadline-aware polling.
4. Expose read-only link/config/counter snapshots and a one-pass poll hook.
5. Add a bounded wait helper used only by booted tests; production boot never
   waits synchronously for DHCP.

**Exit:** under QEMU user networking the guest obtains a `10.0.2.x` lease,
installs router `10.0.2.2`, and renews without stalling the GUI or ring 3.

### U5 — socket registry and UDP/ICMP

**Files:** new `src/net/{socket,abi}.rs`, `src/userland/fdtable.rs`,
`src/userland/abi.rs`, `src/userland/syscalls.rs`.

1. Add bounded socket IDs, registry entries, deferred close, and buffer quotas.
2. Add `FdSlot::Socket` and correct clone/cloexec/status-flag behavior.
3. Implement `socket`, `bind`, UDP `connect`, `sendto`, `recvfrom`, socket-name
   queries, basic options, and shutdown/error mapping.
4. Add ICMP raw sockets sufficient for echo request/reply.
5. Implement user sockaddr and iovec copy-in/copy-out with checked lengths and
   family validation.

**Exit:** in-kernel loopback tests exchange UDP datagrams and ICMP echo data;
FD duplication and final-close tests show one shared socket lifetime.

### U6 — TCP client and server

**Files:** `src/net/socket.rs`, `src/userland/syscalls.rs`.

1. Add ephemeral-port allocation with wraparound/collision checks.
2. Implement blocking/nonblocking TCP connect and state-to-errno mapping.
3. Implement stream send/receive and half/full shutdown.
4. Implement backlog-one listen, accepted-socket transfer, `accept`, and
   `accept4`.
5. Add getsockname/getpeername and `SO_ERROR` for connect completion.

**Exit:** loopback tests cover connect, bidirectional transfer, orderly EOF,
reset/refusal, nonblocking connect, listen/accept, and port reuse.

### U7 — scheduler blocking and readiness

**Files:** `src/userland/lifecycle.rs`, `src/userland/switch.rs`,
`src/userland/syscalls.rs`, `src/net/mod.rs`.

1. Add `WaitingForNetwork` and lock-free wake decisions.
2. Add restart-stable per-process network deadlines and cleanup on completion,
   close, signal interruption, and process exit.
3. Route socket `read`/`write` and FD control operations.
4. Make `poll`/`ppoll` report real socket readiness and honor finite/zero/
   infinite timeouts for socket waits.
5. Wake on state changes, close, lease loss, and deadline.
6. Test that a blocked socket does not stall another ring-3 process or the
   compositor and that callee-saved registers survive the restarted syscall.

**Exit:** blocking I/O consumes no busy loop, nonblocking I/O returns Linux-like
errors, and multi-process scheduling continues making progress.

### U8 — booted static-musl fixture and hermetic transport

**Files:** new `userland/apps/network-test/*`,
`userland/prebuilt/network/NETTEST.ELF`, `src/tests/network_userland.rs`,
`src/tests/mod.rs`, `test.sh`, `tools/net-test-echo.py`.

The self-checking fixture must:

1. create UDP and TCP sockets and validate expected invalid-family errors;
2. exercise nonblocking flags and `poll`;
3. connect by numeric IPv4 address to the QEMU guest-forwarded echo service;
4. send and receive a payload larger than one small write so stream framing and
   partial I/O are exercised;
5. verify `getsockname`, `getpeername`, `SO_TYPE`, and clean shutdown;
6. exit nonzero at the first failed tier.

Stage the committed ELF on every test run, including `--skip-userland`. Missing
fixture input is a failure, not a skip. Add finite DHCP/connect/read deadlines
to both kernel and fixture.

**Exit:** `./test.sh network network_userland` passes without Internet access
or a persistent host daemon.

### U9 — selected BusyBox applets and documentation

**Files:** `userland/apps/busybox/busybox.config`, refreshed
`userland/prebuilt/BB.ELF`, BusyBox README, `CLAUDE.md`,
`src/drivers/CLAUDE.md`, new `src/net/CLAUDE.md`, `docs/ARCHITECTURE.md`,
`docs/IMPLEMENTATION_PLAN.md`, `README.md`.

1. Re-enable only the applets supported by the delivered ABI: `ping`, `nc`,
   and HTTP-only/numeric-address `wget` where its configured BusyBox feature
   set does not imply TLS or DNS.
2. Refresh and commit the prebuilt BusyBox binary through the repository's
   established refresh flow.
3. Document `AGENTICOS_NETWORK`, QEMU's address layout, numeric-address limits,
   DHCP ownership, supported syscalls/options, and the polling latency tradeoff.
4. Update current-state and roadmap checkboxes precisely; do not claim DNS,
   IPv6, TLS, or interrupt-driven I/O.

**Exit:** from zsh, numeric-address `ping 10.0.2.2` and the supported `nc`/HTTP
demo work, and fresh clones retain toolchain-free userland binaries.

## Validation matrix

| Layer | Happy path | Failure/edge path |
|---|---|---|
| Virtqueue | RX/TX completion and descriptor reuse | full queue, wraparound, invalid used ID/length, translation failure |
| VirtIO net | MAC, two queues, MTU frame transfer | NIC absent, feature rejection, RX pool exhaustion, oversize frame |
| DHCP/config | acquire address/router/DNS metadata | timeout, NAK/deconfigure, lease renewal/loss |
| UDP | bind, connected/unconnected send/recv | port collision, no destination, oversize datagram, nonblocking empty read |
| ICMP | echo request/reply | wrong protocol/type, short packet, timeout |
| TCP client | connect, partial send/recv, EOF | refused/reset/timeout, EINPROGRESS, double connect |
| TCP server | listen/accept and reply | backlog clamp, nonblocking empty accept, close listener |
| FD lifecycle | dup/fork sharing, cloexec, final close | table full, dup2 replacement, close while blocked |
| Readiness | POLLIN/POLLOUT/HUP/ERR and timeout | invalid FD, zero timeout, signal wake, spurious network wake |
| Ring 3 | static-musl framed echo fixture | bad pointers/lengths/family/options return errno, never panic |
| Regression | full unfiltered boot suite | no NIC boot remains usable; GUI and two terminals remain responsive |

Required commands at the end of each relevant unit:

```sh
cargo fmt --check
cargo check
./test.sh network
./test.sh network_userland
./test.sh --skip-userland
```

Before completion, also run the full unfiltered `./test.sh` in one boot and an
interactive `./build.sh` smoke test covering DHCP plus the selected BusyBox
commands. Capture serial evidence for the lease and network counters without
adding payload dumps.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Heap buffers cross noncontiguous physical pages | Page-contained/aligned DMA objects; explicit translation failure; no virtual-address fallback |
| Device writes an invalid completion | Validate descriptor ID and length before indexing; count/drop instead of panic |
| Network and process locks deadlock | Stage data and IDs; prohibit overlapping locks; wake only after dropping network lock |
| Blocking syscall loses registers | Reuse the fixed syscall-frame restart primitive and add a callee-saved integration regression |
| Polling burns CPU or adds latency | Use smoltcp deadlines, active/idle sleep caps, and counters; defer IRQ routing |
| DHCP makes tests flaky | QEMU-local restricted backend, fixed MAC, explicit PIT deadlines, no external server |
| TCP buffers exhaust the heap | Fixed socket cap and per-kind quotas; fail with `ENOBUFS`/`EMFILE` |
| BusyBox silently needs more ABI | Land the focused musl fixture first, trace unknown syscalls, enable applets one at a time |
| smoltcp leaks into public kernel types | Keep all smoltcp types private to `src/net/` |
| QEMU changes its implicit NIC | Explicit modern virtio-net model and MAC in both scripts |
| Test networking reaches host/Internet | `restrict=on` in `test.sh`; repository-owned guest-forwarded echo endpoint only |

## Completion criteria

This plan is complete only when all of the following are true:

- A normal QEMU boot initializes the explicit modern VirtIO NIC and acquires a
  DHCP lease without blocking boot.
- Ethernet/ARP/IPv4/ICMP/UDP/TCP/DHCP work through the bounded smoltcp-backed
  stack.
- The documented Linux socket subset works from an unmodified static-musl
  binary, including blocking, nonblocking, `poll`, partial stream I/O, and
  errno behavior.
- Socket FD lifetime is correct across close, dup, dup2, fork, and process exit.
- No lock or user pointer survives a blocking yield.
- Network tests are hermetic, deadline-bounded, and pass in the full unfiltered
  suite.
- Boot without a NIC remains functional and socket creation returns an ordinary
  network/device error.
- Numeric-address BusyBox commands advertised in documentation work from zsh;
  applets requiring deferred DNS/TLS/interface-control features remain disabled.
- Architecture, subsystem guidance, roadmap, and user-facing docs describe the
  delivered scope and its limits accurately.

## Follow-ups

1. DNS socket integration plus `/etc/resolv.conf` ownership/update semantics.
2. PCI INTx/MSI-X receive notifications and deadline-only timer polling.
3. Larger TCP listen backlogs and a more scalable waiter index.
4. IPv4 fragmentation/reassembly and path-MTU handling.
5. IPv6/SLAAC/NDP.
6. Interface-control/query ioctls or a small `/proc/net`/`/sys` surface.
7. TLS-capable userland and certificate/time infrastructure.
8. Agent-to-agent authenticated protocol and resource quotas.
