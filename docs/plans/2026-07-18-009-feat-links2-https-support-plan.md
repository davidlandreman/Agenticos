---
title: "feat: add verified HTTPS support to Links 2"
type: feat
status: implemented
date: 2026-07-18
depth: large
related_docs:
  - CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - userland/README.md
  - userland/apps/links2/README.md
  - userland/prebuilt/README.md
  - docs/plans/2026-07-18-004-feat-taskbar-tray-date-time-plan.md
  - docs/plans/2026-07-18-006-feat-cryptographic-entropy-and-random-interfaces-plan.md
  - docs/plans/2026-07-18-007-feat-links2-userland-browser-plan.md
  - docs/plans/2026-07-18-008-feat-links2-rust-gui-driver-plan.md
  - https://www.openssl-library.org/source/
  - https://www.openssl-library.org/policies/releasestrat/
  - https://docs.openssl.org/3.5/man7/RAND/
  - https://docs.openssl.org/3.5/man1/openssl-verification-options/
  - https://curl.se/docs/caextract.html
---

# feat: add verified HTTPS support to Links 2

## Summary

Enable HTTPS in the existing static-musl Links 2.30 browser without changing
its command names, GUI driver, or committed-prebuilt delivery model:

```text
CMOS RTC (UTC)       VirtIO RNG / RDRAND       Mozilla CA roots
      |                       |                       |
CLOCK_REALTIME      getrandom + /dev/urandom   /etc/ssl/cert.pem
      |                       |                       |
      +-----------------------+-----------------------+
                              |
                  static OpenSSL 3.5.7 LTS
                              |
                 Links chain + hostname checks
                              |
             links / links2, text and GUI HTTPS
```

Build one pinned, static, non-PIE `LINKS.ELF` with OpenSSL 3.5.7, TLS 1.2 and
TLS 1.3, IPv4, SNI, system CA roots, certificate-chain validation, certificate
validity checks, and DNS/IP hostname validation. Keep Links' existing HTTP,
DNS, text-mode, and AgenticOS GUI paths intact.

The security boundary is explicit:

- a fresh profile rejects invalid certificates instead of using upstream
  Links' warning default;
- TLS errors never retry the URL as plaintext HTTP;
- the production trust store is a pinned, reviewable Mozilla-derived PEM
  bundle under the kernel-managed `/etc` namespace;
- the trust store is not published when boot cannot establish a valid RTC
  wall clock;
- automated coverage uses only repository-owned certificates and a restricted
  QEMU `guestfwd` TLS service, never the public internet;
- HTTPS remains unavailable when entropy, trust roots, or valid wall time are
  unavailable.

This plan enables HTTPS only for Links. BusyBox `wget` remains HTTP-only.

## Current-state evidence

- `userland/apps/links2/Makefile` configures Links with `--without-ssl` and
  links only the Rust GUI archive, libpng, zlib, libm, and musl pthread
  compatibility symbols.
- `userland/prebuilt/LINKS.ELF` is one 4,777,192-byte static `ET_EXEC` used by
  both `/bin/links` and `/bin/links2`, in terminal and GUI modes.
- Restricted QEMU coverage already proves numeric and hostname HTTP, a
  relative redirect, DNS fork-helper behavior, pipe readiness, and Links'
  `select(2)` event loop.
- The kernel random broker already backs process `AT_RANDOM`, `getrandom(2)`,
  and `/dev/urandom` from VirtIO RNG or RDRAND and fails closed when neither
  source is usable.
- `CLOCK_REALTIME` and `gettimeofday` already derive Unix time from a validated
  CMOS RTC snapshot plus PIT elapsed time. `time::wall_clock_ns()` retains an
  explicit validity bit, although the public realtime syscall falls back to
  monotonic time for non-TLS compatibility if RTC initialization fails.
- `/etc` is recreated by `src/userland/etc.rs` after `/host` and the writable
  overlay are mounted. It is the right owner for an immutable-at-runtime
  system trust store, just as it owns accounts, hosts, zsh configuration,
  theme state, and DHCP resolver configuration.
- Links 2.30 already sends SNI for non-numeric hosts, checks
  `SSL_get_verify_result`, retrieves the peer certificate, and uses
  `X509_check_host`/`X509_check_ip` when the OpenSSL APIs are available.
  Its upstream default is nevertheless `SSL_WARN_ON_INVALID_CERTIFICATE`, and
  it also contains a 2024-era embedded CA set selectable from the UI. Neither
  behavior should define AgenticOS' secure default.

## Feasibility findings

### Pinned TLS backend

Use OpenSSL 3.5.7:

```text
version: 3.5.7
source:  https://github.com/openssl/openssl/releases/download/openssl-3.5.7/openssl-3.5.7.tar.gz
sha256:  a8c0d28a529ca480f9f36cf5792e2cd21984552a3c8e4aa11a24aa31aeac98e8
series:  3.5 LTS, supported upstream through 2030-04-08
```

OpenSSL 3.5 is preferable to the nearly end-of-life 3.0 LTS line and to the
short-lived 3.6/4.0 lines. The exact patch release remains pinned; updates are
reviewed source changes accompanied by a refreshed `LINKS.ELF`, not floating
downloads during normal builds.

A throwaway build succeeded on the project's supported Apple Silicon host
using `x86_64-linux-musl-gcc` 14.2.0 and this profile:

```text
target: linux-x86_64
prefix: private links2 dependency prefix
OPENSSLDIR: /etc/ssl

no-shared no-module no-dso no-pinshared no-pic
no-threads no-async no-secure-memory
no-apps no-tests no-docs
no-legacy no-fips no-engine no-quic
no-zlib no-zlib-dynamic no-autoload-config
```

Keep the default provider compiled into `libcrypto.a`; there are no loadable
provider files, engines, DSOs, OpenSSL command-line program, or runtime
`openssl.cnf`. `no-threads` matches Links' single-threaded event loop and
avoids adding clone/futex/thread-local OpenSSL behavior. Do not use
`OPENSSL_NO_STDIO`: Links must load `/etc/ssl/cert.pem`.

The verified development install contained an approximately 11 MiB
`libcrypto.a` and 1.3 MiB `libssl.a`. Those are intermediate archives, not
shipped files; the static linker pulls only referenced objects into
`LINKS.ELF`.

### Measured binary-size delta

Two otherwise equivalent stripped, static, text-only Links 2.30 prototypes
measured:

| Prototype | Bytes |
|---|---:|
| HTTP-only | 1,452,968 |
| OpenSSL 3.5.7 HTTPS | 6,642,856 |
| Measured TLS delta | 5,189,888 |

Applying the measured delta to the current 4,777,192-byte GUI browser projects
a roughly 9.8 MiB final ELF. That remains below the existing 16 MiB loader
input gate, but U0 must measure the real patched GUI build. Do not raise the
loader cap in this work. The final binary must remain below 16 MiB and the
plain-git `userland/prebuilt/` total must remain below its documented LFS
review threshold.

### Trust-store source

Commit the dated Mozilla-derived PEM snapshot published by curl's CA Extract
service:

```text
snapshot: 2026-07-16
source:   https://curl.se/ca/cacert-2026-07-16.pem
sha256:   3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91
size:     186,446 bytes
roots:    119
license:  MPL 2.0
```

Use the dated URL, not the moving `cacert.pem` URL. The bundle is committed so
stock builds and tests never fetch trust material. Record that this generic
PEM conversion does not carry every Firefox policy constraint that exists
outside the certificates themselves; it provides the conventional OpenSSL
PEM trust model, not a claim of Firefox trust-policy equivalence.

Do not use Links' compiled `certs.inc` as the production trust source. It is
tied to the 2.30 release date, duplicates trust material inside a multi-MiB
ELF, and can become stale independently of OpenSSL updates.

### Existing ABI coverage

Source and symbol inspection show that the selected OpenSSL build can reach
`getentropy`/`getrandom`, `/dev/urandom`, `clock_gettime`, `time`, `getpid`,
UID queries, `uname`, ordinary file/stat calls, and socket I/O. AgenticOS
already implements the relevant Linux-musl surface. Disabling threads, async,
secure-memory support, modules, engines, DSOs, and runtime configuration
removes the most likely futex, clone, `madvise`, and dynamic-loader gaps.

This is evidence for a small compatibility delta, not permission to skip
runtime discovery. The first TLS-enabled ELF must still run with unknown
syscall tracing before kernel changes are proposed.

## Goals

1. Reproducibly cross-build OpenSSL 3.5.7 and Links 2.30 into one static,
   non-PIE, stripped x86-64 `LINKS.ELF` from SHA256-pinned sources.
2. Support IPv4 HTTPS in `links`, `links2`, `links -dump`, interactive text
   mode, and the existing AgenticOS GUI driver.
3. Negotiate TLS 1.2 or TLS 1.3 only, with SNI for DNS hosts and no plaintext
   retry after any TLS failure.
4. Validate certificate signatures, chains, validity intervals, DNS names,
   wildcard rules, and numeric IPv4 SANs against one system trust store.
5. Make rejection the fresh-profile default and preserve clear terminal/GUI
   errors without rendering protected response bytes.
6. Publish a pinned CA bundle at `/etc/ssl/cert.pem` only when the kernel has a
   valid RTC-derived wall-clock anchor.
7. Prove valid HTTPS, SNI, TLS 1.2, redirects, hostname mismatch, unknown CA,
   expired/not-yet-valid certificates, and HTTP regression in hermetic QEMU
   tests.
8. Keep the existing committed-prebuilt, aliases, GUI launch command, writable
   `/root/.links` profile, and rebuild flags unchanged.
9. Document dependency/update ownership and include the required OpenSSL and
   Mozilla bundle license notices.

## Non-goals

- HTTPS for BusyBox `wget`, a second browser ELF, a new `/bin` alias, or a
  kernel TLS implementation.
- IPv6, QUIC/HTTP/3, HTTP/2, WebSockets, DoH, DNSSEC, or encrypted proxy
  transports.
- NTP, authenticated time synchronization, RTC writes, timezone management,
  or automatic clock repair. This milestone treats a structurally valid CMOS
  RTC read as the OS administrator's UTC wall clock.
- OCSP, CRLs, certificate transparency, HSTS preload lists, public-key
  pinning, DANE, or Firefox-equivalent external trust constraints.
- FIPS mode, FIPS validation, client-certificate acceptance coverage, smart
  cards, engines, external providers, or PKCS#11.
- Persisting TLS session keys/tickets across process exit or adding a shared
  TLS cache. Links' existing in-process session cache is sufficient.
- JavaScript, CSS, renderer, image-decoder, download, cookie, or general
  browser feature work unrelated to TLS transport.
- Automatically refreshing OpenSSL or CA roots. Security updates are explicit
  reviewed changes with new hashes, licenses if needed, tests, and prebuilt
  refreshes.
- Allowing HTTPS when the secure entropy broker, trusted roots, or valid wall
  time are absent.

## Security and compatibility contract

### Protocol policy

- Minimum TLS version is TLS 1.2. TLS 1.3 remains enabled through OpenSSL's
  normal maximum-version selection.
- SSLv2, SSLv3, TLS 1.0, and TLS 1.1 are never negotiated. Do not rely only on
  OpenSSL's current security level: set the minimum version explicitly in the
  AgenticOS Links patch and assert the patch after extraction.
- Preserve Links' nonblocking `SSL_connect`/`SSL_read`/`SSL_write` integration
  with its existing `select(2)` scheduler. No TLS worker thread is introduced.
- SNI is sent for DNS hostnames and omitted for numeric IP addresses, matching
  upstream Links. Numeric HTTPS succeeds only when the certificate has the
  matching IP address SAN.
- A TLS negotiation failure may retry another address or a lower supported TLS
  version, but it must not change `https://` to `http://`. With the TLS 1.2
  floor, Links' compatibility retry cannot descend below TLS 1.2.

### Certificate policy

- Change the compiled default from `SSL_WARN_ON_INVALID_CERTIFICATE` to
  `SSL_REJECT_INVALID_CERTIFICATE`.
- Automated tests always use a fresh isolated `HOME`, so a developer's
  persisted `/root/.links` policy cannot weaken acceptance results.
- A failed chain, unknown root, expired/not-yet-valid leaf, hostname mismatch,
  missing peer certificate, insecure cipher, or protocol downgrade aborts
  before Links sends the HTTP request or renders response content.
- Keep upstream hostname verification through `X509_check_host` and
  `X509_check_ip`; configure assertions must prove both functions were found.
- Disable Links' selectable built-in CA bundle for AgenticOS. The only
  production default is OpenSSL's system file `/etc/ssl/cert.pem`. An explicit
  `SSL_CERT_FILE` remains useful for controlled developer/test environments,
  but is not present in `DEFAULT_USER_ENV`.
- Do not silently continue with an empty or partially parsed CA bundle.
  Staging verifies the exact committed hash, `/etc` import must complete the
  full write, and absence leaves HTTPS untrusted rather than permissive.

### Time policy

The OS trusts a stable, structurally valid CMOS RTC sample as UTC, consistent
with current QEMU launch flags and the taskbar/realtime design. OpenSSL then
uses the RTC-anchored realtime value for X.509 `notBefore`/`notAfter` checks.

`src/userland/etc.rs` must remove any stale `/etc/ssl/cert.pem` on every boot,
then publish the current committed bundle only when
`time::wall_clock_ns().is_some()`. If RTC initialization failed, leave the
trust file absent and log one concise message that HTTPS trust is unavailable.
This prevents the realtime syscall's legacy monotonic fallback from becoming
an accidental TLS trust decision.

This does not prove that a valid-looking RTC is correct; like a conventional
offline OS, AgenticOS trusts the machine's configured system clock. NTP is a
separate milestone.

### Entropy policy

OpenSSL seeds from musl's OS entropy interfaces. The linked build must prove
that `RAND_status()` succeeds using AgenticOS' existing `getrandom(2)` and/or
`/dev/urandom` path. Do not add `RAND_add` calls fed by time, addresses, PIDs,
MAC addresses, or other predictable values. Entropy failure aborts TLS; it
never activates an internal weak fallback.

### Trust-store lifecycle

Keep system roots separate from browser profile state:

```text
repo:  userland/ca-certificates/cacert.pem
stage: /host/ETC/SSL/CERT.PEM          (read-only host share)
boot:  /etc/ssl/cert.pem               (kernel-managed runtime copy)
user:  /root/.links/*                  (mutable browser preferences/history)
```

The repository bundle is required even with `test.sh --skip-userland`; staging
is independent of the musl toolchain and prebuilt rebuild. Runtime mutation of
`/etc/ssl/cert.pem` is rejected by the existing managed-`/etc` policy.

For a CA update, change the dated source metadata and hash, replace the
committed PEM, review added/removed roots, run the TLS matrix, and document the
new snapshot. A CA-only change does not require a new `LINKS.ELF`; an OpenSSL
or Links patch change does.

## Design

### Static dependency graph

Extend the private Links dependency prefix rather than introducing a shared
system OpenSSL installation:

```text
links-2.30.tar.bz2 ─────── patches + agenticos driver ─────┐
zlib-1.3.2.tar.gz ────────────────────────────────────────┤
libpng-1.6.58.tar.xz ─────────────────────────────────────┤
openssl-3.5.7.tar.gz ── libssl.a + libcrypto.a ───────────┤
driver-rs ───────────── liblinks_agenticos_driver.a ──────┤
                                                          v
                                              stripped LINKS.ELF
```

Add OpenSSL to `fetch`, `distclean`, source/hash variables, dependency-prefix
headers/libraries, and the final Links target. Force `--disable-ssl-pkgconfig`
so configure cannot discover Homebrew or another host OpenSSL. Configure with
`--with-ssl=$(DEPS_PREFIX)`, replace `--without-ssl`, and link in dependency
order:

```text
driver archive, png, zlib, ssl, crypto, m, pthread
```

The OpenSSL target should run `build_sw` and install development headers and
static archives only. Set `--libdir=lib` so the existing private prefix has one
deterministic library directory on macOS and Linux.

Build assertions must prove:

1. the OpenSSL archive was built from version 3.5.7 and the checked hash;
2. threads, modules, DSOs, engines, FIPS, legacy provider, QUIC, and runtime
   config autoload are disabled;
3. Links' configure summary says `SSL support: OPENSSL`;
4. `HAVE_X509_CHECK_HOST`, `HAVE_X509_CHECK_IP`, and certificate support are
   defined;
5. the AgenticOS secure-default patch and TLS 1.2 floor are present;
6. the AgenticOS driver and fork-helper patches remain present;
7. the output is static x86-64 `ET_EXEC` with no `PT_INTERP`/`DT_NEEDED`;
8. the stripped ELF is below 16 MiB and contains no absolute host build path;
9. no provider module, OpenSSL executable, config, private key, or CA bundle is
   embedded as a separate staged runtime dependency.

### AgenticOS Links patch

Add one focused patch after the existing fork-helper and GUI-driver patches:

`userland/apps/links2/patches/0003-agenticos-secure-https-defaults.patch`

It should:

- make `SSL_REJECT_INVALID_CERTIFICATE` the initial `ssl_options` value;
- set the OpenSSL minimum protocol version to `TLS1_2_VERSION` and treat a
  failure to install that floor as SSL initialization failure;
- omit the `certs.inc`-backed built-in trust-store choice when an
  `AGENTICOS_SYSTEM_CA_ONLY` build define is set;
- leave upstream SNI, nonblocking I/O, chain checking, hostname checking,
  cipher checks, error states, and in-memory session cache otherwise intact.

Keep this patch small enough to rebase visibly when Links is updated. Do not
fork the TLS client into the Rust graphics driver or create an AgenticOS TLS
API.

### Managed CA publication

Add a toolchain-independent `stage_ca_certificates` function beside
`stage_zsh_config` in `userland/stage-lib.sh`. It verifies the pinned SHA256,
atomically copies the committed PEM to `host_share/ETC/SSL/CERT.PEM`, and
hard-fails if the bundle is missing or altered. Call it explicitly from both
`build.sh` and `test.sh` before userland app staging, just like the committed
zsh configuration; it does not depend on a prebuilt refresh.

Extend `src/userland/etc.rs` with:

```text
CA_SOURCE_PATH = /host/ETC/SSL/CERT.PEM
CA_CERT_PATH    = /etc/ssl/cert.pem
CA_CERT_DIR     = /etc/ssl/certs
```

Create `/etc/ssl` and `/etc/ssl/certs`, remove a stale cert file, gate import
on valid wall-clock state, and use the existing full-write checked copy helper.
The empty certificate directory is intentional: OpenSSL's default hashed-dir
lookup may inspect it, while the authoritative source is the single PEM file.

In test builds only, append the committed test root CA from
`/host/TLS/ROOT.PEM` to the runtime PEM after the production bundle. This lets
Links exercise its unmodified default CA path without setting `SSL_CERT_FILE`
and never adds the test root to production.

### Deterministic TLS fixture

Add `tools/net-test-https.py` using only Python's standard `ssl`, `socket`, and
HTTP parsing modules. Like the current HTTP fixture, it handles one bounded
connection and exits. It must resolve certificate paths relative to
`__file__`, set finite socket timeouts, cap request/header bytes, and never
bind a host port.

Commit test-only material under `tools/tls-fixtures/`:

```text
ROOT.PEM                 trusted test root certificate (staged to guest tests)
VALID.PEM / VALID.KEY    DNS + IPv4 SAN, valid at the fixed test RTC
UNTRUSTED.PEM / .KEY     chain rooted in a different, unstaged test CA
EXPIRED.PEM / .KEY       trusted chain, notAfter before fixed test RTC
FUTURE.PEM / .KEY        trusted chain, notBefore after fixed test RTC
README.md                subjects, SANs, serials, validity, regeneration recipe
```

Private keys are test fixtures only: the server reads them on the host and
staging must prove that no `*.KEY` reaches `host_share` or a guest filesystem.
Use unique fixed serial numbers and ECDSA P-256 or RSA-2048 keys supported by
the minimized default provider. The valid certificate includes:

```text
DNS:agenticos-https.test
DNS:tls12.agenticos-https.test
IP:10.0.2.102
```

The server's SNI callback selects valid, untrusted, expired, or future
contexts by hostname. An unmatched `mismatch.agenticos-https.test` name gets
the otherwise-valid certificate, isolating hostname verification from chain
and time checks. For the TLS 1.2 hostname, cap both server minimum and maximum
at TLS 1.2.

Extend the restricted slirp network with one guest forward:

```text
10.0.2.102:8443 -> cmd:tools/net-test-https.py
```

Add the fixture names to test-only `/etc/hosts`; production hosts stay
unchanged.

Use a fixed test RTC such as `2026-07-18T12:00:00` in `test.sh` instead of the
host's current date. Interactive `build.sh` keeps `-rtc base=utc`. The fixed
test clock makes valid, expired, and future certificate results reproducible
and prevents a committed fixture from becoming a latent date-dependent test
failure. Confirm the exact QEMU date syntax on both supported launch paths
before landing it.

## Implementation units

### U0. Build-only OpenSSL integration and measured gate

**Files:**

- `userland/apps/links2/Makefile`
- `userland/apps/links2/patches/0003-agenticos-secure-https-defaults.patch`
- `userland/apps/links2/OPENSSL-LICENSE.txt`
- `userland/apps/links2/README.md`

**Work:**

1. Add the pinned OpenSSL source, hash, extraction, configure, build, and
   private-prefix install rules with the profile above.
2. Apply the focused Links patch, configure with private OpenSSL and no
   pkg-config, and retain all existing GUI/image/fork assertions.
3. Build and strip the complete GUI-capable browser outside the boot path.
4. Record exact final size and `readelf` properties. Stop and reassess feature
   pruning if it reaches 16 MiB; do not raise the loader gate.
5. Run `-version` and a local `file://` dump on the build host where possible,
   then stage temporarily for U1 discovery.
6. Include the OpenSSL Apache 2.0 license/notice required by the statically
   linked binary and update the dependency documentation.

**Exit bar:** A reproducible TLS-enabled `LINKS.ELF` builds with the private
OpenSSL only, remains static and below 16 MiB, reports SSL support in its
configure evidence, and still launches `-version` and local text/GUI content.

### U1. AgenticOS runtime discovery and minimal compatibility fixes

**Files:**

- `src/userland/abi.rs` — only if a demonstrated syscall is missing
- `src/userland/syscalls.rs` — only if a demonstrated syscall is missing
- `src/userland/devfs.rs` / filesystem code — only if measured behavior is
  insufficient
- focused tests beside any compatibility change

**Work:**

1. Run progressively with syscall trace enabled: `-version`, local file,
   numeric HTTP, hostname HTTP, then numeric and hostname HTTPS against the
   test fixture.
2. Confirm OpenSSL obtains entropy through the existing broker and reaches
   `RAND_status() == 1`; confirm no weak or file-persisted random seed path.
3. Confirm X.509 time checks observe RTC-backed realtime and that TLS socket
   readiness remains correct through Links' `select(2)` loop.
4. Capture every unknown syscall and every approximate implemented call whose
   semantics break OpenSSL. Implement the smallest correct Linux behavior,
   with a focused dispatch test, only when execution demonstrates the need.
5. Re-run HTTP and GUI-driver tests after every ABI change.

Likely calls are already present: getrandom/getentropy, `/dev/urandom`,
clock_gettime, PID/UID/uname queries, file/stat/open/read, socket I/O, poll, and
select. `clone`, futex, DSO loading, provider-file access, and secure-memory
`madvise` are configuration regressions and should fail build/runtime
assertions rather than cause speculative kernel stubs.

**Exit bar:** Valid local HTTPS reaches content with no unexpected syscall,
and HTTP/local/GUI behavior remains unchanged. Any new syscall has a real
OpenSSL trace and direct tests.

### U2. System CA store and trusted-time gate

**Files:**

- `userland/ca-certificates/cacert.pem`
- `userland/ca-certificates/README.md`
- `userland/ca-certificates/LICENSE`
- `userland/stage-lib.sh`
- `build.sh`
- `test.sh`
- `src/userland/etc.rs`
- `src/tests/time.rs` and/or managed-`/etc` tests

**Work:**

1. Commit the dated, hash-verified 2026-07-16 Mozilla-derived bundle and MPL
   2.0 license; document the update and constraint caveat.
2. Stage it on every build/test path, including prebuilt-only and
   `--skip-userland` paths.
3. Recreate `/etc/ssl/cert.pem` only after a valid wall-clock anchor; ensure an
   invalid-clock boot removes any overlay-restored stale copy.
4. Append the test root only under `cfg(feature = "test")` and assert
   production code never references it.
5. Add focused tests for full copy, managed-path immutability, stale removal,
   valid-time publication, invalid-time absence, and missing/corrupt staged
   bundle failure behavior.
6. Avoid logging certificate bodies, hashes as secrets, or any test private
   key content.

**Exit bar:** Production gets the exact committed bundle at the conventional
OpenSSL path only with valid RTC time; tests get that bundle plus one explicit
test root; no private key enters the guest.

### U3. Hermetic HTTPS server and rejection matrix

**Files:**

- `tools/net-test-https.py`
- `tools/tls-fixtures/*`
- `userland/stage-lib.sh`
- `test.sh`
- `src/userland/etc.rs` (test hostnames)
- `src/tests/network_userland.rs`

**Work:**

1. Add the bounded one-connection TLS server, SNI context selection, fixed
   response routes, and certificate fixture documentation.
2. Add a test-only staging helper that copies only `ROOT.PEM` to
   `host_share/TLS/ROOT.PEM`, including with `--skip-userland`, and asserts
   that no private-key suffix is present anywhere below `host_share/TLS`.
3. Add the restricted QEMU guest forward and fixed test RTC. Keep networking
   disabled cleanly when `AGENTICOS_TEST_NETWORK=off`.
4. Extend `run_links_dump` so tests can capture stdout/stderr/status in an
   isolated `HOME=/work/links-https-home` and assert content or failure.
5. Cover this positive matrix:

   | Case | URL/fixture | Required result |
   |---|---|---|
   | DNS + SNI | `https://agenticos-https.test:8443/` | exact protected marker |
   | Numeric IPv4 SAN | `https://10.0.2.102:8443/` | exact protected marker |
   | TLS 1.2 floor | `https://tls12.agenticos-https.test:8443/` | success, TLS 1.2 server |
   | Relative redirect | `/redirect` -> `/second` | second-page marker over HTTPS |
   | HTTP regression | existing HTTP URLs | unchanged success |

6. Cover this negative matrix:

   | Case | Host/context | Required result |
   |---|---|---|
   | Host mismatch | `mismatch.agenticos-https.test` | invalid certificate; no marker |
   | Unknown CA | `untrusted.agenticos-https.test` | invalid certificate; no marker |
   | Expired | `expired.agenticos-https.test` | invalid certificate; no marker |
   | Not yet valid | `future.agenticos-https.test` | invalid certificate; no marker |
   | No test root | isolated CA-file override | rejection; no marker |
   | TLS 1.0/1.1 only | legacy protocol context | handshake failure; no marker |
   | Entropy unavailable | broker test/fault path | TLS initialization failure |
   | Trust file absent | managed `/etc` unit/integration path | rejection, never permissive |

7. Determine Links' stable batch-mode failure contract before asserting an
   exit code. If `-dump` returns zero for a load error, assert its normalized
   diagnostic and absence of the protected marker; do not hide the behavior
   behind a wrapper that falsely claims Links returned failure.
8. Prove a TLS error does not contact the HTTP fixture and does not create a
   cached page containing the protected body.
9. Give every guest wait and server read a PIT/host timeout so a broken
   handshake cannot hang the suite.

**Exit bar:** All valid cases render only after verification, all invalid
cases expose no protected content, TLS 1.0/1.1 cannot connect, and the existing
HTTP/DNS matrix stays green under restricted networking.

### U4. Refresh the shipped browser and complete acceptance

**Files:**

- `userland/prebuilt/LINKS.ELF` (generated and committed)
- `userland/apps/links2/README.md`
- `userland/prebuilt/README.md`
- `userland/README.md`
- `CLAUDE.md`
- `src/userland/CLAUDE.md`
- `src/tests/CLAUDE.md`
- `README.md`
- `docs/IMPLEMENTATION_PLAN.md`

**Work:**

1. Refresh through `./userland/refresh-prebuilt.sh` or the targeted
   `REBUILD_LINKS2=1` path; commit source, patch, licenses, and ELF together.
2. Update measured sizes and the plain-git prebuilt total. Correct the stale
   `userland/prebuilt/README.md` claim that the current GUI browser is a
   1.5 MiB text-only artifact.
3. Update all HTTP-only/TLS-deferred statements. Keep BusyBox clearly labeled
   HTTP-only and document Links as IPv4 HTTP/HTTPS.
4. Run focused, full, rebuild, and interactive acceptance commands.
5. Manually exercise both terminal and GUI against a public HTTPS site after
   hermetic tests pass. This is a compatibility smoke only, never an automated
   acceptance dependency.
6. Verify Start -> Programs -> Web Browser needs no argv change and HTTPS
   errors display in the ordinary Links GUI without crashing or hanging.
7. Record the CA/OpenSSL update procedure and the RTC/no-NTP limitation.

**Exit bar:** A stock checkout boots the refreshed browser without toolchain or
network access, both aliases support verified HTTPS in text and GUI modes,
invalid certificates fail closed by default, and documentation names the
remaining security limitations precisely.

## Test and validation commands

```sh
# Host-side formatting and kernel checks
cargo fmt --check
cargo check

# Focused booted coverage
./test.sh network_userland time etc userland

# Full regression
./test.sh

# Reproducible TLS-enabled prebuilt build
REBUILD_LINKS2=1 ./build.sh -n
./userland/refresh-prebuilt.sh         # documented full prebuilt refresh

# Artifact inspection
x86_64-linux-musl-readelf -h -l -d userland/prebuilt/LINKS.ELF
wc -c userland/prebuilt/LINKS.ELF
shasum -a 256 userland/ca-certificates/cacert.pem

# Manual guest acceptance
links -dump https://agenticos-https.test:8443/
links -dump https://example.com/
links2 -g https://example.com/
```

The public `example.com` checks require an interactive unrestricted launch and
are manual only. Automated tests must remain on QEMU-local services.

## Acceptance matrix

| Area | Acceptance |
|---|---|
| Build | Pinned Links/OpenSSL/zlib/libpng inputs; private prefix; no host autodetection |
| Artifact | One static non-PIE x86-64 `LINKS.ELF`, no interpreter/dependencies, under 16 MiB |
| Entropy | OpenSSL initializes only from existing secure OS entropy; failure aborts TLS |
| Time | X.509 uses RTC-backed realtime; missing valid RTC withholds the system trust store |
| Trust | Exact pinned PEM at `/etc/ssl/cert.pem`; built-in Links roots disabled |
| Protocol | TLS 1.2/1.3 only; IPv4; SNI for DNS; no HTTPS-to-HTTP error fallback |
| Identity | Chain, dates, DNS hostname, wildcard, and IPv4 SAN checks enforced |
| Failure | Invalid peer reveals no protected body and produces stable text/GUI diagnostics |
| Modes | `links`, `links2`, `-dump`, interactive text, GUI driver, and Start launcher work |
| Regression | Local file, DNS, HTTP, redirects, GUI events/rendering, and network-off behavior stay green |
| Delivery | Stock checkout uses committed ELF and CA roots without downloads/toolchains |
| Licensing | OpenSSL Apache 2.0 and Mozilla/curl bundle MPL 2.0 notices included |

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Upstream Links defaults to warning on invalid certificates | Users can continue after a bad chain | Patch fresh default to reject and test with isolated HOME |
| Old embedded `certs.inc` remains selectable | Stale roots bypass maintained system bundle | Compile out AgenticOS built-in-root choice |
| RTC is missing | Monotonic fallback looks like a wall clock | Remove/withhold `/etc/ssl/cert.pem`; fail closed |
| RTC is valid-looking but wrong | Good certificates reject or expired certificates appear current | Document machine-clock trust; add NTP separately |
| OpenSSL auto-discovers host config/provider files | Non-reproducible build or runtime dependency | Private prefix, no pkg-config/DSO/module/autoload config, build assertions |
| OpenSSL introduces futex/clone behavior | Guest hang or unknown syscall | `no-threads`, runtime trace, no speculative syscall stubs |
| TLS inflates browser beyond loader/repo budget | Launch rejection or repository bloat | Measured ~5.19 MiB delta, 16 MiB hard gate, remeasure final ELF |
| CA snapshot becomes stale | Public sites stop validating or removed roots remain trusted | Dated source/hash, explicit update workflow, review root diffs |
| PEM bundle lacks Firefox external name constraints | Trust behavior is broader than Firefox | Document conventional OpenSSL trust semantics; do not claim browser parity |
| Test certificates expire relative to host date | Latent nondeterministic CI failures | Fixed test RTC and fixed certificate validity windows |
| Test private key is staged into guest | Unnecessary secret exposure and confusing trust boundary | Host-only fixture directory and staging assertions excluding `*.KEY` |
| TLS server blocks forever | Entire QEMU suite hangs | One connection, bounded reads, socket timeout, PIT watchdog |
| `-dump` reports load errors with exit 0 | False-positive negative tests | Assert diagnostic plus absence of protected content after characterizing status |
| Static link order drops algorithms/provider objects | Runtime handshake failure despite successful link | Positive RSA/ECDSA TLS fixture, provider assertions, final-symbol inspection |
| Protocol fallback reaches TLS 1.0/1.1 | Obsolete transport is accepted | Explicit TLS 1.2 minimum and legacy-only rejection fixture |
| Cert rejection happens after HTTP bytes are sent | Host/path metadata leaks to unverified peer | Upstream verifies before `connected_callback`; fixture asserts no request on bad peers |
| Browser profile weakens policy | Tests pass/fail based on developer state | Dedicated clean HOME for every automated Links TLS launch |

## Done criteria

- OpenSSL 3.5.7 and the 2026-07-16 CA snapshot are pinned by exact source and
  SHA256; normal builds perform no fetch.
- One refreshed `LINKS.ELF` remains static, non-PIE, below 16 MiB, and serves
  both `links` and `links2` in text and AgenticOS GUI modes.
- The default browser supports IPv4 HTTP plus verified TLS 1.2/1.3 HTTPS with
  SNI, chain/date/hostname/IP checks, and system roots.
- A fresh profile rejects unknown, expired, future, and mismatched
  certificates without exposing response content or retrying plaintext HTTP.
- The system trust store is kernel-managed, exact, immutable to userland, and
  absent when boot has no valid RTC wall-clock anchor.
- Entropy comes only from the existing secure broker; entropy failure stops
  TLS.
- Hermetic QEMU tests cover positive and negative TLS cases with a fixed RTC,
  test-only CA, bounded guest-forward server, and no public network.
- Existing HTTP, DNS, redirects, GUI driver, Start launcher, network-off boot,
  and full kernel tests remain green.
- Documentation no longer calls Links HTTP-only, still calls BusyBox `wget`
  HTTP-only, and states the no-NTP/revocation/CT/HSTS limitations.
- OpenSSL and CA-bundle licenses/notices ship with the source and artifact.

## References

- [OpenSSL downloads and current supported releases](https://www.openssl-library.org/source/)
- [OpenSSL release strategy and 3.5 LTS lifetime](https://www.openssl-library.org/policies/releasestrat/)
- [OpenSSL random-generator behavior](https://docs.openssl.org/3.5/man7/RAND/)
- [OpenSSL certificate path, validity, and trust-anchor verification](https://docs.openssl.org/3.5/man1/openssl-verification-options/)
- [OpenSSL TLS client hostname and SNI guidance](https://docs.openssl.org/3.5/man7/ossl-guide-tls-client-block/)
- [curl Mozilla CA Extract snapshots and policy caveat](https://curl.se/docs/caextract.html)
- [Links 2.30 source archive](https://links.twibright.com/download/links-2.30.tar.bz2)
