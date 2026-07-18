# Prebuilt userland ELFs

Committed binaries for userland apps whose source build pulls upstream
tarballs and / or takes long enough that running it on every kernel
iteration is friction without payoff. `build.sh` and `test.sh` copy
these into `host_share/` by default, so a fresh clone reaches a working
zsh prompt without the `x86_64-linux-musl-cross` toolchain installed
and without an outbound network fetch.

Zsh's toolchain-independent companion artifacts live next door under
`userland/zsh-config/`: `/etc/zshrc`, the vendored agnoster theme, and a
pruned zsh 5.9 function library. They are committed and staged on every build,
including `test.sh --skip-userland`; rebuilding zsh refreshes the function
copies from the same pinned source tarball.

## What lives here

| File | Source | Ship kind | Notes |
|---|---|---|---|
| `ZSH.ELF` | `apps/zsh/` | prebuilt-managed | static-musl zsh + ncurses-widec |
| `BB.ELF` | `apps/busybox/` | prebuilt-managed | BusyBox including ping/nc/HTTP wget |
| `TCC.ELF` | `apps/tcc/` | prebuilt-managed | static-musl TinyCC (compiler+assembler+linker, ~0.4 MiB) |
| `LINKS.ELF` | `apps/links2/` | prebuilt-managed | static-musl Links 2.30 + OpenSSL, text + native GUI IPv4 HTTP(S) (~10 MiB) |
| `binutils/*.ELF` | `apps/binutils/` | prebuilt-managed | GNU binutils 2.46.0, 14 stripped static native tools (~15.5 MiB total) |
| `tcc-sysroot.tar.gz` | `apps/tcc/` | prebuilt-managed (tree) | musl headers + crt/libc + libtcc1 + examples; extracted to `host_share/sysroot/` by `stage_tcc_sysroot` (~1.9 MiB) |
| `compiler-compat/CCCRT.ELF` | `apps/compiler-compat/` | test-fixture | CRT startup rung |
| `compiler-compat/CCLIBC.ELF` | `apps/compiler-compat/` | test-fixture | libc/heap rung |
| `compiler-compat/CCPROBE.ELF` | `apps/compiler-compat/` | test-fixture | fallback/filesystem rung |
| `network/NETTEST.ELF` | `apps/network-test/` | test-fixture | static-musl socket smoke |

(Add a row when a new prebuilt-managed app lands. Keep size approximate
— the reviewer uses it to gut-check binary diffs, not for exactness.)

## When to refresh

Refresh the prebuilt and commit the result whenever you change anything
that affects the binary output for one of the apps above:

- `userland/apps/<app>/Makefile` (compile / link flags, upstream version)
- patches under `userland/apps/<app>/patches/` (if any)
- pinned tarball SHA256 changes

Refresh workflow (also refreshes `userland/zsh-config/functions/`):

```sh
./userland/refresh-prebuilt.sh           # rebuilds all prebuilt-managed apps
git add userland/prebuilt/<NAME>.ELF userland/apps/<app>/ userland/zsh-config/
git commit -m "userland(<app>): <change>; refresh prebuilt"
```

The artifact list comes from `userland/apps.manifest.sh`.
`refresh-prebuilt.sh` hard-fails on any build problem and prints
`git status userland/prebuilt/` when finished. It does NOT auto-commit
— the developer stages and writes the commit message.

## Skipping the prebuilt on a single build

```sh
./build.sh --rebuild-userland       # rebuild every prebuilt-managed app
REBUILD_ZSH=1 ./build.sh            # rebuild just zsh this run
REBUILD_TCC=1 ./build.sh            # rebuild tcc + its sysroot tarball
REBUILD_LINKS2=1 ./build.sh         # rebuild the Links text + GUI browser
REBUILD_BINUTILS=1 ./build.sh       # rebuild all fourteen GNU binutils tools
```

The same flag / env vars work on `test.sh`. With no flag and a prebuilt
present, neither script invokes `make` for the upstream app and neither
script probes for the musl toolchain — fresh clones get no toolchain
noise.

## What does NOT live here

- `HELLO.ELF` (Rust) — builds in seconds using the kernel's standard
  Rust toolchain. No upstream fetch. Built every run.
- `HELLOCPP.ELF` (C++ wrapper) — small in-tree source, no upstream
  fetch. The `x86_64-linux-musl-g++` toolchain is the only requirement
  and is already needed to rebuild zsh anyway. Built every run.

The `compiler-compat/` subdirectory is a separate category: tiny committed
static-musl test inputs, not interactive apps. `test.sh` always stages them,
including with `--skip-userland`, so the booted `compiler_compat` module is
hermetic on machines without a musl cross compiler. Their sources and refresh
recipe live in `userland/apps/compiler-compat/`.

The `network/` subdirectory is likewise a mandatory test-input category.
`NETTEST.ELF` is a self-checking static-musl socket fixture; `test.sh` stages
it even with `--skip-userland`. Its source and refresh recipe live in
`userland/apps/network-test/`.

The rule of thumb: an app belongs here if (a) its build fetches an
upstream tarball, or (b) its compile takes long enough that running
it on every `./build.sh` would slow down kernel iteration. New Linux
ports (bash, vim, coreutils, …) will land here.

## Repo-size note

`userland/prebuilt/` is tracked plain in git — no LFS. The current
total is about 21 MiB, below the ~50 MiB threshold where LFS pays for
itself. If we cross that, revisit and switch.

## Reproducibility

Two developers with different musl-cross versions can produce
byte-different binaries from identical source. That's OK — refreshes
are deliberate, and a binary-only diff PR with no source change is
legitimate as long as the commit message names what changed (e.g.
"bump musl-cross 11.2 → 13.2; refresh ZSH.ELF"). Long-term we may
pin the cross toolchain; not done yet.
