# `src/commands/` — Kernel-side GUI policy and legacy app

This directory contains `guishell`, the desktop/taskbar policy layer, plus
the (empty today) GUI launch table. Every GUI application has migrated to
the ring-3 platform: Settings, File Manager, Notepad, Calc, Painting, GL Arena, and
the Task Manager (`userland/apps/taskmgr/`, replacing the kernel-side
`tasks` app — see
`docs/plans/2026-07-18-003-feat-ring3-task-manager-and-procfs-plan.md`) all
live under `userland/apps/`.

`guishell` also owns taskbar policy. Each frame retains a task button while
minimized; task-button activation delegates to
`WindowManager::activate_frame`, which restores visibility, raises the frame,
and focuses its first focusable descendant without duplicating placement state
inside `GUIShellState`.

`gui_launch_table` retains the `GLAUNCH.ELF` / syscall-5000 dispatch
skeleton for a future workload that genuinely needs ring 0; its match arms
must stay in sync with `GUI_APPLETS` in `src/userland/bin_namespace.rs`
(both are empty). The migrated apps' synthetic `/bin` entries rewrite
directly to staged ELFs under `/host` (`/bin/taskmgr` and the legacy alias
`/bin/tasks` both resolve to `/host/TASKMGR.ELF`; `explorer` is the
compatibility name for `FILEMAN.ELF`).

## Ring-3 desktop shell (`AGENTICOS_SHELL=ring3`)

The **ring-3 `DESKTOP.ELF`** (`userland/apps/desktop/`) is now the default
desktop shell; the in-kernel `guishell` is the legacy fallback. Selected at boot
by `opt/agenticos/shell` fw_cfg (`AGENTICOS_SHELL=ring0|ring3`, default `ring3`),
read in `src/kernel.rs` (`ring3_desktop_shell_requested` — anything but the
literal `ring0` selects the ring-3 shell). In ring-3 mode the kernel calls
`guishell::init_desktop_root_only` (screen + desktop-root wallpaper only — no
kernel taskbar chrome) and `guishell::spawn_ring3_desktop_shell`
(`/host/DESKTOP.ELF`) instead of `init_guishell` + `spawn_guishell_process`.

`DESKTOP.ELF` drives the compositor purely through the **desktop-shell protocol
syscalls** (5013–5016) plus the `GUI_WINDOW_PANEL`/`GUI_WINDOW_UNDECORATED`
chrome flags, and launches apps with ordinary `fork`+`execve` (Terminal uses
`gui_shell_spawn_terminal`). The kernel keeps the compositor, desktop-root
wallpaper, and terminal/PTY service. Its taskbar/Start menu/Run prompt follow
the active Classic/Aero/Futurism theme through the ring-3
`userland/libs/gui::theme` helpers (`draw_taskbar_surface`, `draw_task_button`,
`taskbar_text`, `draw_menu_surface`, `draw_field`) — full parity for the solid
Classic/Aero bars; Futurism uses a solid dark tint approximation because an
opaque ring-3 surface cannot receive the compositor's frosted backdrop blur.
The Start menu mirrors the kernel `start_menu.rs`: a vertical `AgenticOS`
banner, a Programs fly-out, per-row icons (procedural, in
`userland/apps/desktop/src/icons.rs` — ring-3 has no SVG rasterizer), a disabled
Documents placeholder, and a submenu arrow. The root menu and the Programs
fly-out are **two independent windows** (the fly-out has its own height and is
created with the shell-only `GUI_WINDOW_NO_FOCUS` flag so it does not dismiss
the focused root popup); both bottom-align to the taskbar, so opening Programs
never resizes or moves the primary menu. The Run prompt is a decorated `Run` window with the same two
prompt lines, a themed field, and OK/Cancel buttons.
See `docs/plans/2026-07-21-001-feat-ring3-desktop-shell-plan.md`. `guishell` is
still present as the `ring0` fallback pending its eventual deletion.

## Desktop launch paths

- Start → Programs contains Terminal, File Manager, Notepad, Painting,
  Calc, GL Arena, and Task Manager. Standalone apps use
  `userland::process_service::submit`, which queues an owned launch request
  for the persistent process service and returns immediately.
- Start → Run opens the kernel-owned non-blocking Run dialog. Submitted text is
  passed unchanged as the single command argument to `/host/ZSH.ELF -c`, using
  the same `/bin:/host` PATH as interactive terminals.
- Start → Settings launches `/host/CONTROL.ELF`; Documents remains a disabled
  placeholder. Start → Shut Down
  reports that clean shutdown is not implemented; it does not use QEMU's test
  exit port.
- zsh `control`/`settings`, `explorer`, `notepad`, `calc`, `painting`, `glgame`, `taskmgr`, and
  `tasks` resolve through the synthetic `/bin` namespace directly to their
  standalone ELFs. The historical `explorer` command is the compatibility
  name for `FILEMAN.ELF`.
- File Manager launches `NOTEPAD.ELF` for text files and executable ELF files
  directly with ring-3 `fork` + `execve`.

The old kernel `tasks` app could kill arbitrary kernel threads (including the
compositor). That capability was deliberately dropped in the migration: the
ring-3 Task Manager ends processes via `kill(2)` (ring-3 PIDs only) and shows
kernel threads read-only via `/proc/agenticos/kthreads`.

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
