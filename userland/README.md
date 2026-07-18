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
├── Cargo.toml          # no_std Rust workspace
├── apps.manifest.sh    # single source of truth for build/stage/ship policy
├── stage-lib.sh        # shared build.sh/test.sh staging implementation
├── .cargo/config.toml  # target = x86_64-unknown-none, build-std
├── linker.ld           # ENTRY(_start), base 0x40_0000, no PT_INTERP
├── build-support/      # shared per-binary linker-argument helper
├── runtime/            # syscall ABI, startup parsing, brk allocator, GUI events
├── libs/
│   ├── gui/            # Window, Canvas, bitmap text, menus, widgets, dir listing
│   └── dialogs/        # FileDialog, MessageBox, ColorPicker modal compositions
└── apps/
    ├── hello/          # rust app — prints "hello\n", exits 0
    ├── guilaunch/      # rust app — argv[0] → sys_gui_launch syscall
    ├── guidemo/        # minimal ring-3 GUI reference client
    ├── notepad/        # standalone editor with userland dialogs + working Save
    ├── calc/           # standalone four-operation calculator
    ├── painting/       # standalone bouncing-shapes GUI demo (self-driven frame loop)
    ├── zsh/            # prebuilt-managed interactive shell
    ├── busybox/        # prebuilt-managed multicall utilities
    ├── compiler-compat/# tiny C static-musl boot-test fixtures
    ├── network-test/   # static-musl socket test fixture
    └── hello-cpp/      # C++ app — std::cout, exits 0
        ├── Makefile    # invokes x86_64-linux-musl-g++ -static -no-pie
        └── src/main.cpp
```

`hello-cpp` is **not** a Cargo workspace member — Cargo doesn't host C++
projects, and the C++ build uses the host's musl-based g++ cross-compiler
instead of the kernel's nightly Rust target. `build.sh` / `test.sh`
invoke its `Makefile` separately.

## Prebuilt ELFs

Apps that fetch upstream tarballs and / or take long enough that
rebuilding on every kernel iteration is friction ship as **committed
binaries** under `userland/prebuilt/`. Current entries: `ZSH.ELF` and
`BB.ELF` (BusyBox); future Linux ports (bash, vim, …) belong here too.
The committed binary is what `build.sh` / `test.sh` copy into
`host_share/` by default — fresh clones boot a working zsh + coreutils
without the `x86_64-linux-musl-cross` toolchain installed and without
an outbound network fetch.

### `/bin` virtual namespace

The kernel exposes a single virtual `/bin` directory whose entries
resolve into multicall or direct binaries staged under `host_share/`:

- **`BB.ELF` — BusyBox** (core utilities plus IPv4 `ping`, `nc`, `nslookup`,
  and HTTP-only `wget`; IPv6 and TLS are not available).
- **`GLAUNCH.ELF` — kernel-side GUI app launcher** (`painting`, `tasks`,
  `explorer`).
- **`CALC.ELF` / `NOTEPAD.ELF` — direct standalone ring-3 applications**
  (`calc`, `notepad`).

See `src/userland/bin_namespace.rs` for the lists and the
`apply_bin_rewrite` helper. `execve("/bin/ls", argv, envp)` resolves
to `BB.ELF` with `argv[0]` overwritten to `"ls"`; BusyBox's own
dispatcher picks the right applet. `execve("/bin/painting", argv,
envp)` resolves to `GLAUNCH.ELF` with `argv[0]` overwritten to
`"painting"`; GUILAUNCH's `_start` issues the AgenticOS-internal
`sys_gui_launch("painting")` syscall, which spawns the kernel-side
`PaintingProcess`. No symlinks, no per-applet ELF copies — the
namespace is pure kernel synthesis.

`execve("/bin/notepad", ...)` and `execve("/bin/calc", ...)` instead rewrite
directly to `/host/NOTEPAD.ELF` / `/host/CALC.ELF`; there is no `GLAUNCH` round
trip or kernel-side process for either.

`stat`, `access`, `open`, and `getdents64` all recognize `/bin` (the
directory) and `/bin/<applet>` (each entry). PATH discovery from zsh
(`access("/bin/ls", X_OK)` followed by `execve`) finds applets without
any zsh-side hooks. The terminal's default envp seeds
`PATH=/bin:/host` so bare `ls`, `cat`, `painting`, etc. all Just Work
in an interactive shell.

The overlay root and `/data` are writable; `/host` remains read-only. Native
notepad surfaces `-EROFS` in a userland dialog when asked to save under
`/host`.

### `GLAUNCH.ELF` (in-tree, built every run)

`userland/apps/guilaunch/` is a tiny Rust `no_std` ring-3 binary
(`#![no_main]`, custom `_start`). It reads `argv[0]`, issues
`sys_gui_launch(name, len)` (number 5000 in the AgenticOS-internal
syscall range), and exits. It exists so the `/bin/<gui_applet>` PATH
lookups described above have something to `execve`. Built fresh on
every `build.sh` / `test.sh` invocation — too small to bother
prebuilt-managing. See
`docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.

### Booted compiler compatibility fixtures

`apps/compiler-compat/` contains three progressively demanding C programs
covering musl CRT startup, libc/heap/stack behavior, and unknown-syscall
fallback followed by filesystem work. Their stripped static ET_EXEC artifacts
are committed under `prebuilt/compiler-compat/` and staged by `test.sh` even
with `--skip-userland`; ordinary test runs never need a musl compiler.

Run the ladder with `./test.sh compiler_compat`. Refresh instructions and
artifact hashes are in `apps/compiler-compat/README.md`.

Decision tree for adding a new app:

| Property                                           | → Prebuilt? |
|----------------------------------------------------|:-----------:|
| Fetches an upstream tarball during build           | **Yes**     |
| Build takes more than a few seconds                | **Yes**     |
| Only-in-tree source + standard toolchain, fast     | No (build every run) |

`HELLO.ELF` (Rust) and `HELLOCPP.ELF` (small C++ wrapper) are NOT
prebuilt — both build quickly and have no upstream fetch.

**Default**: `build.sh` / `test.sh` copy `userland/prebuilt/<NAME>.ELF`
into `host_share/<NAME>.ELF`. They do NOT invoke `make` for the
upstream app and do NOT probe for the musl toolchain.

**Force rebuild**:

```sh
./build.sh --rebuild-userland     # all prebuilt-managed apps
REBUILD_ZSH=1 ./build.sh          # just zsh
```

When the prebuilt ELF is missing, the scripts fall through to a rebuild
automatically (this is the auto-bootstrap path on a fresh clone that
*does* have the toolchain).

**Refresh after a source change**:

```sh
./userland/refresh-prebuilt.sh
git add userland/prebuilt/<NAME>.ELF userland/apps/<app>/
git commit -m "userland(<app>): <change>; refresh prebuilt"
```

`refresh-prebuilt.sh` hard-fails on any build problem and prints
`git status userland/prebuilt/` when finished. It does NOT auto-commit
— stage and commit yourself, alongside any source/Makefile change.

There is **no automatic staleness check**: if you change source under a
prebuilt-managed app without running `refresh-prebuilt.sh`, the committed
binary will lag the source. The reviewer's job is to flag a source-side
change in `userland/apps/<app>/` without a matching diff in
`userland/prebuilt/<NAME>.ELF`. See `userland/prebuilt/README.md` for the
operational reference.

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

## Adding a new Rust app

1. `mkdir -p userland/apps/<name>/src`
2. Add `Cargo.toml`, including `runtime`; GUI apps also depend on `libs/gui`.
3. Add a one-line `build.rs` calling
   `userland_build_support::configure("<name>")` and its path
   build-dependency.
4. Add the app to `userland/Cargo.toml` and add exactly one row to
   `userland/apps.manifest.sh`. The row declares the source directory, build
   kind, staged 8.3 name, ship policy, toolchain, and output path. Do not edit
   `build.sh` or `test.sh`.
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

6. Rebuild and reboot — the manifest-driven staging library places the ELF in
   `host_share/`.

## Adding a ring-3 GUI app

Use `apps/guidemo` as the minimal reference, `apps/calc` as a compact
single-window canvas-and-hit-test reference, and `apps/notepad` as the
multi-window/filesystem reference. `gui::Window` creates a server-decorated
window, `Canvas` renders XRGB8888 pixels, `gui::next_event()` blocks without
polling, and `Window::present()` performs a full-surface copy. Resize events
must resize the canvas before the next present; Close remains an application
decision. Add a direct `/bin` rewrite only when the app should be discoverable
through `PATH`.

`libs/gui` also ships retained-mode widgets — `Button`, `TextField`,
`ListView`, and `MenuBar` — as manually-positioned structs (no layout engine).
Each draws itself and exposes hit-testing / key routing helpers.

## Using dialogs (`libs/dialogs`)

`libs/dialogs` composes the widgets into four modal dialogs: `FileDialog`
(Open/Save), `MessageBox` (Ok / OkCancel / YesNo), and `ColorPicker`. Each
dialog owns its own `gui::Window` (created in its constructor, destroyed on
drop) and is driven by the retained-mode pattern:

```rust
let mut modal = Some(dialogs::Modal::File(FileDialog::open("/host/")?));
// in the host event loop:
if event.window == main_window.handle() {
    // main window stays live but ignores key/mouse while a modal is open
} else if let Some(m) = modal.as_mut() {
    if event.window == m.window_handle() {
        if let DialogStatus::Done(outcome) = m.handle_event(&event) {
            modal = None;                 // Window dropped → destroyed
            // act on `outcome` (None = cancelled)
        }
    }
}
```

Because each process has **one** GUI event queue, dialogs cannot block: the
host keeps its own loop and routes events by `event.window`. There is no
kernel modality — the host must ignore input to its own main window while a
modal is open (it may still service Resize/Close/Focus). `Modal` is the
four-way convenience wrapper for single-modal apps; hold an `Option<Modal>`
and keep a small app-side enum for *why* the dialog is open so you can route
its outcome. `apps/notepad` (file dialogs + message boxes) and `apps/guidemo`
(color picker + message box) are the reference clients.

To add a new dialog, add a module under `libs/dialogs/src/`, follow the
`window_handle()` + `handle_event() -> DialogStatus<T>` shape, and extend the
`Modal`/`ModalOutcome` wrapper if single-modal hosts should reach it.

## Adding a new C++ app

1. `mkdir -p userland/apps/<name>-cpp/src`
2. Mirror `userland/apps/hello-cpp/Makefile`, changing the `BIN` target.
3. Write `src/main.cpp` — anything compilable with `x86_64-linux-musl-g++`
   that calls `exit_group` either explicitly or via `return` from `main`.
4. Add one `built-every-run` manifest row with the `musl-cxx` toolchain.

## Adding an upstream C app (zsh-style)

For apps with their own autoconf build system (zsh, busybox, dash, etc.),
mirror `userland/apps/zsh/`:

1. `mkdir -p userland/apps/<name>` and add a `Makefile` that fetches the
   upstream tarball into `build/tarballs/`, verifies SHA256, extracts it,
   runs `./configure --host=x86_64-linux-musl` with the static-only flag
   set, runs `make`, and copies a stripped binary to `build/<name>`.
2. Pin both the source version and the SHA256 in the Makefile and the
   app's `README.md`. Bumping a version bumps the SHA in lockstep.
3. Add a `prebuilt-managed` row to `apps.manifest.sh`. The shared staging
   library supplies rebuild-or-copy behavior, ET_EXEC validation, atomic
   staging, and refresh iteration.
4. Add `build/` to `userland/apps/<name>/.gitignore` — the build tree
   contains tarballs, extracted source, and intermediate artifacts that
   shouldn't be tracked.
5. Run `./userland/refresh-prebuilt.sh` once, then `git add
   userland/prebuilt/<NAME>.ELF` so the committed binary lands with
   the rest of the change. Document the entry in
   `userland/prebuilt/README.md`'s per-app table.

The zsh app additionally vendors a build-time-only ncurses inside its
own build tree because the cross-musl toolchain doesn't ship one. If
your upstream app needs other libraries (zlib, libssl, etc.), follow
the same pattern: another fetch + verify + cross-build step before the
app's own configure runs.

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
