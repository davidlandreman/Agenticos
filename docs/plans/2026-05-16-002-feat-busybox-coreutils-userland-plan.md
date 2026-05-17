---
title: "feat: BusyBox userland — single-binary coreutils for ring-3 apps"
type: feat
status: active
date: 2026-05-16
---

# feat: BusyBox userland — single-binary coreutils for ring-3 apps

## Summary

Add a single static-musl `BB.ELF` (BusyBox) to `userland/prebuilt/` so the booted system gains `ls`, `cat`, `echo`, `grep`, `sed`, `wc`, `head`, `tail`, `sort`, `uniq`, `find`, `env`, `printf`, `true`/`false`, `yes`, `seq`, `tr`, `cut`, `basename`, `dirname`, `pwd`, `which`, `id`, `whoami`, `date`, `uname` (plus write-side commands documented as `EROFS`-bound). Mirror the existing zsh prebuilt pattern: `userland/apps/busybox/Makefile` fetches the upstream tarball, builds against the musl cross-toolchain, and `stage_busybox` in `prebuilt-lib.sh` copies the committed binary into `host_share/BB.ELF` on every boot. Teach the kernel to expose a virtual `/bin/<applet>` namespace so `execve("/bin/ls", argv, envp)` resolves to `BB.ELF` with `argv[0] = "ls"` and BusyBox's built-in multicall dispatcher does the rest — making bare `ls` from zsh's PATH lookup Just Work without symlinks or zsh-side wrappers.

---

## Problem Frame

`ZSH.ELF` boots an interactive shell, but the shell has nothing to run beyond the kernel's hand-written builtins (`ls`, `cat`, `pwd`, etc., implemented in `src/commands/`). Real Linux userland is what makes a shell useful: piping `ls | wc -l`, `grep` over the host mount, `find /host -name '*.ELF'`, etc. The zsh bring-up has already proven that the kernel's Linux ABI surface (`fork`, `execve`, `pipe2`, `wait4`, `dup2`, `getdents64`, `read`/`write`/`stat`, signals) is wide enough to host non-trivial static-musl binaries.

Three packaging options were considered:

1. **GNU coreutils** — ~80 separate ELFs at 0.5–1 MiB each, autoconf+gnulib-heavy build.
2. **toybox** — BSD-licensed multicall, smaller than busybox, fewer applets.
3. **BusyBox** — single static ELF, argv[0]-dispatched multicall, ~1 MiB total, ~300 applets, the embedded-Linux default.

The user picked BusyBox (see Key Technical Decisions below).

The kernel's FAT mount is read-only (every `write()` to a `FdSlot::File` returns `EROFS` at `src/userland/syscalls.rs:98`) and vvfat exposes only uppercase 8.3 names with no symlinks. Two consequences shape the design:

- Write-side applets (`cp`, `mv`, `rm`, `mkdir`, `touch`, `chmod`) will surface `EROFS` when the user invokes them. We ship them anyway (the multicall makes their cost zero) and document the limitation. This will dissolve when the FS gains write support.
- The classic BusyBox install model (`/bin/ls` → symlink to `/bin/busybox`) doesn't work. We need another resolution path so `PATH=/bin; ls foo.txt` finds the applet — either a kernel-side virtual `/bin` namespace or a zsh-side `command_not_found_handler`.

This plan picks the kernel-side virtual `/bin` because it (a) extends the `/etc/` → `/host/etc/` rewrite pattern already established in `src/userland/path.rs::apply_fs_rewrite`, (b) works for any caller of `execve`, not just zsh, and (c) doesn't require rebuilding `ZSH.ELF` with different configure flags.

---

## Requirements

- **R1.** A new app directory `userland/apps/busybox/` follows the `userland/apps/zsh/` pattern: `Makefile` that fetches a pinned upstream tarball, verifies SHA256, applies any patches, configures (pre-baked `.config` checked into the directory), builds with `x86_64-linux-musl-gcc`, strips the result, and writes `build/busybox`. Configuration disables build-time features that need glibc (`getpw_r` quirks, NSS, IDN, etc.) and disables applets known not to make sense yet (`init`, `udhcpd`, networking daemons).
- **R2.** A new `userland/prebuilt/BB.ELF` is committed to git so fresh clones boot a working `ls` without the musl toolchain. Size budget: ≤2 MiB stripped. (Reference: zsh is ~1.5 MiB; BusyBox with the applet set in R3 lands around 1 MiB in past projects.)
- **R3.** Applet set enabled in the BusyBox config covers at minimum:
  - Read-only file ops: `ls`, `cat`, `head`, `tail`, `wc`, `tac`, `nl`, `od`, `hexdump`, `file`, `stat`, `du`, `df`, `find`, `which`, `readlink`, `basename`, `dirname`, `pwd`, `realpath`
  - Text processing: `grep`, `egrep`, `fgrep`, `sed`, `awk`, `sort`, `uniq`, `cut`, `tr`, `fold`, `expand`, `unexpand`, `paste`, `comm`, `diff`, `cmp`, `printf`, `echo`, `xargs`, `tee`
  - Shell/env helpers: `env`, `id`, `whoami`, `groups`, `tty`, `uname`, `hostname`, `date`, `sleep`, `true`, `false`, `yes`, `seq`, `test`, `[`, `expr`
  - Write-side (will return `EROFS`, ship anyway): `cp`, `mv`, `rm`, `mkdir`, `rmdir`, `touch`, `chmod`, `chown`, `ln`, `dd`
  - Process/signal: `ps`, `kill`, `killall`, `pidof`, `sleep`, `nice`
- **R4.** `stage_busybox` in `userland/prebuilt-lib.sh` mirrors `stage_zsh`: respect `REBUILD_USERLAND=1` / `REBUILD_BUSYBOX=1`, fall back to the committed prebuilt when missing or rebuild fails, atomic refresh of both `userland/prebuilt/BB.ELF` and `host_share/BB.ELF`, `readelf` ET_EXEC sanity check.
- **R5.** `build.sh`, `test.sh`, and `userland/refresh-prebuilt.sh` invoke `stage_busybox` alongside `stage_zsh`. The CLI flag `--rebuild-userland` and env vars `REBUILD_USERLAND` / `REBUILD_BUSYBOX` are honored. Failure is soft (warn + continue) so a missing musl toolchain doesn't break kernel iteration.
- **R6.** Kernel exposes a virtual `/bin/<applet>` namespace:
  - `execve("/bin/<applet>", argv, envp)` resolves to `BB.ELF` (staged at `host_share/BB.ELF`, visible as `/host/BB.ELF` in the guest), with `argv[0]` rewritten to the applet name so BusyBox's multicall dispatcher selects the right applet.
  - `stat("/bin/<applet>")` and `access("/bin/<applet>", X_OK)` succeed for every applet in the enabled set, so zsh's PATH lookup (which `access`es each candidate) finds the binary.
  - `getdents64` on `/bin` enumerates the applet list so `ls /bin` and `which` listings work.
  - Unknown applet names return `ENOENT` for stat/access and `ENOENT` for execve.
- **R7.** Documentation updated in three places: `CLAUDE.md` (project overview mentions BusyBox + the read-only FS caveat for write applets), `userland/README.md` (new "BusyBox" subsection under "Adding an upstream C app", plus the `/bin` virtual namespace explanation), `userland/prebuilt/README.md` (new row in the table).
- **R8.** Default `PATH` for ring-3 processes set in the kernel-side launcher includes `/bin` (see `src/userland/lifecycle.rs` or wherever the initial envp is constructed) so `ls` resolves without the user having to set `PATH` manually in zsh.
- **R9.** Kernel tests cover the virtual `/bin` namespace: stat/access success for an in-set applet, stat/access ENOENT for an out-of-set name, execve path-rewrite + argv[0] preservation, getdents64 enumeration on `/bin`.
- **R10.** End-to-end smoke (manual + scripted): boot, `ls /host` from inside zsh produces the expected file list; `cat /host/HELLO.ELF | wc -c` returns a sensible byte count; `cp /host/HELLO.ELF /tmp/foo` surfaces `EROFS` cleanly without panicking.

---

## Scope Boundaries

### Outside this plan's scope
- **Write-side filesystem support.** `cp`, `mv`, `rm`, etc. will return `EROFS` until the FS gains write support. That's a separate cross-cutting plan touching `src/fs/`, `src/drivers/ide/`, and `src/userland/syscalls.rs`. Documented as a known limitation in `userland/README.md`.
- **Networking applets.** `wget`, `nc`, `ping`, `ifconfig`, `route`, DHCP — disabled in the BusyBox config because the kernel has no network stack. Re-enabling is a follow-up after `src/net/` lands.
- **TTY job control.** `bg`, `fg`, `jobs` in the shell sense rely on `tcsetpgrp`, which the kernel returns `ENOSYS` for. BusyBox applets that depend on PTY work (`script`, `screen`-style) are excluded.
- **`init`, `udhcpd`, `crond`, `inetd`, daemons.** Out of scope — no service supervision yet.
- **Replacing the kernel's built-in shell commands.** `src/commands/` keeps its hand-written `ls`, `cat`, `pwd` etc. for use before zsh exists and as kernel-side debugging tools. The BusyBox applets are for ring-3 use only.

### Deferred to Follow-Up Work
- **Per-applet selection UI** — a `make config` UI for trimming the applet set further is preserved upstream; we just ship a checked-in `.config`. If the binary grows past the 2 MiB budget we revisit.
- **`/usr/bin` mirror.** Some software hard-codes `/usr/bin/env`. The rewrite layer could be extended to make `/usr/bin/<applet>` an alias for `/bin/<applet>`. Skipped for the first cut; add when something actually needs it.
- **Auto-pickup of new applets when BusyBox's config changes.** Right now the applet enum list lives in the kernel (`src/userland/bin_namespace.rs`). When the BusyBox `.config` changes, a developer has to update the kernel list. A small build-time step that derives the list from the BusyBox config would be nicer; not worth it until the list churns.

---

## High-Level Technical Design

This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

### Path resolution flow

```
zsh: ls foo.txt
 ├─ zsh PATH lookup:
 │    access("/bin/ls", X_OK)
 │     └─ kernel: bin_namespace::is_applet("ls") → true → return 0
 └─ execve("/bin/ls", ["ls", "foo.txt"], envp)
      └─ kernel execve_handler:
           1. copy_user_cstr("/bin/ls") → "/bin/ls"
           2. normalize_path                → "/bin/ls"
           3. apply_fs_rewrite              → "/bin/ls"            (no /etc match)
           4. apply_bin_rewrite             → ("/host/BB.ELF", "ls")
           5. argv[0] := "ls"
           6. load_elf("/host/BB.ELF") + enter_user_mode
      busybox _start:
           dispatch on argv[0] → busybox_ls(argc, argv)
```

The rewrite layer at step 4 is the only new kernel surface; everything else is the existing zsh-tested path.

### Component sketch

```
userland/
├── apps/
│   └── busybox/                 # NEW
│       ├── Makefile             # fetch + verify + configure + build
│       ├── busybox.config       # checked-in trimmed config
│       ├── README.md
│       └── build/               # gitignored — tarballs + extracted src
└── prebuilt/
    └── BB.ELF                   # NEW — committed static-musl binary

src/userland/
├── bin_namespace.rs             # NEW — applet list + rewrite + dirent synth
├── path.rs                      # extended — apply_bin_rewrite alongside apply_fs_rewrite
└── syscalls.rs                  # extended — stat/access/execve/getdents64 call bin_namespace

src/userland/lifecycle.rs        # default envp gains PATH=/bin:/host
```

### Applet list source-of-truth

A single `const APPLETS: &[&str]` in `src/userland/bin_namespace.rs` lists every name the kernel recognizes as a `/bin/*` entry. The list MUST stay in sync with the BusyBox config. A one-line comment in both files cross-references the other. Kernel test `bin_namespace::test_applet_list_matches_busybox_help` is a future option (would require running BusyBox at test time); for now we rely on review discipline.

---

## Implementation Units

### U1. Add `userland/apps/busybox/` and the build pipeline

**Goal:** A `make -C userland/apps/busybox` invocation produces a stripped static-musl ELF at `userland/apps/busybox/build/busybox`.

**Requirements:** R1, R3

**Dependencies:** None

**Files:**
- `userland/apps/busybox/Makefile` — mirrors `userland/apps/zsh/Makefile`: pinned `BUSYBOX_VERSION` + `BUSYBOX_SHA256` + `BUSYBOX_URL`, fetch into `build/tarballs/`, verify SHA256, extract, copy `busybox.config` to `.config`, `make oldconfig` non-interactively, `make CC=$(MUSL_CC) LDFLAGS="-static -no-pie"`, strip.
- `userland/apps/busybox/busybox.config` — checked-in `.config` enabling the applet set from R3, disabling networking / init / daemons. Generated once with `make menuconfig` against an upstream tarball, then committed.
- `userland/apps/busybox/README.md` — short note: what version, what's enabled / disabled, how to regenerate `.config`.
- `userland/apps/busybox/.gitignore` — `build/`.

**Approach:**
- Pin BusyBox 1.36.1 (latest stable as of 2026-05). Bumping requires bumping SHA in lockstep, same as zsh.
- The musl cross-compiler defaults are PIE on some hosts. Mirror zsh's defenses: `LDFLAGS="-static -no-pie"`, `CFLAGS+=-fno-pie`, and let `build.sh`'s existing `readelf` check catch ET_DYN regressions.
- Disable applets known to need glibc-only headers (`getpwent_r`, NSS resolver) or kernel features we lack (`mount`, `swapon`, `losetup`, `ipcs`).
- BusyBox builds quickly compared to zsh (no autoconf), but still has a sizeable source tree. Same prebuilt-vs-rebuild contract from `userland/prebuilt/README.md` applies.

**Patterns to follow:** `userland/apps/zsh/Makefile` for tarball fetch + verify; `userland/apps/zsh/README.md` for documentation shape.

**Test scenarios:**
- Build smoke: run `make -C userland/apps/busybox` on a clean tree; assert `build/busybox` exists, is ET_EXEC, and `readelf -d build/busybox` shows no dynamic dependencies.
- SHA256 mismatch: deliberately corrupt the tarball; assert the Makefile aborts with a clear error.

Test expectation: no kernel-side tests in this unit; the build is host-only.

**Verification:** `file build/busybox` reports `ELF 64-bit LSB executable, x86-64, statically linked, stripped`. `readelf -h build/busybox` reports `Type: EXEC`. Binary size ≤2 MiB.

---

### U2. Wire BusyBox into the prebuilt-staging pipeline

**Goal:** `./build.sh` and `./test.sh` stage `host_share/BB.ELF` automatically — from the committed prebuilt by default, from a fresh build when `--rebuild-userland` or `REBUILD_BUSYBOX=1` is set.

**Requirements:** R2, R4, R5

**Dependencies:** U1

**Files:**
- `userland/prebuilt-lib.sh` — add `stage_busybox()` modeled on `stage_zsh()`.
- `build.sh` — call `stage_busybox || true` after the `stage_zsh || true` line.
- `test.sh` — same.
- `userland/refresh-prebuilt.sh` — add busybox to the list of apps it forces.
- `userland/prebuilt/BB.ELF` — committed after U1 produces the binary and U2 stages it once.

**Approach:**
- `stage_busybox` is a copy-paste of `stage_zsh` with the names swapped: `REBUILD_BUSYBOX`, `BB.ELF`, `userland/apps/busybox/build/busybox`. Resist the urge to abstract this into a single parameterized function until a third app lands — the duplication is small and the parameters differ enough (per-app env var name) to make abstraction lossy at N=2.
- `refresh-prebuilt.sh` should hard-fail if either app fails to build (preserving the existing zsh behavior).

**Patterns to follow:** `userland/prebuilt-lib.sh::stage_zsh` exactly.

**Test scenarios:**
- Default build: with `userland/prebuilt/BB.ELF` present, `./build.sh -n` does NOT invoke `make -C userland/apps/busybox`, does NOT probe for the musl toolchain, and `host_share/BB.ELF` matches `userland/prebuilt/BB.ELF` byte-for-byte.
- Forced rebuild: `REBUILD_BUSYBOX=1 ./build.sh -n` invokes `make`, refreshes both `userland/prebuilt/BB.ELF` and `host_share/BB.ELF` atomically.
- Toolchain missing: with `PATH` scrubbed of `x86_64-linux-musl-gcc`, `REBUILD_BUSYBOX=1 ./build.sh -n` emits the standard "musl toolchain not found" warning and falls back to the committed prebuilt without aborting the build.
- Missing prebuilt: with `userland/prebuilt/BB.ELF` removed, the script attempts a rebuild even without the flag (auto-bootstrap path).

Test expectation: shell-script behavior verified manually; no in-kernel test in this unit.

**Verification:** Boot under QEMU; serial log shows `📦 Staged host_share/BB.ELF from userland/prebuilt/ (N bytes)`.

---

### U3. Kernel `/bin/<applet>` virtual namespace

**Goal:** `execve("/bin/ls", ...)`, `stat("/bin/ls", ...)`, `access("/bin/ls", X_OK)`, and `getdents64` on `/bin` all behave as if `/bin` were a real directory full of BusyBox applets.

**Requirements:** R6, R9

**Dependencies:** None (independent of U1/U2 — can be developed and tested against the embedded test ELF, then exercised end-to-end once BB.ELF stages)

**Files:**
- `src/userland/bin_namespace.rs` — NEW. Owns the canonical applet list, the `/bin/<name>` path-rewrite function, and the `Dirent` synthesizer for `getdents64`.
- `src/userland/path.rs` — extend with `apply_bin_rewrite(normalized: &str) -> Option<(String, String)>` returning `(real_path, applet_name)` when the path is `/bin/<applet>`, `None` otherwise. Document the ordering rule (normalize first, then bin-rewrite, same security pattern as `apply_fs_rewrite`).
- `src/userland/syscalls.rs`:
  - `execve_handler`: after `apply_fs_rewrite`, call `apply_bin_rewrite`. If it matches, swap the resolved path AND overwrite `argv[0]` with the applet name before `load_elf`.
  - `stat_handler` / `newfstatat_handler` / `access_handler` / `faccessat_handler`: if `apply_bin_rewrite` matches, fill `struct stat` with a fixed "regular file, mode 0755, size = BB.ELF size" record (size resolved at call time via the existing `stat` of `/host/BB.ELF`) and return success. Same for `access` — return 0 for any applet name in the list.
  - `getdents64_handler`: if the opened directory is `/bin`, emit a synthesized batch of dirent records, one per applet, plus `.` and `..`.
  - `open_handler` / `openat_handler` on `/bin/<applet>`: open the underlying `/host/BB.ELF` — useful for `which`-style tools that `open`+`fstat`.
- `src/tests/userland/bin_namespace.rs` — NEW test module. Covers the rewrite + dirent + stat behaviors.
- `src/tests/mod.rs` — register the new module.

**Approach:**
- Keep the applet list as a sorted `&'static [&'static str]` so lookup is a `binary_search`. ~150 entries is fine even with linear search, but sorted-and-binary keeps the lookup O(log n) and lets the dirent synthesizer emit entries in deterministic order without extra sorting.
- The applet list MUST exactly match what's enabled in `userland/apps/busybox/busybox.config`. Add a `// Keep in sync with userland/apps/busybox/busybox.config — see plan U3.` comment in both files.
- For `getdents64`: a `Dirent64` record per applet is small (~30 bytes including alignment). 150 applets ≈ 4.5 KiB, comfortably under one page. Emit all entries in one call; subsequent calls return 0 (EOF).
- For `stat`: the size field matters to `cat /bin/ls` but not to PATH lookup; resolve `BB.ELF` size once at first call and cache it (refresh on `host_share/BB.ELF` mtime change is not worth the complexity — kernel restarts on every QEMU launch).
- The applet name in `argv[0]` MUST be the rewritten name even if the user passed something else (e.g., `execve("/bin/ls", ["banana", ...], envp)` should pass `argv[0]="ls"` to BusyBox so dispatch works correctly). Document the deviation from Linux semantics — Linux preserves the caller's argv[0] verbatim. This is a multicall-binary requirement, not a general execve change.

**Execution note:** Add unit tests for `apply_bin_rewrite` and the dirent synthesizer first (pure functions, no kernel state), then extend the syscall handlers. The signal-handling work in the zsh bring-up taught us that touching `execve_handler` without tests in place burns hours.

**Patterns to follow:**
- `src/userland/path.rs::apply_fs_rewrite` for the rewrite function shape and the normalize-first security ordering.
- `src/tests/userland/path.rs` for in-kernel pure-function tests.
- Existing `src/userland/syscalls.rs::execve_handler` argv handling (line 1150+) for how argv is copied from user space.

**Test scenarios:**
- `apply_bin_rewrite` on `/bin/ls` returns `Some(("/host/BB.ELF", "ls"))`.
- `apply_bin_rewrite` on `/bin/nonexistent` returns `None`.
- `apply_bin_rewrite` on `/bin/` (no applet) returns `None`.
- `apply_bin_rewrite` on `/bin/ls/extra` returns `None` (only direct children match).
- Path traversal: `apply_bin_rewrite` called on `normalize_path("/", "/bin/../bin/ls")` (which normalizes to `/bin/ls`) returns `Some(("/host/BB.ELF", "ls"))`. The unnormalized `/bin/../etc/shadow` MUST NOT reach `apply_bin_rewrite` directly — enforced by the normalize-first ordering in handlers.
- `access("/bin/ls", X_OK)` returns 0; `access("/bin/nonexistent", X_OK)` returns `-ENOENT`.
- `stat("/bin/ls", &statbuf)` fills statbuf with mode containing `S_IFREG | 0755`, `st_size == size_of("/host/BB.ELF")`.
- `getdents64` on `/bin` returns the full applet list plus `.` and `..`; second call returns 0 (EOF).
- `execve("/bin/ls", ["banana"], [])` records a debug log line showing `argv[0] := "ls"`, then loads BB.ELF (use the existing `LAST_EXIT_CODE` mirror or a similar test hook to assert without spinning up a real ring-3 process).
- Integration with `apply_fs_rewrite`: confirm that `/etc/passwd` still rewrites (not accidentally captured by `/bin` matcher) and that `/bin/ls` is NOT rewritten by `apply_fs_rewrite` (the two rewrite layers compose cleanly).

**Verification:** All new tests pass under `./test.sh bin_namespace`. The existing test suite (`./test.sh`) still passes (no regressions in `apply_fs_rewrite` or execve).

---

### U4. Default `PATH=/bin:/host` for ring-3 processes

**Goal:** When the kernel-side `shell` (or any other launcher in `src/commands/`) spawns a user process, the initial envp includes `PATH=/bin:/host` so applets resolve without the user setting PATH manually.

**Requirements:** R8

**Dependencies:** U3 (no point setting PATH before `/bin` is real)

**Files:**
- `src/userland/lifecycle.rs` (or wherever `enter_user_mode_with_aspace` accepts initial envp — check the existing zsh launcher to find the right seam).
- The launcher in `src/commands/` that spawns zsh — add `PATH=/bin:/host` to the seeded envp.
- `src/tests/userland/launcher.rs` — extend or add a test asserting envp contents at launch.

**Approach:**
- The kernel currently constructs envp for the spawned binary in a single spot. Add `PATH=/bin:/host` (and confirm `HOME=/`, `TERM=linux` or similar are present — check what zsh currently sees). If envp construction is currently caller-specified (each `src/commands/run/*` builds its own), centralize in a small helper.
- `/host` second so the user can still `run` a bare ELF by name from PATH (`HELLO` would find `/host/HELLO` — actually no, vvfat exposes `HELLO.ELF` with the suffix, so this is mostly aesthetic; leaving `/host` in PATH costs nothing).

**Patterns to follow:** Existing envp construction in the zsh launcher.

**Test scenarios:**
- Spawn a tiny test ELF that prints its envp; assert the output contains `PATH=/bin:/host`.

**Verification:** Boot, drop into zsh, run `echo $PATH` → output is `/bin:/host`.

---

### U5. End-to-end smoke + docs

**Goal:** `BB.ELF` is committed, `ls` works from zsh, write-side EROFS is observably clean, and the docs explain the model.

**Requirements:** R2, R7, R10

**Dependencies:** U1, U2, U3, U4

**Files:**
- `userland/prebuilt/BB.ELF` — committed binary produced by `./userland/refresh-prebuilt.sh` once U1–U4 land.
- `userland/prebuilt/README.md` — new row in the "What lives here" table for `BB.ELF` (source `userland/apps/busybox/`, type EXEC, approximate size, note "BusyBox multicall, see /bin namespace in kernel").
- `userland/README.md` — new section "BusyBox applets and the `/bin` namespace" explaining: where the binary lives, how applets resolve via kernel-side `/bin/<name>` rewrite, which applet set is enabled, write-side EROFS limitation.
- `CLAUDE.md` — update "Current State" to mention BusyBox; add `src/userland/bin_namespace.rs` to the subsystem index if a CLAUDE.md for `src/userland/` is added (otherwise reference inline).
- `userland/apps/busybox/README.md` — operational notes for the app itself (regenerating `.config`, bumping version).

**Approach:**
- Boot manually, walk through the R10 smoke list, capture any surprises in a quick "post-bring-up" addendum in this plan or as follow-up items.
- The doc changes follow the pattern set by the zsh bring-up: keep `CLAUDE.md` short, push detail into `userland/README.md`.

**Patterns to follow:** Documentation diffs from `docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md` and `docs/plans/2026-05-16-001-feat-prebuilt-userland-elfs-plan.md`.

**Test scenarios (manual):**
- `ls /host` from zsh: returns the staged file list, exits 0.
- `cat /host/HELLO.ELF | wc -c`: returns a positive integer matching `wc -c < host_share/HELLO.ELF` on the host.
- `find /host -name '*.ELF' -type f`: lists at least `HELLO.ELF`, `HELLOCPP.ELF`, `ZSH.ELF`, `BB.ELF`.
- `echo foo | grep f`: prints `foo`.
- `cp /host/HELLO.ELF /tmp/foo`: prints `cp: cannot create '/tmp/foo': Read-only file system` (or BusyBox's equivalent message), exits non-zero, kernel does NOT panic.
- `which ls`: prints `/bin/ls`.
- `ls /bin | head -20`: enumerates 20 applet names.
- `busybox --list | wc -l`: matches the applet count in the kernel's `APPLETS` list (off-by-one check for `busybox` itself is fine).

**Verification:** Smoke list above all passes. `./test.sh` is still green.

---

## Key Technical Decisions

### KTD1. BusyBox over toybox or GNU coreutils

User picked BusyBox over toybox and GNU coreutils (see Problem Frame). Justification beyond user preference:
- **Maturity in static-musl builds.** BusyBox has been the default coreutils for embedded musl-libc Linux for ~20 years. Toybox is good but the static-musl story is less battle-tested. GNU coreutils' autoconf + gnulib layer makes static-musl builds substantially harder.
- **Applet breadth.** BusyBox ships ~300 applets vs toybox's ~200 vs GNU coreutils' ~100. We don't enable all of them, but breadth matters for one-off needs (`hexdump`, `strings`, `xxd`, ...).
- **Multicall semantics are widely understood.** The argv[0]-dispatched model has been stable in BusyBox since the late 1990s; documentation and behavior are well-known.

Trade-off accepted: BusyBox applets follow Linux GNU semantics loosely, not strictly. `ls` flags, `grep` regex flavors, and `sed` extensions sometimes differ from their GNU counterparts. Acceptable for a research OS; we can re-evaluate if a future workload demands strict GNU compatibility.

### KTD2. Kernel-side `/bin` rewrite over zsh-side function definitions

Two paths to make `ls` resolve to `BB.ELF`:
- **A.** Kernel-side virtual `/bin` namespace (this plan).
- **B.** Re-build `ZSH.ELF` with `--enable-zshenv`, stage `host_share/etc/zshenv` with a `command_not_found_handler` that rewrites `ls foo` → `/host/BB.ELF ls foo`.

Picked A because it (a) works for any caller of `execve`, not just zsh — `find -exec`, BusyBox's own internal `system()` calls, future C/Rust apps that hard-code `/bin/ls`; (b) extends the existing `apply_fs_rewrite` pattern rather than introducing a parallel zsh-side rewrite layer; (c) doesn't force rebuilding `ZSH.ELF` (which would require a coordinated prebuilt refresh).

Trade-off accepted: argv[0] semantics deviate from Linux (we overwrite the caller's argv[0]). Documented inline at the rewrite site.

### KTD3. Ship write-side applets despite EROFS

User picked "include them, surface EROFS at runtime, document as known limitation." Justification: the multicall binary includes them at zero marginal cost (they're already compiled into the BusyBox binary), removing them would require a custom `.config` line per applet, and the EROFS error path is well-tested in the kernel already (zsh exercises it via `>>` redirection failures). Removing them only to add them later would be churn.

### KTD4. Sorted-static-slice applet list in the kernel

Considered: a `BTreeSet<&'static str>` for membership tests, or a `phf::Map` for compile-time hashing.
Picked: sorted `&'static [&'static str]` with `binary_search`.
Rationale: ~150 entries, `binary_search` is O(log n) and constant-factor cheap, no heap allocation, and the sorted order is what `getdents64` wants to emit anyway. `phf` would pull in a build dependency for negligible win at this scale.

---

## Risks and Mitigations

### R-1. BusyBox build surfaces an unknown syscall and crashes at runtime
**Likelihood:** Medium — BusyBox applets exercise a broader syscall surface than zsh (`getrlimit`, `setrlimit`, `gettimeofday` variants, `clock_nanosleep`, `pselect6`, `epoll`, etc.). zsh already triggered ~15 missing syscalls.
**Impact:** Medium — an unhandled syscall returns `-ENOSYS`, which most applets handle as a soft error; some applets will crash or hang.
**Mitigation:** During smoke testing (U5), watch serial for the kernel's `unhandled_syscall` log lines. File a follow-up plan or add to "Deferred to Follow-Up Work" for any applet that requires a real implementation. Most read-only applets won't hit anything novel.

### R-2. Argv[0] overwrite breaks some applets
**Likelihood:** Low — BusyBox's dispatcher explicitly reads argv[0] and looks up the applet by basename, so overwriting argv[0] with the applet name is exactly what dispatcher wants.
**Impact:** High if it happens — every applet invocation fails.
**Mitigation:** Test U3's `argv[0] := "ls"` rewrite end-to-end with a real BB.ELF before committing. The unit test should mock load_elf and assert the rewritten argv, but the smoke test is the real proof.

### R-3. Applet list drifts between kernel and BusyBox config
**Likelihood:** Medium — whenever the `.config` changes, the kernel list must also change.
**Impact:** Low — a missing kernel entry means PATH lookup fails (`ls` → ENOENT), which is a loud failure mode. An extra kernel entry means `which extra-applet` says `/bin/extra-applet` but execve dispatches to a non-existent applet, which BusyBox handles cleanly by printing "applet not found".
**Mitigation:** Cross-reference comments in both files. Long-term: a build-time derivation step (deferred to follow-up).

### R-4. Prebuilt binary size exceeds 2 MiB budget
**Likelihood:** Low — past projects report ~700 KiB–1.2 MiB for similar applet sets.
**Impact:** Low — git tracks fine, host_share copy is fast. We just want the budget visible so we notice if it doubles.
**Mitigation:** If R3's applet set produces a binary >2 MiB, trim the config (likely candidates: `tar`, `gzip`, `bzip2`, `httpd` if accidentally enabled). Re-cut.

### R-5. Static-musl BusyBox uses a syscall the kernel returns -ENOSYS for in a way that breaks `_start` itself
**Likelihood:** Low — the zsh bring-up already established that the kernel's startup syscall surface (`brk`, `arch_prctl`, `set_tid_address`, `set_robust_list`, etc.) is wide enough for static musl. BusyBox uses the same startup path.
**Impact:** High — BusyBox would crash before printing anything.
**Mitigation:** Smoke test U5 the simplest applet first (`busybox echo hi`) before anything else. If that works, dispatch is fine.

---

## System-Wide Impact

- **`src/userland/`** gains a small new module (`bin_namespace.rs`) and extensions to `path.rs` and `syscalls.rs`. Risk is localized — the rewrite is opt-in (returns `None` for non-`/bin` paths) and changes no existing handler's success path.
- **`userland/`** gains a new app directory and a new prebuilt binary. The pattern is identical to zsh; no cross-cutting build-script restructure.
- **Documentation** updates in three files mirror prior userland-app PRs.
- **No changes to** `src/fs/`, `src/drivers/`, `src/mm/`, `src/graphics/`, `src/window/`, `src/process/`, or the kernel shell command set.

The kernel test suite gains ~6 new tests (`bin_namespace` module). Total impact on test runtime: negligible.

---

## Open Questions

- **Q1.** Does the kernel currently expose `getdents64` on a "virtual" directory that has no underlying FAT inode? If not, the cleanest implementation routes `/bin` through a small `VirtualDir` enum-like type that the FS layer recognizes. Confirm during U3 — if the FS layer doesn't have a hook for virtual directories, the work shifts up into `syscalls.rs::getdents64_handler` to intercept `/bin` before the FS lookup. **Resolution: defer to U3 implementation; investigation is cheap and the answer changes only the seam, not the design.**
- **Q2.** Should `/usr/bin/<applet>` also resolve? Some scripts hard-code `/usr/bin/env`. **Resolution: deferred (see Scope Boundaries → Deferred to Follow-Up Work).** Add when a real workload needs it.
- **Q3.** Should the kernel synthesize `/bin/sh` → BusyBox's `ash` applet? `ash` is in the default applet set. zsh is the interactive shell, but `#!/bin/sh` scripts would need `/bin/sh`. **Resolution: yes, include `sh` as an alias for `ash` in the applet list. Document in U3.**

---

## Origin

This plan was generated directly from a user request — no upstream brainstorm document. The kernel-side prerequisites (zsh, prebuilt-userland pattern, Linux ABI surface) are all in `main` as of 2026-05-16.
