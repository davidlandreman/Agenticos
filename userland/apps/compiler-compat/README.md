# Static-musl compiler compatibility fixtures

These small C programs form the mandatory booted `compiler_compat` test
ladder. They are ordinary compiler- and musl-produced x86-64 binaries, unlike
the hand-encoded ELF fixtures in `src/tests/userland_fixtures.rs`.

| Fixture | Coverage |
|---|---|
| `CCCRT.ELF` | musl CRT, initial argc/argv stack, TLS setup, normal `main` return |
| `CCLIBC.ELF` | envp, malloc/realloc/free, demand-grown stack, time, random, uname |
| `CCPROBE.ELF` | unknown-syscall `ENOSYS` fallback followed by file access/stat/read |

The committed binaries live in `userland/prebuilt/compiler-compat/`. Normal
`test.sh` runs only copy those artifacts and do not require a musl compiler.

Refresh with an x86-64 musl cross toolchain:

```sh
make -C userland/apps/compiler-compat refresh
```

On a non-x86 host, a native x86-64 Alpine container is also sufficient:

```sh
docker run --rm --platform linux/amd64 \
  -v "$PWD:/src" -w /src/userland/apps/compiler-compat gcc:14-alpine \
  make refresh MUSL_CC=gcc READELF=readelf STRIP=strip
```

The Makefile rejects PIE/dynamic output: every artifact must be ELF64 x86-64
`ET_EXEC`, statically linked, with no `PT_INTERP` segment.

Current committed artifacts were built with Alpine 3.20's GCC 13.2.1 and musl
1.2.5:

| Fixture | Bytes | SHA-256 |
|---|---:|---|
| `CCCRT.ELF` | 13,448 | `564c94e49e559e4db014cfed52c0bd81540accae4198397cd18a99ceca403a89` |
| `CCLIBC.ELF` | 25,824 | `86e7712434cf23a42ca5018f41f136202019d18bb1a0174c4ae5af4e93a96e7b` |
| `CCPROBE.ELF` | 13,448 | `c093e51d8ffb3c4fa08f672183a28f142728dc6be0315f57ffbf4ab61c7466e1` |
