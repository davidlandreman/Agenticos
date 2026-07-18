---
title: "feat: DHCP-backed DNS resolution"
status: complete
created: 2026-07-17
completed: 2026-07-17
plan_type: feat
depth: deep
related_docs:
  - CLAUDE.md
  - src/net/CLAUDE.md
  - src/fs/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - docs/ARCHITECTURE.md
  - docs/IMPLEMENTATION_PLAN.md
  - docs/plans/2026-07-17-002-feat-basic-network-stack-plan.md
  - https://git.musl-libc.org/cgit/musl/tree/src/network/resolvconf.c?h=v1.2.5
  - https://git.musl-libc.org/cgit/musl/tree/src/network/res_msend.c?h=v1.2.5
  - https://www.qemu.org/docs/master/system/invocation.html
---

# feat: DHCP-backed DNS resolution

## Summary

Turn the DNS-server metadata already retained from DHCP into hostname lookup for
ordinary static-musl programs:

```text
DHCP option 6
    |
NetworkConfig.dns_servers
    |
kernel-managed /etc/resolv.conf
    |
musl getaddrinfo()/resolver
    |
existing Linux UDP/TCP socket ABI
    |
DNS server (normally QEMU 10.0.2.3)
```

Do not add a second DNS parser to the kernel and do not enable smoltcp's DNS
socket. The committed BusyBox and zsh binaries are statically linked against
musl, whose resolver already reads `/etc/hosts` and `/etc/resolv.conf`, sends A
and AAAA questions over UDP, and falls back to TCP for truncated replies. The
kernel's job is to publish DHCP state with clear ownership semantics and make
the existing Linux socket ABI match the resolver's real call pattern.

The delivered user-visible scope is IPv4 transport and IPv4 destinations:
hostname forms of BusyBox `ping`, `nc`, and HTTP-only `wget`, plus an
`nslookup` diagnostic applet. IPv6 transport, TLS/HTTPS, DNSSEC validation,
mDNS, a caching daemon, and a general NSS framework remain out of scope.

## Implementation outcome

Completed on this branch. The network poll returns a copied configuration
outcome, then publishes resolver state only after the global network lock is
released. Boot now initializes a kernel-managed runtime `/etc` after overlay
restore and before network discovery; DHCP changes atomically replace or clear
`/etc/resolv.conf`. The old `/etc` to `/host/etc` rewrite and staging were
removed, managed-path writes are rejected, and negative `pollfd` entries now
match Linux behavior.

BusyBox ships `nslookup`, hostname forms of `nc` and HTTP `wget` are covered by
restricted-QEMU aliases, and the static-musl fixture covers `getaddrinfo`,
negative poll entries, and vectored TCP I/O. Both committed ELFs were rebuilt.
The full booted suite passes all 767 tests, including the resolver publication
and no-NIC paths. A separate unrestricted QEMU smoke successfully ran the
rebuilt BusyBox `nslookup example.com` through DHCP-provided resolver state.

## Current-state evidence

- `src/net/config.rs` already stores up to three DHCP-provided IPv4 DNS server
  addresses, and `src/net/stack.rs` updates that snapshot on lease configure,
  renewal, and loss.
- The snapshot accessor in `src/net/mod.rs` is test-only. Production userland
  cannot observe it.
- `src/userland/path.rs::apply_fs_rewrite` currently rewrites all `/etc/...`
  paths to the read-only `/host/etc/...` staging directory. `build.sh` and
  `test.sh` place only `passwd` and `group` there; no `resolv.conf` exists.
- The root filesystem is now `overlay(tmpfs, boot-FAT)`, so a small runtime
  `/etc` is practical. It was not available when the original `/host/etc`
  rewrite was introduced.
- `src/userland/network_syscalls.rs` already implements the resolver's main
  Linux calls: IPv4 datagram/stream sockets, `bind`, `connect`, `sendto`,
  `recvfrom`, `sendmsg`, `recvmsg`, socket options, `poll`, and monotonic time.
- musl's UDP resolver keeps inactive TCP-fallback entries in its `pollfd`
  array with `fd = -1`. `src/userland/syscalls.rs::poll_common` currently
  reports those as `POLLNVAL`; Linux requires negative descriptors to be
  ignored with `revents = 0`.
- BusyBox is compiled with `CONFIG_NSLOOKUP=n`, and
  `src/userland/bin_namespace.rs::APPLETS` therefore omits `nslookup`.
- `test.sh` deliberately uses QEMU user networking with `restrict=on`.
  Restricted libslirp omits the DHCP router/DNS options and blocks arbitrary
  guest UDP; QEMU's `guestfwd` facility is TCP-only. A positive public DNS
  query therefore cannot be a deterministic required test in that topology.

## Goals

- Publish every configured DHCP IPv4 nameserver to `/etc/resolv.conf` in offer
  order, and update it when the lease changes.
- Remove stale resolver state on lease loss and on every boot before a new
  lease arrives.
- Give `/etc` a coherent writable-overlay-backed runtime home for the existing
  minimal `passwd`, `group`, and `hosts` files instead of redirecting all of
  `/etc` to `/host`.
- Keep `/etc/resolv.conf` kernel-owned while DHCP owns interface
  configuration; userland reads it but cannot replace, truncate, unlink, or
  rename it.
- Make musl's UDP query and TCP fallback paths work without patching or
  rebuilding libc.
- Support hostname arguments in the already-supported IPv4 BusyBox network
  commands and expose `nslookup` for diagnostics.
- Preserve numeric IPv4 behavior, no-NIC boot, bounded socket resources,
  network/process lock ordering, and the restricted default test suite.

## Non-goals

- IPv6 sockets, AAAA connectivity, SLAAC, or IPv6 DNS-server addresses.
- TLS, HTTPS, certificates, wall-clock trust, DNS-over-TLS, or DNS-over-HTTPS.
- DNSSEC validation, EDNS policy, mDNS/LLMNR, multicast, service discovery, or
  a local recursive/caching daemon.
- A kernel DNS wire parser, kernel hostname API, custom libc, or smoltcp DNS
  types escaping into userland.
- Search-domain support in the first slice. `NetworkConfig` does not retain
  DHCP option 15/119 today; the generated file contains nameservers only.
- Network configuration mutation through `udhcpc`, netlink, `ifconfig`, or
  user edits to `resolv.conf`.
- Making an Internet-dependent DNS lookup part of `./test.sh`.

## Key decisions

### Reuse musl's resolver; do not implement DNS twice

Static musl already supplies `getaddrinfo`, `/etc/hosts` lookup, DNS message
construction/parsing, retries, parallel A/AAAA questions, and truncated-reply
TCP fallback. AgenticOS should provide the Linux substrate that code expects.

Keep smoltcp's DNS feature disabled. Enabling it would create a kernel-only
resolver that ordinary unmodified Linux binaries cannot call, while retaining
the need to support musl for BusyBox and future ports.

### Publish a real managed file in the root overlay

Create `/etc` after the root overlay and `/data` restore are complete. Seed:

```text
/etc/passwd       root:x:0:0::/root:/bin/zsh
/etc/group        root:x:0:
/etc/hosts        127.0.0.1 localhost
/etc/resolv.conf  generated only while DHCP supplies at least one DNS server
```

Then retire the broad `/etc/... -> /host/etc/...` rewrite and the matching
host-share staging in `build.sh` and `test.sh`. A real runtime file gives
normal `open`, `read`, `stat`, `fstat`, `lseek`, `cat`, and directory-listing
semantics without adding another virtual-FD class to the syscall layer.

`/etc/resolv.conf` is runtime state, not durable configuration. The boot path
must remove any restored copy before networking starts. DHCP publication may
appear in an overlay persistence blob after `sync`, but the next boot always
clears and regenerates it, so a previous lease can never become the active
resolver configuration.

### Move network initialization after filesystem initialization

`kernel::init` currently starts the network worker before IDE/VFS setup. Move
`net::init()` until after root/host/data mounts, overlay restoration, and
managed `/etc` initialization. No existing filesystem path depends on the NIC,
and production boot still does not wait for DHCP.

This ordering lets every DHCP event publish configuration without a
"filesystem not ready" mode and prevents a preempted network worker from
racing the mount sequence.

### Never perform filesystem work while holding `NETWORK`

Change the poll orchestration to capture `(socket_changed, config_changed,
config_snapshot)` while `NETWORK` is held, then drop the lock before updating
`/etc/resolv.conf` or waking processes.

Publishing is an atomic replace in the overlay upper:

1. render at most three `nameserver a.b.c.d\n` lines into a bounded `String`;
2. create/truncate `/etc/.resolv.conf.new`;
3. write the complete contents;
4. rename it over `/etc/resolv.conf`;
5. on deconfigure/no nameservers, unlink both the live and temporary paths.

Open readers retain their old tmpfs body; the next libc lookup opens the new
snapshot. Publication failure is a warning and must not stop NIC polling or
boot.

Do not add resolver `options` in v1. musl's defaults remain authoritative, and
the kernel should not silently invent timeout/search policy that DHCP did not
provide.

### Treat the generated resolver file as DHCP-owned

Add one normalized-path guard used by every mutation path (`open` with write
flags, `truncate`, `unlink`, `rename`, and directory mutations affecting
`/etc`). Preserve the old read-only behavior of `passwd` and `group`, and apply
it to `hosts`, `resolv.conf`, and the resolver temporary file: return `EROFS`
for content writes and `EPERM` for attempts to remove/replace the managed
`/etc` namespace. Kernel-internal VFS calls used by the publisher bypass the
userland guard.

This matches the existing rule that DHCP owns address and route state. It also
prevents a root user process from racing a lease renewal and creating resolver
state that the next poll immediately overwrites.

### Fix the exact Linux `poll` contract musl relies on

In `poll_common`, any `pollfd.fd < 0` must produce `revents = 0`, must not
increment the ready count, and must not count as a network socket. Unknown
nonnegative descriptors continue to report `POLLNVAL`.

Retain the current bounded `nfds`, restart-stable timeout, and conservative
network wake behavior. This small distinction prevents musl's inactive TCP
fallback slots from turning DNS waits into a busy loop.

### Keep required tests hermetic and add an explicit live smoke

The required suite remains `restrict=on` and does not contact public DNS.
Coverage is split deliberately:

- pure/in-kernel tests cover formatting, lease transition publication,
  stale-file removal, mutation guards, and negative-`pollfd` behavior;
- the committed static-musl fixture covers `/etc/hosts` lookup and the exact
  `sendmsg`/`recvmsg`/multi-entry-`poll` ABI shape against existing QEMU-local
  TCP services;
- BusyBox hostname tests use test-only `/etc/hosts` aliases for the existing
  `10.0.2.100` echo and `10.0.2.101` HTTP endpoints, proving application-level
  hostname handling without claiming that hosts-file lookup is DNS;
- an interactive unrestricted-QEMU smoke performs the positive DHCP-DNS check.

Do not weaken `restrict=on`, silently depend on the host resolver, or call a
public domain from the automatic suite. If a future QEMU backend supplies a
portable repository-owned UDP peer, promote the live DNS smoke to a required
test then.

## Proposed code layout

```text
src/
  net/
    config.rs                   production snapshot + formatter inputs
    mod.rs                      poll outcome; publish after NETWORK unlock
    resolver_config.rs          render and atomically publish resolv.conf
    stack.rs                    explicit config-change outcome
    CLAUDE.md                   resolver ownership/lock rules
  userland/
    etc.rs                      seed/clear managed runtime /etc
    path.rs                     remove broad /etc host rewrite
    syscalls.rs                 poll fix + managed-path mutation guard
    mod.rs
  tests/
    network.rs                  config rendering/publication transitions
    network_userland.rs         hosts-backed hostname applet smokes

userland/
  apps/network-test/            resolver ABI fixture additions
  apps/busybox/busybox.config   enable nslookup
  prebuilt/network/NETTEST.ELF  refreshed fixture
  prebuilt/BB.ELF               refreshed BusyBox

build.sh, test.sh               remove obsolete /host/etc staging
README.md, userland/README.md, docs/ARCHITECTURE.md,
docs/IMPLEMENTATION_PLAN.md     delivered scope and remaining limits
```

## Implementation units

### U1 — Pin the resolver ABI contract with focused tests

**Goal:** Record the actual musl call shape before changing configuration
publication.

**Files:** `userland/apps/network-test/src/network_test.c`,
`src/tests/network_userland.rs`, optionally syscall trace comments.

1. Extend the fixture with `getaddrinfo("localhost", ..., AF_INET, ...)` and
   assert it returns `127.0.0.1` once the managed hosts file lands.
2. Exercise `poll` with an array containing `fd = -1` plus a real socket and
   assert the negative entry stays at `revents = 0`.
3. Exercise two-iovec `sendmsg`/`recvmsg` over the existing QEMU-local TCP echo
   service. This pins the same scatter/gather path used by musl's DNS TCP
   fallback without needing a public DNS server.
4. Run once with unknown-syscall tracing and record any additional musl calls.
   Do not add speculative syscalls.

**Exit:** the focused fixture identifies only already-supported calls plus the
known negative-`pollfd` bug.

### U2 — Replace `/host/etc` with managed runtime `/etc`

**Goal:** Establish coherent configuration-file ownership before publishing
DNS state.

**Files:** new `src/userland/etc.rs`, `src/userland/mod.rs`,
`src/userland/path.rs`, `src/kernel.rs`, `build.sh`, `test.sh`, path/userland
tests.

1. Add an idempotent `userland::etc::init()` that ensures `/etc` exists, writes
   the minimal `passwd`, `group`, and `hosts` files, removes stale resolver
   temporary/live files, and logs recoverable failures.
2. Under `feature = "test"`, append stable hosts aliases for the existing
   restricted QEMU endpoints; keep production hosts limited to localhost.
3. Remove `apply_fs_rewrite` and route normalized `/etc/...` paths to the root
   overlay normally. Delete or rewrite its tests accordingly.
4. Remove `/host/ETC/{PASSWD,GROUP}` staging from both launch scripts.
5. Reorder boot to initialize network after mounts, overlay restore, and
   `etc::init()`. Keep the absence of a filesystem or NIC nonfatal.

**Exit:** zsh still starts, musl account lookup still finds root, `cat
/etc/hosts` works, and a restored stale `resolv.conf` is absent before DHCP.

### U3 — Publish DHCP nameservers atomically

**Goal:** Make the current lease the sole source of resolver configuration.

**Files:** new `src/net/resolver_config.rs`, `src/net/config.rs`,
`src/net/mod.rs`, `src/net/stack.rs`, `src/net/CLAUDE.md`,
`src/tests/network.rs`.

1. Expose a production-safe copied `NetworkConfig` snapshot; no smoltcp type
   crosses the module boundary.
2. Make the stack poll report configuration change separately from ordinary
   socket readiness.
3. After releasing `NETWORK`, render one nameserver line per valid offered
   server (maximum three) and atomically replace the managed file.
4. Remove the live/temp file on deconfigure or zero-server configuration.
5. Update only when the snapshot changes, so routine 100 Hz polling performs
   no filesystem work.
6. Unit-test ordering, count bounds, repeated identical leases, renewal with a
   different server set, deconfigure, failed publication, and boot cleanup.

**Exit:** `cat /etc/resolv.conf` tracks DHCP changes and no filesystem/network
lock overlap exists.

### U4 — Close Linux ABI gaps used by musl DNS

**Goal:** Make resolver waits and TCP fallback Linux-shaped.

**Files:** `src/userland/syscalls.rs`,
`src/userland/network_syscalls.rs` only if tracing exposes a concrete gap,
`userland/apps/network-test/src/network_test.c`, refreshed
`userland/prebuilt/network/NETTEST.ELF`.

1. Ignore negative `pollfd` entries exactly as Linux does.
2. Preserve `POLLNVAL` for unknown nonnegative fds and preserve timeout state
   when the remaining entries are real sockets.
3. Verify existing `sendto`/`recvmsg` handling accepts musl's `MSG_NOSIGNAL`
   and source-address output shape. Unknown harmless message flags may be
   ignored; unsupported control data remains `ENOPROTOOPT`.
4. Verify `TCP_FASTOPEN_CONNECT` returning `ENOPROTOOPT` drives musl into its
   ordinary nonblocking `connect` fallback; do not implement TCP Fast Open.
5. Refresh and commit the mandatory fixture through its Makefile.

**Exit:** the resolver-shaped ABI fixture completes without unknown syscalls,
busy looping, leaked sockets, or unbounded waits.

### U5 — Protect managed configuration paths

**Goal:** Prevent userland from racing or persisting an override of
DHCP-owned state.

**Files:** `src/userland/etc.rs`, `src/userland/syscalls.rs`, userland syscall
tests.

1. Centralize normalized managed-path classification; do not duplicate string
   checks across handlers.
2. Reject write/truncate/unlink/rename of the seeded `passwd`, `group`, and
   `hosts` files, plus `/etc/resolv.conf` and its temporary publisher path.
3. Reject rename/rmdir operations that would move or remove managed `/etc`.
4. Keep read/stat/access/open-directory behavior normal through the VFS.
5. Test normalized `.`/`..` aliases so path traversal cannot bypass the guard.

**Exit:** userland can inspect but cannot replace DHCP-owned resolver state;
kernel publication still succeeds through internal VFS APIs.

### U6 — Enable hostname-facing BusyBox behavior

**Goal:** Expose and verify the user-visible DNS feature.

**Files:** `userland/apps/busybox/busybox.config`,
`src/userland/bin_namespace.rs`, `userland/apps/busybox/README.md`, refreshed
`userland/prebuilt/BB.ELF`, `src/tests/network_userland.rs`.

1. Set `CONFIG_NSLOOKUP=y`, `CONFIG_FEATURE_NSLOOKUP_BIG=y`, and keep IPv6,
   DNS daemons, DHCP clients, and TLS disabled.
2. Rebuild BusyBox, regenerate/compare its applet list, add `nslookup` to the
   sorted virtual `/bin` namespace, and refresh the committed ELF.
3. Add restricted-QEMU tests using the test-only hosts aliases for hostname
   forms of `nc` and HTTP `wget`. Keep numeric tests as regressions.
4. Do not run `nslookup` automatically in restricted mode; its positive path
   is covered by the explicit live smoke below.

**Exit:** fresh clones expose `/bin/nslookup`; hosts-backed hostname applet
tests pass without host/public network access.

### U7 — Documentation and live acceptance

**Goal:** Advertise exactly the resolver behavior delivered.

**Files:** `CLAUDE.md`, `src/net/CLAUDE.md`, `README.md`,
`userland/README.md`, `userland/apps/busybox/README.md`,
`docs/ARCHITECTURE.md`, `docs/IMPLEMENTATION_PLAN.md`.

1. Replace "numeric IPv4 only" with DHCP-backed hostname resolution where
   appropriate.
2. Document `/etc/resolv.conf` ownership, lease-change behavior, lack of
   search domains/cache/DNSSEC, IPv4-only destination support, and the
   distinction between DNS and TLS.
3. Keep IPv6, HTTPS/TLS, NIC interrupts, and interface mutation listed as
   deferred.
4. On unrestricted interactive QEMU, verify:

   ```sh
   cat /etc/resolv.conf
   nslookup example.com
   ping -c 1 -W 2 example.com
   wget -q -O - http://example.com/
   ```

5. Repeat `cat /etc/resolv.conf` after a forced lease loss/renewal where
   practical, or cover that transition with the deterministic in-kernel test
   if QEMU cannot trigger it reliably.

**Exit:** documentation and observed behavior agree; passing DNS is not
misrepresented as HTTPS support.

## Validation matrix

| Layer | Happy path | Failure/edge path |
|---|---|---|
| Formatter | 1–3 IPv4 nameservers, offer order | zero servers, count clamp, repeated config |
| Publication | atomic create/replace | stale boot file, write/rename failure, deconfigure |
| Ownership | read/stat/list managed files | write, truncate, unlink, rename, traversal alias |
| Poll ABI | negative fd ignored, socket wakes | unknown positive fd `POLLNVAL`, timeout restart |
| UDP resolver ABI | `sendto` + `recvmsg` shape | `EAGAIN`, harmless flags, bounded buffers |
| TCP fallback ABI | nonblocking connect + iovecs | unsupported Fast Open, partial send/receive |
| libc | hosts lookup and resolver config parse | no config/no NIC returns ordinary failure |
| BusyBox | hostname `ping`/`nc`/HTTP `wget`, `nslookup` | NXDOMAIN/timeout is finite; HTTPS still rejected |
| Regression | numeric commands and full boot suite | network off, filesystem absent, lease loss |

Required commands:

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland network network_userland
AGENTICOS_TEST_NETWORK=off ./test.sh --skip-userland \
  'network::test_network_absence_reports_network_down'
./test.sh --skip-userland
```

Before completion, refresh both changed committed ELFs through their documented
Makefiles and run one unrestricted interactive `./build.sh` DNS smoke. The
automatic suite remains public-network-independent.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Filesystem update while `NETWORK` is held | Capture copied outcome, drop lock, then publish |
| Persisted stale lease | Clear managed resolver files after overlay restore on every boot |
| Torn config read | Write temporary file and rename atomically; open handles retain old body |
| DHCP renewal causes write churn | Publish only when copied `NetworkConfig` changes |
| musl DNS wait spins | Ignore negative `pollfd` entries; fixture pins behavior |
| DNS TCP fallback silently breaks | Exercise iovec sendmsg/recvmsg and unsupported Fast Open fallback |
| Tests accidentally reach Internet | Keep `restrict=on`; use hosts aliases and QEMU-local TCP endpoints |
| Hosts test is mistaken for DNS coverage | Name it explicitly and require separate unrestricted live DNS smoke |
| Enabling `nslookup` expands ABI | Use IPv4-only big applet, trace first, keep daemons/TLS/IPv6 disabled |
| User overrides race DHCP | Central managed-path guard across every mutation syscall |

## Completion criteria

- A normal QEMU boot with DHCP DNS data produces a readable
  `/etc/resolv.conf` containing the offered IPv4 servers in order.
- Lease renewal replaces the file atomically; lease loss and reboot remove
  stale server addresses.
- No filesystem operation occurs under `NETWORK`, and no wake occurs before
  that lock is dropped.
- Unmodified static-musl hostname lookup works through the existing Linux
  socket ABI, including correct negative-`pollfd` behavior and TCP fallback.
- BusyBox hostname forms of `ping`, `nc`, and HTTP `wget` work; `nslookup` is
  present; numeric IPv4 behavior remains intact.
- Managed resolver state is readable but not user-mutable.
- Required tests are bounded, hermetic, and pass with no NIC where applicable.
- Documentation does not claim IPv6, TLS, DNSSEC, caching, search domains, or
  interrupt-driven networking.

## Follow-ups

1. Retain DHCP domain/search options and publish `search`/`domain` policy.
2. Add a portable repository-owned UDP network peer so positive DNS can become
   a hermetic required integration test.
3. Add IPv6 sockets, DHCPv6/SLAAC DNS sources, and AAAA connectivity.
4. Add a caching resolver daemon with TTL, negative-cache, and per-process
   resource policy if repeated lookups become a performance issue.
5. Add TLS, certificate roots, and trusted wall-clock synchronization.
