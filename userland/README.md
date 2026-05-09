# Userland

Sibling project tree for ring-3 user apps that run on AgenticOS.

The rust apps and the rust runtime form a Cargo workspace separate from
the kernel's. C++ apps live next to them under `apps/<name-cpp>/` but are
not Cargo members — they have their own `Makefile` driving the host's
`x86_64-linux-musl-g++` cross-compiler. Both kinds of apps are built
before the kernel by `build.sh` / `test.sh` and staged into `host_share/`
(visible in the guest at `/host/`) so the shell can load them with
`run /HOST/<NAME>.ELF`.

The userland speaks Linux x86-64 ABI — the kernel's `syscall` fast-path
handler accepts Linux numbers directly. See `src/userland/abi.rs` for
the dispatcher and the `nr` constants.

See the userland app platform plan at
`docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` for the
historical design and `docs/plans/2026-05-09-001-feat-userland-linux-abi-cpp-hello-plan.md`
for the Linux-ABI cutover and the C++ pipeline.

## Layout

```
userland/
├── Cargo.toml          # workspace manifest (members = runtime, apps/hello)
├── .cargo/config.toml  # target = x86_64-unknown-none, build-std
├── linker.ld           # ENTRY(_start), base 0x40_0000, no PT_INTERP
├── runtime/            # tiny library: print, exit (Linux syscall stubs)
└── apps/
    ├── hello/          # rust app — prints "hello\n", exits 0
    └── hello-cpp/      # C++ app — std::cout, exits 0
        ├── Makefile    # invokes x86_64-linux-musl-g++ -static -no-pie
        └── src/main.cpp
```

`hello-cpp` is **not** a Cargo workspace member — Cargo doesn't host C++
projects, and the C++ build uses the host's musl-based g++ cross-compiler
instead of the kernel's nightly Rust target. `build.sh` / `test.sh`
invoke its `Makefile` separately.

## Building

`build.sh` and `test.sh` build both apps automatically when the host has
the toolchains. The rust hello stages as `host_share/HELLO.ELF`; the
C++ hello stages as `host_share/HELLOCPP.ELF`.

To build by hand:

```sh
# Rust app
cargo build --release --manifest-path userland/Cargo.toml

# C++ app (requires x86_64-linux-musl-g++)
make -C userland/apps/hello-cpp
```

The output binaries land at:

- `userland/target/x86_64-unknown-none/release/hello`
- `userland/apps/hello-cpp/build/hello-cpp`

### C++ host toolchain

`build.sh` / `test.sh` probe for `x86_64-linux-musl-g++` on `PATH`. When
absent, both scripts emit a one-line warning and skip the C++ stage —
the kernel build, rust userland, and kernel test suite all still run, so
day-to-day kernel iteration doesn't require the C++ toolchain.

Install hint (macOS / Homebrew):

```sh
brew install x86_64-linux-musl-cross
```

Or build a toolchain from source via [musl-cross-make](https://github.com/richfelker/musl-cross-make).
The `MUSL_GXX` environment variable overrides the default binary name:

```sh
MUSL_GXX=/opt/musl-cross/bin/x86_64-linux-musl-g++ ./build.sh
```

Both scripts run `readelf -h` on the produced binary and assert
`Type: EXEC`. Some toolchains default to PIE even when `-no-pie` is
passed at link time; the readelf check fails the build loud rather than
deferring the surprise to a confusing kernel-side rejection at run time.

## Adding a new rust app

1. `mkdir -p userland/apps/<name>/src`
2. Add `userland/apps/<name>/Cargo.toml` (mirror `apps/hello/Cargo.toml`,
   replacing the package name and `[[bin]]` entry).
3. Add `userland/apps/<name>/build.rs` (mirror `apps/hello/build.rs`,
   replacing `bin=hello=` with `bin=<name>=`).
4. Add the new app to `userland/Cargo.toml`'s `members` list.
5. Write `src/main.rs`:

   ```rust
   #![no_std]
   #![no_main]

   use runtime::{exit, print};

   #[no_mangle]
   pub unsafe extern "C" fn _start() -> ! {
       let msg = b"goodbye\n";
       let _ = print(msg.as_ptr(), msg.len());
       exit(0);
   }

   #[panic_handler]
   fn panic(_info: &core::panic::PanicInfo) -> ! { unsafe { exit(1) } }
   ```

6. Update `build.sh` / `test.sh` to stage the new binary at an uppercase
   8.3 filename in `host_share/` (e.g., `BYE.ELF`).
7. Rebuild and reboot — see the snapshot caveat below.

## Adding a new C++ app

1. `mkdir -p userland/apps/<name>-cpp/src`
2. Mirror `userland/apps/hello-cpp/Makefile`, changing the `BIN` target.
3. Write `src/main.cpp` — anything compilable with `x86_64-linux-musl-g++`
   that calls `exit_group` either explicitly or via `return` from `main`.
4. Update `build.sh` / `test.sh` to invoke the new `make` target and
   stage the binary at an uppercase 8.3 filename in `host_share/`.

### libstdc++ buffering caveat

The kernel returns `-ENOTTY` for `ioctl(fd, TCGETS, ...)`, which makes
libstdc++'s underlying stdio pick *full buffering* for stdout. With full
buffering, a trailing `"\n"` without an explicit flush is dropped on
`exit_group`. Use `std::endl` (or call `std::cout.flush()`) so the line
lands on serial before the process exits:

```cpp
std::cout << "hello" << std::endl;   // good
std::cout << "hello\n";              // dropped without flush
```

A real tty subsystem will land later; for now this is a host-source
convention.

## Constraints to honor

- **Static, non-PIE, ET_EXEC.** The kernel ELF loader only accepts this
  format. Rust apps inherit this from `userland/linker.ld` plus per-binary
  `-static -no-pie` rustflags. C++ apps inherit this from the `Makefile`'s
  `-static -no-pie -fno-pie`.
- **Linux x86-64 ABI.** The runtime stubs and any inline `syscall`
  instruction follow the Linux convention: nr in RAX, args in RDI/RSI/
  RDX/R10/R8/R9, return in RAX, errors as `-errno`. RCX and R11 are
  clobbered by the `syscall` instruction itself.
- **Filename must be uppercase 8.3.** vvfat exposes only 8.3 names —
  `HELLO.ELF`, `HELLOCPP.ELF` work; `hello.elf` or `MYAPP_TEST.ELF`
  don't.
- **C++ binary size.** A static `g++ -static -no-pie` C++ iostream
  binary typically lands between 1 and 4 MiB. The run command caps user
  binaries at 16 MiB (see `src/commands/run/mod.rs::MAX_USER_BINARY_BYTES`)
  with a clear error when exceeded.

## Iteration cycle

The host-share mount is **read-only** and **snapshots at QEMU launch**.
To test a userland edit:

1. Edit code under `userland/`.
2. Run `./build.sh` (or `./test.sh`). Both rebuild every available
   userland, restage `host_share/*.ELF`, and re-launch QEMU with the
   fresh snapshot.
3. Inside the guest, type `run /HOST/HELLO.ELF` or
   `run /HOST/HELLOCPP.ELF`.

You **cannot** edit the file mid-boot and re-run; vvfat will keep
showing the snapshot from launch time.

## Toolchain notes

- **Rust:** the kernel's nightly toolchain (`rust-toolchain.toml` at
  repo root) is reused. `build-std = ["core", "compiler_builtins"]`
  plus `compiler-builtins-mem` covers all `mem*` shim needs.
- **C++:** any `x86_64-linux-musl` toolchain with libstdc++ works.
  Recommended: musl-cross-make 13+ via Homebrew or built from source.
  No glibc — glibc-static binaries are NSS-fragile and would force a
  much wider kernel surface for the same hello-world result.
- The hello app's `build.rs` emits per-binary linker arguments via
  `cargo:rustc-link-arg-bin=hello=...` so the linker script and `-z`
  flags apply only to the final binary, not to the `runtime` rlib.
