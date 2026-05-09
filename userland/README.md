# Userland

Sibling cargo project for ring-3 user apps that run on AgenticOS.

This is **not** a member of the kernel's workspace. It has its own
`target/`, its own `build-std`, its own custom linker script, and is built
independently before the kernel by `build.sh` / `test.sh`. The resulting ELF
is staged into `host_share/` (visible inside the guest at `/host/`) so the
shell can load it with `run /HOST/<NAME>.ELF`.

See the userland app platform plan at
`docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md` for the full
design.

## Layout

```
userland/
├── Cargo.toml          # workspace manifest (members = runtime, apps/hello)
├── .cargo/config.toml  # target = x86_64-unknown-none, build-std
├── linker.ld           # ENTRY(_start), base 0x40_0000, no PT_INTERP
├── runtime/            # tiny library: print, exit syscall stubs (int 0x80)
└── apps/
    └── hello/          # the first app — prints "hello\n", exits 0
```

## Building

`build.sh` and `test.sh` build userland automatically and stage
`apps/hello/target/.../hello` as `host_share/HELLO.ELF` (uppercase 8.3).

To build by hand:

```sh
cargo build --release --manifest-path userland/Cargo.toml
```

The output ELF lives at
`userland/target/x86_64-unknown-none/release/hello`.

## Adding a new app

1. `mkdir -p userland/apps/<name>/src`
2. Add `userland/apps/<name>/Cargo.toml` (mirror `apps/hello/Cargo.toml`,
   replacing the package name and `[[bin]]` entry).
3. Add `userland/apps/<name>/build.rs` (mirror
   `apps/hello/build.rs`, replacing `bin=hello=` with `bin=<name>=`). The
   build script wires the linker script and `-z` flags per binary.
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

6. Update `build.sh` / `test.sh` to stage the new binary at an uppercase 8.3
   filename in `host_share/` (e.g., `BYE.ELF`).
7. Rebuild and reboot — see the snapshot caveat below.

## Constraints to honor

- **`no_std`, `panic = "abort"`.** The release profile in
  `userland/Cargo.toml` already enforces these for the workspace.
- **Static, non-PIE, ET_EXEC.** The linker script + per-binary `-static -no-pie`
  rustflags lock this in. The U6 ELF loader only accepts this format.
- **Output size ≤ 64 KiB.** FAT16 / vvfat is friendlier under the limit and
  the loader caps file reads.
- **Filename must be uppercase 8.3.** vvfat exposes only 8.3 names; lowercase
  or longer names get truncated or refused. `HELLO.ELF` is fine; `hello.elf`
  or `MYAPP_TEST.ELF` are not.
- **Syscall ABI is name-keyed at the kernel layer (D4 in the plan).** The
  runtime here uses the *numeric-stub* fallback path: `print`/`exit` issue
  `int 0x80` directly with the kernel-assigned IDs (`0` and `1`). The
  symbol-keyed GOT-relocation path (where the loader patches relocations to
  point into the kernel's user-trampoline page at `0x0090_0000`) was found
  to be too fragile in `lld -static -no-pie`, which refuses to emit
  `R_X86_64_GLOB_DAT` / `R_X86_64_JUMP_SLOT` against undefined externals.
  The U6 loader's relocation walk is happy with binaries that have no
  relocations at all — it just finds nothing to resolve. The trampoline
  page remains mapped (it's needed for U7's iretq-to-ring-3 setup) and is
  available for a future toolchain that does emit those relocations.

## Iteration cycle

The host-share mount is **read-only** and **snapshots at QEMU launch**. To
test a userland edit:

1. Edit code under `userland/`.
2. Run `./build.sh` (or `./test.sh`). Both rebuild userland, restage
   `host_share/HELLO.ELF`, and re-launch QEMU with the fresh snapshot.
3. Inside the guest, type `run /HOST/HELLO.ELF`.

You **cannot** edit the file mid-boot and re-run; vvfat will keep showing
the snapshot from launch time. This is unchanged from the host folder mount
landed in `681ef89`.

## Toolchain notes

- The kernel's nightly toolchain (`rust-toolchain.toml` at repo root) is
  reused. `build-std = ["core", "compiler_builtins"]` plus
  `compiler-builtins-mem` covers all `mem*` shim needs.
- The hello app's `build.rs` emits per-binary linker arguments via
  `cargo:rustc-link-arg-bin=hello=...` so the linker script and `-z` flags
  apply only to the final binary, not to the `runtime` rlib.
