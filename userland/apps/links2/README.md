# Links2 — text and GUI userland browser

This directory cross-builds Links 2.30 as a static-musl, non-PIE executable
for AgenticOS. It supports both the original terminal UI and a native windowed
browser backed by a `no_std` Rust graphics/input driver. IPv4 HTTP and HTTPS
are enabled, including DNS and numeric-IP certificate validation.

```sh
make -C userland/apps/links2
REBUILD_LINKS2=1 ./build.sh -n
```

The Links, zlib, libpng, and OpenSSL 3.5.7 archives and SHA256 values are
pinned in `Makefile`. OpenSSL is linked statically with threads, modules,
dynamic loading, configuration autoload, legacy providers, and QUIC disabled.
Local patches register the AgenticOS driver and select Links' existing
fork-and-pipe background helper when
`AGENTICOS_NO_PTHREADS` is defined. Musl exposes pthread symbols, but
AgenticOS does not yet implement the clone/futex/thread-TLS ABI that detached
pthreads require.

The committed artifact is `userland/prebuilt/LINKS.ELF`; normal builds stage
it as `/host/LINKS.ELF`, and the synthetic `/bin` namespace exposes both
`links` and `links2`.

Inside a terminal:

```sh
links http://host/path
links https://host/path
links -dump http://host/path
links2 -g -driver agenticos -no-connect
```

The Start menu's **Web Browser** entry launches the last command directly.
The selectable GUI-event descriptor (`gui_event_open`, syscall 5011) plugs
keyboard, mouse, resize, close, and theme events into Links' normal
`select(2)` loop. Rust owns the XRGB8888 surface, clipping, bitmap blits,
fills, lines, scrolling, presentation, and input translation; a small C file
only implements Links' graphics-driver callback table.

Links stores its per-user state under `/root/.links`; `/root` is provisioned
on the writable overlay at boot. The restricted QEMU tests use the
repository-owned HTTP and HTTPS fixtures. HTTPS uses the kernel-managed
`/etc/ssl/cert.pem` Mozilla trust snapshot, rejects invalid certificates by
default, requires TLS 1.2 or newer, and checks both DNS names and numeric IPs.
The trust store is withheld when the kernel cannot establish a valid RTC wall
clock. Hermetic QEMU tests cover valid hostname/IP chains, SNI, redirects,
TLS 1.2, and rejection of mismatched, untrusted, expired, and future-dated
certificates. BusyBox `wget` remains HTTP-only.
