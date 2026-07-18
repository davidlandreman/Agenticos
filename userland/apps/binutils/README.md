# GNU binutils

GNU binutils 2.46.0 built as fourteen static-musl x86-64 userspace tools for
AgenticOS:

```text
addr2line ar as c++filt elfedit ld nm objcopy objdump
ranlib readelf size strings strip
```

The source archive is fetched from GNU and verified against the SHA256 pinned
in `Makefile`. The macOS build is an out-of-tree Canadian cross: build tools
run on the host, while the shipped programs use `x86_64-linux-musl-gcc`.
Because upstream links programs through libtool, `-all-static -no-pie` is
required; `make validate` rejects `PT_INTERP` and `DT_NEEDED`.
The upstream suite is GPLv3-or-later; the verified source URL and complete
corresponding build recipe are recorded in `Makefile`.

The stripped committed suite is 16,245,912 bytes (about 15.5 MiB): `elfedit`
is 63 KiB; `readelf` is 838 KiB; most inspection/archive tools are 963–1080
KiB; `as` is 1.56 MiB; `objdump` is 2.06 MiB; and `ld` is 2.25 MiB.

## Guest use

The tools are exposed at their conventional `/bin/<name>` paths. Writable
outputs belong under `/work` or `/data`; `/host` and the bundled musl sysroot
are read-only.

```sh
cd /work
as --64 /host/BINUTILS/EXIT42.S -o exit42.o
ld -static -o exit42 exit42.o
./exit42
ar rcs libprobe.a exit42.o
readelf -h exit42
objdump -d exit42
```

GNU `ld` is configured with `/host/sysroot` and `/host/sysroot/lib`, matching
the sysroot shipped for TinyCC. The initial port is static-only and disables
GDB, gold, profilers, plugins/LTO, debuginfod, NLS, and non-native targets.
GNU `strings` replaces the BusyBox applet at `/bin/strings` so every binutils
command has one unambiguous namespace owner.

The booted acceptance suite runs every program with unknown-syscall tracing,
then exercises real read and write workflows. It established four required
ABI details: `readv(2)`, accurate regular-file `fcntl(F_GETFL)`, per-process
`umask(2)`, and `utimensat(2)` timestamp preservation. The accepted workflows
emit no unknown syscall; no optional probe exceptions are currently needed.
This native assembler/linker layer is also the intended binary-utilities
foundation for the future GCC port; GCC itself and dynamic linking remain out
of scope here.

## Rebuild

```sh
make -C userland/apps/binutils validate
REBUILD_BINUTILS=1 ./build.sh
./userland/refresh-prebuilt.sh
```

Normal builds copy committed ELFs from `userland/prebuilt/binutils/`, so a
fresh checkout does not need the musl cross toolchain or network access.
