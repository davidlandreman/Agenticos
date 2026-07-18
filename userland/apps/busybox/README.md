# busybox

Single-binary static-musl BusyBox build, providing `ls`, `cat`, `grep`,
`sed`, `awk`, `find`, `wc`, `head`, `tail`, `sort`, `uniq`, plus the selected
`ping`, `nc`, and `wget` networking applets behind a multicall dispatcher. The kernel exposes a
virtual `/bin/<applet>` namespace so `execve("/bin/ls", argv, envp)`
resolves to `BB.ELF` with `argv[0] = "ls"`, and BusyBox's argv[0]-based
dispatch picks the right applet.

Source for the namespace + dispatch decisions:
[`docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md`](../../../docs/plans/2026-05-16-002-feat-busybox-coreutils-userland-plan.md).

## Build

The `userland/prebuilt/BB.ELF` committed binary is the source of truth
on every `./build.sh` / `./test.sh` invocation. To rebuild from source:

```sh
make -C userland/apps/busybox                            # builds build/busybox
./userland/refresh-prebuilt.sh                           # refreshes userland/prebuilt/BB.ELF
```

Or force the prebuilt-pipeline integration:

```sh
REBUILD_BUSYBOX=1 ./build.sh
./build.sh --rebuild-userland   # rebuilds zsh + busybox + any future prebuilt-managed apps
```

Requires `x86_64-linux-musl-gcc` (set `MUSL_CC` to override the binary
name). Tested with musl-cross-make 15.x via Homebrew.

## Pinned upstream

| Component | Version | Source                                   |
|-----------|---------|------------------------------------------|
| BusyBox   | 1.36.1  | https://busybox.net/downloads/           |

Bumping the version requires bumping `BUSYBOX_SHA256` in the `Makefile`
in lockstep — the SHA verifier hard-fails on mismatch.

## Configuration

`busybox.config` carries our overrides on top of upstream `defconfig`:

- `CONFIG_STATIC=y` — static link against musl.
- `CONFIG_PIE=n` — kernel ELF loader rejects ET_DYN.
- Disabled categories: init/shutdown, login/passwd/accounts, daemons
  (cron, syslog, watchdog, …), module loading, mount/swap/blockdev/mkfs,
  unsupported networking daemons/interface tools, TTY/console manipulation, time/clock
  adjustment, IPC, namespaces, free/top/iostat (no /proc).
- Enabled networking is deliberately limited to IPv4 `ping`, `nc`, and
  HTTP-only `wget`. HTTPS/TLS, IPv6, `udhcpc`, interface configuration, and
  resolver-dependent demos remain disabled.

The Makefile applies these as `make defconfig && (strip overridden keys)
&& cat busybox.config >> .config && make oldconfig`, so any new applets
that appear in a future BusyBox release inherit upstream defaults unless
explicitly disabled here.

## Regenerating the config

If a new BusyBox release introduces applets you want to disable (or
re-enable), add or remove the relevant `CONFIG_<APPLET>=n` lines in
`busybox.config`. Avoid checking in a full `.config` snapshot — the
override-on-defconfig pattern keeps the diff readable.

## Read-only filesystem caveat

The kernel's FAT mount is read-only. BusyBox write-side applets (`cp`,
`mv`, `rm`, `mkdir`, `touch`, `chmod`, `chown`, `ln`, `dd`) ship in
`BB.ELF` and resolve via `/bin/<applet>`, but every `write()` to a
file-backed FD returns `EROFS`. The applets surface the error cleanly —
they do not panic the kernel. When write support lands in `src/fs/`,
they begin to work without further changes here.

## Networking caveat

The kernel currently exposes numeric IPv4 sockets only: DHCP-provided DNS
servers are recorded, but libc name lookup and TLS are not implemented. In the
default interactive QEMU network these are representative commands:

```sh
ping -c 1 10.0.2.2
nc -z -w 2 10.0.2.2 80
wget -O- http://10.0.2.2:8000/
```

The last two require a service listening at the chosen numeric address/port;
the automated suite provides its own QEMU-local endpoints. Do not advertise
hostname URLs or HTTPS until the resolver and certificate/time work lands.

## Applet list ↔ kernel sync

`src/userland/bin_namespace.rs` carries the kernel's view of which
`/bin/<name>` paths are valid. It MUST stay in sync with the applets
actually compiled into `BB.ELF`. To regenerate after a config change:

```sh
make -C userland/apps/busybox
./build/busybox --list | sort
# compare against APPLETS in src/userland/bin_namespace.rs
```

A kernel-side entry that the binary doesn't actually provide is a soft
failure: BusyBox prints `applet not found` and exits non-zero. A missing
kernel-side entry is a louder failure: PATH lookup returns ENOENT and
the applet looks like it isn't installed.
