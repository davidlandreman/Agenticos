---
title: "feat: port GCC 14.2.0 as a static-musl native C compiler"
type: feat
status: in-progress
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-07-18-003-feat-tinycc-port-plan.md
  - docs/plans/2026-07-18-008-feat-binutils-userspace-apps-plan.md
  - docs/plans/2026-05-16-001-feat-prebuilt-userland-elfs-plan.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - userland/prebuilt/README.md
  - userland/apps/tcc/README.md
  - userland/apps/binutils/README.md
---

# feat: port GCC 14.2.0 as a static-musl native C compiler

## Summary

Ship GCC as an on-target native C compiler: a static-musl `gcc` driver plus
`cc1`, `collect2`, `cpp`, `libgcc.a`, the CRT begin/end objects, and GCC's
internal header tree, cross-built on the macOS host and staged read-only
under `/host/gcc`. From zsh:

```sh
cd /work
gcc -O2 -o hello /host/sysroot/examples/hello.c
./hello
```

This is the follow-up the TinyCC plan
(`docs/plans/2026-07-18-003-feat-tinycc-port-plan.md`) was explicitly the
stepping stone for, and the binutils plan
(`docs/plans/2026-07-18-008-feat-binutils-userspace-apps-plan.md`) called
itself "the intended binary-utilities foundation for the future GCC port."
Both dependencies are landed. The compiler pipeline GCC needs — write
semantics, `/work`, sysroot, `as`, `ld`, fork/execve of fresh multi-MiB
ELFs — is proven; what remains is GCC's own build, the multi-process driver
pipeline, and three small kernel-readiness items.

## Prerequisites already landed (verified in-tree)

- **Process pipeline**: `fork` (`src/userland/syscalls.rs:1374`), `vfork`
  (`:1556`, aliased to fork — safe because our fork copies memory), `execve`
  (`:1689`), `pipe`/`pipe2` (`:2319`), `wait4`. The TCC implementation notes
  record fork/execve of arbitrary fresh ELFs, multi-MiB static binaries,
  argv/envp, and exit-code propagation as known-good.
- **Assembler and linker**: GNU binutils 2.46.0 `as` and `ld` at their
  `/bin` names, configured with `--with-sysroot=/host/sysroot
  --with-lib-path=/host/sysroot/lib`. The `as → ld → run` workflow is
  covered by the booted binutils acceptance suite.
- **Sysroot**: musl headers, `libc.a`, and CRT objects staged at
  `/host/sysroot` (shared by TCC and binutils `ld`).
- **Filesystem semantics**: arbitrary-length `write`, seek-past-EOF
  zero-fill, `chmod`/`fchmod`, `umask`, `utimensat`, `readv`, accurate
  `fcntl(F_GETFL)`; writable `/work` provisioned every boot.
- **Toolchain parity**: the Homebrew `x86_64-linux-musl-gcc` cross compiler
  is **GCC 14.2.0**, which fixes the version choice — building native GCC
  14.2.0 with cross GCC 14.2.0 eliminates cross-version libgcc/configure
  skew in the Canadian cross.

## Watch list inherited from the TCC plan, resolved here

The TCC plan left a written watch list for GCC. Disposition of each:

| Item | Disposition |
|---|---|
| Multi-process driver pipeline (`cc1`/`as`/`ld` spawning) | Core of this plan; exercised by U4's booted suite, including the `gcc → collect2 → ld` two-level spawn chain |
| Temp files (no `/tmp`; TCC pointed `TMPDIR` at `/work`) | U1 provisions `/tmp` on the overlay at every boot, like `/work`. GCC's `choose_tmpdir` then works with no env conventions |
| 32-entry fd table | U1 bumps `FD_TABLE_SIZE` (`src/userland/fdtable.rs:24`) 32 → 64 as cheap insurance for driver+`collect2`+`ld` fd inheritance chains |
| `brk` capped at 32 MiB | No change. musl mallocng falls back to `mmap` when `brk` fails; Links/TCC already exercise that path. `cc1` is memory-hungry but mmap-backed |
| Pipe short-writes under `-pipe` | Temp-file compilation is GCC's default; U4 adds one `-pipe` smoke test, and if it exposes a pipe bug we fix it, but `-pipe` is not a completion criterion |
| FAT PIO header-read latency at GCC's header count | Accepted for v1: GCC reads fewer sysroot headers than the worst case feared (musl headers are consolidated), and compile latency is tolerable. Measure in U4; a block cache remains future work |

One deferred-debt item from `CLAUDE.md` becomes load-bearing and is fixed in
U1: **`wait4` never reports `WIFSIGNALED`**. The GCC driver uses
`WIFSIGNALED` to distinguish an internal compiler error (crashed `cc1`) from
an ordinary failure; with the current encoding a crashed `cc1` looks like
`exit 139` and the driver prints a misleading diagnostic. The fix is the one
already sketched in `CLAUDE.md`: extend `ZombieRecord` with a
died-via-signal flag and have `wait4_handler` emit `signum & 0x7f` for
signaled children.

## Goals

1. `GCC.ELF` driver reachable as `/bin/gcc` through the existing direct-app
   namespace (`src/userland/bin_namespace.rs`); `cc1`, `collect2`, and
   `cpp` found through the configured prefix under `/host/gcc` with no
   environment-variable conventions.
2. One-command compile-and-run and separate `-c`/link workflows succeed from
   zsh against `/host/sysroot`, writing to `/work`.
3. `/tmp` exists at every boot; `wait4` reports signaled children with POSIX
   encoding; the fd table holds 64 entries. Each has focused kernel tests.
4. Committed prebuilt artifacts (fresh clones need no musl toolchain),
   `REBUILD_GCC=1` / `--rebuild-userland` integration, and
   `./userland/refresh-prebuilt.sh` reproducibility, matching the binutils
   pattern.
5. A booted `gcc` acceptance module in `./test.sh` with unknown-syscall
   tracing, matching the binutils acceptance style.

## Non-goals

- **C++** (`g++`, `cc1plus`, libstdc++). Doubles artifact size and staging
  surface; a follow-up once C is proven. `--enable-languages=c` only.
- **Self-hosting** (GCC compiling GCC on-target) and a GNU make port — the
  natural next milestones after this plan, not part of it.
- LTO (`--disable-lto`, no `lto-wrapper`), plugins, sanitizers, gcov-based
  profiling workflows, bootstrap comparison.
- Dynamic linking, shared libgcc, `PT_INTERP` — static-only, same bar as
  binutils (`make validate` rejects `PT_INTERP`/`DT_NEEDED`).
- Retargeting `/bin/cc`: it stays aliased to TCC. `gcc` is the only new
  command name; revisiting the `cc` default is a one-line follow-up.
- Fortran/Ada/Go/D, multilib (`--disable-multilib`), NLS.

## Design

### Build recipe (Canadian cross)

`userland/apps/gcc/Makefile`, modeled directly on
`userland/apps/binutils/Makefile`:

- Fetch `gcc-14.2.0.tar.xz` from GNU, SHA256-pinned. Fetch GMP/MPFR/MPC at
  the versions `contrib/download_prerequisites` pins for 14.2.0,
  SHA256-pinned ourselves, unpacked in-tree so configure builds them
  automatically for the *host* (x86_64-linux-musl) — no separate library
  cross-builds.
- Configure out-of-tree:

  ```
  CC=x86_64-linux-musl-gcc CXX=x86_64-linux-musl-g++ \
  CFLAGS="-Os" CXXFLAGS="-Os" LDFLAGS="-static -no-pie" \
  ../gcc-14.2.0/configure \
    --build=<macOS triple> --host=x86_64-linux-musl \
    --target=x86_64-linux-musl \
    --prefix=/host/gcc \
    --with-sysroot=/host/sysroot \
    --with-as=/bin/as --with-ld=/bin/ld \
    --enable-languages=c \
    --disable-shared --disable-multilib --disable-nls \
    --disable-lto --disable-plugin --disable-bootstrap \
    --disable-libsanitizer --disable-libquadmath --disable-libssp \
    --disable-libgomp --disable-libatomic --disable-libitm \
    --disable-libvtv --disable-fixincludes \
    --with-system-zlib=no
  ```

  Key decisions:
  - **`--prefix=/host/gcc`** bakes the on-target install location into the
    driver, so `gcc` finds `cc1`/`collect2` in
    `/host/gcc/libexec/gcc/x86_64-linux-musl/14.2.0/` and its internal
    headers/`libgcc.a`/CRT objects in
    `/host/gcc/lib/gcc/x86_64-linux-musl/14.2.0/` with no `-B`,
    `GCC_EXEC_PREFIX`, or PATH dependence. Read-only vvfat is fine — a
    compiler install tree is never written at runtime.
  - **`--with-as=/bin/as --with-ld=/bin/ld`** hardcodes the shipped
    binutils, removing PATH-search behavior from the pipeline.
  - **`--with-sysroot=/host/sysroot`** matches binutils `ld` and TCC, so
    all three compilers share one libc view.
  - **`--disable-fixincludes`** (supported in GCC 14): musl headers need no
    fixing, and fixincludes output is host-machine-dependent noise we don't
    want in a committed artifact.
  - `make` needs only the standard Canadian-cross inputs, all present: the
    build→host compiler (`x86_64-linux-musl-gcc`) for the programs, the
    same compiler as build→target for `libgcc`/CRT, and host-prefixed
    binutils from the cross toolchain at configure time.
- Post-build: `strip` all shipped executables with the cross `strip`;
  `make validate` re-uses the binutils readelf checks (ELF64 x86-64
  `ET_EXEC`, no `PT_INTERP`, no `DT_NEEDED`, entry point present) over
  `gcc`, `cc1`, `collect2`, and `cpp`.

### Shipped artifact set and staging

`make install DESTDIR=` into a staging dir, then prune to:

```
/host/gcc/bin/gcc                                        (~1.5 MiB)
/host/gcc/libexec/gcc/x86_64-linux-musl/14.2.0/cc1       (~30–40 MiB)
/host/gcc/libexec/gcc/x86_64-linux-musl/14.2.0/collect2  (~1.5 MiB)
/host/gcc/bin/cpp                                        (~1.5 MiB)
/host/gcc/lib/gcc/x86_64-linux-musl/14.2.0/libgcc.a
/host/gcc/lib/gcc/x86_64-linux-musl/14.2.0/crt{begin,end}.o
/host/gcc/lib/gcc/x86_64-linux-musl/14.2.0/include/       (stddef.h, stdarg.h, …)
```

Everything else from `make install` (locale data, man/info, `gcov`,
`lto-wrapper` won't exist, duplicate `x86_64-linux-musl-gcc` hardlink) is
pruned. The tree ships as one committed
`userland/prebuilt/gcc-install.tar.gz` (the sysroot-tarball pattern, not
fourteen flat `.ELF`s — the tree's internal layout is load-bearing for the
driver's relative lookups). `stage-lib.sh` gains `stage_gcc` that extracts
it into `host_share/gcc/`, alongside the existing `stage_tcc_sysroot`.

**Path shape risk to retire first** (U3's opening task): the install tree
contains long, dotted, deeply nested names (`x86_64-linux-musl`, `14.2.0`,
`include-fixed` won't exist but `crtbegin.o` etc. will). The sysroot tree
already proves vvfat LFN *reads* work; U3 verifies the specific
five-component depth with a booted `stat`/`open` probe before anything else
depends on it. Fallback if a FAT-layer limit surfaces: patch the version
directory to `14` at install-prune time (GCC compares the string it was
built with, so pruning must rewrite nothing in the binaries — instead we'd
configure `--with-gcc-major-version-only`, which GCC supports precisely for
this).

### `/bin` namespace

`bin_namespace.rs` gains one direct entry: `"gcc" => "/host/gcc/bin/gcc"`.
No BusyBox collision (BusyBox has no `gcc` applet). `cpp` as a `/bin` name
is deferred — BusyBox has no `cpp` either, but nothing needs it yet.

### Kernel readiness (U1)

1. **`/tmp` provisioning** — same mechanism that provisions `/work` and
   `/root` on the overlay at boot (`src/userland/lifecycle.rs` /
   `src/kernel.rs` boot provisioning). Overlay-backed, so it is naturally
   RAM-first and survives `sync` only incidentally; that is correct for
   temp files.
2. **`wait4` signal encoding** — `ZombieRecord` gains
   `died_via_signal: Option<u8>`; the fatal-signal default-action path
   (dispatcher tail) records it; `wait4_handler` emits `signum & 0x7f`
   (WIFSIGNALED) vs `(code & 0xff) << 8` (WIFEXITED). Removes the
   `CLAUDE.md` deferred-debt item 3; zsh starts printing
   "segmentation fault" for crashed children, which is itself a visible
   correctness win.
3. **`FD_TABLE_SIZE` 32 → 64** (`src/userland/fdtable.rs:24`). Audit the
   constant's consumers (`epoll.rs:223` bounds `maxevents` by it; poll/
   select bitmap sizes) for anything that assumed 32.

Each lands with focused kernel tests (`src/tests/`): `/tmp` exists and is
writable at boot; a child killed by SIGSEGV yields `WIFSIGNALED &&
WTERMSIG==11` through `wait4`; fds up to 63 allocate and round-trip.

## Implementation units

### U1 — Kernel readiness
`/tmp` boot provisioning, `wait4` WIFSIGNALED encoding, fd table 64, with
tests. No dependency on the GCC build; land first so the booted suite in U4
runs against final semantics. Full `./test.sh` stays green.

### U2 — Pinned Canadian-cross build and validation
`userland/apps/gcc/Makefile` (fetch + SHA256 + configure + build + prune +
strip + validate), `userland/apps/gcc/README.md` recording the recipe and
GPLv3 source-correspondence URLs, examples under
`userland/apps/gcc/examples/` (hello, two-TU program, one
compile-flag-exercising fixture). Exit criterion: `make -C
userland/apps/gcc validate` passes on a clean checkout with the Homebrew
musl toolchain.

### U3 — Staging, namespace, and path-shape verification
`stage_gcc` in `stage-lib.sh`, `gcc-install.tar.gz` prebuilt wiring
(`REBUILD_GCC=1`, `--rebuild-userland`, `refresh-prebuilt.sh`),
`bin_namespace.rs` entry, and the booted deep-path LFN probe (first task —
it gates the staging layout decision).

### U4 — Booted end-to-end acceptance suite
A `gcc` test module in `src/tests/` (binutils-suite style, unknown-syscall
tracing enabled):
1. `gcc -o hello hello.c && ./hello` in `/work` (driver → cc1 → as →
   collect2 → ld chain, `/tmp` temp files).
2. Separate compilation: `gcc -c a.c`, `gcc -c b.c`, `gcc -o prog a.o b.o`,
   run, check output.
3. Staged pipeline interop: `gcc -S`, then shipped `as` and `ld` manually —
   proves the binutils layer and GCC agree on syntax and sysroot.
4. `-O2` build of a fixture with real computation (exercises cc1 memory
   behavior under the brk-cap/mmap fallback).
5. Failure modes: missing header → nonzero exit + diagnostic on stderr;
   a deliberately crashed child (fixture raising SIGSEGV) → zsh/driver
   reports the signal (U1's wait4 fix observable end-to-end).
6. `-pipe` smoke (non-gating; documents pipe behavior either way).
Record compile latency numbers in the implementation notes for the FAT PIO
watch item.

### U5 — Documentation and regression
`CLAUDE.md` (current-state paragraph + prebuilt list), `userland/prebuilt/
README.md`, `src/userland/CLAUDE.md` (bin_namespace, fd table, wait4),
remove the resolved deferred-debt item, full `./test.sh` and
`./build.sh` interactive sanity pass.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Canadian-cross configure quirks on macOS (case-insensitive FS, host tool detection) | The binutils port already succeeded from the same host with the same toolchain; GCC is bigger but the same shape. If case-insensitivity bites, build inside a case-sensitive sparse bundle (document in Makefile). |
| `cc1` static size (~30–40 MiB) → repo growth and FAT load latency | One-time ~2.5× binutils growth, consistent with committed-prebuilt policy; `-Os`, `--strip-all`, single tarball. Latency measured in U4; block cache is explicitly future work. |
| In-tree GMP/MPFR/MPC static host builds misdetect | These are the most-trodden paths in GCC's build system (musl-cross-make's NATIVE target does exactly this build shape); pin exact prerequisite versions. |
| vvfat/FAT driver limit on deep long-named paths | U3 probes first; `--with-gcc-major-version-only` fallback flattens the worst component. |
| `cc1` peak memory vs 2 GiB guest | musl malloc mmap fallback is proven; test fixtures sized for real but bounded compiles. `AGENTICOS_QEMU_MEMORY` exists if a fixture needs headroom. |
| Driver spawn-chain depth (`gcc` → `collect2` → `ld`) hits an untested process-tree edge | Primitives are known-good; U4 test 1 exercises the chain directly, and unknown-syscall tracing catches silent ENOSYS fallbacks. |

## Implementation status (2026-07-18)

**Done and working:**
- U1 kernel readiness: `/tmp` provisioned at boot (new) and fd table
  32 → 64 (new), each with a passing kernel test (`filesystem`,
  `userland`). The `wait4` WIFSIGNALED encoding turned out to already be
  implemented (`ZombieRecord::signal_termination` + `wait4_handler`); the
  CLAUDE.md deferred-debt item #3 was stale and can be struck.
- U2: GCC 14.2.0 Canadian cross builds cleanly; `gcc-install.tar.gz`
  (~13.7 MiB) validated static/no-PT_INTERP.
- U3: staging (`stage_gcc_install`/`stage_gcc_fixtures`), `REBUILD_GCC`,
  refresh integration, `/bin/gcc` namespace entry — all working; the deep
  five-component LFN prefix stages and reads correctly through vvfat.
- The driver runs on-target, self-relocates from its argv[0] staged path,
  finds cc1/collect2/`/bin/as`/`/bin/ld`, and drives the pipeline: it forks,
  the child execs `cc1` with correct args, temp files land in `/tmp`.

**Kernel bugs found and fixed while bringing GCC up (each independently
valuable — any large binary would eventually hit them):**
1. `mmap(MAP_FIXED)` returned `ENOSYS`; now implemented (evict + reinsert +
   unmap stale leaves). musl mallocng needs it.
2. musl `posix_spawn`'s `clone(CLONE_VM|CLONE_VFORK|SIGCHLD, stack)` profile
   was rejected; now routed to a COW fork with a caller-supplied child RSP
   (same substitution the existing `vfork` uses). GCC's driver, `system()`,
   and `popen()` all spawn through this.
3. Demand-paging a large file off `/host` (vvfat) was **O(n²)**: every 4 KiB
   page fault re-walked the FAT cluster chain from cluster 0. A 27 MiB
   binary stalled for minutes. Fixed with a single-entry resume-hint cache
   in `FatFilesystem` (`read_file_at`), making sequential page-in O(n).
   Invalidated on any write.
4. execve now honors `FD_CLOEXEC` (`FdTable::close_on_exec`) — required so
   musl posix_spawn's CLOEXEC status pipe signals a successful exec.

**Known gap (blocks U4/completion): `cc1` hangs early in userspace.**
When `cc1` (GCC's 27 MiB C front/back end) runs as a fork+execve child, it
issues only its first few syscalls (`arch_prctl`, …) then spins in userspace
with no further syscalls and no termination — a resolvable-fault or
busy-loop, not an unhandled fault (which would SIGSEGV-kill it). It reaches
further when launched directly via `launch_user_binary` than as a spawned
child, which points at the fork/spawn/execve-of-a-large-binary path rather
than cc1 itself. Not yet root-caused. The `src/tests/gcc.rs` module and its
staged fixtures are complete and kept in-tree but unregistered from the test
`MODULES` list until this is resolved, so `./test.sh` stays green.

Next debugging steps to try: compare the COW-forked child's initial
`UserState`/page tables against a direct launch; verify the spawn-clone
child RSP and the post-execve entry frame for a 27 MiB image; check whether
`close_on_exec` closes a descriptor cc1 still needs; bisect by having the
driver `fork`+`execve` a *small* binary vs cc1.

## Completion criteria

- From zsh: `cd /work && gcc -o hello /host/sysroot/examples/hello.c &&
  ./hello` prints the expected output; separate `-c` + link also works.
- `gcc` resolves via `/bin/gcc`; `cc1`/`collect2` are found through
  `/host/gcc` with no environment setup.
- `/tmp` exists at boot; crashed children report `WIFSIGNALED` through
  `wait4`; fd table holds 64 entries — each with passing kernel tests.
- Booted `gcc` acceptance module passes under `./test.sh` with no unknown
  syscalls in accepted workflows; full suite stays green.
- `./userland/refresh-prebuilt.sh` regenerates `gcc-install.tar.gz`
  reproducibly; fresh clones boot and compile without a musl toolchain.
- Live documentation updated; the `wait4` deferred-debt item removed from
  `CLAUDE.md`.
