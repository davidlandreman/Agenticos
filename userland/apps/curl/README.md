# curl — command-line HTTP/HTTPS transfer tool

This directory cross-builds curl 8.21.0 as a fully static musl, non-PIE
executable for AgenticOS. Scope is deliberately IPv4 HTTP and HTTPS only:
every other curl protocol (FTP, FILE, mail, TELNET, TFTP, DICT, GOPHER,
MQTT, RTSP, SMB, LDAP, WebSockets) is disabled at configure time, along
with IPv6, the threaded resolver, HTTP/2, NTLM, TLS-SRP, and AWS SigV4.

```sh
make -C userland/apps/curl
REBUILD_CURL=1 ./build.sh -n
```

The curl, zlib, and OpenSSL 3.5.7 archives and SHA256 values are pinned in
`Makefile`. The OpenSSL and zlib recipes are byte-for-byte the qualified
profile from `../links2/Makefile` (static, single-threaded TLS,
`OPENSSLDIR=/etc/ssl`). If a third OpenSSL consumer appears, factor the
shared dependency build out rather than copying it again.

Three build details are load-bearing and asserted by the Makefile:

- curl links its tool through libtool, which silently drops a plain
  `-static` from `LDFLAGS`. The build passes `CURL_LDFLAGS_BIN=-all-static`
  at make time and refuses to ship a binary with `PT_INTERP` or
  `DT_NEEDED` — AgenticOS has no dynamic loader.
- `--disable-socketpair` removes libcurl's multi-API wakeup descriptor
  (`ENABLE_WAKEUP`), which otherwise requires `eventfd2` or an AF_UNIX
  socketpair — AgenticOS implements neither, and the failure surfaces as
  curl exit 27. The single-threaded tool never needs cross-thread wakeups.
- The compiled-in CA bundle is `/etc/ssl/cert.pem`, the kernel-managed
  Mozilla trust snapshot. Boot withholds that file when the RTC wall clock
  is invalid, so HTTPS fails closed (verify-locations error) rather than
  accepting certificates it cannot validity-check. `curl -k` remains the
  explicit, user-typed escape hatch, as on any Linux system.

The synchronous resolver uses musl `getaddrinfo` against the DHCP-published
`/etc/resolv.conf`. The committed artifact is `userland/prebuilt/CURL.ELF`;
normal builds stage it as `/host/CURL.ELF`, and the synthetic `/bin`
namespace exposes it as `curl`.

Inside a terminal:

```sh
curl http://host/path
curl -fsSL https://host/path -o /work/file
curl -I https://host/          # headers only
```

BusyBox `wget` remains HTTP-only; Links2 remains the interactive browser.

Licenses: `CURL-LICENSE.txt` (curl, MIT-like) and `OPENSSL-LICENSE.txt`
(Apache-2.0) are copied from the pinned upstream sources.
