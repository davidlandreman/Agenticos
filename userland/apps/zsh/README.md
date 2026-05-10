# ZSH userland app

Statically-linked `zsh` (Z shell) for AgenticOS, built against musl
+ ncurses-widec via the host's `x86_64-linux-musl` cross-compiler.

The Makefile fetches both source tarballs from upstream on first build,
verifies SHA256, builds ncurses (libraries only, no progs) into a local
prefix, then builds zsh against that prefix and ships a stripped static
ET_EXEC at `build/zsh`. `build.sh` and `test.sh` stage that binary as
`host_share/ZSH.ELF` so the guest can `run /HOST/ZSH.ELF`.

See `docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md` for the
overall plan; this directory implements U1.

## Versions (pinned)

| Component | Version | SHA256 (upstream tarball) |
|---|---|---|
| zsh     | 5.9 | `9b8d1ecedd5b5e81fbf1918e876752a7dd948e05c1a0dba10ab863842d45acd5` |
| ncurses | 6.5 | `136d91bc269a9a5785e5f9e980bc76ab57428f604ce3e5a5a90cebc767971cc6` |

The pin matters: the zsh-on-AgenticOS plan asserts musl x86_64 does not
call `rseq` or `set_robust_list` on the main thread, and that zsh's
`acquire_pgrp` clears `MONITOR` on `ioctl(TIOCGPGRP) → -ENOTTY`. Both
were verified against zsh 5.9. A version bump should re-verify both.

The ncurses 6.5 + GCC 15 combination triggers an autoconf-time mismatch
in zsh 5.9: zsh's `boolcodes` symbol detection probes `char **test =
boolcodes;` but ncurses 6.x exports `const char *const boolcodes[]`.
GCC 15 promoted `incompatible-pointer-types` from warning to error, so
the detection fails silently, zsh defines its own `boolcodes` locally,
and the link conflicts. The Makefile passes
`-Wno-error=incompatible-pointer-types` (plus
`-Wno-error=implicit-function-declaration` and
`-Wno-error=int-conversion` for the same class of issue) so the
detection succeeds, zsh skips its local definition, and the build links
cleanly against ncurses' canonical symbols.

## Build

`build.sh` and `test.sh` build zsh automatically when `MUSL_CC` is on
PATH (default: `x86_64-linux-musl-gcc`). Install hint for macOS:

```sh
brew install x86_64-linux-musl-cross
```

To build by hand:

```sh
make -C userland/apps/zsh
```

Output: `userland/apps/zsh/build/zsh`. First build downloads ~3.2 MiB of
zsh source plus ~3.5 MiB of ncurses source into `build/tarballs/`,
extracts both, builds ncurses (~1 min) and then zsh (~30 s) on a recent
laptop.

Override the toolchain:

```sh
make -C userland/apps/zsh MUSL_CC=/opt/musl-cross/bin/x86_64-linux-musl-gcc
```

## Configure flags (and why)

| Flag | Reason |
|---|---|
| `--disable-dynamic` | The kernel loader rejects `PT_INTERP`. Static-only. |
| `--disable-cap` | No libcap on AgenticOS. |
| `--disable-pcre` | No libpcre on AgenticOS. |
| `--disable-restricted-r` | Skip the `rzsh` restricted-shell symlink. |
| `--disable-etcdir` and `--disable-z{shenv,shrc,login,profile,logout}` | Skip all `/etc/zsh*` lookups at startup. Equivalent to forced `--no-rcs --no-globalrcs` at build time — fewer `openat` calls during startup. |
| `--without-tcsetpgrp` | Belt-and-suspenders with the kernel's `ioctl(TIOCGPGRP) → -ENOTTY` trick (U5). Either alone should disable MONITOR; combining both removes a foot-gun. |
| `--enable-multibyte` | Leave UTF-8 on. Disabling has been broken for years and we want UTF-8 in any case. |

`-static -no-pie` link mode is mandatory (the loader rejects ET_DYN);
the `readelf` check in `build.sh` asserts `Type: EXEC` on every build.

## Iteration

The `build/` tree is `.gitignore`d. `make clean` removes extracted
sources and the install prefix but keeps the `build/tarballs/` cache so
re-builds skip network. `make distclean` blows everything away.
