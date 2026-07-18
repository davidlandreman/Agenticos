# GCC

GCC 14.2.0 built as a native x86-64 C compiler for AgenticOS: the `gcc`
driver, `cpp`, `cc1`, `collect2`, `libgcc.a`, the CRT begin/end objects, and
GCC's internal header set, all as static-musl executables.

The source archive is fetched from GNU and verified against the SHA256
pinned in `Makefile`. GMP, MPFR, and MPC ride in-tree at the exact versions
gcc-14.2.0 pins — the tarball names are parsed from the shipped
`contrib/download_prerequisites` and verified against the SHA512 manifest
shipped in the same tree, so those pins live upstream. The build is an
out-of-tree Canadian cross (build = macOS, host = target =
x86_64-linux-musl) using the same Homebrew `x86_64-linux-musl-gcc` 14.2.0
toolchain as binutils; matching native and cross versions keeps
libgcc/configure assumptions skew-free. The suite is GPLv3-or-later; the
verified source URLs and the complete corresponding build recipe are
recorded in `Makefile`.

Unlike the flat binutils ELFs, the shipped artifact is
`gcc-install.tar.gz` — a pruned install prefix. The tree layout is
load-bearing: the driver is configured with `--prefix=/host/gcc` and finds
`cc1`/`collect2` in `libexec/gcc/x86_64-linux-musl/14/` and
`libgcc.a`/CRT/headers in `lib/gcc/x86_64-linux-musl/14/` through that
prefix, with no `-B`, `GCC_EXEC_PREFIX`, or PATH conventions
(`--with-gcc-major-version-only` keeps the version directory short for the
FAT layer). `--disable-fixincludes` places GCC's `limits.h`/`syslimits.h`
in the ordinary internal `include/` directory, so the machine-generated
`include-fixed` tree is empty and pruned. With `--disable-shared` the EH
objects fold into `libgcc.a`; there is no separate `libgcc_eh.a`.

The assembler and linker are deliberately NOT configured with
`--with-as`/`--with-ld`: configure executes those paths on the build host
for feature probes, and `/bin/as` exists on macOS as a Mach-O assembler.
Feature probes use the cross toolchain's `x86_64-linux-musl-as`, and the
installed driver falls back to PATH lookup on-target — the default process
environment (`PATH=/bin:/host`) resolves the shipped GNU binutils at
`/bin/as` and `/bin/ld`.

## Guest use

`gcc` is exposed at `/bin/gcc`. `--with-sysroot=/host/sysroot` shares the
musl sysroot with TinyCC and GNU `ld`
(`--with-native-system-header-dir=/include` matches its `include/` + `lib/`
layout). Temp files go to `/tmp` (provisioned on the overlay at every
boot); writable outputs belong under `/work` or `/data`.

```sh
cd /work
gcc -O2 -o hello /host/sysroot/examples/hello.c
./hello
gcc -c part1.c && gcc -c part2.c && gcc -o prog part1.o part2.o
gcc -S hello.c        # then: as --64 hello.s -o hello.o; ld -static ...
```

Scope matches the port plan: C only (`--enable-languages=c`), static-only
(`make validate` rejects `PT_INTERP`/`DT_NEEDED`), no LTO/plugins,
sanitizers, gcov workflows, decimal float, or fixed-point. `gcov`,
`gcc-ar`-style wrappers, target-prefixed driver copies, `lto-wrapper`,
`libgcov.a`, `install-tools`, and `share/` are pruned from the shipped
tree. C++ (`cc1plus`/libstdc++) and on-target self-hosting are deliberate
follow-ups.

The stripped shipped set is ~32 MiB uncompressed (cc1 27.3 MiB, driver and
cpp 1.8 MiB each, collect2 1.1 MiB, plus libgcc/CRT/headers);
`gcc-install.tar.gz` is ~13.7 MiB.

## Rebuild

```sh
make -C userland/apps/gcc validate
REBUILD_GCC=1 ./build.sh
./userland/refresh-prebuilt.sh
```
