---
title: "feat: port Links2 as a text-mode static-musl userland web browser"
type: feat
status: active
date: 2026-07-18
depth: large
related_docs:
  - docs/plans/2026-05-16-001-feat-prebuilt-userland-elfs-plan.md
  - docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md
  - docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md
  - docs/plans/2026-07-18-003-feat-tinycc-port-plan.md
  - src/userland/CLAUDE.md
  - src/net/CLAUDE.md
  - userland/prebuilt/README.md
---

# feat: port Links2 as a text-mode static-musl userland web browser

## Summary

Port upstream Links 2.30 as a statically linked musl `ET_EXEC` and expose it
as both `/bin/links` and `/bin/links2`. The first shipped milestone is the
interactive text-mode browser over IPv4 HTTP, running inside the existing
AgenticOS terminal. It deliberately excludes graphics, TLS, IPv6, JavaScript,
and optional content-compression libraries.

The port is not only a packaging task. Links' core loop waits in `select(2)`
over its terminal, internal pipes, timers, and TCP sockets. AgenticOS has no
`select(2)`, `pselect6(2)` is an `ENOSYS` stub, and today's `poll(2)` only has
real readiness for sockets. It reports stdin and pipes as immediately ready,
which is sufficient for zsh's narrow use but would make a browser either block
on the wrong fd or spin. Links also marks its signal and helper pipes
`O_NONBLOCK`, while AgenticOS currently ignores that flag for pipes.

End state from zsh:

```sh
links http://agenticos-http.test:8081/
links2 http://10.0.2.101:8081/
links -dump http://agenticos-http.test:8081/
```

The browser can navigate links and forms with the existing xterm key encoder,
resize with its terminal, resolve DHCP-provided DNS names, download to a
writable filesystem, and retain configuration/bookmarks under `/root/.links`
when the overlay is synchronized.

Upstream references used by this plan:

- [Links download page](https://links.twibright.com/download.php) — current
  upstream release and build instructions.
- [Links user documentation](https://links.twibright.com/user_en.html) — text
  mode is the base build; graphics and SSL are optional capabilities.
- [Links project site](https://links.twibright.com/about.php) — project and
  GPLv2-or-later licensing information.

---

## Current state and feasibility findings

### Platform pieces already present

- Static, non-PIE musl `ET_EXEC` programs run through the existing ELF loader,
  fork/exec/wait platform, demand-paged address spaces, signals, PTYs, and
  scheduler.
- The userland manifest and committed-prebuilt workflow already handle pinned
  upstream C applications. `LINKS.ELF` fits the same `prebuilt-managed` model
  as `ZSH.ELF`, `BB.ELF`, and `TCC.ELF`.
- The terminal supplies raw-mode xterm sequences for arrows, Home/End,
  PageUp/PageDown, Insert/Delete, F1-F12, modifiers, and UTF-8 input. PTY
  winsize updates and `SIGWINCH` are already wired.
- IPv4 DHCP, DNS, TCP, nonblocking connect, socket readiness, `SO_ERROR`,
  `TCP_NODELAY`, and blocking/restart behavior exist. BusyBox wget already
  proves numeric and hostname HTTP against repository-owned QEMU endpoints.
- Writable tmpfs overlay paths and persistent ext2 `/data` are available.
  Overlay state can be persisted through `sync(2)`.

### Upstream build baseline

Use Links 2.30 from:

```text
https://links.twibright.com/download/links-2.30.tar.bz2
SHA256 c4631c6b5a11527cdc3cb7872fc23b7f2b25c2b021d596be410dadb40315f166
```

The baseline configuration is intentionally minimal:

```sh
./configure \
  --host=x86_64-linux-musl \
  --disable-graphics \
  --without-ipv6 \
  --without-libevent \
  --without-gpm \
  --without-ssl \
  --without-zlib \
  --without-brotli \
  --without-zstd \
  --without-bzip2 \
  --without-lzma
```

Compile and link with `-O2 -fno-pie` and `-static -no-pie`. This produces one
self-contained executable; no terminfo tree or runtime resource directory is
required for text mode.

Links normally detects pthreads on musl and uses a detached pthread plus a pipe
for asynchronous DNS and other background helpers. AgenticOS does not provide
a general pthread ABI (`clone`, TLS-per-thread, futexes). Upstream already has
an alternate `fork` + pipe implementation of `start_thread`; apply one small
source patch that selects that backend when `AGENTICOS_NO_PTHREADS` is defined.
Do not add a partial pthread implementation for this port.

### Confirmed OS gaps

1. **No usable fd-set multiplexer.** Linux syscall 23 (`select`) is not in the
   dispatcher and `pselect6` returns `ENOSYS`. `poll` has correct socket state
   but treats all other valid fds as immediately ready.
2. **Pipe nonblocking state is discarded.** `F_SETFL` is a success no-op for
   pipes, and pipe reads/writes block rather than returning `EAGAIN`. Links
   makes its signal pipe, terminal pipe, and helper pipes nonblocking.
3. **No general mixed-I/O block reason.** A process can wait for network,
   stdin, or a pipe separately, but `select` must wake when *any* member of a
   set becomes ready or its deadline expires.
4. **`/root` is not provisioned.** Kernel-launched processes receive
   `HOME=/root`, but only `/work` is created at boot. Links needs a writable
   home for `.links/links.cfg`, history, cookies, and bookmarks.
5. **The fd limit is tight.** Each process has 32 descriptors. This is enough
   for the initial low-concurrency browser but must be tested under DNS plus
   several HTTP connections rather than assumed.

### Important non-gaps

- Links' `AF_UNIX` single-instance rendezvous is optional. On AgenticOS,
  `socket(AF_UNIX, ...)` returns `EAFNOSUPPORT`, upstream falls back to a new
  local instance, and `-no-connect` avoids the probe entirely. AF_UNIX is not
  part of this plan.
- Text-mode Links does not need framebuffer, GUI, image decoder, mouse, or
  clipboard syscalls.
- Disabling TLS keeps the first browser milestone independent of the
  cryptographic random broker that has since landed on `main`.

---

## Goals

1. Build Links 2.30 reproducibly from a SHA256-pinned upstream archive as a
   static-musl `LINKS.ELF`, commit it under `userland/prebuilt/`, and stage it
   on ordinary builds without requiring network or a cross toolchain.
2. Expose `/bin/links` and `/bin/links2` as aliases for the direct ELF.
3. Implement correct bounded `select(2)` readiness and blocking for stdin,
   stdout/stderr, regular files, pipes, and IPv4 sockets; share the readiness
   engine with `poll(2)` and `ppoll(2)`.
4. Implement `O_NONBLOCK` for pipe open-file descriptions and report accurate
   pipe readiness/EOF/error state.
5. Preserve timeout deadlines across restarted blocking syscalls and wake a
   mixed-fd waiter on terminal input, pipe transitions, network transitions,
   timeout, close, or a deliverable signal.
6. Provision `/root` idempotently so Links can create `/root/.links` and save
   settings/bookmarks on the writable overlay.
7. Prove numeric HTTP, DNS-backed HTTP, terminal interaction, resize, and
   configuration writes in QEMU with bounded tests.

## Non-goals

- **HTTPS/TLS.** Cryptographic entropy is now available, but Links still needs
  a reviewed TLS library build, CA roots, hostname verification, trusted-time
  policy, syscall discovery, and HTTPS-specific QEMU fixtures.
- `/dev/random`, `/dev/urandom`, CSPRNG design, entropy collection, reseeding,
  or changing `getrandom(2)`; those belong to the landed entropy subsystem.
- Graphics mode, a `/dev/fb0` compatibility device, X11, image rendering,
  native ring-3 GUI integration, or mouse support.
- IPv6, AF_UNIX, multi-terminal attachment to one Links process, libevent, or
  pthread support.
- JavaScript. Modern Links 2.x does not ship a supported JavaScript engine.
- CSS compatibility beyond what upstream Links provides.
- Optional zlib/brotli/zstd/bzip2/lzma response decoding in the first merge.
- Raising the global/per-process fd limit unless measured tests show the
  32-descriptor cap prevents the scoped workload.
- General epoll/kqueue/eventfd APIs.

---

## Design

### Runtime layout

```text
/host/LINKS.ELF                  committed static-musl browser
/bin/links                      synthetic direct-app rewrite -> LINKS.ELF
/bin/links2                     alias -> LINKS.ELF
/root                           writable overlay directory, boot-provisioned
/root/.links/                   created and managed by Links
  links.cfg
  links.his
  bookmarks.html
/work                           optional download/scratch destination
/data                           persistent ext2 destination when requested
```

The process inherits the existing default environment, notably
`HOME=/root`, `TERM=xterm-256color`, `LANG=C.UTF-8`, and `PATH=/bin:/host`.
No Links-specific launcher or environment injection is needed.

### Build and patch policy

Add `userland/apps/links2/` with:

```text
Makefile
README.md
.gitignore
patches/
  0001-use-fork-helper-without-pthreads.patch
```

The Makefile follows the zsh/TinyCC pattern:

- download the exact 2.30 bzip2 archive and hard-fail on SHA mismatch;
- extract into `build/` and apply patches in lexical order;
- cross-configure with the minimal options above;
- define `AGENTICOS_NO_PTHREADS` so `start_thread` takes upstream's existing
  `fork` + pipe backend even though musl provides pthread symbols;
- build with `x86_64-linux-musl-gcc`, strip with the paired strip tool, and
  emit `build/links`;
- verify `ET_EXEC`, no `PT_INTERP`, x86-64 machine, and static linkage before
  the manifest accepts the artifact;
- record the configure capability summary in the build log and assert that
  graphics, IPv6, SSL, libevent, and compression backends are absent.

The patch must only alter backend selection. Do not fork upstream DNS logic or
carry an AgenticOS-specific resolver implementation inside Links.

Manifest row:

```text
app_row links2 apps/links2 make LINKS.ELF prebuilt-managed musl-cc apps/links2/build/links prebuilt/LINKS.ELF
```

`./userland/refresh-prebuilt.sh` then refreshes `LINKS.ELF` through the generic
manifest path. Add `REBUILD_LINKS2=1` examples to the relevant READMEs.

### Unified readiness model

Introduce one internal readiness snapshot used by `select`, `poll`, and
`ppoll`:

```text
FdReady { readable, writable, error, hangup }
```

Class semantics:

| FD class | Readable | Writable | Error/hangup |
|---|---|---|---|
| stdin | PTY slave queue is non-empty | false | terminal detached/dead |
| stdout/stderr | false | PTY master queue has capacity | terminal detached/dead |
| regular/virtual file | true (read reaches data or EOF immediately) | writable file call will not block | underlying fd invalid only |
| directory | true | false | invalid only |
| pipe read end | buffered bytes exist, or no writers (EOF) | false | no separate error |
| pipe write end | buffer has capacity, or no readers | true when capacity exists | no readers maps to poll error and write `EPIPE` |
| socket | existing `socket::readiness` | existing `socket::readiness` | existing error/hangup state |

Readiness inspection must not consume data or hold the process-table lock
while taking the network lock. Snapshot fd identities/handles first, release
the fd-table/process lock, poll the network once, then inspect handles. Keep
the lock-order rules in `src/net/CLAUDE.md` intact.

### `select(2)` ABI

Add Linux x86-64 syscall 23 and implement:

```c
int select(int nfds, fd_set *readfds, fd_set *writefds,
           fd_set *exceptfds, struct timeval *timeout);
```

- Accept `0 <= nfds <= FD_TABLE_SIZE`; reject larger values with `EINVAL`.
- Treat each `fd_set` as Linux's bit array and validate only the bytes needed
  for `nfds`, using checked arithmetic and normal usercopy.
- Invalid set descriptors return `EBADF` (unlike `poll`, which reports
  `POLLNVAL`).
- Clear non-ready bits and return the total number of set bits across the
  three output sets, matching Linux rather than counting unique fds.
- Use error/hangup as readable/writable where POSIX requires the pending
  operation not to block; use the except set only for actual exceptional
  state supported by the socket layer (initially none).
- `timeout == NULL` waits indefinitely. A zero timeval polls. Positive
  timeouts round up to the 100 Hz PIT and use an absolute deadline so syscall
  restart cannot extend them.
- On a deliverable signal, wake and return through the existing `EINTR`
  delivery path. Links' signal handler writes its nonblocking signal pipe;
  the next loop iteration drains that pipe.

Implement real `pselect6(2)` on top of the same engine only to the extent
needed by libc/Links: timespec timeout validation and normal fd sets. If a
non-null temporary signal mask is observed in discovery, implement the atomic
mask swap/restore before declaring it supported; do not silently ignore it.
If Links 2.30's pinned musl binary only issues syscall 23, `pselect6` can remain
a separately tracked follow-up, but it must no longer be described as part of
the Links acceptance surface.

### Blocking and wakeups

Add a general wait reason, for example:

```text
WaitingForIo { deadline_tick: Option<u64> }
```

and a restart-stable per-process wait record keyed by syscall number plus a
stable identity derived from the copied fd sets/pollfds. The record owns the
absolute deadline. It contains no user pointers and no fd-table/network locks
survive a yield.

Wake mixed-I/O waiters conservatively on:

- PTY input arrival or terminal teardown;
- pipe write/read/last-reader/last-writer transitions;
- network worker state changes and deferred socket close;
- fd close/dup replacement in the waiting process;
- the timer service reaching the stored deadline;
- a deliverable signal.

On wake, the SYSCALL instruction re-fires and recomputes exact readiness. This
matches the existing network and pipe strategy and makes spurious wakeups safe.
Do not busy-poll the browser at 100 Hz when no fd is ready and no timer is due.

Once this works, refactor `poll` and `ppoll` to use the same readiness and wait
record. Remove the current `Some(_) => requested events are ready` shortcut.
That prevents future callers from seeing a second, inconsistent readiness
model.

### Pipe status flags

Move `O_NONBLOCK` into the pipe open-file description so dup/fork share it,
while `FD_CLOEXEC` stays descriptor-local. The cleanest shape is status state
on each pipe endpoint handle (or shared endpoint state), not a boolean on each
`FdSlot` clone.

- `F_GETFL` returns the correct access mode plus `O_NONBLOCK`.
- `F_SETFL` changes only supported status bits and preserves access mode.
- Empty nonblocking pipe read with live writers returns `EAGAIN`.
- Full nonblocking pipe write with live readers returns `EAGAIN`.
- Empty pipe with no writers returns EOF even in nonblocking mode.
- Write with no readers returns `EPIPE` and preserves existing signal policy.
- Readiness reflects buffer length/capacity and endpoint counts exactly.

This is required for Links' signal pipe to remain safe: a signal handler must
never park forever trying to write a full notification pipe.

### Home/config persistence

Provision `/root` next to `/work` during boot, after overlay hydration:

```text
mkdir /root; ignore AlreadyExists; log other errors without panicking
```

Links creates `.links` and its files itself. Do not bake mutable configuration
into `/etc` or `/host`. Tests use a test-specific home such as `/work/links-home`
to avoid changing a developer's interactive overlay state; manual use keeps
the normal `/root/.links` location. `sync(2)` persists it through the existing
overlay mechanism.

### Entropy and future HTTPS boundary

This plan does not modify the cryptographic random broker now present on
`main`. That subsystem already provides:

1. a cryptographically secure kernel RNG with explicit initialized/ready
   semantics;
2. correct blocking/nonblocking device behavior for the chosen random device
   paths;
3. `getrandom(2)` backed by that same secure source with correct flags and
   arbitrary-length reads.

With that prerequisite landed, a separate HTTPS plan can add a pinned static
OpenSSL build, certificate trust strategy, Links `--with-ssl`, TLS syscall
discovery, static-musl/OpenSSL entropy compatibility, HTTPS QEMU fixtures,
certificate validation tests, and binary-size review.

---

## Implementation units

### U0. Build-only port and syscall discovery

**Goal:** Produce a pinned static `LINKS.ELF` outside the normal boot path and
measure its actual AgenticOS behavior before kernel compatibility changes.

**Files:**

- `userland/apps/links2/Makefile` — new
- `userland/apps/links2/README.md` — new
- `userland/apps/links2/.gitignore` — new
- `userland/apps/links2/patches/0001-use-fork-helper-without-pthreads.patch` — new

**Work:**

1. Fetch/hash/configure/build the baseline binary.
2. Prove the fork helper backend is compiled and no graphics/TLS/thread worker
   path is active.
3. Stage the binary temporarily and run progressively:
   `-version`, `-dump file:///host/...`, numeric HTTP dump, hostname HTTP dump,
   then interactive text mode with syscall trace enabled.
4. Record every unknown syscall and every implemented syscall returning an
   approximation that violates Links' expectations. Update U1/U2 scope with
   evidence before adding handlers.

**Exit bar:** static `ET_EXEC` builds reproducibly; startup reaches the known
`select`/pipe gaps rather than failing in loader/libc initialization.

### U1. General fd readiness and `select(2)`

**Goal:** Correctly block on mixed terminal/pipe/socket fd sets and deadlines.

**Likely files:**

- `src/userland/abi.rs`
- `src/userland/syscalls.rs` (or new `src/userland/readiness.rs` if the shared
  code would otherwise make the syscall file harder to audit)
- `src/userland/fdtable.rs`
- `src/userland/lifecycle.rs`
- `src/net/socket.rs`
- `src/terminal/pty.rs`
- `src/userland/stdin.rs`
- timer/wake call sites identified during implementation
- focused tests under `src/tests/userland/`

**Tests:**

- zero-timeout `select` clears non-ready bits;
- finite no-fd timeout returns near the requested PIT deadline;
- stdin is unreadable before input and readable after input;
- pipe read end transitions empty -> readable -> empty -> EOF-readable;
- pipe write end transitions writable -> full -> writable, and detects no
  readers;
- socket connect/read/write/error readiness maps correctly;
- one wait containing stdin + pipe + socket wakes for each class in separate
  cases;
- invalid fd is `EBADF`; null fd-set pointers are accepted;
- signal interruption yields `EINTR` without extending a later retry timeout;
- fd close and process exit cannot leave a parked waiter orphaned;
- existing zsh/BusyBox poll behavior still passes after unification.

**Exit bar:** a test process can idle in `select` with no CPU spin, then wake
for either a keypress or an HTTP response.

### U2. Nonblocking pipes and Links helper compatibility

**Goal:** Make Links' signal/terminal/DNS helper pipes behave like POSIX pipes.

**Likely files:**

- `src/userland/pipe.rs`
- `src/userland/fdtable.rs`
- `src/userland/syscalls.rs`
- `src/userland/lifecycle.rs`
- pipe/fcntl tests under `src/tests/userland/`

**Tests:**

- `F_SETFL(O_NONBLOCK)` is visible through dup and fork;
- cloexec remains per-fd;
- empty nonblocking read and full nonblocking write return `EAGAIN`;
- EOF and `EPIPE` take priority over `EAGAIN`;
- blocking pipe behavior and existing zsh pipelines do not regress;
- a full signal pipe write does not block the signal handler.

Run Links' `-dump` numeric-IP path after this unit. It should complete without
unknown syscall spam, deadlock, or a hot loop.

### U3. Packaging, namespace, and writable home

**Goal:** Make the browser a normal shipped command with mutable user config.

**Files:**

- `userland/apps.manifest.sh`
- `userland/prebuilt/LINKS.ELF` — generated/committed
- `userland/prebuilt/README.md`
- `userland/refresh-prebuilt.sh` comments/examples as needed
- `src/userland/bin_namespace.rs`
- `src/kernel.rs`
- namespace/filesystem tests

**Work:**

- add the prebuilt-managed manifest row;
- add sorted direct applets `links` and `links2`, both mapped to
  `/host/LINKS.ELF` while preserving invoked `argv[0]`;
- provision `/root` idempotently;
- update project orientation docs to name Links and state HTTP-only scope.

**Tests:**

- manifest staging works with and without the musl cross toolchain;
- `REBUILD_LINKS2=1` rebuilds only the browser;
- ELF validation rejects dynamic/PIE artifacts;
- `/bin` listing/stat/access/exec rewrite covers both aliases;
- `/root` is writable after fresh boot and after overlay hydration;
- Links can create config, history, and bookmarks in a test HOME.

### U4. HTTP and DNS integration

**Goal:** Prove useful browser behavior against deterministic QEMU services.

**Files:**

- extend `tools/net-test-http.py` or add a dedicated bounded Links fixture;
- `src/tests/network_userland.rs` or a new `src/tests/links2.rs`;
- test registration and app documentation.

**Fixture pages:**

- a small HTML page with title, heading, relative link, UTF-8 text, and form;
- a linked second page;
- redirect response;
- a downloadable payload larger than one TCP receive buffer;
- a slow/chunked response to exercise repeated readiness wakeups;
- a response without compression (the first build advertises none).

**Automated acceptance:**

1. `links -dump http://10.0.2.101:8081/` writes expected normalized text to a
   file and exits 0.
2. The same dump through `http://agenticos-http.test:8081/` proves musl DNS.
3. Redirect and relative-link cases produce expected content.
4. A download to `/work` is byte-exact and does not exhaust fds.
5. A bounded slow response proves the process sleeps between readiness events.
6. All waits have watchdog deadlines; missing `LINKS.ELF` is a test failure,
   not a skip, once the artifact is committed.

**Manual interactive acceptance:**

- launch `links` from zsh and enter a URL;
- navigate with arrows/Tab/Enter, go back, search, open menus, and quit;
- type into and submit a form;
- resize the Terminal window and verify a clean redraw after `SIGWINCH`;
- leave a slow page idle and confirm zsh/GUI/network workers remain responsive;
- save a bookmark/config option, quit, restart, and confirm it reloads;
- boot with `AGENTICOS_NETWORK=off` and confirm a bounded, recoverable error.

### U5. Regression and documentation closeout

**Goal:** Merge with platform-wide confidence and a precise feature boundary.

**Verification:**

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland '<new readiness modules>'
./test.sh --skip-userland network network_userland
./test.sh --skip-userland '<pipe/fcntl modules>'
./test.sh links2 network_userland
./test.sh --skip-userland
./build.sh -n
```

Also run one rebuild from source:

```sh
REBUILD_LINKS2=1 ./build.sh -n
```

Update `CLAUDE.md`, `README.md`, and `userland/prebuilt/README.md` with:

- command names and HTTP-only status;
- rebuild/refresh workflow;
- Links source version/hash and local patch purpose;
- known lack of TLS, graphics, IPv6, JS, and compression;
- explicit handoff to the TLS trust-stack follow-up.

---

## Dependency order

```text
U0 pinned build + discovery
  -> U1 mixed-fd readiness/select
      -> U2 nonblocking helper pipes
          -> U3 committed packaging + /bin + /root
              -> U4 deterministic HTTP/DNS acceptance
                  -> U5 full regression/docs

Landed entropy subsystem
  -> secure getrandom + random devices proven
      -> future HTTPS/OpenSSL trust-stack plan (not this plan)
```

U1 and U2 may be developed in parallel only after U0 records the exact pinned
binary's calls, but both must land before interactive acceptance. U3 packaging
can be prepared earlier; the committed artifact should not be advertised as a
working browser until U1/U2 are complete.

---

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Readiness checks take process and network locks in the wrong order | Single-CPU deadlock | Snapshot handles under process lock, drop it, then inspect network; add lock-order comments/tests. |
| Wake occurs between readiness check and park | Browser sleeps forever | Keep the check-to-park transition under the existing interrupt/preemption discipline; recheck immediately before committing blocked state. |
| Restarted `select` recomputes a relative timeout | Slow responses never time out | Store one absolute PIT deadline keyed to syscall identity. |
| Links accidentally uses pthread backend | `clone`/futex failure on first DNS request | Compile-time backend macro, source/config assertion, hostname integration test. |
| Nonblocking flag is per descriptor instead of open-file description | dup/fork observe inconsistent behavior | Store status on shared pipe endpoint state; keep cloexec in `FdSlot`. |
| 32 fds are insufficient | EMFILE during parallel fetch/DNS | Test measured peak; lower Links connection limits first, then raise the bounded table only with evidence. |
| Missing compression limits public sites | Some content cannot display | HTTP milestone uses correct negotiation and deterministic uncompressed fixtures; add zlib in a later small plan. |
| Public HTTP redirects to HTTPS | Manual browsing appears broken | Document HTTP-only scope and ship a reliable local/demo HTTP endpoint; HTTPS waits for the TLS trust-stack follow-up. |
| Writable browser state contaminates tests | Nondeterministic results | Automated tests override HOME to a unique `/work` directory and clean it up. |
| Large/fuzzed HTML stresses memory | Browser or OS OOM | Keep Links cache/connection defaults bounded, test a large page, and document current memory limits. |

---

## Done criteria

- `LINKS.ELF` is reproducibly built from the pinned 2.30 archive, committed,
  statically validated, and staged from the manifest.
- `/bin/links` and `/bin/links2` both execute it from a stock checkout.
- The browser spends idle time blocked, not spinning, while simultaneously
  waiting for terminal input, pipes, timers, and sockets.
- Pipe `O_NONBLOCK` behavior is correct across dup/fork and does not regress
  zsh pipelines.
- Numeric and DNS-backed HTTP dumps pass in restricted QEMU; interactive text
  browsing, navigation, forms, resize, and downloads work manually.
- Settings/bookmarks can be written beneath `/root/.links` and reload after
  restart (and after overlay sync when persistence is requested).
- Existing kernel, network, terminal, zsh, BusyBox, and TinyCC tests pass.
- No TLS, entropy-device, graphics, IPv6, JavaScript, or pthread work is hidden
  in the implementation; those boundaries remain documented.
