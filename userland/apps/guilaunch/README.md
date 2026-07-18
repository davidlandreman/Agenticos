# GUILAUNCH

Static ring-3 Rust binary. One file (`src/main.rs`, ~25 LOC).

## What it does

Reads `argv[0]`, issues the AgenticOS-internal `sys_gui_launch(name, len)`
syscall, and exits.

## Why it exists

Tasks is the one remaining kernel-side GUI app. With zsh as the default shell,
typing `tasks` still needs to use normal PATH lookup and `execve`:

```text
execve("/bin/tasks") → execve("/host/GLAUNCH.ELF", argv[0]="tasks")
GUILAUNCH._start:
   argv[0] = "tasks"
   sys_gui_launch("tasks", 5)
   exit(0)
```

This is the same multicall idea BusyBox uses, but with one kernel dispatch
syscall instead of an applet dispatcher.

New and migrated GUI apps use the ring-3 GUI toolkit instead. File Manager,
Calc, Notepad, and Painting are standalone staged ELFs and do not pass through
GUILAUNCH.

## Build

Built every run by `build.sh` / `test.sh` (the binary is small enough that
prebuilt management would add no value).

```sh
cargo build --release --manifest-path userland/Cargo.toml
# → userland/target/x86_64-unknown-none/release/guilaunch
```
