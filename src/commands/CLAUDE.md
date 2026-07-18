# `src/commands/` — Kernel-side GUI policy and legacy apps

This directory now contains two kernel-side GUI applications (`tasks`,
`explorer`) plus `guishell`, the desktop/taskbar policy layer. `notepad` was
the first application migrated to the ring-3 GUI platform, and `calc` and
`painting` followed; all three live under `userland/apps/`.

`gui_launch_table` still dispatches the two legacy applications for
`GLAUNCH.ELF` and syscall 5000. Its names must match `GUI_APPLETS` in
`src/userland/bin_namespace.rs`. Calc, notepad, and painting are deliberately
absent: `/bin/calc`, `/bin/notepad`, and `/bin/painting` rewrite directly to
`/host/CALC.ELF`, `/host/NOTEPAD.ELF`, and `/host/PAINTING.ELF`.

## Launch paths

- Start → Notepad, Start → Calc, and Start → Painting call
  `terminal_factory::spawn_gui_user_app`, which launches the standalone ELF on
  a blocking kernel wrapper thread.
- zsh `notepad` / `calc` / `painting` resolve through the synthetic `/bin`
  namespace directly to `NOTEPAD.ELF` / `CALC.ELF` / `PAINTING.ELF`.
- Explorer launches `NOTEPAD.ELF` with the selected text path as `argv[1]`.
- The two remaining kernel apps continue through `GLAUNCH.ELF` →
  `sys_gui_launch` → `gui_launch_table::spawn_by_name`.

## Adding GUI applications

New native applications should use the ring-3 pattern documented in
`userland/README.md`: add a no_std workspace app, depend on `runtime` and
`libs/gui`, and add one manifest row. Add a Start-menu action only when the app
should be pinned there. Do not add new kernel widgets or launch-table arms
unless the workload genuinely requires ring-0 privileges.

The remaining kernel apps may migrate incrementally. Each migration removes
its module and launch-table arm and changes its `/bin` entry to a direct ELF
rewrite, following notepad's pattern.

## Cross-references

- Ring-3 GUI ABI and ownership: `src/userland/CLAUDE.md`.
- `RemoteSurface` and event routing: `src/window/CLAUDE.md`.
- Kernel scheduler: `src/process/CLAUDE.md`.
