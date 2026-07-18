# Links2 — text and GUI userland browser

This directory cross-builds Links 2.30 as a static-musl, non-PIE executable
for AgenticOS. It supports both the original terminal UI and a native windowed
browser backed by a `no_std` Rust graphics/input driver. HTTP and IPv4 are
enabled; TLS remains intentionally disabled.

```sh
make -C userland/apps/links2
REBUILD_LINKS2=1 ./build.sh -n
```

The Links, zlib, and libpng archives and SHA256 values are pinned in
`Makefile`. Local patches register the AgenticOS driver and select Links'
existing fork-and-pipe background helper when
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
repository-owned `agenticos-http.test:8081` HTML fixture and cover numeric
HTTP, hostname resolution, normalized dump output, and a relative redirect.

HTTPS is not merely a build toggle. Cryptographic entropy is available,
but a reviewed TLS stack, CA roots, hostname verification, trusted-time policy,
and HTTPS-specific QEMU coverage remain a separate follow-up.
