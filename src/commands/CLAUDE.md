# `src/commands/` — Kernel-side GUI policy and legacy app

This directory contains the one remaining kernel-side GUI application
(`tasks`) plus `guishell`, the desktop/taskbar policy layer. File Manager,
Notepad, Calc, and Painting are standalone ring-3 applications under
`userland/apps/`.

`gui_launch_table` dispatches `tasks` for `GLAUNCH.ELF` and syscall 5000. Its
name must match `GUI_APPLETS` in `src/userland/bin_namespace.rs`. The migrated
apps are deliberately absent: their synthetic `/bin` entries rewrite directly
to staged ELFs under `/host`.

## Desktop launch paths

- Start → Programs contains Terminal, File Manager, Notepad, Painting, and
  Calc. Standalone apps use `terminal_factory::spawn_gui_user_app`, which
  launches an ELF on a blocking kernel wrapper thread.
- Start → Run opens the kernel-owned non-blocking Run dialog. Submitted text is
  passed unchanged as the single command argument to `/host/ZSH.ELF -c`, using
  the same `/bin:/host` PATH as interactive terminals.
- Start → Documents and Settings are disabled placeholders. Start → Shut Down
  reports that clean shutdown is not implemented; it does not use QEMU's test
  exit port.
- zsh `explorer`, `notepad`, `calc`, and `painting` resolve through the
  synthetic `/bin` namespace directly to their standalone ELFs. The historical
  `explorer` command is the compatibility name for `FILEMAN.ELF`.
- File Manager launches `NOTEPAD.ELF` for text files and executable ELF files
  directly with ring-3 `fork` + `execve`.
- Tasks continues through `GLAUNCH.ELF` → `sys_gui_launch` →
  `gui_launch_table::spawn_by_name`.

## Adding GUI applications

New native applications should use the ring-3 pattern documented in
`userland/README.md`: add a no_std workspace app, depend on `runtime` and
`libs/gui`, and add one manifest row. Add a Programs action only when the app
should be pinned there. Do not add new kernel widgets or launch-table arms
unless the workload genuinely requires ring-0 privileges.

The remaining Tasks app may migrate using the same pattern: remove its module
and launch-table arm and change its `/bin` entry to a direct ELF rewrite.

## Cross-references

- Ring-3 GUI ABI and ownership: `src/userland/CLAUDE.md`.
- `RemoteSurface` and event routing: `src/window/CLAUDE.md`.
- Kernel scheduler: `src/process/CLAUDE.md`.
