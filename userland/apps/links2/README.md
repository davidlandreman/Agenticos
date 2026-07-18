# Links2 — text-mode userland browser

This directory cross-builds Links 2.30 as a static-musl, non-PIE executable
for AgenticOS. The shipped first milestone is intentionally text-mode,
IPv4/HTTP-only: TLS, graphics, IPv6, libevent, GPM, and optional compression
libraries are disabled.

```sh
make -C userland/apps/links2
REBUILD_LINKS2=1 ./build.sh -n
```

The source archive and SHA256 are pinned in `Makefile`. The one local patch
selects Links' existing fork-and-pipe background helper when
`AGENTICOS_NO_PTHREADS` is defined. Musl exposes pthread symbols, but
AgenticOS does not yet implement the clone/futex/thread-TLS ABI that detached
pthreads require.

The committed artifact is `userland/prebuilt/LINKS.ELF`; normal builds stage
it as `/host/LINKS.ELF`, and the synthetic `/bin` namespace exposes both
`links` and `links2`.

Inside a terminal:

```sh
links http://host/path
links -dump http://host/path
```

Links stores its per-user state under `/root/.links`; `/root` is provisioned
on the writable overlay at boot. The restricted QEMU tests use the
repository-owned `agenticos-http.test:8081` HTML fixture and cover numeric
HTTP, hostname resolution, normalized dump output, and a relative redirect.

HTTPS is not merely a build toggle. Cryptographic entropy is now available,
but a reviewed TLS stack, CA roots, hostname verification, trusted-time policy,
and HTTPS-specific QEMU coverage remain a separate follow-up.
