---
title: "feat: port git as a static-musl userland app"
type: feat
status: implemented
date: 2026-07-18
depth: medium
related_docs:
  - CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - userland/prebuilt/README.md
  - userland/apps/curl/Makefile
  - docs/plans/2026-07-18-010-feat-curl-port-plan.md
  - docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md
  - https://git-scm.com/downloads
---

# feat: port git as a static-musl userland app

## Summary

Bring real version control to AgenticOS by porting upstream **git** as a
prebuilt-managed static-musl app, following the curl playbook exactly:
pinned tarball + SHA256, `x86_64-linux-musl-gcc` cross build, static-link
gates, committed prebuilt binaries, `/bin` namespace names.

Deliverables:

- `GIT.ELF` → `/bin/git` — all builtins in one binary (init, add, commit,
  log, branch, checkout, merge, diff, status, tag, fsck, clone/fetch/push
  over local paths).
- `GITRHTTP.ELF` → `/bin/git-remote-https` (alias `git-remote-http`) —
  the HTTP(S) transport helper, linked against the *same* pinned
  OpenSSL 3.5.7 + zlib profile and `/etc/ssl/cert.pem` trust store as
  `CURL.ELF`, so `git clone https://…` works with strict certificate
  verification by default.
- Kernel-managed `/etc/gitconfig` with system defaults suited to this OS.
- In-kernel tests: local repo round-trip plus a restricted-QEMU HTTP(S)
  clone from a repository-owned guest-forwarded service.

## Why not BusyBox (the user's first instinct)

- **BusyBox has no git applet.** Upstream BusyBox has never shipped one and
  there is no maintained patch set; "adding git to BusyBox" would mean
  writing a git implementation from scratch inside BusyBox.
- **ToyBox's git toys are not viable.** ToyBox carries experimental
  `gitclone` / `gitfetch` / `gitinit` / `gitcheckout` in its pending
  directory — read-only, incomplete (no `add`/`commit`/`log`/`push`), and
  explicitly not production quality.
- **Real git is an easy static port.** Git is portable C, builds with plain
  `make` (no autoconf run required), links static against musl cleanly, and
  its only hard dependency is zlib — which we already pin for curl/Links.
  The heavyweight optional deps (perl, python, tcl/tk, gettext, expat,
  pcre2) all have `NO_*` knobs.

### Alternatives considered

- **gitoxide (`gix`/`ein`, Rust)** — real and statically buildable, but
  porcelain-incomplete (no ergonomic commit/merge workflow yet) and would
  add a Rust-std-musl toolchain requirement. Not chosen.
- **libgit2 + thin CLI** — libgit2 has no official CLI; we'd be writing
  and maintaining porcelain ourselves. Not chosen.

## Current-state evidence (what the kernel already provides)

The syscall dispatcher (`src/userland/abi.rs::syscall_dispatch`) already
covers git's core profile:

- **Process machinery**: `fork`, `vfork`, `execve`, `wait4`, `pipe`/`pipe2`,
  `dup`/`dup2`, `kill`, signals with real delivery. Git shells out
  constantly (transport helpers, hooks, pager); this all exists and is
  exercised daily by zsh.
- **Filesystem**: `openat`, `getdents64`, `newfstatat`, `pread64`/`pwrite64`,
  `rename(at)` (lockfile commit), `link(at)` (loose-object finalize),
  `symlink(at)`, `unlink(at)`, `mkdir(at)`, `ftruncate`, `fsync`,
  `utimensat`, `umask`, `getcwd`/`chdir`/`fchdir`.
- **File-backed private mmap**: `VmaBacking::FilePrivate` in
  `src/userland/vm.rs` — git maps packfiles and the index read-only
  MAP_PRIVATE. (`NO_MMAP=1` builds a pread-based compat fallback if this
  path misbehaves; keep it as an escape hatch, not the default.)
- **Time and identity**: RTC-anchored UTC wall clock (commit timestamps),
  kernel-managed `/etc/passwd` (`src/userland/etc.rs::PASSWD_PATH`) for
  musl `getpwuid`, `/bin/sh` via BusyBox for hooks and `!`-aliases.
- **Multi-MiB ELF loading** is a solved problem (see the
  `2026-05-09-multi-mib-user-binary-load` learning); the loader gate is
  16 MiB per binary, same as curl's Makefile asserts.

Known-gap watch item: `wait4` reports signaled children with the
cooperative-exit encoding (CLAUDE.md known issue #3). Git checks exit
status of helpers; a crashed helper will look like "exited 139" rather
than "killed by SIGSEGV" — cosmetically wrong error text, functionally
harmless. No blocker.

## Design

### Version pinning

Pin the latest stable git 2.x at implementation time (2.50+ era) with
SHA256, fetched from `https://mirrors.edge.kernel.org/pub/software/scm/git/`
(primary) with the same fetch/verify shape as the curl Makefile. zlib
stays on the pinned 1.3.2 recipe copied verbatim from
`userland/apps/curl/Makefile`; Phase 2 adds the pinned OpenSSL 3.5.7
recipe, also verbatim.

### Build recipe (`userland/apps/git/Makefile`)

Git builds with plain `make` and a `config.mak`-style variable set — no
`./configure` needed, which keeps the recipe as auditable as curl's:

```
CC=x86_64-linux-musl-gcc  CFLAGS="-O2 -fno-pie"  LDFLAGS="-static -no-pie"
prefix=/  gitexecdir=/bin  sysconfdir=/etc       # helpers + system config resolve to /bin, /etc
NO_GETTEXT=1 NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1    # drop scripting-language porcelain
NO_EXPAT=1                                        # expat only serves legacy dumb-HTTP push
NO_ICONV=1                                        # skip iconv path; UTF-8 only
NO_PTHREADS=1                                     # single-threaded bring-up (see below)
NO_CURL=1                                         # Phase 1 only; Phase 2 replaces with static libcurl
NO_INSTALL_HARDLINKS=1 SKIP_DASHED_BUILT_INS=1    # one binary, no git-<builtin> hardlink farm
NO_SYS_SELECT_H / NO_MMAP etc. only if probing demands it
```

Rationale for the load-bearing choices:

- **`gitexecdir=/bin`** — git locates transport helpers and any dashed
  externals via its compiled-in exec path. Pointing it at `/bin` means
  `git clone https://…` execs `/bin/git-remote-https`, which the virtual
  bin namespace rewrites to `GITRHTTP.ELF`. No `GIT_EXEC_PATH` env
  plumbing, works for processes not launched from zsh.
- **`NO_PTHREADS=1`** — git only uses threads for pack/delta parallelism
  (`pack-objects`, `index-pack`, `grep`). The musl pthread runtime exists
  but is young and pthread groups pin to one home CPU, so threading buys
  nothing today. Single-threaded git is a fully supported upstream
  configuration. Revisit once user TLB shootdown lands.
- **`SKIP_DASHED_BUILT_INS=1`** — modern git dispatches builtins inside
  the one `git` binary; the dashed forms are only needed on `$PATH` for
  ancient scripts. Keeps the shipped surface to two ELFs.
- Perl/python removal costs `git-send-email`, `git-svn`,
  `git-request-pull` and friends — all irrelevant here.

Static gates copied from curl's Makefile: no `PT_INTERP`, no `DT_NEEDED`,
`strip --strip-all`, per-binary size assert under the 16 MiB loader gate.
Expected sizes: `GIT.ELF` ≈ 4–7 MiB stripped; `GITRHTTP.ELF` ≈ 5–7 MiB
(it carries static libcurl + OpenSSL).

Shell-script porcelain that survives the `NO_*` pruning
(`git-sh-setup`, a few `git-*.sh`) either lands in the exec path or is
dropped; audit at build time which scripts remain and stage only ones
that run against BusyBox `sh`. Template directory (`templates/`) ships
empty (`--template` users get plain `git init` behavior; hook samples are
noise here).

### Kernel integration

- **Manifest**: two `app_row` entries in `userland/apps.manifest.sh`
  (`git` → `GIT.ELF`, `git-remote-https` → `GITRHTTP.ELF`), both
  `prebuilt-managed` / `musl-cc`, prebuilts committed under
  `userland/prebuilt/`. `REBUILD_GIT=1` env knob falls out of the
  existing manifest machinery, as does `refresh-prebuilt.sh` coverage.
- **Bin namespace** (`src/userland/bin_namespace.rs`): add `git` →
  `GIT.ELF` host path; add `git-remote-https` **and** `git-remote-http`
  both → `GITRHTTP.ELF` (the helper keys behavior off `argv[0]`, exactly
  the pattern the namespace already uses for `links`/`links2` and
  BusyBox applets).
- **`/etc/gitconfig`** via the kernel-managed `/etc`
  (`src/userland/etc.rs`), so every process sees the same system defaults:

  ```ini
  [user]        name = root / email = root@agenticos.local   # no per-user identity yet
  [init]        defaultBranch = main
  [safe]        directory = *                                # single-uid system; kill dubious-ownership refusals
  [core]        fileMode = false                             # FAT lower layer has no exec bit
                pager = cat                                  # BusyBox less is line-oriented; opt back in per-user
  [advice]      detachedHead = false
  ```

  Two entries to add **only if bring-up shows the need** (verify first,
  don't preload): `core.createObject = rename` (if `link()`-based
  loose-object finalize misbehaves on the overlay upper layer) and
  `core.symlinks = false` on `/` (overlay symlink support exists but is
  FAT-backed below).
- **Working location guidance**: repos live under `/work` (scratch) or
  `/data` (persistent ext2, real Unix metadata, no FAT 2-second-mtime
  index races). cwd starts at read-only `/host`, same story as TinyCC.
- **Editor**: default commit path is `git commit -m`; interactive commit
  gets `GIT_EDITOR`/`core.editor = busybox vi` documented, not defaulted.

### HTTP(S) transport (Phase 2)

- Build static libcurl inside the git Makefile using the **identical**
  configure profile as `userland/apps/curl/Makefile` (same pinned
  OpenSSL 3.5.7 + zlib 1.3.2, `--with-ca-bundle=/etc/ssl/cert.pem`,
  IPv4-only, HTTP/HTTPS-only, no threads, socketpair disabled), then
  link `git-remote-http` against it. The security-relevant `config.h`
  asserts carry over verbatim.
- Smart-HTTP clone/fetch/push need only libcurl (expat is dumb-WebDAV
  push only, which stays out).
- DNS through musl's resolver against the DHCP-managed
  `/etc/resolv.conf`; certificate verification strict by default, with
  `http.sslVerify=false` / `GIT_SSL_NO_VERIFY` as the explicit
  user-typed override — the same posture line curl drew with `-k`.
- git ↔ helper speak over pipes with fork/execve — no new kernel ABI.

### Syscall-gap discovery

Before writing any kernel code, run the strace-mode discovery loop the
libuv port used: boot with syscall trace, run the git workflow, and read
the `[strace] first unknown nr=…` lines. Anticipated possible stragglers
(all cheap if they appear): `fchmodat`, `fadvise64` (stub to 0),
`getpgrp`/`setpgid` (pager job control), `fstatfs`. Everything on git's
hot path is already implemented.

## Implementation phases

1. **Phase 1 — local git.** Makefile (git + pinned zlib, `NO_CURL`),
   static gates, manifest rows, bin-namespace `git`, `/etc/gitconfig`,
   strace gap sweep, fix any straggler syscalls. Exit criteria: in the
   QEMU guest, `cd /work && git init t && cd t && echo hi > f &&
   git add f && git commit -m x && git log && git fsck` all succeed, and
   a `file://`-free local clone (`git clone /work/t /work/t2`) works.
2. **Phase 2 — HTTPS remotes.** Add libcurl/OpenSSL recipe, build and
   ship `GITRHTTP.ELF`, bin-namespace helper names. Exit criteria:
   restricted-QEMU clone over HTTP(S) from a repository-owned
   guest-forwarded service (host side: `git http-backend` behind the
   existing test HTTP infrastructure, or a static `update-server-info`
   dumb export for fetch-only coverage); strict-verification failure
   cases mirror the curl test matrix (mismatched host, untrusted CA).
3. **Phase 3 — tests + polish.** In-kernel test module under
   `src/tests/` scripting the Phase 1 round-trip plus an HTTP clone;
   `userland/prebuilt/README.md` rows; CLAUDE.md orientation paragraph;
   `./userland/refresh-prebuilt.sh` run and both ELFs committed.
   Manual QA: clone a small public GitHub repo over HTTPS in an
   interactive boot.

## Implementation notes (post-landing)

Landed as git 2.52.0 (`GIT.ELF`) + the `git-remote-http{,s}` helper
(`GITRHTTP.ELF`), staged/registered exactly like the curl port.

Deviations and discoveries from the plan:

- **musl regex**: musl lacks `REG_STARTEND`, so the Makefile adds
  `NO_REGEX=NeedsStartEnd` (git's bundled compat regex). Not anticipated
  in the plan; it's the standard Alpine choice.
- **Darwin build host**: the cross build runs on macOS, so the git
  Makefile needs explicit `uname_S=Linux uname_M=x86_64 …` overrides or
  it configures for Darwin. Added to `GIT_MAKE_VARS`.
- **`/dev/null` was missing.** git's `sanitize_stdfds` opens `/dev/null`
  O_RDWR unconditionally at startup, and the kernel's synthetic `/dev`
  only had `urandom`. Added `DeviceNode::Null` / `FdSlot::DevNull` across
  `devfs.rs`, `fdtable.rs`, and every `syscalls.rs` fd site
  (open/read/write/writev/pread/pwrite/lseek/stat/fstat/getdents/poll/
  describe/access); it is the one writable device node. Covered by
  `userland::test_dispatch_dev_null_rdwr_read_eof_write_sink`. This is a
  general Linux-compat gain, not git-specific.
- **Auto-maintenance disabled in `/etc/gitconfig`.** `git commit` runs
  `git gc --auto` → `git maintenance run --auto`, forking a helper on
  every commit. Seeded `gc.auto = 0` and `maintenance.auto = false`.
- **Tests launch git directly through the loader**, like the binutils
  suite, rather than through zsh `-c` — keeping coverage on git + the
  kernel ABI rather than zsh's job-control signal path.

### Kernel bugs found and fixed while bringing git up

Bringing git up surfaced three genuine, pre-existing kernel defects (all
in the fork/signal/pipe area the repo already flagged as fragile). Each
was fixed as part of this work because git is the first workload to
exercise them hard:

1. **`/dev/null` was missing** (see above) — `DeviceNode::Null` /
   `FdSlot::DevNull`.
2. **Signal frame clobbered the System V red zone.** `deliver_signal`
   (`src/userland/syscalls.rs`) built the handler frame at `user_rsp`
   without skipping the 128-byte red zone, so an async signal (git's
   SIGALRM progress timer) delivered at a syscall boundary corrupted the
   interrupted function's red-zone temporaries; on `rt_sigreturn` the
   process resumed with a corrupted pointer and `#PF`'d at a garbage RIP.
   Fixed by subtracting `RED_ZONE_BYTES` on the normal-stack path (Linux
   semantics). This is the likely cure for the documented links2-HTTPS
   `rt_sigreturn`-to-kernel-address crash too.
3. **fds stayed open until reap, deadlocking fork→pipe→wait.** A cooperative
   /abnormal exit retained the whole child `Process` (fd table included)
   until the parent reaped it, so the child's pipe write end never
   closed. A parent blocked reading that pipe never saw EOF, so it never
   reached `wait4` to reap — a deadlock. Fixed with `close_group_fds`
   (drop the exiting group's fd table at exit, with `PROCESS_TABLE`
   unlocked so the pipe-handle `Drop` wake lands), plus reliable
   (blocking-lock) wakes on the live `Pipe::read`/`Pipe::write` data
   paths (`wake_ring3_blocked_by_locking`) since those handlers hold no
   process lock and a dropped wake strands a pipe peer.

### Known limitation: pack-protocol transports

`git clone`/`fetch`/`push` over the pack protocol (a local
`git-upload-pack` spawned via `sh -c`, or the HTTP helper) still do not
complete on this kernel. They spawn a helper and hold a *bidirectional*
pipe conversation across two or three processes; that multi-process
pipe/poll IPC hits a deeper scheduler lost-wake interaction beyond the
single-child EOF case fixed above — the same "its own kernel project"
concurrency area as the links2-HTTPS hang. The transport is wired and
reachable (`git-upload-pack`/`git-receive-pack`/`git-upload-archive`
resolve to `GIT.ELF` via `argv[0]` dispatch; `git-remote-http{,s}` →
`GITRHTTP.ELF`; `tools/git-fixture` is a committed dumb-HTTP repo), so
the clone tests can be switched on once that lands. Everything
in-process — init, add, commit, branch, checkout, merge, log, diff,
status, cat-file, rev-parse, config, fsck's non-forking checks — works.

## Out of scope

- **ssh transport** — no ssh client on the system; `git@github.com:` URLs
  will fail with a clear "helper not found". Document HTTPS as the way.
- **Interactive-zsh signal robustness** — see the note above; a separate
  kernel signal-delivery fix, not part of this port.
- Threads (`pack.threads > 1`), IPv6, gitweb/gitk/git-gui, perl-based
  commands, fsmonitor, sparse-checkout niceties, credential helpers
  (public HTTPS clone needs none; pushes to real forges need a PAT typed
  into the URL until a credential story exists).

## Risks

- **Subprocess churn**: git forks itself and helpers frequently (hooks,
  pager, transport). Each exec reloads a multi-MiB static binary.
  Marginally slower than Linux but bounded; `SKIP_DASHED_BUILT_INS`
  and builtin dispatch keep the common porcelain in-process.
- **Loose-object `link()` finalize on overlay** — first write path to
  exercise `link()` under `overlay(tmpfs, FAT)`. Mitigation is one
  config line (`core.createObject = rename`); test both.
- **Index mtime races on FAT-backed `/`** — racy-git handling copes but
  costs rehashing; guidance (repos on `/data`/`/work`) sidesteps it.
- **Upstream build probes** — git's Makefile occasionally wants
  `uname -s`-conditional knobs when cross-compiling; the config.mak
  variable set above is the standard musl-static recipe used by Alpine
  and sabotage-linux, so surprises should be shallow.
