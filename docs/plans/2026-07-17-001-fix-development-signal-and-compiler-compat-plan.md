---
title: "fix: Restore test isolation and add booted compiler-compat coverage"
status: completed
created: 2026-07-17
plan_type: fix
depth: medium
related_docs:
  - .claude/rules/testing-flow.md
  - src/tests/CLAUDE.md
  - src/fs/CLAUDE.md
  - src/userland/CLAUDE.md
  - userland/README.md
---

# fix: Restore test isolation and add booted compiler-compat coverage

## Summary

Restore the full booted test suite as a trustworthy development signal before
expanding compatibility coverage. The work has four connected parts:

1. Remove the second writable FAT mount used by `fat_write` tests and run all
   directory mutations through the already-mounted `/data` filesystem.
2. Update the demand-grown-stack test to cover the implementation's supported
   gap-filling behavior instead of asserting the retired low-water-mark rule.
3. Make every unknown syscall return Linux `-ENOSYS`, including for a live
   ring-3 process; trace mode remains a logging mode, not a survival mode.
4. Add a hermetic, booted `compiler_compat` module that launches committed
   static-musl ET_EXEC fixtures of increasing complexity through the same
   `/host` filesystem, ELF loader, process lifecycle, and syscall path used by
   real programs.

The final acceptance criterion is not merely that each module passes alone:
`./test.sh` must pass in a single boot, and the former order-dependent FAT
reproducer must pass unchanged.

Implementation completed on 2026-07-17. The final unfiltered boot passed all
702 tests. Restoring that signal also required isolating synthetic scheduler,
process-table, shared-page-table, and PTY tests that the formerly earlier FAT
failure had prevented the suite from reaching.

## Evidence and problem frame

### FAT tests share a disk but not mount state

`./test.sh filesystem fat_write` currently reproduces the failure. The
`filesystem` module writes files and overlay-state blobs through the VFS-mounted
`FatFilesystem`. Later, `fat_write::shared_data_fs()` constructs a second
writable `FatFilesystem` over the same Secondary Master disk. The two instances
have independent `alloc_hint` and short-name caches. In the reproduced run,
`test_u9_create_long_name_writes_lfn_run` creates `notes.markdown`, then the
second instance cannot find it and panics at `src/tests/fat_write.rs:329`.

QEMU `snapshot=on` isolates one boot from the next; it does not isolate tests or
modules within the same boot. The tests must therefore share the production
mount and clean up their mutations.

### The stack test asserts an obsolete invariant

`try_grow_user_stack` intentionally accepts any unmapped page inside
`[stack_max_growth_floor, stack_top)`. A fault may be above `stack_bottom` when
filling a hole left in an inherited or partially mapped stack. The test at
`src/tests/userland.rs:3294` still says every address at or above
`stack_bottom` is `NotStackGrow`, contradicting the implementation and its
bookkeeping comments.

### Unknown syscalls have two control-flow contracts

`abi::unhandled_syscall` returns `ENOSYS` in trace mode and for synthetic tests,
but calls `unimplemented_syscall_exit` for a live ring-3 process when trace mode
is off. This makes ordinary libc feature detection fatal unless a diagnostic
flag is enabled. Linux programs expect an unsupported syscall to return
`-ENOSYS` so they can select an older implementation or report a normal error.

### Existing compiler-produced coverage is optional

The hand-built ELF fixtures are deterministic and valuable, but they do not
exercise a real musl CRT and compiler-generated code. `HELLOCPP.ELF` is skipped
when a cross compiler is absent, while zsh and BusyBox tests are not a small,
progressive compatibility ladder. A fresh clone therefore lacks a mandatory
booted test that says which level of static-musl output the kernel can run.

## Goals

- `filesystem` and `fat_write` pass together in the normal registry order and
  when selected explicitly in one boot.
- Mutable filesystem tests use the same `/data` mount production code uses and
  delete files they create when the assertion path succeeds.
- Stack-growth tests distinguish contiguous downward growth, in-window hole
  filling, already-mapped faults, out-of-window faults, overflow, and budget
  exhaustion.
- Unknown syscall numbers always return `-ENOSYS` to ring 3 without changing
  process exit kind.
- Trace mode controls first-occurrence argument logging only.
- `compiler_compat` is mandatory, deterministic, filterable with
  `./test.sh compiler_compat`, and runs real committed static-musl binaries.
- The complete unfiltered booted suite passes.

## Non-goals

- Implementing every syscall probed by musl, libc++, compilers, or build tools.
- Adding dynamic linking or ET_DYN/PIE support; fixtures remain static ET_EXEC.
- Turning the compatibility suite into a full C/C++ conformance suite.
- Fixing unrelated filesystem crash consistency or short-name-cache design.
- Requiring a musl cross toolchain for ordinary `./test.sh` runs.

## Key decisions

### Use the mounted `/data` instance as the test boundary

Remove `SHARED_FS`, its leaked `IdeBlockDevice`, and the forced second
`enable_writes(true)` call from `src/tests/fat_write.rs`. Keep direct
`FatTable` tests only for operations that save and restore the exact FAT entry
they touch. Run create, lookup, read/write, LFN, and unlink tests through
`File`, VFS operations, or a test-only accessor to the already-mounted FAT
wrapper if a low-level assertion cannot be expressed through the public API.

Prefer the public API for the existing tests:

- Create/open/write/read via `/data/<unique-test-name>`.
- Verify long-name and case-insensitive lookup via `exists`, `metadata`, and
  `File::open_read` on the same mount.
- Remove every created file with `vfs_unlink` at the end of the test.
- Exercise short-name collision caching by creating several long names through
  the mounted instance, verifying all resolve uniquely, then unlinking them.

Also add cleanup to successful `filesystem` tests that currently leave
`u10-test.txt`, `u10-big.bin`, overlay-state slots, or marker files behind when
later tests do not require them. Where a persistence test needs shared state,
give that sequence explicit setup/teardown helpers rather than relying on a
near-empty disk or registry position.

Do not solve the issue by reordering `MODULES` or spawning a separate QEMU per
test. Either would hide shared-state bugs and make the full-suite signal weaker.

### Model stack hole filling explicitly

Replace `test_try_grow_user_stack_not_stack_grow_above_bottom` with two clear
contracts:

1. `test_try_grow_user_stack_fills_gap_above_bottom` maps the synthetic initial
   stack, unmaps one interior page, faults that page, and asserts `Grew`. Because
   this is a hole rather than downward extension, `stack_bottom` and
   `stack_mapped_bottom` stay unchanged while the fault budget decrements.
2. A range-rejection test keeps the `addr >= stack_top` assertion and verifies
   an already-mapped in-window page returns `NotStackGrow` through the mapper's
   `PageAlreadyMapped` result.

Use explicit mapping and unmapping in the fixture so the result does not depend
on pages left by an earlier userland test.

### Unknown syscalls are recoverable in every mode

Change `unhandled_syscall` to return `ENOSYS` unconditionally. Preserve the
current bounded once-per-number trace table, but make its effect purely
observational:

- Trace mode: first occurrence logs number plus arguments at info; repeats log
  at trace.
- Normal mode: emit a concise warning (or first-occurrence warning if the same
  table is shared) and return `ENOSYS`.

Delete `lifecycle::unimplemented_syscall_exit` and
`ExitKind::UnimplementedSyscall` if they have no remaining callers; otherwise
mark them as an explicit fatal-policy hook rather than dispatcher behavior.
Update comments in `abi.rs`, `launcher.rs`, and userland documentation so no
text says trace mode is required to survive unsupported calls.

Update the hand-built live fixture so it issues syscall 999, verifies RAX is
`-38`, and then calls `exit_group(0)`. Rename the fixture/test around observable
behavior (for example, `unknown_syscall_enosys_elf` and
`test_run_unknown_syscall_returns_enosys`) instead of the old termination path.
Keep dispatcher-level tests for trace bookkeeping and for numbers above the
tracking capacity.

### Commit tiny compiler fixtures; never build them in the default test path

Add a dedicated source tree such as
`userland/apps/compiler-compat/` with a Makefile that uses
`x86_64-linux-musl-gcc` and enforces:

- `-static -fno-pie -no-pie`
- ELF64 x86-64 `ET_EXEC`
- no `PT_INTERP`
- stripped, size-bounded output

Commit the resulting test artifacts under a clearly test-only directory such
as `userland/prebuilt/compiler-compat/`. `test.sh` copies those known binaries
atomically into `host_share/` on every run, including `--skip-userland`, because
they are inputs to mandatory kernel tests rather than optional applications.
The default test path must not probe for or invoke the musl compiler. Provide an
explicit refresh target/script for maintainers and document the toolchain used
when refreshing binaries.

Use uppercase 8.3 staged names to keep the host FAT transport uninteresting.
Start with three self-checking fixtures whose exit codes identify the failing
tier:

1. **CRT fixture** (`CCCRT.ELF`): ordinary C `main`, argc sanity, return 0.
   Covers compiler output, musl CRT entry, initial stack consumption, TLS init,
   and `exit_group`.
2. **libc fixture** (`CCLIBC.ELF`): checks argv/envp, `errno`,
   malloc/realloc/free, stack locals, string/formatting, and a small set of
   supported time/random/identity calls. It exits nonzero at the first failed
   invariant rather than requiring serial-output matching.
3. **probe/tool fixture** (`CCPROBE.ELF`): performs an intentionally unknown
   syscall through musl's `syscall()` wrapper, requires `-1` with
   `errno == ENOSYS`, then continues into representative file metadata/read and
   process-compatible work. This pins the normal-mode fallback behavior that
   libc and build tools depend on.

Keep fixtures single-process unless a fixture explicitly targets fork/wait;
the initial suite should identify ABI regressions without conflating them with
unrelated scheduler coverage.

### Run fixtures as booted integration tests

Add `src/tests/compiler_compat.rs` and register `compiler_compat` in
`src/tests/mod.rs`. Each test must:

- Require its staged file to exist; missing committed/staged input is a failure,
  not a skip.
- Launch through `userland::launcher::launch_user_binary` with explicit argv
  and a small deterministic envp.
- Run with normal unknown-syscall policy, not trace mode.
- Assert `ExitKind::Cooperative` and exit code 0, with a message naming the
  fixture and returned exit kind/code.
- Leave no active user process, VA bounds, trace flag, or terminal stream state
  for the next test.

This keeps the ladder filterable while exercising the production path:

`vvfat /host -> File/VFS -> ELF loader -> musl CRT -> ring 3 -> syscalls -> exit`.

## Implementation sequence

### Phase 0 — repair the current signal

1. Refactor the higher-level `fat_write` tests onto the mounted `/data`
   instance and add deterministic cleanup.
2. Add cleanup/setup boundaries to the preceding `filesystem` mutations.
3. Replace the stale above-bottom stack assertion with explicit gap-fill and
   mapped-page tests.
4. Run the focused reproducer and the complete existing suite before adding
   compatibility fixtures. Any remaining failure is fixed or recorded before
   proceeding; the new module must not bury a red baseline.

### Phase 1 — normalize unsupported-syscall behavior

1. Make the dispatcher return `ENOSYS` for live and synthetic callers in both
   trace modes.
2. Retire the unimplemented-syscall process-exit path if unused.
3. Rewrite dispatcher and live-ring-3 tests to assert continuation after
   `ENOSYS`.
4. Update comments and docs describing trace mode.

### Phase 2 — add hermetic static-musl artifacts

1. Add the three C sources, shared self-check helpers if useful, and a strict
   Makefile/readelf validation target.
2. Build and inspect the artifacts once with the cross toolchain; record their
   hashes, sizes, ELF type, and refresh command in a fixture README.
3. Commit the binaries and stage them unconditionally from `test.sh`.
4. Ensure `--skip-userland` still stages only these mandatory test fixtures
   while skipping optional application builds.

### Phase 3 — add the booted `compiler_compat` module

1. Add a small launch/assert helper and one test per tier.
2. Register the module and update `src/tests/CLAUDE.md` plus userland fixture
   documentation.
3. Confirm the probe fixture runs with trace mode off and exits cooperatively
   after observing `ENOSYS`.

### Phase 4 — final verification

Run, in order:

```sh
cargo fmt --check
cargo check
./test.sh fat_write
./test.sh filesystem fat_write
./test.sh userland
./test.sh compiler_compat
./test.sh --skip-userland compiler_compat
./test.sh
```

The combined FAT command is a permanent regression check; the final unfiltered
command is the release gate.

## Acceptance criteria

- No test-only second `FatFilesystem` writes to the mounted `/data` disk.
- Mutable FAT/VFS tests pass alone and after preceding filesystem mutations.
- The stack suite proves gap filling and does not assert the retired
  above-`stack_bottom` rule.
- A live ring-3 syscall 999 returns `-ENOSYS`, executes subsequent user code,
  and exits cooperatively.
- Trace mode changes logging detail only.
- All compiler-compat inputs are present in a fresh clone and are not skipped
  based on host tools.
- Each compatibility tier can be filtered independently and the whole
  `compiler_compat` module passes in a booted kernel.
- `./test.sh --skip-userland compiler_compat` and the complete `./test.sh`
  both pass.

## Risks and mitigations

- **Public VFS tests may not expose a low-level FAT detail.** Prefer observable
  behavior; add a narrow test-only accessor to the mounted wrapper only when an
  invariant cannot be checked through public operations.
- **Cleanup can mask the directory-extension case that exposed the bug.** Keep
  a focused test that deliberately fills enough slots to cross a FAT32 root
  directory cluster boundary, but do so through one mount and clean up after.
- **Committed binaries can drift from source.** Validate ELF headers and hashes
  during refresh, keep artifacts tiny, and require source plus binary changes
  in the same review when a fixture changes.
- **A fixture may accidentally demand an unsupported subsystem.** Give each
  tier narrowly documented calls and distinct failure codes; grow the top tier
  deliberately rather than turning it into a second BusyBox test.
- **Removing the fatal exit kind affects diagnostics.** Preserve the syscall
  number in normal-mode warning logs and in trace-mode first-occurrence logs;
  compatibility should not depend on killing the caller for observability.
