---
title: "feat: port TinyCC as a static-musl ring-3 compiler with a /host sysroot"
type: feat
status: completed
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-05-16-001-feat-prebuilt-userland-elfs-plan.md
  - docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md
  - docs/plans/2026-05-16-005-feat-filesystem-write-and-long-names-plan.md
  - docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md
  - userland/prebuilt/README.md
  - src/userland/CLAUDE.md
  - src/fs/CLAUDE.md
---

# feat: port TinyCC as a static-musl ring-3 compiler with a /host sysroot

## Summary

Stage a statically linked, musl-built TinyCC (`TCC.ELF`) plus a minimal musl
sysroot under `/host`, provision a writable `/work` directory on the root
overlay tmpfs, and close the small set of kernel syscall gaps that a
compile-and-link workload exercises. End state: from zsh,

```sh
cd /work
tcc -o hello /host/sysroot/examples/hello.c
./hello
```

compiles against the staged musl headers, links against the staged
`crt1.o`/`crti.o`/`crtn.o`/`libc.a`/`libtcc1.a`, produces a static `ET_EXEC`
binary on tmpfs, and executes it through the existing fork/execve path.

TinyCC is deliberately chosen over GCC as the first compiler port: it is a
complete C compiler + assembler + linker in one ~400 KiB static binary with no
subprocess spawning, no temp-file pipeline, and a small memory footprint. It
will surface the next real kernel blockers (large writes, seek semantics,
permission syscalls) without first requiring GCC's build machinery and
multi-process driver model.

---

## Current state and motivation

Research findings this plan is built on (verified against the tree at head):

**Userland platform (ready — no changes needed to the pattern):**

- `userland/apps.manifest.sh` is the single source of truth; a
  `prebuilt-managed` `app_row` gives us fetch-from-tarball builds via the
  Homebrew `x86_64-linux-musl-gcc` toolchain, with a committed stripped binary
  in `userland/prebuilt/` so fresh clones need neither network nor toolchain.
  `./userland/refresh-prebuilt.sh` regenerates committed artifacts.
- `validate_exec_elf` requires static, non-PIE `ET_EXEC` with no `PT_INTERP`
  — exactly what `-static -no-pie -fno-pie` musl builds produce, and exactly
  what the loader (`src/userland/loader.rs`) accepts.
- `/host` is the repo's `host_share/` directory exposed as read-only vvfat.
  Arbitrary nested trees with long, mixed-case filenames work: the FAT driver
  decodes VFAT LFN and matches names case-insensitively, so a sysroot tree
  containing `sys/socket.h`, `crt1.o`, `libc.a` stages with no image tooling.
- The Homebrew `musl-cross` toolchain on the dev host already contains
  everything the sysroot needs: full musl + Linux-uapi headers (22 MiB),
  `crt1.o`, `crti.o`, `crtn.o`, and a 2.6 MiB `libc.a`.

**Process/exec layer (ready):**

- zsh spawns children via real `fork()` + `execve()`
  (`src/userland/syscalls.rs:1049`, `:1245`). `execve` opens the literal path
  after cwd normalization, so a freshly written ELF on the overlay (e.g.
  `/work/hello`) is executable by path with no registration anywhere.
- Multi-MiB static binaries load (5.79 MiB proven); PT_LOAD is demand-paged.
- Memory model fits a compiler: anonymous mmap up to 512 MiB per call,
  demand-paged; brk capped at 32 MiB with musl falling back to mmap.

**Kernel gaps a compiler workload hits (the real work in this plan):**

1. **`write`/`pwrite64` with `len > 4096` return `EFAULT`**
   (`src/userland/syscalls.rs:112-114`, `:2624`; `WRITE_MAX_LEN`). musl's
   `fwrite` bypasses its 1 KiB stdio buffer for large payloads and issues one
   direct `write()`/`writev()` of the full remaining length, so TCC writing a
   multi-KiB object or executable fails on the first output flush. This is a
   hard blocker.
2. **`lseek` past EOF is rejected** (`SeekOutOfBounds`,
   `src/fs/file_handle.rs:289-295`) even on writable filesystems.
   Linker-style writers commonly seek forward and back-patch headers; musl
   stdio `fseek` on an update stream can also land past EOF. Probable blocker,
   cheap to fix correctly.
3. **`chmod`(90)/`fchmod`(91) are ENOSYS.** TCC creates executable output with
   an explicit mode and may `fchmod` it. FAT/tmpfs have no permission bits and
   `execve` does not check them, so success no-ops are correct here.
4. **No writable working directory by default.** A ring-3 process starts with
   cwd `/host` (read-only, `src/userland/lifecycle.rs:1156`). `/` (overlay
   tmpfs) is writable with no size cap on newly created files, and
   `mkdir`(83) works on it — but nothing provisions a conventional scratch
   directory. `/data` is unsuitable (no subdirectories, single-level only).
5. **W^X rejects `PROT_WRITE|PROT_EXEC` mmap** (`syscalls.rs:464-466`), which
   blocks `tcc -run`'s single RWX JIT mapping. RW→`mprotect`(R|X) is allowed,
   so `-run` is achievable with a small TCC patch — kept a stretch goal, not
   a blocker for compile-to-file.

Non-blockers confirmed: 32-fd table (TCC opens files mostly sequentially),
`openat` restricted to `AT_FDCWD` (musl uses `AT_FDCWD` for path opens),
`MAP_FIXED`/`clone`/`statx` ENOSYS (unused by static musl TCC's hot paths),
overlay copy-up 64 KiB cap (applies only to modifying pre-existing boot-FAT
files, not to new files in `/work`).

## Goals

1. `TCC.ELF` builds from a SHA256-pinned upstream TinyCC source via the
   existing prebuilt-managed pipeline, is committed under `userland/prebuilt/`,
   and stages to `/host/TCC.ELF` on every build.
2. A minimal musl sysroot (pruned headers, crt objects, `libc.a`, TCC's own
   headers and `libtcc1.a`, a couple of example sources) is committed as a
   single reproducible tarball and staged to `host_share/sysroot/` →
   `/host/sysroot/`.
3. TCC's compiled-in default search paths point at `/host/sysroot/...` and its
   default link mode is static, so `tcc -o out in.c` works with no flags, no
   environment variables, and no `-B`.
4. `/work` exists on the writable overlay at every boot; compile outputs there
   survive `sync(2)` like any other overlay write.
5. `write`/`pwrite64`/`writev` accept arbitrary lengths (kernel-side
   chunking); `lseek` beyond EOF works on writable files with zero-fill on
   subsequent write; `chmod`/`fchmod` succeed as no-ops. Each has focused
   in-kernel tests.
6. `/bin/tcc` (and alias `/bin/cc`) resolve through the existing direct-app
   namespace, and an automated in-QEMU test compiles and runs a program
   end-to-end.

## Non-goals

- **GCC.** This plan is the deliberate stepping stone; GCC's multi-process
  driver (`cc1`/`as`/`ld`), temp-file pipeline, and memory footprint are a
  follow-up informed by what TCC surfaces.
- **`tcc -run` / libtcc JIT** — stretch goal only (documented W^X patch
  sketch); compile-to-file is the acceptance bar.
- Dynamic linking, shared objects, `PT_INTERP` support, or staging
  `ld-musl-x86_64.so.1` as a working loader.
- C++ (no `g++`/`libstdc++` in the sysroot), C11 threads/atomics runtime
  (no `clone`), or debug-info consumers (no in-guest gdb).
- Writable `/host`, FAT LFN writing, or `/data` subdirectory support.
- A general `/tmp` tmpfs mount; `/work` is a directory on the existing root
  overlay, not a new mount.
- Optimizing FAT read performance (per-cluster chain walks make many-header
  compiles slow but functional; noted as follow-up).

---

## Design

### Runtime layout

```text
/host/TCC.ELF                       static musl TinyCC (prebuilt-managed)
/host/sysroot/
  include/...                       pruned musl + linux-uapi headers
  lib/
    crt1.o  crti.o  crtn.o          musl CRT objects (from musl-cross)
    libc.a                          static musl libc
    tcc/
      libtcc1.a                     TCC runtime (built by our Makefile)
      include/...                   TCC-private headers (stddef.h, stdarg.h, …)
  examples/
    hello.c  args.c                 smoke-test sources
/work                               writable dir on overlay tmpfs, created at boot
```

Compiled-in TCC defaults (set at configure time, no runtime flags needed):

```text
--tccdir         /host/sysroot/lib/tcc
--sysincludepaths {B}/include:/host/sysroot/include
--libpaths       {B}:/host/sysroot/lib
--crtprefix      /host/sysroot/lib
```

plus a small patch making `-static` the default (the loader rejects
`PT_INTERP`, so dynamic output would produce unexecutable binaries; an
explicit `-shared`/`-r` still behaves normally).

### Compile-and-run data path

```text
zsh: tcc -o hello hello.c        (cwd /work)
  └─ execve("/bin/tcc") → bin_namespace direct rewrite → /host/TCC.ELF
       ├─ open/read headers        /host/sysroot/include/**      (FAT ro, LFN)
       ├─ open/read crt + libc.a   /host/sysroot/lib/*           (FAT ro)
       └─ open(O_CREAT|O_WRONLY|O_TRUNC)/write/lseek  /work/hello  (overlay tmpfs)
zsh: ./hello
  └─ fork + execve("/work/hello") → static ET_EXEC loads via existing loader
```

### Kernel: unbounded write sizes (U1)

Replace the `len > WRITE_MAX_LEN → EFAULT` guards in `write_handler`,
`pwrite64_handler`, and the per-iov path of `writev` with a kernel-side loop:
copy-in and write in ≤4096-byte chunks, accumulating the byte count, stopping
at the first chunk error (return bytes-written-so-far if > 0, else the error).
This keeps the bounded kernel-buffer property while giving POSIX semantics.
`read`'s existing short-read behavior is already POSIX-legal and stays as-is.

### Kernel: seek-past-EOF with zero-fill (U2)

Relax `File::seek` (`src/fs/file_handle.rs`) to allow `position > size` on
files whose filesystem is writable; keep the rejection for read-only mounts
(a read at such a position returns 0/EOF anyway, but writable is the case
that matters). On write-at-offset beyond current size, tmpfs already
zero-fills the gap on resize; the FAT writer extends with explicit zero
clusters. Add the gap-fill test for both tmpfs and `/data` FAT.

### Kernel: chmod/fchmod no-ops and /work provisioning (U3)

- Add syscalls 90 (`chmod`) and 91 (`fchmod`) to the dispatch table: validate
  the path/fd exists, then return 0. No mode storage — FAT and tmpfs have no
  permission bits and `execve` performs no +x check.
- In `src/kernel.rs`, after the overlay mount and sync-state hydration,
  create `/work` idempotently (mkdir, ignore AlreadyExists). Hydration of a
  previously synced state that already contains `/work` must not fail boot.

### TCC build (U4)

`userland/apps/tcc/` follows the zsh/busybox anatomy: `Makefile`, `patches/`,
`README.md`, `.gitignore` (`build`).

- **Source pin:** a TinyCC `mob`-branch snapshot (needed for `--config-musl`
  and current x86-64 codegen), fetched as a commit-pinned tarball with a
  recorded SHA256 that the Makefile hard-fails on. Fallback if snapshot
  checksums prove unstable across mirrors: vendor the release 0.9.27 tarball
  plus musl patches. The exact commit and hash are chosen at implementation
  time and recorded in the Makefile and README.
- **Cross build of `tcc` itself:**
  `CC=x86_64-linux-musl-gcc`, `--config-musl`, `--cpu=x86_64`,
  `--targetos=Linux`, `--cross-prefix=x86_64-linux-musl-`, the path options
  above, `--extra-cflags="-O2 -fno-pie"`,
  `--extra-ldflags="-static -no-pie"`. Two known cross-build wrinkles, both
  handled in our Makefile rather than by forking upstream logic:
  - `c2str`/`conftest` helpers must run on the macOS build host → build them
    with host `cc` (Makefile variable override or a small patch).
  - `libtcc1.a` is normally compiled by the just-built `tcc`, which is a
    Linux binary → compile `lib/libtcc1.c`, `lib/alloca86_64.S`, and the
    other x86-64 runtime sources with `x86_64-linux-musl-gcc` and archive
    with `x86_64-linux-musl-ar`. If a runtime source uses a TCC-only asm
    construct, patch that file minimally (risk table).
- **Patches** (kept in `patches/`, applied after checksum-verified extract):
  default `static_link = 1`; anything needed from the two wrinkles above.
- **Sysroot assembly:** the Makefile builds `build/sysroot/` from
  (a) the musl-cross toolchain's `include/` and `lib/{crt1.o,crti.o,crtn.o,libc.a}`,
  pruned of `c++/` and other non-C trees, (b) TCC's `include/` and the built
  `libtcc1.a`, (c) in-repo `examples/*.c`. It is then packed into a
  normalized tarball (sorted entries, zeroed mtimes/owners where the host
  tar allows) so the committed artifact diffs meaningfully.
- **Committed artifacts:** `userland/prebuilt/TCC.ELF` (stripped) and
  `userland/prebuilt/tcc-sysroot.tar.gz`, both produced by
  `./userland/refresh-prebuilt.sh`.

### Staging and manifest (U4 continued)

- One `app_row` in `userland/apps.manifest.sh`:
  `app_row tcc apps/tcc make TCC.ELF prebuilt-managed musl-cc apps/tcc/build/tcc prebuilt/TCC.ELF`.
- A new small `stage-lib.sh` helper, `stage_tree`, that extracts a committed
  tarball into `$HOST_SHARE_STAGE/<subdir>` (idempotent: extract to tmp dir,
  atomic rename, skip when the staged tree's stamp matches the tarball hash).
  The manifest calls
  `stage_tree tcc-sysroot prebuilt/tcc-sysroot.tar.gz sysroot`.
  `refresh-prebuilt.sh` treats the sysroot like other prebuilt artifacts:
  force-rebuild via the tcc Makefile's sysroot target, repack, write back.
- No edits to `build.sh`/`test.sh`; the manifest remains the source of truth.

### Namespace and tests (U5)

- `src/userland/bin_namespace.rs`: add `tcc` and `cc` to the direct-app list,
  both rewriting to `/host/TCC.ELF` (argv[0] preserved, so `cc` behaves as
  `cc`). Lists stay sorted and disjoint from BusyBox applets.
- Automated end-to-end test following the `NETTEST.ELF` fixture pattern: a
  kernel test spawns `/host/TCC.ELF` with argv
  `["tcc", "-o", "/work/hello", "/host/sysroot/examples/hello.c"]`, waits,
  asserts exit 0, then spawns `/work/hello`, asserts its exit code and
  captured output. This exercises U1–U4 in one path and runs under
  `./test.sh`.

### Stretch: `tcc -run` under W^X

Documented in `userland/apps/tcc/README.md`, implemented only if the main
milestones land early: patch TCC's runtime-memory path to mmap RW, write the
generated code, then `mprotect` to R|X (the kernel already permits the RW→RX
transition). Not part of done criteria.

---

## Implementation units

### U1. Kernel: arbitrary-length write/pwrite/writev

Kernel-side ≤4096-byte chunk loop replacing the `EFAULT` guards; partial
progress returned on mid-stream errors.

Verification:

- new in-kernel tests: a 1 MiB single `write()` to a tmpfs file round-trips
  byte-exact; `pwrite64` > 4 KiB; `writev` with a > 4 KiB iov; a large write
  to an `EROFS` fd still fails cleanly.
- `cargo check`, `cargo fmt --check`, `./test.sh <new module> fdtable` (plus
  existing write-path modules).
- Manual: in zsh, `busybox dd if=/dev/zero bs=65536 count=4 > /work/blob`
  equivalent via existing applets, then `wc -c /work/blob`.

### U2. Kernel: seek-past-EOF and zero-fill

`File::seek` allows past-EOF positions on writable filesystems; write-at-gap
zero-fills on tmpfs and FAT; read-at-gap returns EOF-consistent results.

Verification:

- new tests: seek to size+N then write on tmpfs → file length and zero gap
  verified; same on `/data` FAT; seek past EOF on `/host` (read-only) still
  rejected; regression run of existing seek/overlay/fat modules
  (`./test.sh '*seek*' '*overlay*' '*fat*'` or the listed module names).

### U3. Kernel: chmod/fchmod no-ops and /work at boot

Dispatch entries for 90/91 with existence validation; idempotent `/work`
creation after overlay hydration.

Verification:

- tests: `chmod` on an existing file returns 0, on a missing path returns
  ENOENT; `/work` exists and is writable after boot; boot-with-hydrated-state
  (existing sync tests) still passes.
- Manual QEMU: `cd /work && echo hi > f && cat f`; `sync`; reboot; confirm
  `/work/f` survives.

### U4. TCC app, sysroot, prebuilt, staging

The `userland/apps/tcc/` Makefile (fetch, verify, patch, cross-build tcc,
build libtcc1.a, assemble + pack sysroot), `stage_tree` in `stage-lib.sh`,
manifest row + sysroot call, committed `TCC.ELF` and `tcc-sysroot.tar.gz`,
`userland/prebuilt/README.md` rows.

Verification:

- `./userland/refresh-prebuilt.sh` succeeds and is stable on a second run
  (no artifact churn beyond expected).
- `readelf -h` on `build/tcc`: static `ET_EXEC`, x86-64, no `INTERP`
  (`validate_exec_elf` also enforces this at stage time).
- `./build.sh -n` stages `host_share/TCC.ELF` and `host_share/sysroot/` from
  the committed artifacts with the musl toolchain absent from PATH
  (fresh-clone simulation).
- Sanity cross-check on the host: run the staged-identical compile under a
  Linux container or via `qemu-x86_64` if available — optional, the in-guest
  test in U5 is the real gate.

### U5. Namespace, end-to-end test, smoke

`bin_namespace.rs` entries for `tcc`/`cc`, the fixture-pattern in-QEMU
compile-and-run test, examples staged in the sysroot.

Verification:

- `./test.sh bin_namespace <new tcc e2e module>`.
- Manual QEMU smoke matrix:
  1. `which tcc` resolves; `tcc -v` prints version.
  2. `cd /work && tcc -o hello /host/sysroot/examples/hello.c && ./hello`
     prints the expected line; `echo $?` is 0.
  3. `tcc -c` to `/work/hello.o` then `tcc -o hello2 hello.o` links and runs
     (separate compile+link exercises the .o reader/writer).
  4. A program using `printf`, `malloc`, `argv`, and `open`/`read` on
     `/host/sysroot/examples/args.c` behaves correctly.
  5. An intentional syntax error yields a TCC diagnostic and nonzero exit;
     zsh prompt returns normally.
  6. `cc -o hello …` works identically via the alias.
  7. `sync`, reboot, `/work/hello` still present and executable.

### U6. Documentation refresh

Update live docs: root `CLAUDE.md` (current-state paragraph + known-issues
deltas: write cap removed, seek semantics, `/work`), `src/userland/CLAUDE.md`,
`src/fs/CLAUDE.md` (seek/zero-fill), `userland/README.md`,
`userland/prebuilt/README.md`, `src/userland/bin_namespace.rs` module
comment, and `userland/apps/tcc/README.md` (build, pin, sysroot layout,
`-run` stretch sketch). Historical plans untouched.

Verification: `rg` for stale statements (`WRITE_MAX_LEN` semantics,
"no writable cwd", app inventory lists); `cargo fmt --check` both workspaces;
full `./test.sh`; `./build.sh -n`.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| TinyCC mob snapshot tarball checksum instability across mirrors | Pin one URL + SHA256, hard-fail on mismatch (existing convention); fallback documented: vendor release 0.9.27 + musl patch set. |
| Cross-build helpers (`c2str`/`conftest`) or `libtcc1.a` fight the macOS-host cross setup | Build helpers with host `cc`, build `libtcc1.a` objects directly with `x86_64-linux-musl-gcc`/`ar` in our Makefile; patch individual runtime sources only if a TCC-only asm construct appears. |
| TCC exercises an unanticipated ENOSYS (e.g. `umask`, real-dirfd `openat`, `getdents` on output dir) | The U5 in-QEMU test catches it early; syscall trace mode exists for diagnosis; add the same-shaped minimal handler as U1–U3 in a follow-up commit within this plan's scope. |
| Chunked writes change partial-failure semantics for existing callers | Return bytes-written-so-far on mid-stream errors (POSIX short write); keep first-chunk errors as errno; run full existing write-path test modules in U1. |
| Seek-past-EOF relaxation breaks an existing caller relying on the rejection | Gate the relaxation on filesystem writability; run overlay/FAT/seek regression modules; `/host` behavior unchanged. |
| Compile latency from PIO FAT header reads (no cluster-chain caching) | Accept for bring-up; prune the header set (drop `c++/`, non-Linux trees) to cut file count; note cluster-chain caching as an existing known FAT follow-up, out of scope here. |
| vvfat scalability with a few thousand sysroot files | Prune aggressively (musl proper + linux-uapi only, ≈ 20–25 MiB); verified at U4 by `ls -R`/stat sweep of `/host/sysroot` in the guest before the e2e test. |
| Overlay `sync` blob grows with `/work` build products | Existing double-buffered overlay-state path already handles multi-file uppers; document that `/work` is scratch and large artifacts belong on `/data` (single-level) until FAT subdir write support lands. |
| `tcc` default-static patch diverges from upstream flag handling | Patch only the default value of `static_link`; explicit `-shared`/`-r`/`-static` flags keep upstream behavior; e2e test covers default and `-c`+link modes. |

## Expected file changes

Add:

- `docs/plans/2026-07-18-003-feat-tinycc-port-plan.md` (this file)
- `userland/apps/tcc/Makefile`, `README.md`, `.gitignore`,
  `patches/*.patch`, `examples/hello.c`, `examples/args.c`
- `userland/prebuilt/TCC.ELF`, `userland/prebuilt/tcc-sysroot.tar.gz`
- new in-kernel test module(s) for large writes, seek/zero-fill, chmod,
  `/work`, and the TCC end-to-end fixture test

Modify:

- `src/userland/syscalls.rs` (write/pwrite/writev chunking; chmod/fchmod),
  `src/userland/abi.rs` (dispatch entries 90/91)
- `src/fs/file_handle.rs` (seek relaxation), `src/fs/tmpfs/filesystem.rs` /
  `src/fs/fat/fat_filesystem.rs` (gap zero-fill where not already implicit)
- `src/kernel.rs` (`/work` provisioning)
- `src/userland/bin_namespace.rs` (`tcc`, `cc`)
- `userland/apps.manifest.sh`, `userland/stage-lib.sh` (`stage_tree`),
  `userland/refresh-prebuilt.sh` (sysroot artifact)
- `userland/prebuilt/README.md`
- live docs listed in U6 (root `CLAUDE.md`, `src/userland/CLAUDE.md`,
  `src/fs/CLAUDE.md`, `userland/README.md`)

Delete: nothing.

No changes expected in: `build.sh`, `test.sh`, the loader
(`src/userland/loader.rs`), the GUI syscall surface, or the scheduler.

## Done criteria

- `./build.sh` on a fresh clone (no musl toolchain) boots with
  `/host/TCC.ELF` and `/host/sysroot/` staged from committed artifacts.
- `/work` exists and is writable on every boot; contents survive
  `sync` + reboot.
- In-guest: `tcc -o hello /host/sysroot/examples/hello.c && ./hello` works
  from zsh with no extra flags; separate `-c` + link also works; `cc` alias
  works.
- Kernel tests for arbitrary-length writes, seek-past-EOF zero-fill,
  chmod/fchmod, `/work`, and the TCC end-to-end fixture all pass under
  `./test.sh`; the full suite stays green.
- `./userland/refresh-prebuilt.sh` regenerates `TCC.ELF` and the sysroot
  tarball reproducibly.
- Live documentation reflects the new syscall semantics, `/work`, and the
  compiler; the GCC follow-up inherits a written list of everything TCC
  surfaced.

---

## Implementation notes (2026-07-18, all units landed)

What actually happened, for the GCC follow-up to inherit:

- **The e2e suite passed on the first full run** once U1–U3 were in. The
  kernel gaps the audit predicted were the real ones; nothing new
  surfaced. No unexpected ENOSYS during compile, link, or execution of
  compiled programs.
- **Zero TCC source patches.** Three upstream mechanisms covered
  everything: `--tcc-switches=-static` (default-static without touching
  `libtcc.c`), host-`cc` pre-generation of `c2str.exe`/`tccdefs_.h`, and
  `make x86_64-libtcc1-usegcc=yes` for `libtcc1.a`. One configure
  gotcha: `--cross-prefix` composes with `--cc`, so pass bare
  `--cc=gcc --ar=ar`.
- **Kernel gaps actually exercised by TCC:** >4 KiB `write()` (musl
  `fwrite` direct path — hard blocker, U1), `chmod` on the output
  executable (U3), and a writable cwd-adjacent output directory (U3's
  `/work`). Seek-past-EOF (U2) was implemented defensively and did not
  block TCC's own writer, but is POSIX-required and now covered by
  tests.
- **Deviation from plan:** examples live in `userland/apps/tcc/examples/`
  (not staged loose); `stage_tree` became the tcc-specific
  `stage_tcc_sysroot` in `stage-lib.sh` (one consumer, simpler); the
  e2e test is a dedicated `tcc` test module using
  `launch_user_binary` directly rather than a NETTEST-style fixture ELF.
- **Known-good for GCC:** fork/execve of arbitrary fresh ELFs, multi-MiB
  static binaries, `.o` write/read round-trips, argv/envp, exit-code
  propagation. **Watch list for GCC:** multi-process driver pipeline
  (cc1/as/ld spawning), temp files (point `TMPDIR` at `/work`; there is
  no `/tmp`), the 32-fd table, brk capped at 32 MiB (musl falls back to
  mmap), pipe short-writes under `-pipe`, and FAT PIO header-read
  latency at GCC's much larger header count.
