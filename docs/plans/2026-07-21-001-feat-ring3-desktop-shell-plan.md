# Plan: Pull the desktop shell out of ring 0 into userland (`DESKTOP.ELF`)

**Date:** 2026-07-21
**Status:** Implemented and **default** (`AGENTICOS_SHELL=ring3` is the default;
`ring0` selects the legacy `guishell`, which is retained as a fallback pending
eventual deletion). Taskbar/menu follow the active theme.

## Implementation status (2026-07-21)

- **Phases 1–2 (kernel ABI): done + tested.** Syscalls 5013–5016, the
  `GUI_WINDOW_UNDECORATED`/`GUI_WINDOW_PANEL` flags + work-area strut, the
  shell registry with exit-clear, and `WindowManager::shell_window_list`/
  `shell_window_action`. Three new tests in `src/tests/gui_userland.rs` cover
  registration exclusivity/exit-clear, non-shell `-EPERM` gating, and the
  list/action round trip. Ring-3 wrappers added to `userland/runtime` +
  `userland/libs/gui`.
- **Phase 3 (`DESKTOP.ELF`): done.** `userland/apps/desktop/` — panel, Start
  menu, tray clock, Run prompt, fork/execve launcher, taskbar sync via
  `gui_shell_list_windows`, activate via `gui_shell_window_action`, Terminal via
  `gui_shell_spawn_terminal`.
- **Phase 4 (boot wiring): done and now the default.**
  `ring3_desktop_shell_requested` (`src/kernel.rs`) defaults to true; only
  `AGENTICOS_SHELL=ring0` (fw_cfg `opt/agenticos/shell`) selects `guishell`. The
  launch scripts (`scripts/qemu-compositor.sh`) and Conductor's `.conductor/
  run.sh` default to `ring3`. Validated on a real default boot: the shell
  registers, docks the panel (1280×30 at y=690), opens the Start menu, launches
  Calc (fork/execve) and a Terminal (5016) — no panics.
- **Theme parity: done (with one approximation).** The taskbar/Start menu/Run
  prompt draw through new `userland/libs/gui::theme` chrome helpers
  (`draw_taskbar_surface`, `draw_task_button`, `taskbar_text`) plus the existing
  `draw_menu_surface`/`draw_field`/`palette`. Classic (Win98 bevels) and Aero
  (solid raised panel + gradient buttons) reach full parity; Futurism uses a
  solid dark-tint approximation because an opaque ring-3 surface cannot receive
  the compositor's frosted **backdrop blur** (kernel-window-only). Live theme
  switches from Control Center repaint via `GUI_EVENT_THEME_CHANGED`.
- **Remaining:** deleting `guishell` / `window/dialogs/run.rs` (retained as the
  `ring0` fallback); optional compositor plumbing to give panel remote surfaces
  the real frosted-blur chrome. Taskbar sync polls `gui_shell_list_windows` (the
  proven `guishell` cadence); the `WINDOW_*` push events sketched in §2 were
  dropped to shrink kernel surface.


**Scope:** Move the graphical desktop shell (taskbar, Start menu, tray, Run
dialog, and app-launch policy) from the in-kernel `guishell` process into a
single blessed ring-3 process, adding the minimal "desktop-shell protocol"
syscalls required to make that possible.

---

## 0. Important framing — what "shell" means here

The **command-interpreter shell is already in userland.** The legacy kernel
`shell/` process and its hardcoded built-ins (`cat`, `ls`, `run`, …) were
deleted when zsh became the default terminal. Every "run a command" path in the
kernel today is a *launcher* or a *byte-forwarder* that hands an unparsed string
(or a fixed ELF path) to ring-3 `/host/ZSH.ELF`:

- **Start → Run** passes the typed string verbatim as `zsh -c <string>`
  (`src/commands/guishell/mod.rs:495-501`); zsh does all parsing.
- **Terminal** wires a kernel PTY/VT to `/host/ZSH.ELF`
  (`src/window/terminal_factory.rs:223-249`).
- No command registry, tokenizer, or built-in remains in ring 0. The
  `gui_launch_table` skeleton and `GUI_APPLETS` are both empty.

What *is* still in ring 0 and named "shell" is **`src/commands/guishell/`** — the
**graphical desktop shell**: it owns the desktop root, taskbar, Start button,
notification tray, Start menu, Run dialog, taskbar-button synchronization, and
the hardcoded app-launch mapping. It runs as an in-kernel background process
(`spawn_guishell_process`, `guishell/mod.rs:693`). **This is what this plan
moves to ring 3.**

> If the intent was actually the command interpreter, no work is needed — it is
> already ring-3 zsh. The only other optional purity item is the cooked-mode
> line editor in `src/window/windows/terminal.rs` (a TTY line discipline, not a
> command parser, and inactive while zsh runs in raw mode); it is out of scope
> here.

### What stays in the kernel (by design, not moved)

Analogous to a display server vs. a desktop shell: the **compositor stays in the
kernel**. Specifically we keep in ring 0:

- The `WindowManager` / compositor (`src/window/manager.rs`).
- The **desktop root window + wallpaper** (`DesktopWindow`), which is already
  kernel-managed and whose wallpaper/theme are owned by `system_control`
  (syscall 5010). Keeping the root in the kernel guarantees the screen is never
  rootless and gives a robust fallback if the ring-3 shell crashes.
- The **terminal/PTY/VT service** (`terminal_factory`, `terminal::pty`,
  `TerminalWindow`) — a legitimate kernel service, not shell policy.
- `process_service` — used to boot-spawn the shell and for terminal-bound
  launches.

The ring-3 shell owns only **policy and chrome**: panel/taskbar, Start menu,
tray clock, Run prompt, launcher, and window-list/taskbar bookkeeping.

---

## 1. Current architecture (verified)

| Concern | Today (ring 0) | Reference |
|---|---|---|
| Desktop root + wallpaper | `guishell::init_guishell` creates `DesktopWindow`, sets screen root | `guishell/mod.rs:143-158` |
| Taskbar + Start button + tray | Created via `with_window_manager` + `set_window_impl` | `guishell/mod.rs:161-204` |
| Start menu | `StartMenuWindow`, `on_select` → `queue_action` | `guishell/mod.rs:266-326` |
| App launch mapping | Hardcoded `spawn_*` → `process_service::submit` | `guishell/mod.rs:363-453` |
| Terminal launch | `terminal_factory::spawn_terminal_with_shell` | `terminal_factory.rs:137-249` |
| Run dialog | Kernel modal `open_run_dialog`/`poll_run_dialog` | `window/dialogs/run.rs` |
| Run exec | `[ZSH, "-c", cmd]` → `process_service::submit` | `guishell/mod.rs:455-501` |
| Taskbar button sync | Polls `wm.get_frame_windows()` every 10 ticks | `guishell/mod.rs:518-561` |
| Activate window | `wm.activate_frame(frame_id)` on unowned frames | `guishell/mod.rs:673-678` |
| Event wakeups | WM calls `signal_guishell()` on click; deferred `PendingAction` | `manager.rs:681-683`, `guishell/mod.rs:80-97` |
| Error/info dialogs | Kernel modal `dialogs::show_error/show_info` | `window/dialogs/message_box.rs` |

### What ring 3 can already do (low risk)

- Create a decorated window, present an XRGB8888 buffer or GL frame, receive
  KEY/MOUSE/RESIZE/CLOSE/FOCUS/THEME/SETTINGS events, retitle, destroy, and use
  a selectable event fd — syscalls **5001–5005, 5011** via `userland/libs/gui`.
- **`fork` + `execve` + `wait4`** — real POSIX process spawning, already used by
  `FILEMAN.ELF` to launch `/bin/notepad` and ELFs
  (`userland/apps/fileman/src/main.rs:1073-1092`).
- Message/error boxes and file pickers via `userland/libs/dialogs`
  (cooperative, app-side modality).

### The 5 capability gaps (what the kernel must newly expose)

1. **Desktop-shell registration** — one blessed ring-3 PID that may use the
   privileged shell syscalls below. No such concept today.
2. **Panel/taskbar surface + work-area strut** — ring 3 can only create a
   `FrameWindow`+`RemoteSurface` in the content area; it cannot create a
   root/panel-type window or reserve work area (`gui_syscalls.rs:105-153`).
3. **Global window-list events + query** — no `GuiEvent` kind fires when
   *another app's* top-level window is created/destroyed/retitled/min-maxed;
   guishell learns this only by polling `wm.get_frame_windows()`.
4. **Cross-window control** — no syscall to activate/minimize/maximize/restore a
   frame the caller does not own (`wm.activate_frame` is kernel-only).
5. **Terminal creation from ring 3** — `TerminalWindow`, PTY allocation,
   `register_terminal`, and `LaunchSpec.with_terminal` are all kernel-only, and
   there is no `/dev/ptmx`.

---

## 2. Target architecture

```
 ring 0 (kernel)                          ring 3
 ┌──────────────────────────────┐         ┌─────────────────────────────┐
 │ WindowManager / compositor   │◀──5001..│ DESKTOP.ELF (blessed shell) │
 │ DesktopWindow root+wallpaper │  ..5016─▶│  • panel (taskbar+start+tray)│
 │ Terminal/PTY/VT service      │         │  • start menu               │
 │ process_service (boot spawn) │         │  • run prompt (libs/dialogs)│
 │ system_control (theme/wall)  │         │  • launcher (fork/execve)   │
 │ NEW: desktop-shell protocol  │         │  • taskbar sync + activate  │
 └──────────────────────────────┘         └─────────────────────────────┘
```

The kernel keeps the compositor + desktop root + terminal service and exposes a
small **desktop-shell protocol** (new syscalls 5013–5016) to exactly one
registered ring-3 process. `DESKTOP.ELF` becomes the shell; `guishell` is
deleted.

### New ABI: desktop-shell protocol (syscalls 5013–5016)

All four require the caller to be the currently-registered shell PID; others get
`-EPERM`.

- **5013 `gui_shell_register(flags) -> 0 | -EEXIST`** — claim the shell role.
  Records the caller PID. Rejected if a *live* shell is already registered.
  Registration is cleared automatically when the PID exits (see §6).
- **5014 `gui_shell_list_windows(buf, len) -> count`** — snapshot of top-level
  frames as fixed-size records `{ frame_id: u64, state: u32 (normal/min/max),
  title: [u8; N] }`. Replaces `wm.get_frame_windows()`.
- **5015 `gui_shell_window_action(frame_id, action) -> 0 | -errno`** — actions:
  `activate | minimize | maximize | restore | close`. Replaces
  `wm.activate_frame` and enables taskbar button behavior. Ownership check =
  "is registered shell."
- **5016 `gui_shell_spawn_terminal() -> frame_id | -errno`** — performs exactly
  today's `terminal_factory::spawn_terminal_with_shell()` in the kernel and
  returns the new frame id. Keeps the kernel PTY/VT service intact while moving
  the *trigger* to ring 3. (Full ring-3 terminal emulator + `/dev/ptmx` is
  explicitly deferred — see §7.)

### New window flag + events

- **`GUI_WINDOW_PANEL`** flag on `gui_win_create` (5001) — only the registered
  shell may set it. Marks a surface as always-on-top chrome and declares a
  work-area strut so the compositor's maximize/placement avoids it. Generalizes
  the existing kernel-taskbar work-area logic to a shell-declared rect. (The
  desktop background stays the kernel root; the shell does **not** create a
  desktop-root window — it only adds panels/menus above the kernel root.)
- **New `GuiEvent` kinds** delivered **only to the registered shell PID**:
  `WINDOW_CREATED`, `WINDOW_DESTROYED`, `WINDOW_TITLE_CHANGED`,
  `WINDOW_STATE_CHANGED` (payload carries `frame_id`; shell re-queries titles via
  5014 or reads them from the event payload). Emitted from the WM at the same
  points that create/destroy/retitle/min-max frames and that currently call
  `signal_guishell`.

Everything else the shell needs — launching apps, the Run prompt, error
dialogs — uses capabilities ring 3 **already has** (`fork`/`execve`/`wait4`,
`libs/dialogs`). No system-modal support is added; the shell's own dialogs use
cooperative modality, which is acceptable for a Run box and error toasts.

---

## 3. Phased implementation

### Phase 1 — Desktop-shell protocol foundation (kernel)
- Add a `DesktopShell` registry (blessed PID + declared panel strut rect) next to
  the WM, guarded by the existing WM/preemption lock discipline.
- Implement **5013 register**, **5014 list_windows**, **5015 window_action**.
- Add `GUI_WINDOW_PANEL` to `gui_win_create`; wire the strut into the compositor
  work-area/maximize logic (reuse the current taskbar work-area path,
  parameterized by the shell's declared rect instead of `taskbar_id`).
- Emit the four `WINDOW_*` events to the registered shell from the WM
  create/destroy/retitle/min-max sites; delete the `signal_guishell` coupling in
  `manager.rs:681-683`.
- **Tests:** registration exclusivity + auto-clear on exit; non-shell caller gets
  `-EPERM` on 5014/5015 and `EINVAL`/`-EPERM` on `GUI_WINDOW_PANEL`; event
  emitted exactly once per frame create/destroy; strut reserves work area so a
  maximized window stops above the panel.

### Phase 2 — Terminal spawn syscall (kernel)
- Implement **5016 `gui_shell_spawn_terminal`** delegating to
  `terminal_factory::spawn_terminal_with_shell`, gated to the registered shell.
- **Tests:** returns a valid frame id; child zsh binds to the new PTY; close
  teardown still runs `cancel_for_terminal`.

### Phase 3 — `DESKTOP.ELF` ring-3 app (userland)
- New `userland/apps/desktop/` (`no_std`, deps `runtime`, `libs/gui`,
  `libs/dialogs`). Manifest row + stage into image `/host`.
- Startup: `gui_shell_register`; create the `GUI_WINDOW_PANEL` taskbar surface
  with a Start button + tray clock (`clock_gettime`); render per active theme via
  `/etc/theme` + `THEME_CHANGED`/`SETTINGS_CHANGED` events (already delivered to
  ring-3 windows).
- Start menu: Start click → borderless menu surface; select →
  - Terminal → **5016**;
  - File Manager/Notepad/Painting/Calc/GL Arena/Task Manager/Web Browser/Settings
    → `fork`+`execve` of the corresponding `/host/*.ELF` (mapping copied from
    `guishell/mod.rs:363-419`, argv/env from `DEFAULT_USER_ENV`);
  - Run → `libs/dialogs` text prompt → `fork`+`execve("/bin/zsh", ["zsh","-c",cmd])`;
  - Shut Down → info dialog (unchanged message).
- Taskbar: consume `WINDOW_*` events + `gui_shell_list_windows` to add/remove/
  relabel buttons (dedupe by `frame_id`, mirroring `sync_taskbar_buttons`);
  click → `gui_shell_window_action(frame_id, activate)`.
- Reap launched children via `wait4(-1, …, WNOHANG)` (pattern from FILEMAN); on
  non-zero/failed exit show a `libs/dialogs` error box.

### Phase 4 — Boot wiring + retire kernel guishell
- Kernel boot keeps creating the screen + `DesktopWindow` root + wallpaper, then
  `process_service::submit(DESKTOP.ELF)` **instead of** `spawn_guishell_process()`.
- Delete `src/commands/guishell/`, the kernel Run dialog (`window/dialogs/run.rs`),
  and `dialogs::show_error/show_info`/`message_box` **iff** no other kernel caller
  remains (grep first — dialogs may be used elsewhere; keep what is still used).
- Remove/retire the empty `gui_launch_table.rs` + `GUI_APPLETS` skeleton (or
  leave the empty skeleton; recommend delete since the live mapping now lives in
  `DESKTOP.ELF`).
- Update CLAUDE.md: root, `src/commands/`, `src/window/`, `src/userland/`, plus a
  `userland/apps/desktop/README.md`.

### Phase 5 — Robustness & polish
- **Shell crash recovery:** on registered-PID exit, kernel clears registration
  and re-`submit`s `DESKTOP.ELF` (supervisor). Because the desktop root stays in
  the kernel, a crashed shell degrades to a bare (but usable) wallpaper until the
  panel respawns.
- Verify SMP/locking: the WM-lock-reentrancy `queue_action` dance disappears —
  the ring-3 shell only touches the WM through syscalls, so no in-kernel deferred
  actions are needed.
- Optional: theme parity pass so the ring-3 taskbar/Start menu match the kernel
  `ThemeSpec` finishes exactly (Classic/Aero/Futurism).

---

## 4. Files touched (anticipated)

**Kernel (new ABI):** `src/userland/abi.rs` (syscall numbers/dispatch),
`src/userland/gui_syscalls.rs` (5013–5016, `GUI_WINDOW_PANEL`), `src/userland/gui.rs`
(new `GuiEvent` kinds + shell-only delivery), `src/window/manager.rs` (event
emission, strut work-area, remove `signal_guishell`), a new
`src/window/desktop_shell.rs` (registry), `src/kernel.rs` (boot spawns
DESKTOP.ELF).

**Kernel (deletions):** `src/commands/guishell/`, `src/window/dialogs/run.rs`,
possibly `gui_launch_table.rs`, guishell-specific `WakeEvents` coupling.

**Userland (new):** `userland/apps/desktop/`, manifest row, image staging,
`userland/libs/gui` panel-flag + shell-syscall wrappers.

---

## 5. Risks & open questions

- **Work-area strut generalization** — the current maximize/placement logic keys
  off `taskbar_id`; it must accept a shell-declared rect. Moderate, localized.
- **Window-list event/query consistency** — race between `WINDOW_CREATED` and a
  `list_windows` snapshot; resolve by deduping on `frame_id` (guishell already
  does this) and treating the list as authoritative.
- **Start-menu / taskbar visual parity** — re-implementing `StartMenuWindow` and
  `TaskbarWindow` chrome in the ring-3 toolkit is real UI work; budget for it.
- **Performance** — the taskbar now presents via copy-blit `RemoteSurface` on
  each update rather than in-kernel direct draw; the surface is small, so this is
  expected to be negligible, but worth an `AGENTICOS_RENDER_STATS=1` check.
- **Terminal stays kernel-side** — pragmatic; a fully ring-3 terminal emulator is
  a separate, larger effort (§7).

## 6. Non-goals / deferred

7. **Full ring-3 terminal emulator** — a userland VT100 emulator owning a
   `RemoteSurface` with a child on `/dev/ptmx`. Requires adding `/dev/ptmx` to
   `devfs` and porting VT emulation to userland. `gui_shell_spawn_terminal`
   (5016) is the interim bridge.
8. **Moving the desktop root/wallpaper to ring 3** — kept in the kernel for the
   rootless-screen and crash-fallback guarantees; can revisit later.
9. **System-modal ring-3 dialogs** — not added; cooperative modality suffices.
10. **Cooked-mode line editor relocation** (`terminal.rs`) — TTY line discipline,
    not a command interpreter; out of scope.

---

## 7. Suggested sequencing for review

Land Phase 1 + 2 (kernel ABI, fully tested, with `guishell` still the live shell
using the *old* paths) first — the new syscalls are dormant until a client uses
them. Then land Phase 3 (`DESKTOP.ELF`) behind a boot flag, validate side by
side, and finally flip the boot in Phase 4 and delete `guishell`.
