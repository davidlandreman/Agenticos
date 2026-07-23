# Plan: Remove the kernel terminal emulator and make `TERMINAL.ELF` the only terminal

**Date:** 2026-07-22  
**Status:** Implemented  
**Depends on:** `2026-07-21-001-feat-terminal-emulator-userland-plan.md`
(`TERMINAL.ELF`, the ring-3 VTE/renderer, and the PTY master fd ABI) and
`2026-07-21-001-feat-ring3-desktop-shell-plan.md` (`DESKTOP.ELF` owns the
Start menu and application launching).

## Goal

Finish the terminal migration by deleting the interactive terminal emulator,
renderer, window factory, and launch policy from ring 0. `TERMINAL.ELF` becomes
the only interactive terminal implementation, whether launched from the Start
menu or by typing `terminal` in zsh.

The kernel still owns the parts that belong in a kernel:

- the PTY master/slave queues;
- termios, winsize, canonical line discipline, and ISIG handling;
- fd readiness and the private PTY syscalls used by `TERMINAL.ELF`;
- process `terminal_id` routing and PTY cleanup when the emulator window dies;
- early boot, panic, serial, and crash-diagnostic output.

This is the deletion phase (U6) deferred by the ring-3 terminal plan. It is not
a second terminal rewrite.

## Current state (verified at `416951172`)

- `/host/TERMINAL.ELF` exists, is staged by `userland/apps.manifest.sh`, and is
  exposed as `/bin/terminal` by `src/userland/bin_namespace.rs`.
- The ring-3 app creates its own GUI window, opens a PTY master with syscall
  5013, forks/execs zsh on the slave, parses VT output with
  `userland/libs/vte`, and renders with `userland/libs/termgrid`.
- The direct `terminal` command therefore already launches the ring-3 app.
- The Start menu is the remaining wrong link:
  `userland/apps/desktop/src/main.rs::activate(MenuAction::Terminal)` calls
  `gui_shell_spawn_terminal` (syscall 5018), whose kernel handler invokes
  `window::terminal_factory::spawn_terminal_with_shell`. That constructs the
  old `TerminalWindow`/`TextWindow` and launches zsh directly into it.
- The compositor still polls the old terminal buffers on every tick through
  `invalidate_dirty_terminals()` and `process_terminal_output()`.
- `src/terminal/` contains two implementations side by side: `pty.rs`, which
  the ring-3 terminal needs, and the old kernel copies of VTE/screen/caret/
  colors/key encoding, which only `TerminalWindow` needs.
- `TextWindow` has no production consumer other than `TerminalWindow`.
- The old window-console routing has no live setter for its default terminal.
  Without a kernel terminal, `crate::print!` can use the existing framebuffer
  text fallback directly; panic/crash diagnostics do not depend on
  `TerminalWindow`.

## Scope boundary

### Keep

- `userland/apps/terminal/` (`TERMINAL.ELF`).
- `userland/libs/vte/` and `userland/libs/termgrid/`.
- `src/terminal/pty.rs`, reduced only where APIs are exclusively for the old
  in-kernel emulator.
- `src/userland/pty_syscalls.rs`, syscall 5013 (`PTY_OPEN`), syscall 5014
  (`PTY_SET_WINSIZE`), and `FdSlot::PtyMaster`.
- `src/userland/stdin.rs` and `src/userland/tty.rs` as PTY-backed slave/termios
  shims used by the Linux ABI. Remove only compatibility methods whose sole
  caller was `TerminalWindow`.
- `Process.terminal_id`, terminal-scoped signal/wake helpers, and
  `userland::gui::release_window_pty`.
- The generic `Window::prepare_for_render` pass only if another window uses it;
  otherwise delete the now-empty hook and manager traversal.
- The low-level framebuffer/serial/crash output paths.

### Delete

- `src/window/terminal_factory.rs`.
- `src/window/terminal.rs`.
- `src/window/windows/terminal.rs`.
- `src/window/windows/text.rs`.
- The kernel emulator copies:
  `src/terminal/{vte,screen,caret,colors,keys,config}.rs`.
- The compositor terminal pumps and terminal-specific window trait hooks.
- Desktop-shell syscall 5018 and its userland wrapper.
- The obsolete window-console buffer/routing and synthetic
  `RPC_TERMINAL_ID`, after direct PTY output routing is in place.
- Kernel-only terminal-well theme fields/tests and the 16 px terminal-font
  singleton if no remaining kernel caller exists.

Historical plan documents may continue to mention the deleted design as
history. Current architecture and subsystem documentation must not.

## Decisions

### D1 — Start launches the ELF like every other ring-3 application

Change the desktop action to:

```rust
MenuAction::Terminal => self.spawn("/host/TERMINAL.ELF", &["terminal"]),
```

Use the desktop shell's existing `fork` + `execve` helper. Do not add another
kernel launcher or redirect syscall 5018 to a kernel process service. The
terminal app owns its window and child shell lifetime already.

This makes both public launch paths identical:

```text
Start → Programs → Terminal ─┐
                             ├─> /host/TERMINAL.ELF ─> PTY_OPEN ─> zsh
zsh: terminal ───────────────┘
```

### D2 — Retire syscall 5018 without renumbering

Remove `GUI_SHELL_SPAWN_TERMINAL` from the kernel dispatcher, runtime wrapper,
handler, and documentation. Do not renumber syscalls 5000–5017; 5018 simply
becomes an unused historical hole and returns `-ENOSYS`.

The remaining desktop-shell protocol is registration, window listing, and
window actions. Terminal creation is ordinary user-process policy, not a
desktop-shell privilege.

### D3 — Keep the PTY in ring 0

“Remove the kernel terminal” means remove the interactive emulator and visual
window, not move TTY semantics into userspace. `TERMINAL.ELF` currently depends
on:

- `PtyMaster` as a readable/writable/selectable fd;
- slave lookup by the process's `terminal_id`;
- kernel canonical editing, echo, VEOF, and signal generation;
- winsize updates and SIGWINCH;
- teardown keyed to the app's `RemoteSurface` `WindowId`.

Those remain. Do not rekey the PTY registry to a new `PtyId` in this change:
the shipped ring-3 app deliberately uses its owned surface ID, and GUI cleanup
already releases that entry. A standalone `PtyId`/`/dev/ptmx` model is a
separate POSIX-fidelity project.

### D4 — Route standard output directly to the PTY slave

The old `window::terminal::write_to_terminal_id` is only an adapter from
syscall output to `pty::slave_for_terminal(id).write(bytes)`. Replace it with a
PTY/userland helper that accepts bytes, not text.

Update `write`, `writev`, and `sendfile` stdout/stderr paths to:

1. resolve the current process's `terminal_id`;
2. look up the PTY slave;
3. write the staged bytes to the slave and preserve short-write accounting;
4. fall back to `crate::print!` only when the process has no terminal.

This removes the `String::from_utf8_lossy` detour for PTY output, preserves
arbitrary terminal bytes and ANSI sequences, and keeps the existing readiness
notification in `PtySlave::write`.

### D5 — Keep boot and panic output, remove only the dead window console

After terminal stdout no longer references `window::terminal`, simplify
`drivers/display/display.rs::_print` to the existing framebuffer text
implementation. Delete the `window::console` buffer, its unused pending-
invalidation queue, `process_terminal_output`, and the synthetic RPC terminal
registration.

Do not remove or weaken:

- `drivers/display/{double_buffered_text,text_buffer}.rs`;
- serial debug logging;
- `diagnostics::crash`;
- the panic handler.

The removal gate includes a boot/test run with no interactive terminal open so
kernel diagnostics are proven independent of `TERMINAL.ELF`.

### D6 — Accept the current ring-3 v1 behavior; do not retain a fallback

`TERMINAL.ELF` is currently fixed at 80×24 and presents an opaque `#202020`
surface. The deleted kernel terminal is resizable and has kernel-only
alpha-232 terminal-well theming. This plan does not keep the kernel fallback to
mask those known ring-3 parity gaps.

Live resize/SIGWINCH rendering and a compositor contract for translucent
ring-3 default cells remain follow-ups. Current docs must describe the shipped
ring-3 behavior instead of claiming the old kernel rendering behavior still
applies.

## Implementation units

### U1 — Cut the Start menu over to `TERMINAL.ELF`

Files:

- `userland/apps/desktop/src/main.rs`

Changes:

- Add a `TERMINAL_PATH` constant beside `ZSH_PATH`.
- Replace `runtime::gui_shell_spawn_terminal()` with
  `self.spawn(TERMINAL_PATH, &["terminal"])`.
- Rewrite the module-level protocol comment: the desktop uses only register,
  list, and window-action syscalls; it launches Terminal with normal
  `fork`/`execve`.

Gate:

- Start → Programs → Terminal produces a `RemoteSurface` owned by
  `TERMINAL.ELF`, not a kernel `TerminalWindow`.
- Two Start-menu launches create independent emulator/zsh process trees.
- The `terminal` zsh command still launches the same ELF.

Land this cutover before deleting any kernel path so the user-visible launcher
never points at missing code.

### U2 — Remove the terminal-spawn desktop-shell syscall

Files:

- `src/userland/abi.rs`
- `src/userland/gui_syscalls.rs`
- `userland/runtime/src/lib.rs`
- `src/tests/gui_userland.rs`

Changes:

- Remove the syscall constant, dispatcher arm, handler, runtime constant, and
  wrapper for `GUI_SHELL_SPAWN_TERMINAL`.
- Remove its non-shell authorization assertion; registration/list/action
  coverage remains.
- Leave the numeric slot unused. Do not shift any ABI number.

Gate:

- No production reference to `gui_shell_spawn_terminal`,
  `GUI_SHELL_SPAWN_TERMINAL`, or `NR_GUI_SHELL_SPAWN_TERMINAL`.
- An explicit syscall 5018 probe returns `-ENOSYS` if a regression test is
  useful.

### U3 — Delete the kernel terminal window and compositor integration

Files to delete:

- `src/window/terminal_factory.rs`
- `src/window/terminal.rs`
- `src/window/windows/terminal.rs`
- `src/window/windows/text.rs`

Files to edit:

- `src/window/mod.rs`
- `src/window/windows/mod.rs`
- `src/window/manager.rs`
- `src/window/compositor.rs`

Changes:

- Remove module declarations and `TerminalWindow` export.
- Remove `terminal_factory::on_window_destroyed` from generic window teardown;
  ring-3 terminal close/crash is already handled by the app plus
  `gui::cleanup_process`/`release_window_pty`.
- Remove `invalidate_dirty_terminals` and the old terminal-output call from the
  compositor loop.
- Remove `Window::grid_size`, which was only used by the terminal factory.
- Remove `Window::prepare_for_render` and
  `WindowManager::prepare_windows_for_render` if the post-deletion call-site
  audit still shows no other override. Otherwise keep the generic hook and
  remove only terminal-specific comments.
- Remove stale `TextWindow` incremental-render comments from generic
  compositor/window tests without weakening the underlying dirty-rectangle
  assertions.

Gate:

- `rg 'TerminalWindow|TextWindow|terminal_factory|invalidate_dirty_terminals' src`
  finds no production code.
- Creating, closing, and crashing a `TERMINAL.ELF` window releases its PTY and
  does not orphan zsh.

### U4 — Reduce `src/terminal` to the kernel PTY service

Files to delete:

- `src/terminal/vte.rs`
- `src/terminal/screen.rs`
- `src/terminal/caret.rs`
- `src/terminal/colors.rs`
- `src/terminal/keys.rs`
- `src/terminal/config.rs`

Files to edit:

- `src/terminal/mod.rs`
- `src/terminal/pty.rs`
- `src/userland/stdin.rs`
- `src/userland/tty.rs`
- `src/tests/mod.rs`

Changes:

- Export only `pty` from the kernel terminal module.
- Move `DEFAULT_ROWS`/`DEFAULT_COLS` to `pty.rs` (or the PTY syscall module)
  and update the remaining callers.
- Remove `TerminalWindow`-specific raw-input and compositor-drain APIs only
  when no PTY test or userland ABI path needs them. Keep generic master/slave
  operations and line-discipline tests.
- Remove stdin/tty compatibility methods used only by the deleted kernel
  window (`push_bytes_for_terminal`, per-terminal echo/canonical queries,
  etc.); retain current-process slave reads and termios access.
- Remove kernel test topics `terminal`, `vte`, `screen`, `caret`, and `keys`.
  Keep the `pty` topic. The authoritative VTE implementation is now under
  `userland/libs/vte`.

Gate:

- `src/terminal/` contains only `mod.rs`, `pty.rs`, `CLAUDE.md`, and agent
  guidance.
- PTY canonical/raw input, echo, VEOF, ISIG, output translation, queue caps,
  readiness, independent instances, winsize, and cleanup tests remain green.

### U5 — Remove old output adapters and dead kernel-terminal presentation data

Files:

- `src/userland/syscalls.rs`
- `src/window/console.rs` (delete)
- `src/window/mod.rs`
- `src/window/manager.rs`
- `src/drivers/display/display.rs`
- `src/kernel.rs`
- `src/tools/shell_run.rs`
- `src/window/theme/mod.rs`
- `src/graphics/fonts/core_font.rs`
- associated tests

Changes:

- Add/use a byte-preserving PTY slave output helper for `write`, `writev`, and
  `sendfile`.
- Remove the window console buffer and manager pending-invalidation plumbing.
- Remove `process_terminal_output` and its compositor call.
- Remove boot registration of `RPC_TERMINAL_ID`; remove the placeholder
  constant from the already-disabled `shell_run` tool.
- Keep `crate::print!` directed to the framebuffer text writer when no PTY is
  involved.
- Remove `ThemeSpec.terminal_well`, `TerminalWellMaterial`, accessors, and the
  kernel `TextWindow` terminal-well tests if no remaining production caller
  exists.
- Remove the kernel 16 px terminal font singleton/accessor and its tests if the
  audit confirms only `TextWindow` used it. Keep the shared system font and
  ordinary UI/caption font paths.

Gate:

- `write`, `writev`, and `sendfile` from zsh children arrive unchanged at the
  ring-3 emulator's PTY master.
- Boot/test diagnostics render before `DESKTOP.ELF` or `TERMINAL.ELF` exists.
- No kernel theme/font API remains solely for the deleted emulator.

### U6 — Tests and documentation

Tests:

- Keep/adapt the existing sendfile-to-current-terminal regression so it reads
  from the PTY master after direct byte routing.
- Replace old synthetic `TerminalWindow` stdin injection with a PTY-master
  line-discipline write, or delete it if equivalent PTY coverage already
  exists.
- Remove `TextWindow` alpha/incremental tests and kernel VTE tests with their
  deleted implementations.
- Retain generic dirty-region tests, renamed/reworded around generic child
  windows.
- Add a focused assertion that closing a ring-3 GUI surface clears the
  surface-keyed PTY entry and wakes the slave side.

Current docs to update:

- root `CLAUDE.md`
- `src/terminal/CLAUDE.md`
- `src/window/CLAUDE.md`
- `src/userland/CLAUDE.md`
- `src/commands/CLAUDE.md`
- `src/process/CLAUDE.md`
- `src/graphics/CLAUDE.md`
- `docs/ARCHITECTURE.md`
- `docs/window_system_design.md`
- `docs/shell_window_integration.md` (mark superseded or replace its current
  architecture section; do not leave `TerminalWindow` presented as current)
- `userland/apps/desktop/src/main.rs`
- stale terminal comments in sample apps and syscall handlers
- `2026-07-21-001-feat-terminal-emulator-userland-plan.md` implementation
  status: mark U6 complete when this plan lands

Document the final ownership split:

```text
DESKTOP.ELF / /bin/terminal
            │ exec
            ▼
      TERMINAL.ELF (ring 3)
      VTE + grid + font + GUI surface
            │ master fd (5013/5014)
            ▼
      kernel PTY + termios + line discipline
            │ slave stdio
            ▼
             zsh
```

## Validation

### Static and build

1. `cargo fmt --check`
2. `cargo check`
3. `cargo check --features test`
4. `./build.sh -n`
5. Targeted QEMU tests:
   `./test.sh pty userland gui_userland window_theme fonts window_manager_render`
   (drop deleted topic names from this command as appropriate).
6. Full `./test.sh`.

Static searches must find no current-code references to:

```text
TerminalWindow
TextWindow
terminal_factory
gui_shell_spawn_terminal
GUI_SHELL_SPAWN_TERMINAL
invalidate_dirty_terminals
RPC_TERMINAL_ID
```

References in historical plan documents are allowed when clearly historical.

### Interactive

1. Default boot reaches `DESKTOP.ELF` with no kernel terminal window.
2. Start → Programs → Terminal opens `TERMINAL.ELF` and shows the zsh prompt.
3. The `terminal` command opens the same implementation.
4. Open two terminals; input, output, termios, scrollback, and process
   lifetimes remain independent.
5. Exercise `ls --color`, `git log`, `cat`, `sendfile` via BusyBox, Ctrl-C,
   Ctrl-D, backspace, Shift+PgUp/PgDn, and OSC title changes.
6. Close one terminal while a child command is running; its zsh/process tree
   exits and the other terminal remains healthy.
7. Kill `TERMINAL.ELF`; GUI cleanup releases the PTY and its child cannot stay
   parked forever on stdin.
8. Switch Classic/Aero/Futurism while a terminal is open; the app remains
   usable. The current opaque v1 well is accepted and documented.
9. Confirm early boot/test output and a controlled failure diagnostic still
   reach framebuffer/serial output with no terminal app running.

## Risks and mitigations

- **Start-menu launch failure becomes an ordinary exec failure.** There is no
  kernel fallback by design. Keep `TERMINAL.ELF` in the build manifest and
  validate the staged `/host/TERMINAL.ELF` during build-only and boot tests.
- **Accidentally deleting the PTY while deleting the terminal.** Treat
  `pty.rs`, 5013/5014, `FdSlot::PtyMaster`, and slave stdio routing as
  load-bearing keep items; require PTY round-trip tests before visual code is
  removed.
- **Stdout regression during adapter removal.** Cut over `write`/`writev`/
  `sendfile` to direct PTY bytes before deleting `window::terminal`, and keep
  regression coverage for all three.
- **Orphaned shell on emulator crash.** Preserve
  `gui::release_window_pty(surface_id)` and verify process-exit cleanup wakes
  terminal-bound readers. `TERMINAL.ELF` already terminates/reaps zsh on a
  normal close.
- **Loss of boot/panic text.** Delete only the window console; retain and test
  framebuffer, serial, and crash paths first.
- **Feature-parity loss.** Fixed 80×24 sizing and the opaque ring-3 content
  well are known, explicit follow-ups. They do not justify retaining a second
  privileged terminal implementation.

## Completion criteria

- Every user-visible Terminal launch executes `/host/TERMINAL.ELF`.
- Syscall 5018 and the kernel terminal creation path are gone.
- No VT parser, screen grid, caret, key encoder, terminal renderer, or terminal
  window remains in the kernel.
- The kernel PTY/termios/line-discipline service remains tested and is the only
  terminal-related ring-0 component.
- Boot, targeted tests, the full suite, and the interactive checklist pass
  without a kernel terminal fallback.

## Implementation result (2026-07-22)

Completed U1–U7:

- The desktop Start menu launches `/host/TERMINAL.ELF` with its ordinary
  `fork`/`execve` path.
- Desktop-shell syscall 5018 was removed without renumbering the remaining
  ABI.
- The kernel terminal/window factory, text window, console adapter, VT parser,
  screen, caret, key encoder, colors, config, compositor pumps, terminal font,
  and terminal-only theme material were deleted.
- `write`, `writev`, and `sendfile` route stdout/stderr bytes directly to the
  process PTY slave, preserving arbitrary non-UTF-8 bytes and ANSI sequences.
- The kernel retains the PTY, termios, winsize, line discipline, readiness,
  process terminal routing, and GUI-surface cleanup needed by the ring-3 app.
- Current architecture and subsystem documentation now describe the
  ring-3-only terminal ownership model.

Validation completed:

- `cargo fmt --all`
- `cargo check`
- `cargo check --features test`
- `cargo check --manifest-path userland/Cargo.toml --workspace`
- `./build.sh -n`
- Focused QEMU suite: 284 tests passed across `pty`, `userland`,
  `gui_userland`, `window_theme`, `fonts`, and `window_manager_render`
- Two full QEMU runs passed every terminal-related test but later latched the
  SMP scheduler-shadow diagnostic in the final module; the isolated
  `diagnostics` rerun passed all 16 tests.

Interactive multi-window checks were not repeated during this deletion pass;
the ring-3 terminal round trip was runtime-validated when the preceding plan
landed, and this pass preserves that ELF while removing its fallback.
