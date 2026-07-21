# feat: implement `pselect6(2)`

**Date:** 2026-07-20
**Status:** Implemented
**Owner:** userland / syscall ABI

## Motivation

`pselect6` (syscall 270) is currently a hard `-ENOSYS` stub
(`src/userland/syscalls.rs::pselect6_handler`, dispatched from
`src/userland/abi.rs:410`). This blocks any static-musl program that reaches
for `pselect()` instead of `select()`/`poll()`.

The immediate driver is the GNU Make port. Make's default **jobserver** uses
`pselect()` to wait *atomically* for either a jobserver pipe token to become
readable **or** `SIGCHLD` — the atomic unblock-of-`SIGCHLD`-during-the-wait is
the whole point of `pselect` over `select` (it closes the classic
signal-race window). We do **not** implement a real temporary signal mask on a
blocking syscall (neither `ppoll` nor `rt_sigsuspend`'s blocking path gives Make
what it needs here), so the jobserver's correctness guarantee cannot be honored.

**Therefore this plan implements the `pselect6` *mechanism* (fd readiness +
timeout wait) but explicitly does NOT implement the atomic signal-mask swap.**
Make must keep `--disable-job-server`. With the jobserver disabled, `make`,
`make -j1`, and non-recursive `make -jN` drive `pselect6` only as a plain
readiness/timeout wait, which this plan fully supports.

## Background: what `pselect6` is, vs. our existing `select`

We already have a complete, battle-tested `select_handler`
(`src/userland/syscalls.rs:5266`) — Links uses it as its central event loop. It
samples per-fd readiness through the shared `fd_readiness` snapshot, computes
ready bits, and on "nothing ready + non-zero timeout" parks the process on the
restart-stable `readiness::block` deadline path. `pselect6` is `select` with
three differences:

| Aspect | `select` (nr 23) | `pselect6` (nr 270) |
|---|---|---|
| Args 1–4 | nfds, readfds, writefds, exceptfds (rdi/rsi/rdx/r10) | **identical** (rdi/rsi/rdx/r10) |
| Timeout (r8) | `struct timeval` (sec + **micro**sec), Linux may modify it | `struct timespec` (sec + **nano**sec), **const**, never written back |
| 6th arg (r9) | — | pointer to `struct { const sigset_t *ss; size_t ss_len; }` |
| Signal mask | none | temporarily install `*ss` for the duration of the wait |

Key ABI facts:

- **First five registers are identical** to `select`, so the existing
  block-identity hash (`args.rsi ^ args.rdx.rotate_left(11) ^
  args.r10.rotate_left(23) ^ (nfds).rotate_left(37)`) and the mask
  write-back paths work unchanged for `pselect6`.
- The 6th argument is **not** the sigmask pointer directly (as it is in
  `ppoll`, where r10=sigmask-ptr and r8=sigsetsize are separate registers).
  Because Linux syscalls cap at 6 args, `pselect6` packs the pointer and its
  length into a caller-supplied **16-byte struct** at `r9`:
  ```c
  struct __kernel_pselect6_sigmask {
      const kernel_sigset_t *ss;  // offset 0, 8 bytes
      size_t                 ss_len;  // offset 8, 8 bytes
  };
  ```
  `r9 == 0` means "no sigmask" (behave exactly like `select`).
- POSIX `pselect` does **not** modify the timeout on return. Our `select_handler`
  already never writes the timeout back, so this is automatically satisfied —
  we just parse it as `timespec` instead of `timeval`.

## Design

### 1. Extract a shared core from `select_handler`

Refactor `select_handler` so the readiness scan + block + mask write-back live
in one helper that both entry points call. The only thing the two entry points
compute differently is `timeout_ticks` (from `timeval` vs `timespec`) and the
sigmask arg (`pselect6` only). The masks are read from the same registers
(rsi/rdx/r10) and written back to the same registers, so the helper can take the
raw `args` plus the pre-read masks and the computed `timeout_ticks`.

```rust
fn select_common(
    args: &mut SyscallArgs,
    nfds: usize,
    read_in: u64,
    write_in: u64,
    except_in: u64,
    timeout_ticks: Option<u64>,
) -> i64 {
    // ...everything from `crate::net::poll_once()` through the final
    //    `ready` return in today's select_handler, verbatim...
}
```

`select_handler` becomes: validate `nfds`, read the three masks with
`select_read_mask`, parse the `timeval` timeout (existing code), then
`select_common(args, nfds, read_in, write_in, except_in, timeout_ticks)`.

This is a pure, behavior-preserving refactor of `select_handler` — no logic
changes to the existing select path.

### 2. `pselect6_handler`

```rust
/// `pselect6(nfds, *readfds, *writefds, *exceptfds, *timeout, *sig) -> int`
///
/// select(2) with a nanosecond `timespec` (const — never written back) and a
/// 6th argument that packs {sigset_t *ss, size_t ss_len} into a 16-byte struct.
///
/// The temporary signal mask is validated but NOT applied: like `ppoll`, we do
/// not implement atomically unblocking a signal for the duration of a blocking
/// wait. This is why GNU Make's jobserver must stay `--disable-job-server` — its
/// SIGCHLD-race protection depends on the mask actually taking effect. Plain
/// `-j1`/non-recursive readiness+timeout waits work regardless.
pub fn pselect6_handler(args: &mut SyscallArgs) -> i64 {
    // --- nfds + masks: identical to select_handler ---
    let nfds_signed = args.rdi as i64;
    if nfds_signed < 0 || nfds_signed as usize > crate::userland::fdtable::FD_TABLE_SIZE {
        return EINVAL;
    }
    let nfds = nfds_signed as usize;
    let read_in   = match select_read_mask(args.rsi, nfds) { Ok(v) => v, Err(e) => return e };
    let write_in  = match select_read_mask(args.rdx, nfds) { Ok(v) => v, Err(e) => return e };
    let except_in = match select_read_mask(args.r10, nfds) { Ok(v) => v, Err(e) => return e };

    // --- timeout: timespec (nanoseconds), const, mirrors ppoll_handler ---
    let timeout_ticks = if args.r8 == 0 {
        None
    } else {
        let ts = match crate::userland::usercopy::read_unaligned::<Timespec>(args.r8) {
            Ok(v) => v,
            Err(e) => return e,
        };
        if ts.seconds < 0 || !(0..1_000_000_000).contains(&ts.nanoseconds) {
            return EINVAL;
        }
        let ms = (ts.seconds as u64)
            .saturating_mul(1000)
            .saturating_add((ts.nanoseconds as u64 + 999_999) / 1_000_000);
        Some((ms + 9) / 10)
    };

    // --- 6th arg: {const sigset_t *ss, size_t ss_len} at r9 ---
    // Validate the ABI but do not apply the mask (see doc comment).
    if args.r9 != 0 {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Sigmask { ss: u64, ss_len: u64 }
        let sig = match crate::userland::usercopy::read_unaligned::<Sigmask>(args.r9) {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Only a non-NULL ss carries a length constraint. sigset_t is one u64.
        if sig.ss != 0 && sig.ss_len != 8 {
            return EINVAL;
        }
        // Touch the mask bytes so a bogus pointer fails here rather than being
        // silently ignored (matches how a real kernel would fault on it).
        if sig.ss != 0 {
            if let Err(e) = crate::userland::usercopy::read_unaligned::<u64>(sig.ss) {
                return e;
            }
        }
    }

    select_common(args, nfds, read_in, write_in, except_in, timeout_ticks)
}
```

`Timespec` already exists as a local struct inside `ppoll_handler`; hoist it to
module scope (next to `SelectTimeval`) so both `ppoll_handler` and
`pselect6_handler` share it. It is a plain `#[repr(C)] { seconds: i64,
nanoseconds: i64 }`.

### Decisions / rationale

- **Validate the sigmask struct, don't apply it.** Reading the 16-byte struct
  and (when `ss != NULL`) validating `ss_len == 8` plus touching `*ss` makes the
  handler ABI-honest: a caller passing a garbage `sig` pointer or wrong length
  gets `EFAULT`/`EINVAL` exactly as it would on Linux, instead of us silently
  ignoring the arg. We deliberately go slightly stricter than `ppoll_handler`
  (which ignores its sigmask args entirely) because `pselect6`'s struct-wrapped
  form is the one Make actually exercises, and a clear `EINVAL` is a better
  failure than a mask that looks accepted but isn't.
- **We do NOT install the mask.** Implementing atomic "unblock SIGCHLD only
  while parked" would require threading a temporary mask through
  `readiness::block` and re-checking pending signals on wake — the same gap
  called out for `ppoll` and `rt_sigsuspend`. Out of scope; documented so the
  jobserver stays disabled.
- **Reuse, don't fork, the select core.** Extracting `select_common` avoids a
  second copy of the readiness-scan/block/write-back logic (which is subtle:
  `poll_once` ordering, `observed_sequence` sampling before the scan, the
  `clear_network_wait` vs `block` branch). One code path, two thin arg parsers.

## Files to change

1. **`src/userland/syscalls.rs`**
   - Hoist `Timespec` to module scope (shared by `ppoll_handler`,
     `pselect6_handler`); update `ppoll_handler` to use the shared type.
   - Extract `select_common(...)` from `select_handler` (behavior-preserving).
   - Rewrite `select_handler` to parse + delegate.
   - Replace the `pselect6_handler` stub with the real implementation above.
2. **`src/userland/abi.rs`** — no change needed; `nr::PSELECT6 => ...` is already
   wired at line 410. (Confirm the dispatch still points at `pselect6_handler`.)
3. **`src/userland/CLAUDE.md`** — under the syscall-surface notes, record that
   `pselect6` is implemented as select-with-`timespec`, that its temporary
   signal mask is validated-but-ignored (same class as `ppoll`), and that GNU
   Make's jobserver must stay disabled because of it. Cross-link the existing
   "signal mask not applied on blocking wait" limitation.
4. **CLAUDE.md "Deferred" / known-issues** (root) — optional: add a one-line
   note that `pselect6`'s atomic-sigmask semantics are unimplemented, alongside
   the existing `ppoll`/`rt_sigsuspend` notes, so the Make jobserver constraint
   is discoverable.

## Testing

There is **no** existing in-kernel unit test for `select`/`poll`/`ppoll`; these
are validated end-to-end by real programs (Links' select loop, musl's poll).
Follow that precedent, but add cheap synthetic coverage where it's easy:

1. **Synthetic dispatch tests** (`src/tests/userland_switch.rs` or a new
   `src/tests/select.rs` topic module — see `src/tests/CLAUDE.md` for
   registration). Drive `pselect6_handler` with a hand-built `SyscallArgs`:
   - `nfds < 0` and `nfds > FD_TABLE_SIZE` → `EINVAL`.
   - `timeout` with `nanoseconds >= 1_000_000_000` or `seconds < 0` → `EINVAL`.
   - `r9` sigmask struct with `ss != 0 && ss_len != 8` → `EINVAL`.
   - `r8 == 0` (NULL timeout) + all-zero masks + `r9 == 0` → returns `0`
     (nothing requested, no block) — mirrors a `select` with empty sets.
   - Zero timeout (`timespec{0,0}`) with a readable synthetic fd requested →
     returns `1` without parking. (Reuse whatever fd fixture the readiness
     tests can construct; if none exists cheaply, keep this case for the
     end-to-end pass instead.)
   These require the compatibility user-pointer bounds path used by other
   synthetic syscall tests (`abi.rs` notes it); model the `SyscallArgs` setup on
   the existing `wait4`/switch tests.
2. **Regression guard for the refactor.** Because `select_handler` is being
   split, run the Links HTTP(S) suite / any select-driven test that currently
   passes to confirm no behavior change. (Note: per repo memory, links2 **HTTPS**
   tests do not pass on this machine due to an unrelated kernel signal bug — use
   the HTTP path or curl for the regression check, not HTTPS.)
3. **End-to-end (the real acceptance):** build/boot GNU Make with
   `--disable-job-server` and run `make`, `make -j1`, and a non-recursive
   `make -jN` over a small `Makefile`. Confirm `pselect6` no longer returns
   `-ENOSYS` (trace mode) and the builds complete. This belongs to the Make
   port plan; this change is its prerequisite.

## Out of scope / follow-ups

- **Atomic temporary signal mask on blocking `pselect6`/`ppoll`.** The real fix
  for the Make jobserver. Requires `readiness::block` to accept a temporary
  block mask and re-evaluate pending/deliverable signals on wake (the same
  machinery `rt_sigsuspend` would need for its blocking path). Tracked with the
  existing "signal mask not applied" limitations in `src/userland/CLAUDE.md`.
- Writing remaining time back to the timeout — N/A: `pselect` is defined not to
  modify it.

## Risk

Low. The core readiness/block logic is unchanged (extracted verbatim); the new
code is arg parsing that mirrors two existing handlers (`select` for masks,
`ppoll` for the `timespec`). The one genuinely new surface is the `r9`
struct-wrapped sigmask decode, which is validated defensively and cannot park
or mutate process state.
