---
title: "feat: add musl-compatible user threads with clone, futex, TLS, and TIDs"
type: feat
status: completed
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md
  - docs/plans/2026-07-17-002-feat-reusable-memory-and-sparse-user-address-spaces-plan.md
  - docs/plans/2026-07-18-003-feat-tinycc-port-plan.md
  - docs/plans/2026-07-18-006-feat-smp-support-plan.md
  - src/process/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - userland/apps/tcc/README.md
---

# feat: add musl-compatible user threads with clone, futex, TLS, and TIDs

## Summary

Add the first real ring-3 thread runtime: Linux x86-64 `clone(2)` with the
flag set used by static musl, private futex wait/wake/requeue, per-thread TLS
and TIDs, and correct per-thread versus whole-process exit. The acceptance
workloads are ordinary musl `pthread` C programs covering:

- creation and `pthread_join`, including return values;
- contended `pthread_mutex_t` locking;
- `pthread_cond_t` wait, signal, and broadcast;
- detached-thread completion and resource cleanup;
- distinct thread-local values and TIDs with one shared PID.

Implementation completed on 2026-07-18. The booted acceptance module passes
all five workloads with both one and four QEMU CPUs, and the unfiltered kernel
suite passes all 912 tests. Join, mutex, condvar, and detached sources compile
on-target with TinyCC. The TLS source uses the documented static-musl
cross-built fixture because this TinyCC pin accepts `_Thread_local` syntax but
emits shared storage for it.

The tests compile and link on-target with the existing TinyCC and
`/host/sysroot` wherever TinyCC's x86-64 TLS/linker support permits. The
committed sysroot already contains musl 1.2.5's full `pthread.h`, pthread
objects inside `libc.a`, and the empty compatibility `libpthread.a`; this is
a kernel/runtime feature, not a new libc port. A cross-built static fixture is
kept only as a diagnostic fallback if TinyCC cannot emit one particular TLS
relocation, and that limitation must be documented rather than weakening the
runtime test.

The current SMP kernel intentionally relies on “one CPU per address space”
and has no remote user-TLB shootdown. The first thread release preserves that
invariant by assigning a CPU affinity to a thread group when its second task
is created. Threads in one group are preemptively interleaved on that CPU;
unrelated processes and kernel threads still use all CPUs. Parallel execution
of one address space is a follow-up gated on a real TLB-shootdown protocol.

Upstream behavior used as the ABI reference:

- [musl `pthread_create`](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_create.c)
- [musl `pthread_join`](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_join.c)
- [musl condition variables](https://git.musl-libc.org/cgit/musl/tree/src/thread/pthread_cond_timedwait.c)
- [musl timed futex wait](https://git.musl-libc.org/cgit/musl/tree/src/thread/__timedwait.c)
- [Linux x86-64 syscall table](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/arch/x86/entry/syscalls/syscall_64.tbl)

---

## Current state and feasibility findings

### The execution substrate is ready

The unified scheduler already treats every ring-3 PID as a separately saved
execution entity with its own:

- `UserState` register image;
- 64 KiB kernel entry stack;
- FS_BASE and FPU state;
- scheduler state and blocking reason;
- per-CPU current-entity ownership under SMP.

That is almost exactly the state a kernel thread/TID needs. A cloned pthread
does not need a new switch primitive. It needs a second scheduled task that
points at the same process resources, starts with a caller-supplied user RSP,
and restores a different FS_BASE.

### The current `Process` object mixes two lifetimes

`src/userland/lifecycle.rs::Process` presently owns both task-local execution
state and process-wide resources. It includes registers, FS_BASE, FPU, kernel
stack, signal mask, address space, file table, cwd, `brk`, executable metadata,
GUI ownership, parentage, and zombie state in one PID-indexed object.

That layout cannot represent pthreads safely:

- `CLONE_VM`, `CLONE_FILES`, `CLONE_FS`, and `CLONE_SIGHAND` require shared
  objects, not value clones;
- each thread needs a distinct TID, TLS pointer, register image, signal mask,
  kernel stack, clear-child-TID pointer, and blocking state;
- `SYS_exit` destroys one task while `exit_group` destroys all tasks;
- a group must outlive its leader if the leader calls `pthread_exit`;
- `wait4` and SIGCHLD observe process/thread-group death once, not each TID.

The plan therefore separates a scheduled `UserTask` from a `ProcessGroup`
before enabling `clone`. Keeping the fused object and making one member the
implicit resource owner is rejected: leader exit, detached teardown, and
cross-thread resource lookup would all become special cases.

### Exact musl clone contract

Static musl 1.2.5 calls the raw clone wrapper with:

```text
CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND |
CLONE_THREAD | CLONE_SYSVSEM | CLONE_SETTLS |
CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID | CLONE_DETACHED
```

For Linux x86-64 the kernel-facing syscall arguments are:

```text
clone(flags, child_stack, parent_tid, child_tid, tls)
      rdi    rsi          rdx         r10        r8
```

Musl passes `&new->tid` as `parent_tid`, the new TCB as `tls`, and its
thread-list lock as `child_tid`. The kernel must write the TID into
`parent_tid` before publishing the child, load the child FS_BASE from `tls`,
remember `child_tid`, then clear that word and futex-wake it on task exit.
Musl's assembly wrapper handles the `clone` return-to-child path and invokes
the C start function; the kernel only resumes the child after the syscall with
`rax = 0` and `rsp = child_stack`.

The main thread calls `set_tid_address(&__thread_list_lock)` during musl TLS
initialization and stores the returned TID in its TCB. The current fake return
value `1` must become the caller's real TID and the pointer must be retained.

### Futex operations required by the acceptance workloads

Musl's normal private locks use futex only after their userspace atomic fast
path contends. The kernel needs:

| Operation | Use in musl | First-release support |
|---|---|---|
| `FUTEX_WAIT[_PRIVATE]` | mutex, join, condvar barriers, libc locks | yes |
| `FUTEX_WAKE[_PRIVATE]` | unlock, join, signal/broadcast | yes |
| `FUTEX_REQUEUE[_PRIVATE]` | condvar-to-mutex handoff | yes |
| relative timeout on `FUTEX_WAIT` | timed wait helpers | yes |
| `FUTEX_WAIT_BITSET` / absolute clocks | advanced timed APIs | deferred |
| PI operations | priority-inheritance mutexes | deferred |
| robust owner-death recovery | robust/process-shared mutexes | deferred |

Private futexes are keyed by `(tgid, aligned_user_address)`. A plain virtual
address is insufficient because unrelated address spaces reuse addresses.
Physical-page keys and cross-process shared futexes are not required until
shared memory exists.

### The on-target toolchain is already suitable

`userland/prebuilt/tcc-sysroot.tar.gz` contains:

- musl 1.2.5 `pthread.h` and related headers;
- real pthread implementations in `libc.a`;
- musl's empty `libpthread.a` compatibility archive;
- CRT objects and TCC's runtime library.

The TCC Makefile copies the full C sysroot before pruning unrelated kernel
header subtrees. Add pthread acceptance sources under
`userland/apps/tcc/examples/`; the existing deterministic sysroot packaging
then stages them as `/host/sysroot/examples/*.c`. No new upstream artifact or
dynamic loader is needed.

### SMP changes the safe implementation boundary

The SMP plan explicitly deferred threads to preserve this invariant:

> A user address space is active on at most one CPU, so local TLB invalidation
> is sufficient for `munmap`, `mprotect`, COW, and page reclamation.

Running two tasks from one group on two CPUs would immediately violate that
invariant. `munmap` could free a page while another CPU retains a writable TLB
entry, and the existing mapper releases frames before any remote
acknowledgement is possible.

The first release adds scheduler affinity metadata and pins every member of a
multithreaded group to the CPU that performs the first thread clone. This is a
correct pthread implementation with no simultaneous execution of one group;
preemption and blocking still provide concurrency. A later plan may split PTE
removal from frame release, add address-space active CPU masks and a TLB IPI
ack protocol, then remove the affinity.

---

## Goals

1. Represent a process/thread group separately from its scheduled tasks, with
   explicit TGID/TID identity and correct resource ownership.
2. Implement the musl x86-64 pthread `clone` flag set with shared VM, files,
   cwd, signal dispositions, TLS installation, parent-TID write, and
   child-clear-TID registration.
3. Implement `gettid`, real `set_tid_address`, per-thread `arch_prctl` FS_BASE,
   and `getpid == tgid` for every member.
4. Implement race-free private futex wait, wake, requeue, relative timeout,
   signal interruption, and exit-time clear/wake.
5. Split `SYS_exit` from `exit_group`: one task exits without generating a
   zombie; whole-group or last-task exit reports one process death to the
   parent and process service.
6. Preserve existing fork/exec/wait, signals, GUI ownership, procfs, task
   manager, and detached top-level launch behavior for single-threaded apps.
7. Preserve the SMP one-CPU-per-address-space invariant with group affinity,
   and test it under both one and four QEMU CPUs.
8. Pass booted static-musl pthread programs for mutex, condvar, join, detached
   cleanup, TLS, and TID semantics, preferably compiled on-target by TinyCC.
9. Keep `./test.sh` green and add focused in-kernel tests for every lifecycle
   and futex primitive.

## Non-goals

- Simultaneously execute threads from one address space on multiple CPUs.
  That requires remote user-TLB shootdown and safe deferred frame release.
- `clone3`, namespace flags, `vfork`-style shared-VM processes, or arbitrary
  non-pthread clone flag combinations.
- Process-shared futexes, shared-memory mappings, robust owner-death recovery,
  priority inheritance, `FUTEX_WAIT_BITSET`, futex2, or `rseq`.
- Full POSIX pthread cancellation, per-thread interval timers, alternate signal
  stacks, CPU affinity APIs, thread scheduling policies, or realtime priority.
- `/proc/<tgid>/task/<tid>` or individual pthread rows in Task Manager. The
  existing process view remains one row per TGID with aggregate CPU time.
- Guaranteeing fork/exec from a group while another thread is live. The first
  release returns `-EAGAIN` for `fork` and `-EBUSY` for `execve` when the
  caller's group has more than one live task; single-task behavior remains
  unchanged. Correct POSIX atfork/de-thread semantics are a follow-up.
- Dynamic linking or a new libc. Tests remain static musl executables.

---

## Design

### Identity and terminology

- **TID** identifies one scheduled `UserTask` and comes from the existing
  monotonic ring-3 ID allocator.
- **TGID/PID** identifies one `ProcessGroup`. The first task has
  `tid == tgid`; cloned pthreads receive new TIDs but retain the TGID.
- `gettid()` returns the current task's TID.
- `getpid()` returns its group's TGID.
- `getppid()` returns the parent process group's TGID.
- Scheduler entities are renamed from `UserProcess(u32)` to `UserTask(u32)` so
  logs, assertions, and future code do not accidentally treat a TID as a PID.
- Per-CPU `current_user_pid` becomes `current_user_tid`. A helper resolves the
  current TGID through the user table for process-owned services.

Use one allocator for both leader IDs and secondary TIDs. Non-reuse remains
the simplest defense against stale `pthread_t`/TID confusion.

### State split

Refactor `PROCESS_TABLE` into one `InterruptMutex<UserTable>` containing
`tasks`, `groups`, task block reasons, and membership indexes. Keeping the
maps under one lock makes task/group lookup and final-member transitions
atomic without introducing nested lifetime locks.

| `UserTask` — per TID | `ProcessGroup` — per TGID |
|---|---|
| `tid`, `tgid` | `tgid`, parent TGID, child/zombie relation |
| saved `UserState` | `AddressSpace` and VMA set |
| kernel entry stack/continuation | fd table/open-file descriptions |
| FS_BASE and FPU state | cwd and umask |
| per-thread blocked/pending signals | shared signal dispositions + group pending |
| per-thread stack-growth window | brk/mmap metadata |
| scheduler/block/futex restart state | executable path, cmdline, image metadata |
| clear-child-TID and robust-list pointer | terminal and GUI/GL ownership |
| per-thread CPU ticks | process timers and aggregate accounting |
| exit/deferred-drop state | exit code/kind and process-service ownership |

Keep compatibility accessors small and explicit:

```text
with_current_task(...)
with_current_group(...)
with_current_task_and_group(...)
with_task(tid, ...)
with_group(tgid, ...)
current_tid()
current_tgid()
```

Do not retain a generic `with_current_process` whose closure can silently use
the wrong lifetime. Convert call sites by category:

- switch, FS_BASE, FPU, signal mask, blocking → task;
- mmap/brk/fds/cwd/exe/procfs/GUI → group;
- signal delivery and exit → both.

The kernel sentinel remains a synthetic task/group only if tests still need
it after the conversion. Prefer test helpers that install explicit synthetic
tasks so production lookups do not carry PID-0 fallback semantics forever.

### Address-space ownership and scheduler affinity

`ProcessGroup` owns one `AddressSpace`. Every task resume copies the group's
L4 frame while taking its own saved registers, kernel stack, FS_BASE, and FPU
state. Page faults and usercopy resolve VMAs from the group.

Add `cpu_affinity: Option<u8>` to scheduler entity policy:

- ordinary kernel threads and single-task user groups remain unpinned;
- on the first successful pthread clone, set the group `home_cpu` to the
  current CPU and apply that affinity to the leader and child entities;
- later clones inherit the same CPU;
- scheduler selection skips an entity pinned to another CPU without removing
  or losing it from the shared queue;
- work notification targets the home CPU for a pinned wake;
- when a group returns to one task, retain affinity for the group's lifetime
  in v1. This avoids races while relaxing affinity and is operationally cheap.

Required debug invariant:

```text
for every live tgid with member_count > 1:
    all UserTask entities have the same affinity
    at most one member is Running across Scheduler.current[]
```

This scheduler change lands and is tested before shared-address-space clone.

### Clone implementation

Implement only the musl pthread profile. Parse the low signal byte separately
and reject unsupported combinations with `-EINVAL`. Require:

- the complete sharing dependency chain (`CLONE_THREAD` implies
  `CLONE_SIGHAND`, which implies `CLONE_VM`);
- `CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD`;
- `CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID`;
- allow and ignore `CLONE_SYSVSEM | CLONE_DETACHED`;
- zero exit signal for a thread clone.

Before allocation/publication:

1. Validate flags and canonical nonzero `child_stack`/`tls` in the caller's
   user range.
2. Validate writable `parent_tid` and `child_tid` words through usercopy.
3. Capture the post-SYSCALL RIP/RFLAGS, user RSP, and callee-saved registers
   using the same assembly contract as `fork_handler`.
4. Allocate a TID and a fresh kernel entry stack.
5. Build a child register snapshot equal to the caller except
   `rax = 0` and `rsp = child_stack`.
6. Copy the caller's signal mask, clear pending signals, snapshot the caller's
   live FPU environment into the child (POSIX inheritance), set FS_BASE to
   `tls`, clear restart/deadline state, and record
   `clear_child_tid = child_tid`.
7. Store the new TID to `parent_tid` with release ordering while the parent
   address space is active.
8. Insert the task, attach it to the group, apply group affinity, register its
   scheduler entity, and mark it ready.
9. Return the new TID to the parent.

No child can run before the parent-TID write and table insertion are complete.
Any failure before publication drops the kernel stack and leaves no task,
scheduler entity, affinity mutation, or user-visible partial TID. If the final
TID copy fails, abort before publication.

### TLS and architecture state

No kernel allocation of secondary TLS is needed. Musl maps the new stack/TLS
area, copies TLS, constructs its TCB, and passes the adjusted thread pointer
through `CLONE_SETTLS`.

- `clone` initializes only the child's saved `fs_base` from the syscall TLS
  argument; the parent's live MSR and saved field remain unchanged.
- `arch_prctl(ARCH_SET_FS)` updates the current task, not the group.
- switch-out/in continues to save and restore FS_BASE and FPU per task.
- validation accepts any canonical mapped user pointer; secondary TCBs live in
  anonymous mmap VMAs, not the loader's original PT_TLS region.

Add a switch test with two TIDs in one TGID, distinct FS_BASE values, and the
same L4 frame. End-to-end `_Thread_local`/`__thread` values must remain isolated
across forced preemption and futex blocking.

### TID syscalls and signal targeting

Add Linux x86-64 syscall 186 (`gettid`) and replace the thread stubs:

- `set_tid_address(ptr)` validates a nullable/aligned writable `int *`, stores
  it on the current task, and returns `current_tid()`;
- `gettid()` returns `current_tid()`;
- `getpid()`/`getppid()` resolve the current group;
- `tkill(tid, sig)` targets an exact task;
- `tgkill(tgid, tid, sig)` verifies membership before targeting;
- `kill(pid, sig)` remains process-directed and addresses a TGID.

Split `SignalState` into shared dispositions and per-task mask/pending state.
For v1, process-directed `kill` selects the leader if live, otherwise the
lowest live TID that does not block the signal. Thread-directed delivery uses
the exact target. This is enough for existing signals and musl's internal
thread targeting without implementing process-wide signal arbitration.

`set_robust_list` should validate and record `(head, len)` per task rather than
remain a global no-op, but robust-list walking is deferred. Basic and default
musl mutexes perform cleanup in userspace and do not require owner-death
recovery. Return `-EINVAL` for a non-null head with an unexpected Linux robust
head length.

### Futex subsystem

Add `src/userland/futex.rs` with a bounded waiter registry. A key is:

```text
FutexKey { tgid: u32, uaddr: u64 }
```

Waiters are identified by TID and carry an optional absolute PIT deadline.
The registry needs a hard global bound and per-key bound; use limits high
enough for the thread cap but fail with `-EAGAIN` rather than allocate without
limit.

#### Atomic wait/park

`FUTEX_WAIT` must not lose a wake between comparing the user word and blocking
the scheduler entity. First validate/fault-in the four-byte user range without
the futex lock. Because every member of the address space is pinned to this
CPU, another member cannot unmap it during the transition. Then use one
prepared-park helper with this sequence:

1. capture the caller's complete rewound-SYSCALL resume snapshot while it is
   still the running task;
2. acquire the futex registry lock;
3. atomically load the aligned 32-bit user word while the caller's CR3 is
   active, without entering the memory mapper;
4. if it differs from `val`, release and return `-EAGAIN`;
5. reserve the waiter and install the captured snapshot/restart state;
6. under the documented futex → user-table → scheduler lock order, mark the
   entity blocked before releasing the futex lock;
7. release every lock and dispatch through a helper that knows the task is
   already saved and parked.

Every wake path takes the same futex lock before selecting waiters, so a
waker that stores the userspace word and enters `FUTEX_WAKE` cannot pass the
waiter's compare/enqueue transition.

The existing generic “rewind SYSCALL and re-fire” blocking helper needs a
futex-aware completion marker:

```text
Waiting -> Woken | TimedOut | Interrupted
```

On re-entry the handler consumes the marker and returns `0`, `-ETIMEDOUT`, or
`-EINTR` instead of comparing and parking again. The marker also makes
spurious scheduler wakes harmless.

#### Wake and requeue

- `FUTEX_WAKE`: remove at most `val` waiters in queue order, mark them Woken,
  make their entities ready, and return the number woken.
- `FUTEX_REQUEUE`: wake at most `val`, then move at most `val2` remaining
  waiters from key 1 to key 2 without waking them; return total affected as
  Linux does. Reject a null, unaligned, or cross-group destination.
- Accept both private and unflagged operations but key both by TGID in v1.
  Musl deliberately uses unflagged futexes for its internal thread-list lock
  and also falls back from private to unflagged when a kernel returns ENOSYS.
  Cross-process semantics remain unsupported until shared mappings provide a
  physical/shared-object key.
- Validate timeout `tv_nsec`, convert relative durations to a ceiling number
  of 100 Hz ticks, and use an absolute stored deadline so syscall re-fire does
  not extend the wait.
- Timer housekeeping expires due futex waits and marks `TimedOut`.
- Signal wake removes the waiter from its queue and marks `Interrupted` before
  using the existing pending-signal dispatch path.

Never hold the futex registry, user table, scheduler, or memory-mapper locks
across a context switch. Document and test the lock order used by wait, wake,
signal, timeout, and task exit.

### Per-thread and group exit

Route syscall 60 (`exit`) to a new `exit_thread_handler`; keep syscall 231
(`exit_group`) group-wide.

#### `SYS_exit`

1. Mark the current task exiting so it cannot be selected again.
2. If `clear_child_tid` is non-null, best-effort perform an aligned release
   store of zero while the group address space is active, then futex-wake one
   waiter on that key, matching Linux's join handoff and publishing all prior
   userspace teardown to the joiner.
3. Remove any outstanding futex waiter/restart state and timers for the TID.
4. Unregister only this task's scheduler entity.
5. If other group members remain, enqueue the dead task for deferred drop and
   dispatch another entity. Do not create a zombie or SIGCHLD.
6. If this was the last member, finalize the process group exactly once using
   the existing parent zombie/process-service path.

The dead `UserTask` cannot be dropped on its own kernel stack. Extend the
existing process-service/deferred-reaper mechanism with a dead-task queue and
drop kernel stacks only from another safe kernel stack.

#### `exit_group`

Because v1 pins all group members to one CPU, no sibling can be concurrently
executing when the caller enters the syscall. Atomically mark the group
exiting, unregister every member entity, perform clear-child-TID/wake for each
member, queue every task for deferred drop, clean GUI/timers once by TGID, and
file one zombie/exit completion for the group. Then diverge from the caller.

Fatal process-directed signals use the same group-exit primitive. A fatal
thread-directed signal may initially terminate the whole group, matching
Linux's default action for the relevant fatal signals and avoiding a second
partial-exit path.

Detached pthreads need no kernel detach syscall. Musl decides joinability,
unmaps detached stack/TLS with `munmap`, and finally calls `SYS_exit`; the
kernel task reaper must therefore own only kernel-side task state, never the
userspace pthread mapping.

### Fork, exec, wait, and process ownership

The single-task path remains behaviorally identical:

- `fork` creates a new group with a leader `tid == tgid`, clones the caller's
  address space through COW, fork-clones the fd table, and copies the calling
  task's FS_BASE/signal mask into the child leader;
- `execve` replaces the current group's address space/image and resets the
  current task's entry state;
- `wait4`, zombies, SIGCHLD, orphan adoption, and process-service completion
  remain keyed by TGID;
- GUI/GL/terminal resources and `/proc/<pid>` remain group-owned;
- process CPU time becomes the sum of member task ticks.

Until atfork and de-thread behavior are implemented, guard `fork` and `execve`
when `member_count > 1` as stated in Non-goals. This is preferable to silently
copying or destroying state with incorrect races.

Top-level launch returns a TGID. Process-service exit notifications are sent
only on final group death, never for secondary TIDs.

### Limits and failure behavior

Define explicit caps rather than allowing pthread creation to exhaust the
kernel heap:

- 128 live ring-3 tasks globally (leaves half of the scheduler's 256-entity
  budget for kernel threads and transition headroom);
- 64 live tasks per group;
- 128 futex waiters globally and 64 per key;
- clamp one wake/requeue traversal to 128 waiters even if userspace requests
  a larger count.

Use `-EAGAIN` for task/waiter capacity exhaustion, `-ENOMEM` for kernel-stack
allocation failure, `-EINVAL` for flags/alignment/timespec errors, `-EFAULT`
for invalid user pointers, and `-ESRCH` for stale TIDs/TGIDs. Musl translates
clone failures from `pthread_create` to `EAGAIN`.

---

## Work sequence

Each unit is a review boundary. Keep `cargo check`, focused tests, and the
single-threaded userland regression set green before moving to the next unit.

### U0 — Characterization and acceptance fixtures

- Add the pthread C sources to `userland/apps/tcc/examples/` without yet
  registering them as passing tests.
- Verify with the host musl toolchain that they link statically against the
  exact sysroot contents and inspect their syscall/TLS relocations.
- Add a trace-only QEMU probe that establishes the observed first failure
  (`clone` today) and records any extra syscall such as `membarrier` rather
  than speculatively stubbing it.
- Pin expected musl version/source behavior in the plan and TCC README.

Acceptance: fixtures are deterministic, bounded by timeouts, and fail for a
known missing runtime feature rather than hanging.

### U1 — Split task and process-group state

- Introduce `UserTask`, `ProcessGroup`, and `UserTable`.
- Move fields according to the state table above and replace ambiguous
  lifecycle accessors.
- Rename per-CPU current PID to current TID and scheduler entity
  `UserProcess` to `UserTask`.
- Retarget switch, usercopy, VM, fd/cwd, signal, GUI, procfs, process service,
  fork/exec/wait, and cleanup call sites.
- Keep one task per group only; no new syscall behavior yet.

Tests: constructor ownership, accessor routing, one shared L4 snapshot,
switch round trips, group-only procfs rows, existing fork/exec/wait tests.

Acceptance: full `./test.sh` passes with behavior unchanged.

### U2 — Scheduler affinity for shared address spaces

- Add optional CPU affinity to scheduler entities and affinity-aware dequeue.
- Add targeted wake notification for pinned entities.
- Add group home-CPU assignment helpers and debug invariants.
- Exercise two synthetic TIDs sharing an L4 without enabling clone.

Tests: pinned selection on SMP, wrong-CPU skip without queue loss, wake of an
idle home CPU, at-most-one-running member, unrelated processes still execute
on other CPUs.

Acceptance: `./test.sh smp scheduler userland_switch` passes under SMP=1 and
SMP=4.

### U3 — TID, TLS, and signal-state foundations

- Add `gettid(186)` dispatch.
- Implement real `set_tid_address`, task-local FS_BASE updates, robust-list
  registration, and process/thread signal targeting.
- Split shared signal actions from task-local masks/pending bits.
- Add clone-register snapshot builder reusable by fork and clone.

Tests: PID/TGID/TID identities, clear-TID pointer registration, TLS switch
between tasks, `tkill`/`tgkill` membership, shared dispositions with distinct
masks.

### U4 — Musl pthread clone profile

- Add clone flags/constants and strict profile validation.
- Allocate and publish a child task sharing its group.
- Set child stack, TLS, parent TID, clear-child-TID, inherited live FPU state,
  and group affinity with rollback on every failure edge.
- Enforce task/group caps.

Tests: flag dependency matrix, bad pointers, parent/child register values,
same L4/fds/cwd/dispositions, distinct kernel stacks/TLS/TIDs, no partial task
on failure, child cannot run before parent TID is visible.

Boot gate: a minimal `pthread_create` worker starts and reaches userspace;
join may still block until U5/U6.

### U5 — Private futex wait/wake/requeue

- Add the bounded futex registry and syscall 202 dispatch.
- Implement atomic compare-and-park, completion markers, wake, requeue,
  relative timeout, signal interruption, and timer expiry.
- Integrate block reasons and scheduler wake paths without lock inversion.

Tests: mismatch `EAGAIN`, no lost wake at the compare/enqueue boundary,
wake counts, FIFO/key isolation, requeue counts/destination, timeout not
extended on re-fire, signal `EINTR`, stale-task cleanup, capacity errors.

Boot gate: mutex and condvar acceptance programs pass.

### U6 — Thread exit, clear-child-TID, join, and detached cleanup

- Split syscall 60 from 231.
- Add per-task exit and group exit primitives.
- Clear/wake child TID on every task-death path.
- Add safe deferred task reaping and final-member group teardown.
- Aggregate process accounting and preserve one zombie/completion per TGID.

Tests: worker exit leaves group alive, joiner wakes only after clear-TID,
leader exit with surviving member, last-task finalization once, exit_group
removes all members, detached stress returns kernel stacks/task slots to
baseline, parent observes one child status.

Boot gate: join, return-value, and detached acceptance programs pass.

### U7 — On-target TinyCC pthread acceptance suite

- Package pthread examples in the TCC sysroot tarball and update its README.
- Extend `src/tests/tcc.rs` or add `src/tests/pthreads.rs` with a shared helper
  that writes/compiles sources to `/work`, executes each binary, enforces a
  bounded completion deadline, checks output/exit status, and unlinks outputs.
- Keep unknown-syscall tracing enabled for the first qualification run and
  assert the trace is empty at completion.
- Run the same binaries repeatedly to expose lost wakes and detached leaks.

Required programs:

1. `pthread_join.c`: worker returns a sentinel pointer; PID is shared, TIDs are
   distinct, and the joiner receives the sentinel.
2. `pthread_tls.c`: main plus multiple workers mutate the same `_Thread_local`
   variable and observe isolated values across scheduling points.
3. `pthread_mutex.c`: at least four workers increment one counter under a
   mutex with enough iterations to force contention; exact final count.
4. `pthread_cond.c`: wait in a predicate loop, signal one waiter, then
   broadcast to multiple waiters; every waiter re-acquires the mutex.
5. `pthread_detached.c`: create detached workers repeatedly, wait on an
   application predicate/condvar, and verify all complete without join.

Run the pthread module with `AGENTICOS_QEMU_SMP=1` and `=4`; the results must
match. The SMP=4 run also asserts through diagnostics that the pthread group
used only its home CPU while unrelated scheduled work progressed elsewhere.

### U8 — Documentation and final qualification

- Update root and subsystem `CLAUDE.md` files with the task/group model,
  supported clone/futex surface, affinity rule, and deferred TLB work.
- Update README current state and TinyCC usage examples (`-lpthread` accepted
  but musl symbols come from `libc.a`).
- Document unsupported robust/PI/process-shared operations and multithreaded
  fork/exec behavior.
- Run formatting, compile checks, focused tests, full tests, and an interactive
  zsh/TinyCC smoke.

---

## Test matrix

### In-kernel tests

| Area | Required coverage |
|---|---|
| identity | leader TID=TGID, worker TID differs, getpid/gettid/getppid |
| resource model | same L4/fds/cwd/actions; distinct regs/kstack/TLS/mask |
| clone | exact musl flags, bad dependencies/pointers, rollback, publication order |
| scheduler | group affinity, no simultaneous member execution, cross-group SMP |
| futex wait | mismatch, park, wake, timeout, EINTR, completion re-entry |
| futex wake/requeue | count limits, key isolation, move then wake destination |
| lifecycle | thread exit, clear-TID wake, last member, exit_group, deferred drop |
| process ABI | fork/exec single-task regression, multithread guards, one zombie |
| TLS/FPU | distinct values survive preempt, block, and resume |
| ownership | GUI/procfs/process service keyed by TGID, aggregate ticks |

### Booted userland tests

Run through the production loader and scheduler, not synthetic syscall calls:

```text
tcc -o /work/pthread_join /host/sysroot/examples/pthread_join.c -lpthread
/work/pthread_join

tcc -o /work/pthread_mutex /host/sysroot/examples/pthread_mutex.c -lpthread
/work/pthread_mutex

tcc -o /work/pthread_cond /host/sysroot/examples/pthread_cond.c -lpthread
/work/pthread_cond

tcc -o /work/pthread_detached /host/sysroot/examples/pthread_detached.c -lpthread
/work/pthread_detached
```

Compile TLS coverage on-target too. If the pinned TinyCC cannot emit the
required TLS relocation, retain on-target compilation for the four core
pthread programs and run one cross-built static-musl TLS fixture; record the
exact TinyCC limitation and add a separate toolchain follow-up.

All programs use watchdog-safe deadlines and return nonzero on failure. A hang
is a test failure, not an accepted skip.

### Regression/qualification commands

```sh
cargo fmt --check
cargo check
./test.sh scheduler userland userland_switch smp tcc pthreads
AGENTICOS_QEMU_SMP=1 ./test.sh pthreads
AGENTICOS_QEMU_SMP=4 ./test.sh pthreads
./test.sh
```

Also boot interactively and verify existing zsh fork/exec/wait, BusyBox tools,
TinyCC hello compilation, GUI launch/close, Task Manager process rows, and
terminal-close cleanup.

---

## Acceptance criteria

The feature is complete when all of the following are true:

1. Static-musl `pthread_create` creates a schedulable child with shared process
   resources, a distinct TID/kernel stack/FS_BASE, and the caller's requested
   user stack.
2. Every member observes the same `getpid()` and a unique `gettid()`; musl TCB
   TIDs and `set_tid_address` use those real values.
3. Contended musl mutex and condvar programs complete with correct results and
   no unknown syscalls.
4. `pthread_join` receives the worker result and cannot return before the
   exiting task's clear-child-TID protocol completes.
5. Detached threads repeatedly exit without leaking scheduler entities,
   kernel stacks, futex waiters, task slots, or process-service records.
6. `_Thread_local`/`__thread` state is isolated across at least three threads
   through preemption and blocking.
7. `SYS_exit` terminates one task; `exit_group`, fatal process death, and final
   member exit tear down the group once and produce at most one zombie/SIGCHLD.
8. Existing single-threaded fork/exec/wait, signals, GUI, procfs, networking,
   TCC, and multi-process SMP tests remain green.
9. The suite passes under QEMU SMP=1 and SMP=4 while preserving the asserted
   one-CPU-per-thread-group address-space invariant.
10. Mutex, condvar, join, and detached programs are compiled on-target by TCC;
    only a demonstrated TCC TLS-relocation limitation may use one cross-built
    TLS fixture.
11. `./test.sh` passes in full.

---

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Lost wake between futex compare and scheduler block | permanent hang | one atomic park helper under the futex lock; adversarial boundary test |
| Re-fired futex wait parks again after a real wake | intermittent join/condvar hang | explicit Woken/TimedOut/Interrupted completion marker |
| One address space runs on two CPUs | use-after-free or stale permissions through TLB | group-wide home-CPU affinity and cross-CPU running invariant |
| Current PID/TID ambiguity leaks into GUI/procfs/kill | wrong owner or target | rename scheduler/per-CPU identity to TID; explicit current_tgid helpers |
| Leader exits before workers | shared resources dropped too soon | independent `ProcessGroup` lifetime keyed by TGID/member set |
| Current task kernel stack dropped on exit | kernel stack use-after-free | deferred task reaper on a safe kernel stack |
| Detached musl unmaps its own stack before exit | kernel touches unmapped user stack | kernel owns no pthread user mapping; exit path uses kernel stack and saved clear-TID only |
| `exit_group` races a sibling on SMP | partial teardown | affinity guarantees no sibling simultaneously runs in v1 |
| Signal wake leaves stale futex waiter | later wake targets dead/ready task | signal/timeout/exit remove under futex lock before scheduler wake |
| Clone publishes before parent TID store | musl observes incomplete TCB | validate/write first, insert/register/ready last |
| Process table refactor regresses mature ABI | broad userland breakage | U1 behavior-only boundary and full test suite before clone work |
| TinyCC cannot link emitted TLS code | on-target TLS test blocked | characterize first; keep core pthread tests on-target and one explicit cross-built TLS fallback |
| Musl invokes an unplanned syscall | pthread startup fails | trace U0/U7; implement only evidence-backed semantics, never silent success stubs |
| Bounded task/futex tables exhaust | confusing hangs or kernel OOM | documented caps, deterministic EAGAIN, stress and rollback tests |

---

## Expected file changes

Primary implementation surface:

- `src/userland/lifecycle.rs` — task/group table, identity, membership, exit,
  signal and deferred-reap integration.
- `src/userland/futex.rs` — new bounded private futex subsystem.
- `src/userland/syscalls.rs` — clone, futex, gettid/set_tid_address, exit split,
  signal target semantics, fork/exec guards.
- `src/userland/abi.rs` — syscall numbers and dispatch.
- `src/userland/switch.rs`, `src/arch/x86_64/preemption.rs`,
  `src/arch/x86_64/percpu.rs` — current TID and shared-group L4 resume.
- `src/process/entity.rs`, `src/process/scheduler.rs`,
  `src/process/run_queue.rs` — `UserTask` naming and CPU affinity.
- `src/userland/usercopy.rs`, `procfs.rs`, `gui*.rs`, `process_service.rs`,
  `network_syscalls.rs` — explicit task/group ownership accessors.
- `src/tests/userland.rs`, `userland_switch.rs`, `scheduler.rs`, `smp.rs` —
  focused kernel tests.
- `src/tests/pthreads.rs` and `src/tests/mod.rs` — booted acceptance tests.
- `userland/apps/tcc/examples/pthread_*.c`, TCC README/Makefile-produced sysroot,
  and refreshed `userland/prebuilt/tcc-sysroot.tar.gz`.
- root/subsystem `CLAUDE.md` and README — supported runtime contract.

Do not refresh `TCC.ELF` unless the compiler itself changes. Adding example
sources requires regenerating only the deterministic sysroot tarball through
the existing prebuilt refresh workflow.

---

## Follow-ups unlocked by this work

1. Remote user-TLB shootdown and parallel execution of one thread group.
2. POSIX multithreaded fork/exec (`pthread_atfork`, de-threading on exec).
3. Robust and process-shared futexes once shared memory exists.
4. PI futexes/realtime scheduling, cancellation completeness, and per-thread
   `/proc/<tgid>/task` observability.
5. Thread rows and per-thread CPU accounting in Task Manager.
