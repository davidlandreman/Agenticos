# GUILAUNCH

Static ring-3 Rust binary. One file (`src/main.rs`, ~25 LOC).

## What it does

Reads `argv[0]`, issues the AgenticOS-internal `sys_gui_launch(name, len)`
syscall, and exits.

## Why it exists

The kernel-side GUI apps (`painting`, `calc`, `notepad`, `tasks`,
`explorer`) live in `src/commands/<app>/` and run as kernel processes.
With zsh as the default terminal shell, the user typing `painting`
needs to land in ring 3 and ride zsh's normal PATH-lookup +
`execve` flow.

The `/bin/<gui_applet>` rewrite in `src/userland/bin_namespace.rs`
sends `execve("/bin/painting", ["painting"], envp)` here:

```
execve("/bin/painting") → execve("/host/GLAUNCH.ELF", argv[0]="painting")
GUILAUNCH._start:
   argv[0] = "painting"
   sys_gui_launch("painting", 8)     // kernel: spawn PaintingProcess
   exit(0)
```

Same multicall trick BusyBox uses, but with a single syscall instead
of a 240-entry applet dispatcher.

## Build

Built every run by `build.sh` / `test.sh` (the bin is ~4 KB stripped —
no value to prebuilt-managing it).

```sh
cargo build --release --manifest-path userland/Cargo.toml
# → userland/target/x86_64-unknown-none/release/guilaunch
```

Linker args are wired through `build.rs` mirroring `userland/apps/hello/`.

## See also

- `src/commands/gui_launch_table.rs` — kernel-side dispatch table.
- `src/userland/syscalls.rs::gui_launch_handler` — syscall handler.
- `src/userland/bin_namespace.rs` — `GUI_APPLETS` list and `/bin` rewrite.
- `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`
