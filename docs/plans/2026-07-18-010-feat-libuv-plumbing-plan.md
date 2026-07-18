---
title: "feat: complete the Linux syscall plumbing needed by libuv"
type: feat
status: complete
date: 2026-07-18
completed: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-07-17-002-feat-reusable-memory-and-sparse-user-address-spaces-plan.md
  - docs/plans/2026-07-18-005-refactor-unified-kernel-ring3-scheduler-plan.md
  - docs/plans/2026-07-18-006-feat-smp-support-plan.md
  - docs/plans/2026-07-18-009-feat-musl-pthread-runtime-plan.md
  - src/mm/CLAUDE.md
  - src/process/CLAUDE.md
  - src/tests/CLAUDE.md
  - src/userland/CLAUDE.md
---

# feat: complete the Linux syscall plumbing needed by libuv

## Implementation result

Completed on 2026-07-18. The ABI now implements the eight missing syscall
families with bounded semantics, shared lost-wake-safe descriptor readiness,
last-close epoll pruning, task-local alternate signal stacks, stable anonymous
mapping identities, refcount-neutral resident-page relocation, and the
single-home-CPU private-expedited membarrier profile.

Acceptance is covered by focused in-kernel tests and the committed 9,168-byte
static-musl `UVPLUMB.ELF` fixture. The fixture traverses the production VFS,
loader, scheduler, syscall dispatcher, and process teardown, and exercises
legacy/modern eventfd and epoll, epoll edge delivery, socketpair, sched_yield,
sigaltstack, madvise, mremap, and membarrier. The focused fixture test and 239
related userland/VM/switch/procfs regression tests pass under four-vCPU QEMU.

The repository-wide run also reached the pre-existing
`network_userland::test_links_https_valid_hostname` deadline failure. An
unchanged detached `origin/main` worktree reproduces the same timeout and
Links child state, so it is recorded as a baseline failure rather than a
regression from this implementation.

## Summary

Complete the remaining Tier 3 Linux x86-64 ABI surface needed to bring up a
static-musl libuv event loop and the runtimes that embed it. The kernel already
dispatches the adjacent process/file calls (`pipe2`, `pread64`, `pwrite64`,
`dup2`, `prlimit64`, `gettid`, and `wait4`), but `src/userland/abi.rs` has no
entries for:

```text
epoll       eventfd      socketpair   sched_yield
sigaltstack madvise      mremap       membarrier
```

This plan adds real, bounded semantics rather than success stubs. The central
deliverable is a restart-safe descriptor-readiness layer shared by
`poll`/`select` and the new epoll implementation. `eventfd` and a full-duplex
local stream pair become first-class open-file descriptions, including
`dup`/`fork`, `fcntl`, blocking I/O, close, and readiness behavior. The VM and
signal calls integrate with the existing VMA, page-mapper, task, and signal
frame models. `sched_yield` and `membarrier` honor the current pthread design,
where every thread group is pinned to one CPU until remote user-TLB shootdown
exists.

Upstream libuv's current Linux backend directly uses `epoll_create1`,
`epoll_ctl`, `epoll_pwait`, `eventfd`, `socketpair`, and `sched_yield`:

- [libuv Linux event loop](https://github.com/libuv/libuv/blob/v1.x/src/unix/linux.c)
- [libuv async/eventfd path](https://github.com/libuv/libuv/blob/v1.x/src/unix/async.c)
- [libuv socketpair helper](https://github.com/libuv/libuv/blob/v1.x/src/unix/tcp.c)
- [libuv cooperative spin fallback](https://github.com/libuv/libuv/blob/v1.x/src/uv-common.c)

`sigaltstack`, `madvise`, `mremap`, and `membarrier` are included in the same
tier because libc allocators, sanitizing/runtime layers, and embedders probe or
use them around libuv even though the current libuv source does not call them
directly. They receive deliberately finite contracts below; this is not a
promise of full Linux VM, NUMA, or cross-CPU barrier compatibility.

---

## Current state and feasibility findings

### Poll/select already contain most readiness producers

`src/userland/syscalls.rs::fd_readiness` can currently snapshot:

- stdin and GUI queues;
- regular, virtual, directory, and urandom descriptors;
- pipe buffer/peer state;
- smoltcp socket state.

`poll_common` and `select_handler` both use that snapshot, call
`net::poll_once()`, and block by re-firing the original syscall. Pipe, GUI,
socket, close, and timer paths conservatively wake `WaitingForNetwork`
processes. Despite the name, `NetworkWaitState` already serves general
`poll`/`select` deadlines.

The reusable pieces are therefore present, but they need one cleanup before
epoll is safe:

1. move readiness types and sampling out of the monolithic syscall file;
2. sample a cloned `FdSlot`, not only a live numeric fd, so an epoll
   registration can retain the watched open-file description;
3. close the scan-to-block lost-wake window with a global readiness sequence;
4. rename the restart/deadline state so it truthfully covers poll, select,
   epoll, eventfd, and local streams.

### The existing wake path has a race that epoll would amplify

Today a readiness source can change after `poll_common` scans all descriptors
but before `block_current_ring3_and_yield` publishes the blocked reason. The
source's wake sees no blocked process, and the waiter can sleep indefinitely.
Interactive workloads usually hide this because networking and PIT timeouts
produce later wakes; an infinite `epoll_pwait` on an eventfd cannot rely on
that accident.

Add a global monotonically increasing readiness sequence. Every producer
increments it before waking waiters. A new readiness-specific park helper:

1. captures the caller's restart state;
2. publishes `WaitingForReadiness`;
3. compares the sequence with the value observed before the readiness scan;
4. immediately makes the task ready again if it changed; otherwise dispatches
   away.

An event before publication is caught by the sequence recheck; an event after
publication finds the blocked reason. This is the same compare-and-park shape
already required by the futex implementation.

### Epoll must understand libuv's eventfd edge trigger

The current libuv Linux backend registers its async eventfd with `EPOLLET` and
does not drain the eventfd on Linux. A pure level-trigger implementation would
keep returning that permanently-readable fd and spin the loop. A simplistic
“ready now but not ready last scan” edge model would lose later eventfd writes
because its counter never returns to zero.

`EventFd` therefore carries a notification generation incremented on every
successful write. An edge-triggered epoll registration remembers the last
generation it delivered. For ordinary descriptors the first release uses
ready-state transitions; eventfd uses its explicit generation. Libuv uses
edge-triggering on the eventfd path and level-triggering for its normal pipe,
TTY, TCP, and UDP watchers, so this is the required correctness boundary.

### The FD table can represent the new objects without a global registry

`FdSlot` is cloneable and already distinguishes descriptor-local
`FD_CLOEXEC` from open-file-description state such as `O_NONBLOCK`. Add:

```text
EventFd { handle: Arc<EventFd>, cloexec }
Epoll   { handle: Arc<EpollInstance>, cloexec }
LocalStream { handle: Arc<LocalStreamEndpoint>, cloexec }
```

`dup` and `fork` then share counters, epoll interest sets, local-stream
buffers, and nonblocking flags naturally, while every descriptor retains its
own close-on-exec bit. No kernel-global epoll or eventfd registry is needed.

Epoll registrations store a cloned target `FdSlot` plus the numeric fd used by
`epoll_ctl`. Retaining the slot keeps the underlying open-file description
alive while it is legitimately registered, which is closer to Linux than
re-looking up a possibly reused fd on every wait. Add
`FdSlot::same_open_description` and an FD-table close/replacement hook: after
removing a descriptor, if no remaining descriptor refers to that description,
prune it from every epoll instance in the table and drop the retained clone.
If a dup remains, the registration and underlying description stay alive.
Nested epoll descriptors are rejected in the first release to avoid reference
cycles and recursive readiness walks.

### A socketpair should not enter the IPv4 registry

The network subsystem intentionally supports `AF_INET` only and every network
socket consumes a smoltcp registry entry. `socketpair(AF_UNIX, SOCK_STREAM)` is
local IPC and should keep working when no NIC is configured.

Build each endpoint from two existing bounded pipe buffers:

```text
endpoint 0 write -> buffer 0 -> endpoint 1 read
endpoint 1 write -> buffer 1 -> endpoint 0 read
```

Wrap the inbound read handle and outbound write handle in one
`LocalStreamEndpoint`. Final endpoint drop closes both directions; duplicated
descriptors share the endpoint through `Arc`. This reuses the mature short
read/write, EOF, backpressure, wake, and bounded-memory behavior without
pretending an `AF_UNIX` socket is an IPv4 socket.

### The VMA model supports discard, but moving needs one mapper primitive

`madvise(MADV_DONTNEED)` can retain VMA metadata and unmap resident leaves.
The next fault already recreates anonymous pages as zero and private-file
pages from their backing file. That gives useful allocator semantics with no
new paging mechanism.

`mremap(MREMAP_MAYMOVE)` is different. `unmap_page_from` always releases a
frame reference, and there is no public refcount-neutral operation that moves
one installed leaf to a new virtual address. Add a targeted
`MemoryMapper::move_user_leaf`/`move_user_range` primitive that:

- maps the same physical frame and flags at the destination;
- clears the old leaf without changing the frame refcount;
- prunes empty source page-table paths;
- flushes source/destination addresses when that L4 is active;
- rolls back already-moved leaves if a destination page-table allocation
  fails.

The thread-group affinity invariant means only the local CPU can have this
address space active. This plan must not weaken that invariant; remote TLB
shootdown remains a prerequisite for parallel execution of one group.

### Signal delivery has one clear alternate-stack insertion point

`deliver_signal` already chooses a user stack pointer, builds the private
168-byte signal frame, and transfers to the handler. Add a task-local
`SignalAltStack` record and select `ss_sp + ss_size` when:

- the action has `SA_ONSTACK`;
- the alternate stack is enabled; and
- the interrupted user RSP is not already within it.

`rt_sigreturn` restores the saved pre-signal RSP, so no extra active-depth
counter is required. `sigaltstack` can compute `SS_ONSTACK` from the syscall's
current user RSP and the registered interval. Fork inherits the record,
pthread clone starts disabled because musl's `CLONE_VM` profile does not
inherit an active alternate stack, and exec disables it.

### Membarrier can be correct only for the advertised commands

The kernel pins every member of a pthread group to one home CPU. Advertise
only:

```text
MEMBARRIER_CMD_PRIVATE_EXPEDITED
MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
```

`QUERY` returns those bits. Registration is stored on the thread-group owner.
`PRIVATE_EXPEDITED` requires prior registration and executes a sequentially
consistent fence. Because no sibling in the same address space can run
simultaneously on another CPU, that is a truthful first implementation. When
group affinity is eventually removed, this command must grow an IPI/ack
protocol before the scheduler permits the address space on multiple CPUs.

---

## Goals

1. Add Linux x86-64 syscall numbers and dispatch for the complete epoll,
   eventfd compatibility aliases, socketpair, sched_yield, sigaltstack,
   madvise, mremap, and membarrier surface defined below.
2. Provide one race-free readiness sampler and restartable blocking mechanism
   shared by poll, select, epoll, eventfd, local streams, GUI descriptors,
   pipes, and IPv4 sockets.
3. Implement bounded epoll interest sets with `ADD`/`MOD`/`DEL`, level
   triggering, eventfd-compatible `EPOLLET`, timeouts, `EINTR`, descriptor
   sharing, and exact user `epoll_event.data` round trips.
4. Implement eventfd counter, semaphore, blocking/nonblocking, overflow,
   poll/epoll, dup/fork, close-on-exec, and fcntl semantics.
5. Implement full-duplex `AF_UNIX`/`SOCK_STREAM` socketpairs with independent
   endpoint status flags, blocking I/O, backpressure, EOF, poll/epoll,
   dup/fork, and close behavior.
6. Make `sched_yield` actually surrender the current ring-3 entity while
   preserving its post-syscall register state.
7. Implement task-local alternate signal stacks and route `SA_ONSTACK`
   handlers onto them without regressing nested signal return.
8. Implement useful bounded VM semantics: advisory validation and resident
   discard for `madvise`, plus shrink/in-place-grow/anonymous-move for
   `mremap`.
9. Implement the private-expedited membarrier profile that is correct under
   the current group-affinity rule.
10. Prove the ABI through focused in-kernel tests and one booted static-musl
    fixture that exercises cross-thread eventfd/epoll wakeup, socketpair I/O,
    alternate-stack delivery, VM discard/remap, yield, and membarrier probes.
11. Keep the existing poll/select, networking, pthread, fork/exec/wait,
    signals, GUI events, and full kernel suite green.

## Non-goals

- Bundling or shipping libuv, Node.js, Julia, or another libuv embedder. This
  plan prepares and proves the kernel ABI; the actual port is a later tier.
- `io_uring`, inotify/fs-event watching, signalfd, timerfd, Unix filesystem
  path sockets, datagram/seqpacket socketpairs, abstract Unix sockets, or
  listening Unix sockets.
- `SCM_RIGHTS`/descriptor passing. Plain local-stream `read`/`write` and
  vectored I/O are in scope; libuv IPC handle passing is a follow-up.
- General epoll nesting, `EPOLLEXCLUSIVE`, `EPOLLWAKEUP`, a ready-list sized
  beyond the FD-table bound, or perfect Linux behavior for an fd number that
  is closed and reused while another duplicate of the old description lives.
- `epoll_pwait2`. The libuv path uses millisecond `epoll_pwait`.
- Non-null temporary signal masks in `epoll_pwait`. The normal libuv path
  passes `NULL`; `UV_LOOP_BLOCK_SIGPROF` remains deferred until ppoll/pselect
  temporary-mask behavior is implemented consistently.
- `mremap(MREMAP_FIXED)`, `MREMAP_DONTUNMAP`, remapping stacks/ELF/TLS/brk,
  or moving mixed-protection/file-backed VMA sets in the first release.
- Kernel read-ahead, NUMA placement, huge-page policy, page locking, fork
  exclusion, or persistence guarantees for every Linux `madvise` command.
- Global, global-expedited, shared-expedited, or cross-CPU membarrier modes.
- Removing pthread group CPU affinity or adding remote user-TLB shootdown.

---

## ABI contract

### Syscall numbers

Add the Linux x86-64 numbers to `src/userland/abi.rs::nr` and dispatch them:

| Syscall | Number | First-release contract |
|---|---:|---|
| `sched_yield` | 24 | real voluntary ring-3 reschedule |
| `mremap` | 25 | shrink, in-place grow, anonymous `MAYMOVE` |
| `madvise` | 28 | validated hints + `DONTNEED`/`FREE` discard |
| `socketpair` | 53 | `AF_UNIX`, `SOCK_STREAM`, protocol 0 |
| `sigaltstack` | 131 | task-local set/query/disable + `SA_ONSTACK` |
| `epoll_create` | 213 | compatibility alias; positive size required |
| `epoll_wait` | 232 | compatibility alias for wait without mask |
| `epoll_ctl` | 233 | `ADD`, `MOD`, `DEL` |
| `epoll_pwait` | 281 | NULL-mask path, ms timeout |
| `eventfd` | 284 | compatibility alias with flags 0 |
| `eventfd2` | 290 | `CLOEXEC`, `NONBLOCK`, `SEMAPHORE` |
| `epoll_create1` | 291 | flags 0 or `EPOLL_CLOEXEC` |
| `membarrier` | 324 | query/register/private-expedited |

Do not hide unsupported flags behind success. Return Linux-shaped errors so
libc and future runtime probes can choose a fallback.

### Epoll

Use the packed x86-64 12-byte `struct epoll_event` layout:

```text
u32 events
u64 data
```

The interest map is bounded by `FD_TABLE_SIZE`. Supported readiness bits are
`EPOLLIN`, `EPOLLOUT`, `EPOLLERR`, `EPOLLHUP`, and `EPOLLRDHUP`; accept
`EPOLLET` as a behavior flag. `ERR` and `HUP` are reported even when not in
the requested mask. Preserve the caller's opaque 64-bit `data` field exactly.

Error behavior:

- create1 unknown flags -> `EINVAL`;
- non-epoll `epfd` -> `EINVAL`; bad descriptor -> `EBADF`;
- `epfd == target fd`, target epoll, or unsupported event flags -> `EINVAL`;
- duplicate `ADD` -> `EEXIST`;
- `MOD`/`DEL` of a missing registration -> `ENOENT`;
- `maxevents <= 0` -> `EINVAL`;
- invalid output/event pointers -> `EFAULT` before mutation/output;
- timeout `< -1` -> `EINVAL`.

`epoll_pwait` accepts a null signal-mask pointer. A non-null pointer returns
`ENOSYS` in this tier rather than silently ignoring the requested atomic mask
swap. `epoll_wait` and `epoll_pwait(..., NULL, 8)` share the same wait engine.

### Eventfd

Use an open-file-description object with:

- counter range `0..=UINT64_MAX-1`;
- `EFD_SEMAPHORE`, `EFD_NONBLOCK`, and `EFD_CLOEXEC` validation;
- exactly 8-byte reads and writes (`EINVAL` otherwise);
- read drains the counter, or decrements/returns 1 in semaphore mode;
- writing `UINT64_MAX` -> `EINVAL`;
- overflow would block, or return `EAGAIN` when nonblocking;
- zero writes succeed without changing readiness;
- readable when counter is nonzero; writable when at least 1 can be added;
- a notification generation incremented for every successful nonzero write.

Read/write must stage the complete 8 bytes before changing the counter, so a
bad user pointer cannot partially consume or publish a notification.

### Socketpair/local streams

Accept only:

```text
domain   = AF_UNIX (1)
type     = SOCK_STREAM | optional SOCK_NONBLOCK | optional SOCK_CLOEXEC
protocol = 0
```

Return `EAFNOSUPPORT`, `EPROTONOSUPPORT`, or `EINVAL` for other inputs. Reserve
both descriptor slots before publishing either fd; on `EMFILE`, roll back all
handles and leave the user output array untouched.

Both endpoints support `read`, `write`, `readv`, and `writev`; share
`O_NONBLOCK` through dup/fork; expose `O_RDWR` through `F_GETFL`; report socket
mode through `fstat`; return `ESPIPE` from `lseek`; and participate in
poll/select/epoll. Closing the peer yields EOF after buffered inbound bytes
drain and `EPIPE` on later writes. Raising SIGPIPE remains the existing pipe
follow-up; return `EPIPE` consistently now.

Route control-free `sendmsg`/`recvmsg` to the same byte stream if the existing
iovec helpers make this small. Any nonempty ancillary-data request returns
`EOPNOTSUPP`; descriptor passing is not silently discarded.

### Sched yield

`sched_yield()` returns 0 but first snapshots a post-syscall continuation
(`rip` after `SYSCALL`, `rax = 0`), saves FS/FPU state, marks the current user
entity ready, and dispatches through the unified scheduler. If it is selected
again immediately, behavior is still valid. Never call the kernel-thread-only
`process::yield_current` directly from a ring-3 syscall stack.

### Sigaltstack

Implement Linux x86-64 `stack_t` (`ss_sp`, `ss_flags`, padding, `ss_size`) and:

- query with `ss == NULL`;
- enable with flags 0 and size at least 2048 bytes;
- disable with `SS_DISABLE` and no other flags;
- report `SS_ONSTACK` when the live user RSP lies within the registered range;
- reject replacement/disable while on-stack with `EPERM`;
- reject unsupported flags, overflow/noncanonical ranges, and undersized
  stacks with `EINVAL`/`ENOMEM` as appropriate;
- validate writable coverage before accepting a stack.

Honor `SA_ONSTACK` (`0x08000000`) in `deliver_signal`, align the frame at the
alternate stack top, and keep nested handlers on the same stack. Fork copies
the record; pthread clone and exec reset it to disabled.

### Madvise

Require a page-aligned address and a fully mapped page-rounded interval.
Support:

- `MADV_NORMAL`, `MADV_RANDOM`, `MADV_SEQUENTIAL`, `MADV_WILLNEED` as
  validated no-ops;
- `MADV_DONTNEED` by unmapping resident anonymous/private-file/heap leaves
  while retaining the VMAs;
- `MADV_FREE` with the stronger immediate-discard behavior of DONTNEED,
  documented as an allowed first-release simplification.

Reject unsupported advice and special mappings (ELF, TLS, stack) with
`EINVAL`; return `ENOMEM` for holes. A DONTNEED refault must produce zeros for
anonymous/heap pages and original bytes for private-file pages. Discard only
complete pages and never hold the mapper lock across backing-file I/O.

### Mremap

Accept flags 0 or `MREMAP_MAYMOVE`. Require a page-aligned old address,
nonzero sizes, and one uniform private anonymous mmap allocation. Preserve its
protection and resident-page contents.

- Equal rounded size -> return old address.
- Shrink -> remove/unmap the tail and return old address.
- Grow into an immediately free successor gap -> extend metadata in place.
- Blocked grow without `MAYMOVE` -> `ENOMEM`.
- Blocked grow with `MAYMOVE` -> reserve a top-down gap, move resident leaves,
  preserve lazy holes, remove the old range, and return the new base.

Add a stable mapping identifier to mmap-created anonymous VMAs so adjacent
independent mmaps do not merge into an indistinguishable region. VMA splits
retain the identifier; merge requires matching identifiers. `mremap` rejects
a range that is not the complete allocation or has mixed protections.

### Membarrier

Support commands and bits:

```text
MEMBARRIER_CMD_QUERY                         = 0
MEMBARRIER_CMD_PRIVATE_EXPEDITED             = 1 << 3
MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED    = 1 << 4
```

`QUERY` returns the latter two bits. Registration is idempotent and group
owned. Private expedited requires registration, flags 0, and the existing
single-home-CPU group invariant, then executes `fence(Ordering::SeqCst)` and
returns 0. Unsupported commands/flags return `EINVAL` or `ENOSYS`; an
unregistered private command returns `EPERM`.

---

## Design

### Module boundaries

Keep the descriptor objects out of `syscalls.rs`:

```text
src/userland/readiness.rs     shared readiness snapshot, sequence, park/wake
src/userland/eventfd.rs       counter object + syscall handlers/read/write
src/userland/epoll.rs         interest set + create/ctl/wait handlers
src/userland/local_stream.rs  socketpair endpoints and byte-stream helpers
```

`syscalls.rs` retains the generic read/write/readv/writev dispatch and the
small scheduler/signal/VM handlers. `network_syscalls.rs` remains the AF_INET
owner; it should only gain local-stream sendmsg/recvmsg routing if needed.

### Shared readiness model

Refactor `FdReady` into a public-in-userland snapshot:

```text
readable, writable, error, hangup, read_hangup, edge_generation
```

Provide:

- `readiness_for_fd(fd)` for poll/select;
- `readiness_for_slot(&FdSlot)` for epoll's retained descriptions;
- event-mask conversion helpers shared by poll and epoll;
- open-file-description identity comparison used by last-close epoll pruning;
- `notify_readiness_changed()` to increment the sequence then conservatively
  wake `WaitingForReadiness` tasks;
- `block_on_readiness(args, identity, timeout, observed_sequence)` for the
  race-free restart/deadline protocol.

Rename:

```text
NetworkWaitState          -> RestartableWaitState
Process.network_wait      -> readiness_wait
WaitingForNetwork         -> WaitingForReadiness
prepare_network_wait      -> prepare_readiness_wait
expire_network_wait       -> expire_readiness_wait
clear_stale_network_wait  -> clear_stale_readiness_wait
```

AF_INET connect/read/write may continue to use the generalized state; the
identity field already distinguishes syscall + socket. Update every process
constructor and existing test fixture in the same behavior-only change.

### Epoll object and lock discipline

`EpollInstance` owns an interrupt-safe mutex around a bounded `BTreeMap` of
registrations. Each registration contains target fd, cloned slot, requested
mask, opaque data, and edge-delivery state.

Never hold:

- the epoll mutex while looking up or mutating the process FD table;
- the process-table lock while sampling a socket (which takes the network
  lock);
- the epoll mutex across a scheduler yield or user copy.

`epoll_ctl` clones/validates the target before locking the epoll map.
`epoll_wait` snapshots registrations, releases the map, polls the network and
samples readiness, then briefly re-locks only to commit edge generations.
Copy events to user space after every kernel lock is released.

`FdTable::close`, dup2 replacement, cloexec cleanup, and table clear first
remove the real descriptor, test whether another slot has the same open-file
description, and only then ask resident epoll instances to prune a last-closed
target. Issue the readiness wake after releasing the FD/process lock; dropping
the retained target under that lock could otherwise re-enter pipe/local-stream
wake code.

Because the FD table has 32 entries, snapshotting the bounded map is cheap and
avoids lock inversion. If a concurrent ctl changes a registration between
snapshot and edge commit, compare a per-registration revision and retry that
entry rather than overwriting the newer state.

### Restartable wait identity and timeout

Keep the existing 100 Hz rounding and absolute-deadline behavior. Use a stable
identity derived from the epoll open-file-description identity plus the user
events pointer and `maxevents`, not only the numeric epfd. A re-fired syscall
must not extend its timeout. Success, timeout, close, signal interruption, and
a different syscall number clear the restart record.

`epoll_wait` follows the existing signal-interrupted blocking contract:
`wake_ring3_for_signal` removes the wait, sets `pending_syscall_interrupt`, and
the dispatcher delivers/returns `EINTR` before the syscall can park again.

### Local stream ownership

`LocalStreamEndpoint` contains two pipe handles plus endpoint status. Keep
pipe reader/writer counts at endpoint-open-description granularity: cloning
the endpoint `Arc` for dup/fork does not create a new logical peer, while final
drop closes both halves and emits readiness notifications.

Add local targets to the generic I/O enums once, then make read/writev reuse
the same one-shot operations. Preserve the existing restart rule: a blocking
pipe/socket syscall re-fires from the beginning, so never consume bytes and
then park. Return a short count after any progress.

### VM transaction ordering

For `madvise`, validate VMA coverage/backing under the process lock, copy out
the L4/range, release the process lock, and unmap leaves under the mapper lock.
VMA metadata stays unchanged.

For `mremap`:

1. validate and reserve the metadata transformation under the process lock;
2. move/unmap leaves under the mapper lock with rollback data bounded by the
   512 MiB mmap cap (store only resident pages, not every virtual page);
3. commit the VMA change under the process lock;
4. if metadata commit unexpectedly fails, reverse the mapper move before
   returning.

Prefer adding transactional helpers to `VmaSet` (`allocation_extent`,
`resize_allocation`, `relocate_allocation`) over editing its `Vec<Vma>` from a
syscall handler. Unit-test those helpers without page tables first.

---

## Dependency graph

```text
readiness refactor + lost-wake fix
  |-- eventfd -------\
  |                   +--> epoll --> booted event-loop fixture
  |-- local stream --/
  `-- sched_yield -----------------> pthread wake/yield fixture

signal task state --> sigaltstack --> SA_ONSTACK fixture

VMA mapping IDs --> madvise ----\
                 `-> mremap -----+--> VM fixture

group state --> membarrier -------> pthread barrier fixture
```

The readiness refactor is the critical path. Signal and VM work can proceed
independently after ABI constants land. The final fixture waits for every
branch so trace mode can assert that all thirteen syscall numbers above are
handled and no new Tier 3 gap appears.

---

## Work sequence

### U0 — Characterization and contract tests

- Add dispatcher tests proving every new syscall number currently reaches
  `ENOSYS`; keep them failing/ignored only long enough to establish the
  baseline, then convert them into positive/error-contract tests.
- Record the exact syscall/flag sequence used by a pinned static-musl build of
  a minimal libuv loop, including io_uring probes that are expected to fall
  back via `ENOSYS`.
- Add pure tests for packed epoll-event and stack_t sizes/offsets.
- Add failure-injection cases for FD-table exhaustion and mapper allocation
  rollback before building the objects.

Acceptance: the observed direct libuv path matches create1 -> ctl -> pwait,
eventfd2, socketpair, and yield assumptions; any extra required syscall is
added explicitly to this plan rather than stubbed.

### U1 — Generalize readiness and close the lost-wake race

- Add `readiness.rs` and move `FdReady`/descriptor sampling out of
  `syscalls.rs`.
- Rename restartable wait state/reason/functions from network-specific to
  readiness-generic terminology.
- Add the global sequence and compare-and-park helper.
- Route pipe, GUI, network, stdin, close, dup2, and timer wake sites through
  `notify_readiness_changed` without changing existing readiness results.
- Add open-file-description identity comparison and characterize close/dup2
  cleanup ordering before epoll consumes it.
- Retarget poll/select to the new module.

Tests: every current descriptor readiness mask; finite/infinite timeout; stale
deadline clearing; event-before-publish and event-after-publish race tests;
signal EINTR; existing network/pipe/GUI poll/select regressions.

Acceptance: `./test.sh userland userland_switch network network_userland
gui_userland` passes before new fd types are enabled.

### U2 — Eventfd open-file description

- Add `eventfd.rs`, `FdSlot::EventFd`, and syscall 284/290 dispatch.
- Implement atomic/staged read/write, counter overflow, semaphore mode,
  nonblocking mode, generation, and readiness notifications.
- Audit `close`, `dup`, `dup2`, `fork_clone`, exec cloexec cleanup, fcntl,
  fstat, lseek, poll/select, and `/proc/self/fd` synthesis.

Tests: initial zero/nonzero readiness; drain; semaphore sequence; zero/max
write; overflow block/EAGAIN; bad size/pointer no mutation; fcntl sharing;
dup/fork sharing; cloexec; last-close cleanup; cross-thread wake with no lost
notification.

### U3 — Bounded epoll

- Add `epoll.rs`, the epoll FD slot, syscall aliases and dispatch.
- Implement create/create1, exact ctl errors, registration revisions, event
  data, readiness scanning, timeout/re-fire, and output bounds.
- Implement level-trigger delivery and eventfd notification-generation
  delivery for `EPOLLET`.
- Reject nested epoll and unsupported mask flags.
- Make ctl mutations and target close/peer changes wake a blocked epoll wait;
  prune a registration only after the target's last duplicate closes.

Tests: add/mod/del; duplicate/missing errors; packed data; IN/OUT/ERR/HUP/
RDHUP; maxevents truncation; timeout 0/finite/infinite; EINTR; edge eventfd
writes while counter remains nonzero; writes after a delivered generation
produce a new edge, while writes coalesced before one wait may produce one
edge; no level-trigger busy loop on that fd; dup/fork epoll sharing; bad
pointer leaves registrations/events untouched; closing one target duplicate
keeps the registration, while closing the last removes it and releases the
underlying pipe/socket/local-stream endpoint.

Boot gate: a two-thread raw-syscall program blocks in epoll_pwait while the
worker writes eventfd repeatedly, coordinating each new write after the prior
generation was delivered. Every new generation wakes the waiter without
draining the counter and without a PIT-timeout rescue.

### U4 — AF_UNIX stream socketpair

- Add `local_stream.rs`, construct paired endpoints from two pipe buffers, and
  add the FD slot.
- Add socketpair syscall 53 with two-fd atomic allocation/rollback.
- Extend read/write/readv/writev, readiness, fcntl, fstat, lseek, close/dup/
  fork, and proc-fd naming.
- Add plain-data sendmsg/recvmsg routing if it can reuse existing bounded iovec
  parsing; reject ancillary data.

Tests: bidirectional ping-pong; vectored I/O; independent nonblock flags;
buffer-full backpressure; peer-close EOF/EPIPE; poll/epoll masks; dup keeps
peer alive; fork sharing; output EFAULT and one-slot-left EMFILE rollback.

Boot gate: libuv's `uv_socketpair`-shaped flags and ping-pong pattern pass.

### U5 — Ring-3 sched_yield

- Factor post-syscall `UserState` capture from the blocking helper so both
  blocking and voluntary yield preserve the same callee-saved contract.
- Implement syscall 24 by saving a successful post-syscall continuation,
  enqueueing the current entity, and dispatching through the unified
  scheduler.
- Preserve group affinity and pending context-publication ordering under SMP.

Tests: register preservation; another runnable thread/process progresses;
single-runnable-task returns safely; SMP=4 never migrates a pthread group;
repeated libuv spin fallback cannot corrupt kernel stacks or queue an entity
twice.

### U6 — Task-local sigaltstack and SA_ONSTACK delivery

- Add the task-local alt-stack record and syscall 131.
- Implement set/query/disable, live-RSP on-stack detection, writable range
  validation, and lifecycle rules for fork/clone/exec.
- Teach `deliver_signal` to choose and validate the alternate stack for
  `SA_ONSTACK`, preserving the existing private frame/rt_sigreturn format.

Tests: layout and flags; enable/query/disable; undersize/overflow/bad mapping;
EPERM while executing on the stack; handler RSP inside the alternate range;
nested signal remains on-stack; rt_sigreturn restores original RSP/mask; fork
inherits, pthread clone and exec disable.

### U7 — Madvise discard semantics

- Add syscall 28 and supported advice constants.
- Add VMA coverage/backing validation helper.
- Implement hint no-ops and resident-leaf discard without VMA removal.
- Keep RSS/procfs accounting coherent automatically through the page-table
  walk.

Tests: invalid alignment/advice/hole/special mapping; anonymous DONTNEED drops
RSS and refaults zeros; file-private DONTNEED refaults backing bytes; MADV_FREE
documented stronger discard; shared COW frame refcounts stay balanced.

### U8 — Anonymous mremap

- Add mapping IDs to mmap-created anonymous VMAs and adjust merge rules.
- Add VmaSet allocation resize/relocation helpers.
- Add refcount-neutral mapper move with injected-failure rollback.
- Implement syscall 25 shrink, in-place grow, MAYMOVE, and strict unsupported
  flag/backing errors.

Tests: same-size no-op; shrink releases tail; grow in place with zero/lazy new
pages; blocked grow ENOMEM; MAYMOVE preserves bytes/protection and zero new
tail; sparse mappings remain sparse; COW flags/refcounts survive; destination
allocation failure restores old mapping byte-for-byte; adjacent independent
anonymous mappings never merge IDs.

### U9 — Private expedited membarrier

- Add group-owned registration state and syscall 324.
- Implement query, idempotent register, registered private fence, strict
  flags, and unsupported command errors.
- Add an invariant check that the group has one home CPU before advertising
  success.

Tests: exact query mask; EPERM before register; register/fence success from
leader and worker; fork/exec lifecycle chosen and documented; SMP=4 group
stays on one CPU while unrelated entities use others.

### U10 — Booted static-musl Tier 3 fixture

- Add `userland/apps/libuv-plumbing-test/` with a small deterministic C
  program and Makefile following the compiler/network fixture pattern.
- Add `UVPLUMB.ELF` as a committed test-fixture row in
  `userland/apps.manifest.sh` and document refresh/hash provenance.
- Add `src/tests/libuv_plumbing.rs` and register module `libuv`.
- Run through the production loader with unknown-syscall trace bookkeeping
  reset before/after; fixture returns nonzero on the first failed subtest.

Required subtests:

1. create epoll + nonblocking eventfd; worker writes multiple notifications;
   main blocks/wakes through `epoll_pwait` with `EPOLLET` and opaque data;
2. full-duplex nonblocking socketpair ping-pong through readv/writev and
   epoll; peer-close produces HUP/EOF;
3. worker loops through `sched_yield` and proves peer progress;
4. `SA_ONSTACK` handler verifies its local address lies in the registered
   alternate stack, returns, and the program continues;
5. anonymous mmap -> fill -> DONTNEED -> zero refault; mmap -> mremap grow/
   move -> original data and zero extension;
6. membarrier query/register/private-expedited from a pthread;
7. every created fd closes and every worker joins before exit.

Acceptance: the fixture exits 0 under SMP=1 and SMP=4, has bounded deadlines,
does not depend on live networking, and records no unhandled Tier 3 syscall.

### U11 — Documentation and qualification

- Update `src/userland/CLAUDE.md` with new fd types, readiness race protocol,
  epoll boundary, local-stream boundary, alt-stack lifecycle, VM limits, and
  membarrier affinity dependency.
- Update `src/mm/CLAUDE.md` for refcount-neutral leaf moves and mapping IDs.
- Update root `CLAUDE.md`, userland/prebuilt docs, and README current-state
  summaries without claiming that libuv itself ships.
- Run formatting, compile checks, focused modules, SMP variants, full tests,
  and an interactive zsh/Links/TinyCC/network regression smoke.

---

## Test matrix

### Focused kernel tests

| Area | Required coverage |
|---|---|
| ABI | all numbers dispatch; exact bad flags, fds, pointers, lengths |
| readiness | every FD variant; no lost wake on both race sides; timeout/EINTR |
| eventfd | counter, semaphore, overflow, generation, block/nonblock, sharing |
| epoll ctl | add/mod/del, revisions, errors, retained descriptions, data |
| epoll wait | LT masks, eventfd ET generations, maxevents, timeout, EINTR |
| local stream | duplex bytes, vectors, backpressure, EOF/EPIPE, dup/fork |
| yield | saved register contract, scheduler fairness, affinity, queue safety |
| alt stack | task lifecycle, bounds, nested delivery, rt_sigreturn |
| madvise | VMA validation, discard/refault, RSS and refcounts |
| mremap | mapping IDs, resize/move, sparsity, COW flags, rollback |
| membarrier | query/register/fence/errors, group ownership and affinity |

### Booted userland tests

Run the committed fixture through the production VFS/loader/scheduler:

```sh
AGENTICOS_QEMU_SMP=1 ./test.sh libuv
AGENTICOS_QEMU_SMP=4 ./test.sh libuv
```

The test must use real pthreads for eventfd and yield/membarrier interaction.
It must use an infinite epoll wait for at least one eventfd wake, guarded by a
separate fixture/test watchdog, so periodic timeout polling cannot hide a lost
wake.

### Regression and qualification commands

```sh
cargo fmt --check
cargo check
./test.sh vm memory userland userland_switch scheduler pthreads
./test.sh network network_userland gui_userland libuv
AGENTICOS_QEMU_SMP=1 ./test.sh libuv pthreads
AGENTICOS_QEMU_SMP=4 ./test.sh libuv pthreads smp scheduler
./test.sh
```

Interactive smoke:

- zsh pipeline, redirection, Ctrl-C, and child reaping;
- Links text/GUI HTTP(S) browse;
- BusyBox ping/nc/wget on the existing restricted test network;
- TinyCC compile/run and pthread example;
- GUI event descriptors and Task Manager process/accounting views.

---

## Acceptance criteria

The tier is complete when:

1. Every syscall listed in the ABI table has a constant, dispatcher arm, real
   bounded handler, focused tests, and no unknown-syscall trace hit.
2. `poll`, `select`, and epoll share one readiness implementation, and an
   event on either side of the scan-to-block boundary cannot be lost.
3. Libuv's eventfd `EPOLLET` pattern observes a new edge for writes after the
   prior generation was delivered, permits Linux-style coalescing before a
   wait, and does not require draining the counter or cause an always-ready
   spin.
4. Epoll preserves the user's event data, supports ADD/MOD/DEL and timeout/
   EINTR behavior, and reports pipe, GUI, eventfd, local-stream, and IPv4
   socket readiness correctly.
5. Eventfd and local streams behave as shared open-file descriptions across
   dup/fork while keeping close-on-exec descriptor-local.
6. `socketpair(AF_UNIX, SOCK_STREAM | flags, 0, fds)` supports full-duplex
   vectored ping-pong, backpressure, nonblocking mode, peer EOF/EPIPE, and
   poll/epoll.
7. `sched_yield` lets another runnable entity progress without corrupting the
   syscall continuation or violating group affinity.
8. An `SA_ONSTACK` handler executes on the registered task-local stack and
   returns through the existing rt_sigreturn path to the original stack/mask.
9. MADV_DONTNEED releases resident pages without removing mappings, and the
   supported mremap paths preserve contents, flags, sparsity, and frame
   ownership with rollback on failure.
10. Membarrier advertises only private-expedited support and remains correct
    under the one-home-CPU pthread rule.
11. The static-musl fixture passes under QEMU SMP=1 and SMP=4 with no hangs,
    leaked fds/tasks, network dependency, or unexpected syscalls.
12. The full `./test.sh` suite passes.

---

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| readiness change between scan and park | infinite epoll/poll hang | global sequence + publish-then-recheck protocol and boundary tests |
| level-triggered handling of libuv eventfd EPOLLET | event loop busy-spins or loses later wakes | eventfd notification generation stored per epoll registration |
| epoll/FD/process/network lock inversion | SMP deadlock in syscall context | clone/snapshot under one lock, sample/copy under none; documented order |
| epoll retains cyclic/nested descriptors | leaked Arc cycle or recursive scan | reject epoll targets in v1 |
| epoll-retained target outlives its last real fd | socketpair peer never sees EOF; resource leak | same-description scan and last-close pruning before wake/drop |
| fd reused after epoll registration while an old dup remains | numeric key cannot represent Linux's two description+fd registrations | retain the old description and document/reject the rare second ADD; libuv deletes before close |
| socketpair publishes one fd then allocation fails | leaked half-pair/user sees partial result | reserve/allocate both, write output last, full rollback tests |
| restarted local-stream write duplicates bytes | corrupted IPC stream | never park after progress; return short count exactly like pipes |
| alternate stack overflows or is unmapped before delivery | kernel fault while constructing signal frame | VMA validation at registration and usercopy validation at delivery; fatal user SIGSEGV path |
| mremap releases/multicounts a moved frame | COW corruption/use-after-free | refcount-neutral mapper primitive, rollback log, frame-count tests |
| adjacent anonymous mappings merge | mremap moves the wrong allocation | stable mapping IDs and merge only within the same allocation |
| mremap partially moves then OOMs | old mapping corrupted | destination reservation + mapper rollback before metadata commit |
| MADV_DONTNEED on special mappings | executable/TLS/stack corruption | explicit backing allowlist and negative tests |
| membarrier claimed after group becomes multi-CPU | weak memory ordering visible to runtime | assert home-CPU invariant; add IPI/ack before removing affinity |
| fd-table cap is too small for a future libuv embedder | early EMFILE despite correct primitives | keep current cap for this tier, measure real libuv port, raise separately if proven |
| current libuv probes io_uring first | noisy ENOSYS or unexpected abort | characterize pinned build; preserve deliberate ENOSYS fallback, do not fake io_uring |

---

## Expected file changes

Primary implementation:

- `src/userland/abi.rs` — constants and dispatch arms.
- `src/userland/mod.rs` — new userland modules.
- `src/userland/readiness.rs` — shared readiness and race-free blocking.
- `src/userland/eventfd.rs` — eventfd object and handlers.
- `src/userland/epoll.rs` — epoll object and handlers.
- `src/userland/local_stream.rs` — paired local stream endpoints/socketpair.
- `src/userland/fdtable.rs` — new slots, flags, allocation/dup/fork behavior.
- `src/userland/syscalls.rs` — generic I/O/fcntl/fstat/lseek/proc-fd routing,
  sched_yield, sigaltstack, madvise, and mremap.
- `src/userland/network_syscalls.rs` — generalized readiness naming and
  optional control-free local sendmsg/recvmsg routing.
- `src/userland/lifecycle.rs`, `switch.rs`, `signal.rs` — generalized wait
  state, yield snapshot helper, alt-stack/group membarrier state/lifecycle.
- `src/userland/vm.rs` — mapping IDs and allocation resize/relocation helpers.
- `src/mm/paging.rs` — transactional/refcount-neutral leaf move.

Tests and fixture:

- `src/tests/userland.rs`, `userland_switch.rs`, `vm.rs`, `memory.rs`,
  `scheduler.rs`, `smp.rs` — focused primitive/contract tests.
- `src/tests/libuv_plumbing.rs`, `src/tests/mod.rs` — booted acceptance module.
- `userland/apps/libuv-plumbing-test/{Makefile,README.md,src/*.c}` — fixture.
- `userland/apps.manifest.sh` — `UVPLUMB.ELF` test-fixture row.
- `userland/prebuilt/libuv/UVPLUMB.ELF` and provenance README — committed
  fixture staged even with `--skip-userland`.
- `userland/prebuilt/README.md`, `userland/README.md` — fixture docs.

Context/documentation:

- `src/userland/CLAUDE.md`, `src/mm/CLAUDE.md`, root `CLAUDE.md`, and README.

Do not refresh unrelated committed prebuilts. The new fixture is the only
binary artifact this plan adds.

---

## Follow-ups unlocked by this work

1. Build and ship a pinned static-musl libuv with its upstream event-loop
   tests, then qualify TCP/UDP/TTY/process/timer APIs individually.
2. Port a first libuv embedder such as a small standalone sample runtime.
3. Add inotify and filesystem event APIs.
4. Add Unix path sockets and SCM_RIGHTS for full libuv IPC/handle passing.
5. Implement temporary signal-mask semantics across ppoll, pselect6, and
   epoll_pwait, plus signalfd if an embedder needs it.
6. Add timerfd/signalfd and broader epoll edge/oneshot/nesting semantics.
7. Add io_uring only after its shared rings, mmap contract, opcode surface,
   cancellation, and teardown can be implemented truthfully.
8. Add remote user-TLB shootdown and an IPI-backed membarrier before allowing
   one thread group to execute on multiple CPUs.
9. Extend mremap to file-backed/mixed-protection allocations and fixed moves;
   add richer madvise policy when a measured workload requires it.
