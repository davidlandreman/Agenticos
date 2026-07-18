---
title: "feat: port curl as a static-musl userland app"
type: feat
status: implemented
date: 2026-07-18
depth: medium
related_docs:
  - CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - userland/prebuilt/README.md
  - userland/apps/links2/README.md
  - docs/plans/2026-07-18-009-feat-links2-https-support-plan.md
  - docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md
  - https://curl.se/download.html
  - https://curl.se/docs/install.html
---

# feat: port curl as a static-musl userland app

## Summary

Ship real curl as `CURL.ELF`, a pinned static-musl binary reachable as
`/bin/curl`, with IPv4 HTTP and verified HTTPS. This is **not** a BusyBox
change: BusyBox has no curl applet at all — its closest relative is the
`wget` applet, which this repo deliberately keeps HTTP-only. curl therefore
lands as a standalone prebuilt-managed port, following the exact delivery
model Links2 established:

```text
pinned curl tarball (SHA256)          pinned OpenSSL 3.5.7 + zlib
            |                                    |
   x86_64-linux-musl-gcc, static, non-PIE, IPv4-only
            |
        CURL.ELF  →  userland/prebuilt/CURL.ELF  →  /host/CURL.ELF
            |
   bin_namespace: /bin/curl  →  zsh: curl https://example.com
```

Everything curl needs at runtime already exists because Links2 HTTPS built
it: the kernel-managed `/etc/ssl/cert.pem` Mozilla trust snapshot, DHCP-backed
musl DNS resolution, `getrandom(2)`-fed OpenSSL entropy, RTC-derived
`CLOCK_REALTIME` for certificate validity, and `poll(2)`/`ppoll(2)` syscall
handlers for curl's event loop. The port is chiefly a build recipe plus
integration glue, not new kernel capability.

Deliberate scope for the first cut:

- Protocols: HTTP and HTTPS only. FTP, SMTP/IMAP/POP3, TELNET, TFTP, DICT,
  GOPHER, MQTT, RTSP, SMB, LDAP, WS are all disabled at configure time.
- HTTP/1.1 only (`--without-nghttp2`, no HTTP/3). Matches Links2.
- IPv4 only (`--disable-ipv6`) — the network stack has no IPv6.
- Synchronous resolver (`--disable-threaded-resolver`). This matches the
  single-threaded (`no-threads`) OpenSSL build profile and avoids depending
  on the young pthread runtime for a core tool.
- `tool` + `libcurl` static; no shared libs, no `curl-config` on target.

## Current-state evidence

- `userland/apps/links2/Makefile` already contains a pinned, verified fetch
  and static single-threaded build of OpenSSL 3.5.7 (`no-dso no-quic
  no-threads`, `OPENSSLDIR=/etc/ssl`) and zlib — the same recipe curl needs.
- `src/userland/etc.rs` publishes `/etc/ssl/cert.pem` (Mozilla snapshot,
  withheld when the RTC wall clock is invalid) and DHCP-backed
  `/etc/resolv.conf`; musl `getaddrinfo` resolution is proven by Links2 and
  BusyBox `nslookup`.
- `src/userland/syscalls.rs` has `poll_handler` and `ppoll_handler`
  (curl's `Curl_poll` uses `poll`), plus `select(2)`, non-blocking sockets,
  and the socket-option surface exercised by Links2 and `NETTEST.ELF`.
- `src/userland/bin_namespace.rs` maps command names to `/host/*.ELF`
  (`links`/`links2` → `LINKS.ELF`); adding `curl` is one table row plus tests.
- `userland/apps.manifest.sh` drives `build.sh`, `test.sh`,
  `refresh-prebuilt.sh`, and the `REBUILD_<APP>=1` overrides from one
  `app_row` line per app.
- Restricted-QEMU test infrastructure from the Links2 HTTPS plan provides
  repository-owned certificates and `guestfwd` HTTP/TLS services — no public
  internet in tests.

## Design

### Version pinning

Pin the current stable curl release at implementation time (8.x line),
recording tarball URL and SHA256 in the Makefile exactly as Links2 pins
OpenSSL. Updates are deliberate: bump the pin, rebuild, commit the refreshed
prebuilt.

### Build recipe (`userland/apps/curl/Makefile`)

Self-contained, mirroring `apps/links2/Makefile` structure (fetch → verify →
deps → configure → build → stage):

1. Fetch and SHA256-verify curl, zlib, and OpenSSL 3.5.7 tarballs into the
   shared tarball cache.
2. Build zlib and OpenSSL into a private `DEPS_PREFIX` using the **identical
   configure profile Links2 uses** (`linux-x86_64`, static, `no-dso no-quic
   no-threads`, `--openssldir=/etc/ssl`). Copying the recipe keeps each app
   self-contained per the established pattern; if a third OpenSSL consumer
   appears later, factor a shared deps Makefile then.
3. Configure curl:

   ```sh
   ./configure --host=x86_64-linux-musl \
     --disable-shared --enable-static \
     --with-openssl=$(DEPS_PREFIX) --with-zlib=$(DEPS_PREFIX) \
     --with-ca-bundle=/etc/ssl/cert.pem --without-ca-path \
     --disable-ipv6 --disable-threaded-resolver --disable-unix-sockets \
     --disable-ftp --disable-file --disable-ldap --disable-ldaps \
     --disable-rtsp --disable-dict --disable-telnet --disable-tftp \
     --disable-pop3 --disable-imap --disable-smtp --disable-gopher \
     --disable-mqtt --disable-smb --disable-websockets \
     --disable-ntlm --disable-manual --disable-docs \
     --without-libpsl --without-libidn2 --without-nghttp2 \
     --without-brotli --without-zstd --without-librtmp
   ```

   with `CC=x86_64-linux-musl-gcc`, `LDFLAGS=-static -no-pie`, `-Os`, and a
   post-link `strip`. (`--disable-file` may be revisited; `file://` is
   harmless but widens surface for no demonstrated need.)

   Note `--without-libpsl`: curl only uses the public-suffix list to scope
   cookies; the tool's cookie engine is off unless asked for, and vendoring
   libpsl is not worth it for a first cut.
4. Sanity-assert the result in the Makefile the way Links2 does: static
   `ET_EXEC`, `curl --version` output lists `https` and `SSL`, and the
   compiled-in CA bundle path is `/etc/ssl/cert.pem`
   (`curl-config --ca` equivalent via `strings` or configure log grep).
5. Stage `build/curl` → the manifest copies it to `host_share/CURL.ELF`.

Add a `patches/` directory only if the syscall-gap probe (below) demands it;
expectation is zero patches — curl's syscall footprint is a strict subset of
what Links2 + BusyBox already exercise, with `poll` the only notable addition
and it is already implemented.

### Kernel integration

- `src/userland/bin_namespace.rs`: add `CURL_HOST_PATH = "/host/CURL.ELF"`,
  a `"curl"` entry in the listing table, and the `"curl" => CURL_HOST_PATH`
  rewrite arm; extend the existing unit tests.
- `userland/apps.manifest.sh`: one row —
  `app_row curl apps/curl make CURL.ELF prebuilt-managed musl-cc
  apps/curl/build/curl prebuilt/CURL.ELF`. This automatically wires
  `REBUILD_CURL=1`, `--rebuild-userland`, and `refresh-prebuilt.sh`.
- `userland/prebuilt/README.md`: add the table row (expect ~3–4 MiB
  stripped; total prebuilt size stays well under the 50 MiB LFS threshold).
- No new syscalls, no kernel dialog/GUI work, no theme work.

### Fail-closed HTTPS posture (inherited, verify, don't reinvent)

curl's verification defaults are already correct (`--cacert` default on,
hostname + chain + validity checks). The AgenticOS-specific behaviors come
free from the existing `/etc` machinery and must be asserted, not built:

- RTC invalid → `/etc/ssl/cert.pem` absent → curl HTTPS fails with a
  verify-locations error. No silent insecure fallback.
- `curl -k` remains available as an explicit, user-typed override — same
  stance as a normal Linux system; no patch to remove it.
- Entropy unavailable → OpenSSL init fails → HTTPS unavailable.

## Implementation phases

1. **Probe build** — Write the Makefile, build `CURL.ELF`, boot it manually
   (`curl --version`, numeric-IP HTTP against the restricted-QEMU service).
   Chase any unexpected syscall gap here before committing to anything.
2. **Integration** — bin_namespace + manifest + prebuilt refresh
   (`./userland/refresh-prebuilt.sh`, commit binary alongside source).
3. **Tests** — Extend the userland test module (per `src/tests/CLAUDE.md`)
   reusing the Links2 restricted-QEMU HTTP/TLS fixtures:
   - `curl --version` reports https/ssl;
   - HTTP GET by numeric IP and by DNS name, body written to `/work` and
     verified;
   - `-L` follows a redirect;
   - HTTPS GET against the repository-owned valid cert succeeds;
   - mismatched-host and expired certs are rejected with nonzero exit;
   - `-k` succeeds against the mismatched cert (proves the override path);
   - exit codes surface correctly through zsh (`$?`).
4. **Docs** — CLAUDE.md current-state blurb (curl alongside the BusyBox
   `wget remains HTTP-only` sentence), `userland/apps/curl/README.md`
   (upstream version, configure profile, license note — curl license is
   MIT-like; include `COPYING` as Links2 does OpenSSL's).

## Alternatives considered

- **BusyBox curl applet** — does not exist upstream; not an option.
- **Enable TLS in BusyBox `wget`** — BusyBox's internal TLS does no real
  certificate verification; shipping it would undercut the fail-closed HTTPS
  posture the Links2 plan established. Rejected.
- **wolfSSL/mbedTLS backend for size** — a second TLS stack to pin, audit,
  and feed entropy; OpenSSL 3.5.7 is already pinned, built, and qualified
  here. Rejected.

## Implementation notes (post-landing)

- Pinned curl 8.21.0 (2026-06-24), `sha256
  d9b327997999045a24cda50f3983e69e51c516bd8be6ef9842fc7f99135e33bb`.
  Stripped static binary is ~5.6 MiB.
- The one real surprise: curl links its tool through **libtool**, which
  silently drops a plain `-static` from `LDFLAGS` — the first build produced
  an ET_EXEC with `PT_INTERP` and `DT_NEEDED libc.so`. The fix is
  `make CURL_LDFLAGS_BIN=-all-static` (a make-time variable consumed by
  `src/Makefile.am`, not a configure variable). The Makefile now hard-fails
  on any `PT_INTERP`/`DT_NEEDED` in the output so this cannot regress.
- `--without-librtmp` no longer exists in curl 8.21 (librtmp support was
  removed upstream) and was dropped from the configure profile.
- One real syscall gap surfaced at runtime: libcurl's multi-API wakeup
  descriptor wants `eventfd2` (nr 290, `-ENOSYS` here) with an AF_UNIX
  `socketpair` fallback (also unavailable), and the failure cascades into
  curl exit 27. `--disable-socketpair` removes `ENABLE_WAKEUP` entirely
  (`lib/multihandle.h`), which is correct for the single-threaded tool; the
  Makefile asserts `CURL_DISABLE_SOCKETPAIR` in `curl_config.h`. No kernel
  change was needed.
- While validating, the Links2 HTTPS tests turned out to fail on this
  machine at their introducing commit (`e831db7`) — a pre-existing,
  timing-dependent kernel bug where SIGCHLD from Links' DNS fork helper,
  delivered while Links is blocked in `select(2)`, corrupts the
  `rt_sigreturn` context (observed ring-3 jump to a kernel-half address).
  curl's synchronous in-process resolver never forks, so curl avoids that
  path entirely. Fixing the signal/blocked-select interaction is follow-up
  kernel work, not part of this port.
- Coverage landed in `src/tests/network_userland.rs` reusing the Links2
  fixtures, including a `-k`-overrides-mismatch test and verification-exit-code
  (60) assertions on the rejection paths.

## Risks

- **Configure under musl-cross on macOS host** — curl's autotools are
  well-behaved for `--host=x86_64-linux-musl` static builds (widely done for
  curl-static distributions); Links2 and binutils prove the toolchain path.
  Low risk, contained to phase 1.
- **Hidden syscall dependence** (e.g. `getpeername`, `SO_ERROR` after
  non-blocking connect, `MSG_NOSIGNAL`) — probed in phase 1; Links2's
  `select`-driven connect path already exercises most of it.
- **`--disable-alarm`-class timeout behavior** — with the sync resolver,
  curl uses `SIGALRM` for resolve timeouts; if `alarm(2)`/`SIGALRM` delivery
  proves incomplete, pass `--disable-alarm-timeout`-equivalent (`CURL_DISABLE`
  define) rather than patching signal semantics for this port.
