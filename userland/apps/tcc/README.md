# TinyCC (tcc) — on-target C compiler

Static musl TinyCC for AgenticOS: compiler, assembler, and linker in one
~400 KiB `ET_EXEC` binary, plus the minimal musl sysroot it compiles
against inside the guest. TinyCC is the deliberate stepping stone toward
GCC — it exercises the same kernel surface (many header reads, large
output writes, seek semantics, chmod, execve of fresh binaries) without
GCC's multi-process driver and build machinery.

## In-guest usage

```sh
cd /work                                        # writable overlay scratch dir
tcc -o hello /host/sysroot/examples/hello.c     # compile + link, no flags needed
./hello
tcc -c /host/sysroot/examples/args.c -o args.o  # separate compile...
tcc -o args args.o                              # ...and link
cc -o hello /host/sysroot/examples/hello.c      # `cc` is an alias
```

Defaults compiled in at configure time (no flags, no env, no `-B`):

- system includes: `/host/sysroot/lib/tcc/include`, `/host/sysroot/include`
- libraries: `/host/sysroot/lib/tcc`, `/host/sysroot/lib`
- crt objects: `/host/sysroot/lib` (`crt1.o`, `crti.o`, `crtn.o`)
- **implicit `-static`** (`--tcc-switches=-static`): the kernel loader
  rejects `PT_INTERP`, so dynamic output would be unexecutable.

`tcc -run` (in-memory JIT) is **not supported**: it mmaps one RWX region
and the kernel enforces W^X. If it's ever wanted, the path is a small
patch to `tccrun.c` to map RW, write the code, then `mprotect` to R|X
(the kernel allows RW→RX transitions).

## Artifacts

| Committed artifact | Built from | Staged as |
|---|---|---|
| `userland/prebuilt/TCC.ELF` | tinycc mob snapshot (pinned) | `/host/TCC.ELF` |
| `userland/prebuilt/tcc-sysroot.tar.gz` | musl-cross toolchain + tcc build + `examples/` | `/host/sysroot/` |

The sysroot tarball contains pruned musl + linux-uapi headers (no `c++/`,
`drm/`, `sound/`, `rdma/`, `scsi/`), `crt1.o`/`crti.o`/`crtn.o`/`libc.a`,
the empty POSIX compat archives (`libm.a`, `libpthread.a`, …) so `-lm`
style link lines work, TCC's private headers plus `libtcc1.a` under
`lib/tcc/`, and the example sources.

## Source pin

TinyCC `mob` branch snapshot `d9d02c56401e43be43760b63f7d82f771a7ed1f6`
(the 0.9.27 release predates `--config-musl`), fetched from repo.or.cz
with a pinned SHA256 the Makefile hard-fails on. Bump the commit and SHA
in lockstep, then run `./userland/refresh-prebuilt.sh` and commit the
regenerated artifacts.

## Cross-build notes (macOS host)

Toolchain: `x86_64-linux-musl-gcc` (Homebrew `musl-cross`), override with
`MUSL_CC`. Two upstream-supported mechanisms make the cross build work
without source patches:

1. `c2str.exe` (generates `tccdefs_.h`) must run on the build host; the
   Makefile pre-generates both with the host `cc` after configure.
2. `libtcc1.a` is normally compiled by the freshly built `tcc` — a Linux
   binary the host can't run. `make x86_64-libtcc1-usegcc=yes` builds it
   with the cross gcc instead (upstream switch, see `lib/Makefile`).

The sysroot's headers and libs are copied from `$(MUSL_CC) -print-sysroot`.
