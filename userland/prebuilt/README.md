# Prebuilt userland ELFs

Committed binaries for userland apps whose source build pulls upstream
tarballs and / or takes long enough that running it on every kernel
iteration is friction without payoff. `build.sh` and `test.sh` copy
these into `host_share/` by default, so a fresh clone reaches a working
zsh prompt without the `x86_64-linux-musl-cross` toolchain installed
and without an outbound network fetch.

## What lives here

| File       | Source                | Type | Size      | Notes                                      |
|------------|-----------------------|------|-----------|--------------------------------------------|
| `ZSH.ELF`  | `userland/apps/zsh/`  | EXEC | ~1.5 MiB  | static-musl zsh, vendors ncurses-widec     |

(Add a row when a new prebuilt-managed app lands. Keep size approximate
— the reviewer uses it to gut-check binary diffs, not for exactness.)

## When to refresh

Refresh the prebuilt and commit the result whenever you change anything
that affects the binary output for one of the apps above:

- `userland/apps/<app>/Makefile` (compile / link flags, upstream version)
- patches under `userland/apps/<app>/patches/` (if any)
- pinned tarball SHA256 changes

Refresh workflow:

```sh
./userland/refresh-prebuilt.sh           # rebuilds all prebuilt-managed apps
git add userland/prebuilt/<NAME>.ELF userland/apps/<app>/
git commit -m "userland(<app>): <change>; refresh prebuilt"
```

`refresh-prebuilt.sh` hard-fails on any build problem and prints
`git status userland/prebuilt/` when finished. It does NOT auto-commit
— the developer stages and writes the commit message.

## Skipping the prebuilt on a single build

```sh
./build.sh --rebuild-userland       # rebuild every prebuilt-managed app
REBUILD_ZSH=1 ./build.sh            # rebuild just zsh this run
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

The rule of thumb: an app belongs here if (a) its build fetches an
upstream tarball, or (b) its compile takes long enough that running
it on every `./build.sh` would slow down kernel iteration. New Linux
ports (bash, vim, coreutils, …) will land here.

## Repo-size note

`userland/prebuilt/` is tracked plain in git — no LFS. The current
total (~1.5 MiB) is below the ~50 MiB threshold where LFS pays for
itself. If we cross that, revisit and switch.

## Reproducibility

Two developers with different musl-cross versions can produce
byte-different binaries from identical source. That's OK — refreshes
are deliberate, and a binary-only diff PR with no source change is
legitimate as long as the commit message names what changed (e.g.
"bump musl-cross 11.2 → 13.2; refresh ZSH.ELF"). Long-term we may
pin the cross toolchain; not done yet.
