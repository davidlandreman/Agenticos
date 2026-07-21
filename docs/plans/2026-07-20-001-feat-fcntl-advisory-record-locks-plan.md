---
title: "feat: advisory fcntl record locks (F_GETLK / F_SETLK / F_SETLKW)"
type: feat
status: implemented
date: 2026-07-20
depth: medium
related_docs:
  - CLAUDE.md
  - src/userland/CLAUDE.md
  - src/fs/CLAUDE.md
  - docs/plans/2026-07-18-011-feat-gcc-port-plan.md
  - docs/plans/2026-07-18-009-feat-musl-pthread-runtime-plan.md
---

# feat: advisory fcntl record locks (F_GETLK / F_SETLK / F_SETLKW)

## Summary

`fcntl(fd, cmd, arg)` today (`src/userland/syscalls.rs::fcntl_handler`, ~line
3827) implements only descriptor/status operations: `F_DUPFD`,
`F_DUPFD_CLOEXEC`, `F_GETFD`, `F_SETFD`, `F_GETFL`, `F_SETFL`. Every other
`cmd` returns `-ENOSYS`. This plan adds the three POSIX advisory
**record-locking** commands:

- `F_GETLK` (5) — test whether a lock could be placed; report a conflictor.
- `F_SETLK` (6) — acquire or release a lock, non-blocking (`-EAGAIN` on
  conflict).
- `F_SETLKW` (7) — acquire a lock, blocking until it can be granted.

The motivating consumer is a future **GNU Make** port (a documented follow-up
to the GCC port, `docs/plans/2026-07-18-011-feat-gcc-port-plan.md`). Make's
`--output-sync` serializes stdout/stderr across the recursive make process tree
by taking an exclusive whole-file write lock (`F_SETLKW`, `F_WRLCK`) on a shared
"sync mutex" file descriptor, writing the buffered output, then releasing it
(`F_UNLCK`). Without working record locks, `--output-sync` either fails at
startup or silently interleaves output from parallel sub-makes.

These are **advisory** locks (they constrain only other `fcntl` lockers, never
`read`/`write`) and **process-associated** (classic POSIX semantics: owned by
the process, released when the process closes *any* descriptor referring to the
file, or exits). Open-file-description locks (`F_OFD_*`, cmds 36–38) and
mandatory locking are out of scope.

---

## Current state and findings

### The fcntl surface is descriptor-only

`fcntl_handler` matches the six descriptor/status cmds and falls through to
`_ => ENOSYS` (`src/userland/syscalls.rs:3918`). The `cmd` constants live at
`src/userland/syscalls.rs:2710-2716`. There is no lock state anywhere in the
kernel — nothing tracks byte ranges, no registry, no owner accounting.

### File identity is available but private

Record locks must be keyed on the *file*, not the descriptor: two independent
`open()` calls on the same path, and an inherited fd shared across a
fork+exec'd sub-make, must all contend on one lock space. The `File` handle
carries that identity but does not expose it:

- `src/fs/file_handle.rs` — `FileHandleInner { filesystem: &'static dyn
  Filesystem, fs_handle: Option<FileHandle>, .. }`. `fs_handle.inode` is a
  `u64` inode number; `filesystem` is a stable `'static` pointer.
- `FdSlot::File { handle: Arc<File>, .. }` (`src/userland/fdtable.rs:71`) is the
  descriptor-side wrapper.

A `(filesystem-pointer, inode)` pair is a stable per-file key: the filesystem
pointer discriminates inode-number collisions between mounts (ext2 inode 12 vs
tmpfs inode 12), and the inode is stable across repeated opens of the same path
within one filesystem. **Open question to verify during implementation
(Step 0):** confirm tmpfs and overlay return a *stable, per-path* inode across
separate `open()` calls — if either synthesizes fresh inode numbers per handle,
fall back to keying on the normalized absolute path (safe for the whole-file
use case; loses hard-link/rename correctness, which Make does not need).

### The process model gives us owner identity and teardown hooks

- POSIX process = thread-group leader TGID. `task_tgid(tid)`
  (`src/userland/lifecycle.rs`) maps any task to its owning process. Locks are
  keyed by TGID so all pthreads of a process share one lock owner and `fork`
  children (new TGID) do not inherit locks — exactly POSIX semantics, for free.
- **Close hook:** `close_handler` (`src/userland/syscalls.rs:3044`) already
  computes whether any other descriptor in the group still references the closed
  open-file description (the epoll-prune path). This is the natural site to
  release the process's locks on a file when a descriptor to it closes.
- **Exit hook:** `finish_group` (`src/userland/lifecycle.rs:2826`) calls
  `close_group_fds(tgid)` before publishing the zombie. Releasing the group's
  locks belongs here (and in the thread/group exit paths that reach
  `finish_group`).

### There is an established blocking-syscall pattern to copy

`F_SETLKW` must park the caller and re-fire on wake, exactly like the futex and
pipe waits. The template:

- `Ring3BlockReason` enum (`src/userland/lifecycle.rs:294`) already has
  `WaitingForFutex`, `WaitingForPipeRead`, `WaitingForInput`, etc.
- The futex wait (`src/userland/futex.rs:224`) blocks on
  `Ring3BlockReason::WaitingForFutex { .. }` and is woken by a registry event;
  the re-fired SYSCALL re-evaluates and either completes or re-blocks.
- Signal interruption uses `pending_syscall_interrupt` → the woken blocking
  syscall re-enters as `-EINTR` (documented in `src/userland/CLAUDE.md`).

A blocking record lock is the same shape: park on a new
`WaitingForFileLock` reason, broadcast-wake all lock waiters whenever any lock
is released, re-fire, re-attempt.

---

## Design

### 1. ABI structs and constants

Linux x86-64 `struct flock` (32 bytes; `off_t`/`pid_t` are 64/32-bit, so no
separate `flock64` is needed on this arch — musl passes plain `struct flock`
for all three cmds):

```
offset  size  field
  0      2    l_type    (short)  F_RDLCK=0, F_WRLCK=1, F_UNLCK=2
  2      2    l_whence  (short)  SEEK_SET=0, SEEK_CUR=1, SEEK_END=2
  4      4    (padding)
  8      8    l_start   (off_t, i64)
 16      8    l_len     (off_t, i64)   0 = "to EOF"; negative allowed
 24      4    l_pid     (pid_t, i32)   F_GETLK output only
 28      4    (padding)
```

Add cmd + type constants beside the existing block at
`src/userland/syscalls.rs:2710`:

```rust
const F_GETLK: i32 = 5;
const F_SETLK: i32 = 6;
const F_SETLKW: i32 = 7;
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
```

Define a `#[repr(C)]` `LinuxFlock` mirror and read/write it through `usercopy`
(the VMA-aware path — never a raw user deref). Reuse existing errno constants
from `src/userland/abi.rs` (`EAGAIN`, `EACCES`, `EINVAL`, `EBADF`, `ENOLCK`,
`EINTR`; add `EDEADLK` only if not already present, though it is unused — see
deferred).

### 2. New module: `src/userland/record_lock.rs`

Owns the global lock table and all range algebra. `no_std` + `alloc`.

```rust
pub struct FileKey { fs: usize, inode: u64 }   // (filesystem ptr addr, inode)

enum LockKind { Read, Write }

struct LockRecord {
    start: u64,          // absolute byte offset
    end: u64,            // inclusive; u64::MAX == to EOF / whole file
    kind: LockKind,
    owner_tgid: u32,
}
```

Global registry:
`static LOCKS: InterruptMutex<BTreeMap<FileKey, Vec<LockRecord>>>`.

`InterruptMutex` (not `spin::Mutex`) matches the load-bearing SMP discipline
used by `PROCESS_TABLE`. Bound growth: cap records per file (e.g. 256) and total
files; on exhaustion return `-ENOLCK` from `F_SETLK`/`F_SETLKW`. Never allocate
or take this lock from an interrupt/crash path.

**Lock ordering.** Take `LOCKS` on its own; do not hold it across a filesystem
call, a user copy, or a scheduler block. The syscall handler resolves the
`FileKey` and the absolute byte range *first* (reading fd position/size under
`PROCESS_TABLE`), then does the pure range algebra under `LOCKS`, then releases
`LOCKS` before parking (for `F_SETLKW`) or copying results back to the user.

Public API (all pure range algebra except the block/wake glue):

- `fn conflict(key, range, kind, owner) -> Option<LockRecord>` — first record
  owned by a *different* TGID that overlaps `range` with an incompatible kind
  (Write conflicts with anything; Read conflicts only with Write). Same-owner
  records never conflict.
- `fn set(key, range, kind, owner) -> Result<(), Errno>` — assumes no
  conflict; installs the lock, splitting/merging the owner's own overlapping
  records so the region reflects the new kind. Coalesce adjacent same-kind
  same-owner ranges. (Whole-file is the common path and collapses to a single
  record.)
- `fn unlock(key, range, owner)` — remove/split the owner's records over
  `range`; drop the file's entry when its vector empties.
- `fn getlk(key, range, kind, owner) -> Option<(LockKind, u64, u64, u32)>` —
  test; return conflictor descriptor for the `F_GETLK` reply, else `None`.
- `fn release_all(owner_tgid, key)` — drop every record the owner holds on one
  file (close hook).
- `fn release_owner(owner_tgid)` — drop every record the owner holds on every
  file (exit hook). Returns whether anything was removed (to gate the wake).

The split/merge algorithm (standard POSIX record-lock coalescing):
1. Remove all same-owner records that overlap or abut `range`, retaining their
   fragments that fall strictly outside `range` (may produce a left and/or
   right remnant with the *old* kind).
2. Insert a single record `{range, new kind, owner}` (for `set`) or nothing
   (for `unlock`).
3. Merge with adjacent same-owner same-kind neighbors.

Unit-testable in isolation (Step 5) — this is where correctness bugs hide.

### 3. Range resolution

Resolve `l_whence` + `l_start` + `l_len` to an absolute inclusive `[start,
end]`:

- `SEEK_SET` → base 0. `SEEK_CUR` → base = fd's current position.
  `SEEK_END` → base = file size. For `FdSlot::File` both are reachable
  (`File::position()` / `File::size()`).
- `abs_start = base + l_start`; reject `< 0` with `-EINVAL`.
- `l_len == 0` → `end = u64::MAX` (lock through EOF; standard "whole file"
  when combined with `start = 0`).
- `l_len > 0` → `end = abs_start + l_len - 1`.
- `l_len < 0` → range is `[abs_start + l_len, abs_start - 1]` (POSIX allows a
  negative length; the lock ends just before the requested offset). Reject if
  the resulting start `< 0`.

### 4. File-identity accessor

Add to `src/fs/file_handle.rs`:

```rust
impl File {
    /// (filesystem pointer address, inode) — stable per-file lock identity.
    pub fn lock_identity(&self) -> (usize, u64) { .. }
}
```

Reads `inner.filesystem` (as `*const dyn` → address) and
`inner.fs_handle.inode` under the handle lock. If Step 0 finds inode identity
unreliable for tmpfs/overlay, fall back to hashing the normalized path stored in
`inner.path` for those filesystems (documented at the accessor).

### 5. Handler dispatch (`fcntl_handler`)

Add three arms before `_ => ENOSYS`:

```
F_GETLK | F_SETLK | F_SETLKW => fcntl_lock(fd, cmd, arg /* user *flock */)
```

`fcntl_lock`:
1. Resolve the fd. Only `FdSlot::File` is lock-capable here. For non-File
   descriptors (sentinels, pipes, sockets, virtual files) return **`-EINVAL`**
   — *not* a silent success (a fake grant would defeat `--output-sync`). See
   "Make lock target" below: Make 4.4 locks a real temp file, so `File` is the
   only case that must work. Revisit if a consumer needs pipe/tty locks.
2. `usercopy`-read the `flock`. Validate `l_type` ∈ {RDLCK, WRLCK, UNLCK}.
3. Resolve the absolute range (Step 3) and `FileKey` = `File::lock_identity()`.
   Owner = `task_tgid(current)`.
4. Branch:
   - **F_GETLK:** `getlk`. If a conflictor exists, write its
     `l_type/l_start/l_len/l_pid` back into the user `flock`; else set
     `l_type = F_UNLCK`. Return 0.
   - **F_SETLK with F_UNLCK:** `unlock`; wake lock waiters; return 0.
   - **F_SETLK with RD/WR:** if `conflict` → `-EAGAIN`; else `set` (mapping
     `ENOLCK` on table exhaustion) and return 0.
   - **F_SETLKW with F_UNLCK:** same as F_SETLK unlock.
   - **F_SETLKW with RD/WR:** if no conflict → `set`, return 0. If conflict →
     **block** (Step 6).

### 6. Blocking F_SETLKW

Add `Ring3BlockReason::WaitingForFileLock` (no payload needed — wake is a
conservative broadcast, matching the pipe-wait model). In `fcntl_lock`, on
conflict under `F_SETLKW`:

1. Release `LOCKS` (never park holding it).
2. Rewind RIP to the SYSCALL boundary and park via the existing
   `switch::block_current_ring3_*` helper with `WaitingForFileLock` (model on
   `futex.rs`'s block call). The handler is **idempotent**: on wake the SYSCALL
   re-fires, re-reads the same user `flock`, re-resolves the range, and
   re-attempts — succeeding or re-blocking.
3. **Wake source:** every lock release — `F_SETLK`/`F_SETLKW` unlock,
   `close_handler`, and group/thread exit — calls a new
   `lifecycle::wake_ring3_blocked_on_file_lock()` that marks every
   `WaitingForFileLock` task ready (broadcast; each re-checks and re-blocks if
   still contended — same "thundering herd, self-correcting" contract as
   `WaitingForPipeRead`).
4. **Signal interruption:** honor `pending_syscall_interrupt` so a signal
   returns `-EINTR` (POSIX: `F_SETLKW` is interruptible). Reuse the same
   interrupt-check the other blocking syscalls perform on re-entry.

No deadline/timer is needed — `F_SETLKW` blocks indefinitely.

### 7. Release hooks

- **`close_handler` (`src/userland/syscalls.rs:3044`):** after a successful
  close of a `FdSlot::File`, call `record_lock::release_all(tgid,
  file_key)` for that file's key, then
  `wake_ring3_blocked_on_file_lock()`. Classic POSIX: closing *any* fd on the
  file drops *all* the process's locks on it, regardless of other open fds to
  the same file. (Capture the `File::lock_identity()` from the slot *before* it
  is dropped.)
- **`finish_group` (`src/userland/lifecycle.rs:2826`):** call
  `record_lock::release_owner(tgid)` alongside `close_group_fds(tgid)`, then
  broadcast-wake. This covers `exit`, `exit_group`, and fatal-signal
  termination (all funnel through `finish_group`). Confirm the single-thread
  `cooperative_thread_exit` non-final-member path does **not** release the
  group's locks (only the final member, via `finish_group`, should) — locks are
  per-process, not per-thread.

### 8. fork / exec interaction (no code, but verify)

- **fork:** child gets a new TGID; registry is TGID-keyed, so the child holds
  no locks and the parent's persist. Correct POSIX. No code.
- **exec:** `execve` replaces the image but keeps the TGID and non-CLOEXEC fds.
  POSIX keeps process-associated locks across exec. Since the registry is keyed
  by TGID and untouched by exec, locks correctly persist. Verify no exec path
  incidentally calls a lock-release hook.

---

## Make lock target (why File-backed is sufficient)

GNU Make 4.4's output-sync (`src/posixos.c` `osync_setup`) creates a **real
temporary file** (in `TMPDIR`/`/tmp`), takes `F_SETLKW`/`F_WRLCK` over the whole
file, and passes the fd number to sub-makes via `MAKEFLAGS
--sync-mutex=<fd>`. Older versions that locked `stdout` were changed precisely
because locking terminals/pipes is unportable. So a `File`-backed record lock
covers the intended consumer.

**Cross-dependency (out of scope here, flag for the Make port):** the sync file
lands in `TMPDIR` or `/tmp`. AgenticOS provisions `/work` and `/root` but not
necessarily a writable `/tmp` (`src/fs/CLAUDE.md` mount topology). The Make port
must either provision `/tmp` on the overlay or set `TMPDIR=/work`. This plan
does not depend on it — it only needs record locks on any writable `File`.

---

## Implementation steps

0. **Verify inode identity.** Add a throwaway probe (or a targeted test) that
   opens the same `/work` and `/data` path twice and confirms
   `File::lock_identity()` matches. Decide inode-key vs path-key fallback per
   filesystem before building the registry.
1. **ABI + accessor.** Add the cmd/type constants, `LinuxFlock` repr(C) +
   usercopy read/write, and `File::lock_identity()`.
2. **`record_lock.rs`.** Registry, `FileKey`, `LockRecord`, and the pure range
   algebra (`conflict`/`set`/`unlock`/`getlk`/`release_all`/`release_owner`).
   Wire into `src/userland/mod.rs`.
3. **Non-blocking handlers.** `fcntl_lock` for `F_GETLK`, `F_SETLK`
   (RD/WR/UNLCK), and `F_SETLKW` fast path (grant-without-conflict). Dispatch
   from `fcntl_handler`.
4. **Blocking path.** `Ring3BlockReason::WaitingForFileLock`,
   `wake_ring3_blocked_on_file_lock`, the park/re-fire in `fcntl_lock`, and
   signal-interrupt (`-EINTR`) handling.
5. **Release hooks.** `close_handler` + `finish_group`, each followed by a
   broadcast wake.
6. **Docs.** Update the `fcntl_handler` doc comment (line 3823),
   `src/userland/CLAUDE.md` (the "Native-tool compatibility" line and a note on
   the new block reason + release hooks), and CLAUDE.md's fcntl mention if any.

## Testing

New in-kernel test module `src/tests/record_lock.rs` (register in
`src/tests/mod.rs::get_tests`), pure range-algebra tests — no QEMU networking,
fast:

- Read/read compatible; read/write and write/write conflict (different owners).
- Same-owner re-lock replaces/upgrades/downgrades without conflict.
- Whole-file (`start=0,len=0`) vs bounded range overlap.
- `unlock` of a sub-range splits a larger held range into two remnants.
- Coalescing of adjacent same-kind same-owner ranges.
- `getlk` reports the correct conflictor `type/start/len/pid`, and `F_UNLCK`
  when free.
- Negative `l_len` range resolution; `SEEK_CUR`/`SEEK_END` base resolution;
  `abs_start < 0` → `EINVAL`.
- `release_all` (one file) and `release_owner` (all files) drop exactly the
  owner's records.
- Table-exhaustion → `ENOLCK`.

Syscall-level test (model on `src/tests/userland_switch.rs` synthetic ring-3
fixtures): a process takes `F_SETLK` `F_WRLCK`, a second owner's `F_SETLK`
returns `EAGAIN` and `F_GETLK` reports the holder; after the first releases (or
its fd closes / it exits), the range is grantable. The **blocking** `F_SETLKW`
wake is best validated end-to-end during Make bring-up; if a two-process
blocking fixture is impractical in-kernel, document manual validation
(`make -j --output-sync=target` producing non-interleaved output across
recursive invocations) as the acceptance gate.

## Deferred / out of scope

- **`F_OFD_*` locks** (cmds 36–38) — open-file-description-owned; not needed by
  Make. The registry could later key by open-file-description identity to add
  them.
- **Deadlock detection (`EDEADLK`).** Linux detects simple lock cycles; Make
  never creates one. `F_SETLKW` will block indefinitely on a genuine cycle
  rather than returning `EDEADLK`. Acceptable; document it.
- **Mandatory locking** — always advisory here.
- **Non-`File` fd locking** (pipes, ttys, sockets) — returns `-EINVAL`. Add
  only when a real consumer needs it.
- **Provisioning `/tmp` / `TMPDIR`** — belongs to the Make port plan.
```
