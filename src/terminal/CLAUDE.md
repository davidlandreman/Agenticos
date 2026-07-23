# `src/terminal/` — Kernel PTY Service

The interactive terminal emulator is a ring-3 application:
`userland/apps/terminal/` builds `TERMINAL.ELF`, with VT parsing and screen
state in `userland/libs/vte` and glyph rendering in
`userland/libs/termgrid`.

The kernel retains only `pty.rs`: bounded master/slave byte queues, per-PTY
termios and winsize, canonical line discipline, VEOF, echo, ISIG delivery, and
the registry that associates a GUI surface `WindowId` with a process
`terminal_id`.

## Data flow

```text
TERMINAL.ELF key event
  -> write(master fd)
  -> PtyInner::push_slave_input
  -> termios / canonical editing / echo / ISIG
  -> slave queue
  -> zsh read(0)

zsh write(1/2)
  -> pty::write_slave_for_terminal
  -> master queue
  -> TERMINAL.ELF read(master fd)
  -> userland VTE + termgrid
  -> gui_win_present
```

## Ownership and lifecycle

- `pty_open` (private syscall 5013) verifies that the caller owns the supplied
  GUI window, installs a PTY keyed by that window's content-surface
  `WindowId`, binds the caller's `terminal_id`, and returns
  `FdSlot::PtyMaster`.
- A forked zsh inherits `terminal_id`; sentinel fds 0/1/2 resolve the matching
  slave through `userland::stdin`, `userland::tty`, and the syscall layer.
- `pty_set_winsize` (5014) updates the same PTY and raises SIGWINCH on
  processes with the matching `terminal_id`.
- GUI process cleanup calls `userland::gui::release_window_pty`. Removing the
  registry entry and waking blocked readers makes an abandoned slave observe
  EOF instead of parking forever.
- Each PTY direction is capped at 64 KiB. Canonical input is capped at
  `MAX_CANON` (4096 bytes).

## Line discipline

Master-fd writes are the production input path. In canonical mode the kernel
handles CR-to-LF mapping, echo, VERASE, VKILL, VEOF, line buffering, and
VINTR/VQUIT signal generation. In raw mode bytes pass directly to the slave.
`TERMINAL.ELF` parses terminal answerback replies and writes them through the
same master fd.

The raw `PtyMaster::push_input` helper and the unkeyed legacy PTY exist only
for low-level QEMU test fixtures; interactive input never bypasses line
discipline.

## Invariants

- Never hold the PTY mutex while waking a process, raising a signal, or
  notifying fd readiness.
- Slave output is byte-preserving. Do not route it through UTF-8 strings or
  kernel window widgets.
- `PtySlave::write` owns the master-readiness notification.
- A PTY remains a kernel service. Do not move termios or line discipline into
  the emulator merely because the visual terminal lives in ring 3.

## Cross-references

- Ring-3 app: `userland/apps/terminal/src/main.rs`
- Master-fd ABI: `src/userland/pty_syscalls.rs`
- Standard-stream shims: `src/userland/stdin.rs`, `src/userland/tty.rs`
- Migration plan:
  `docs/plans/2026-07-21-001-feat-terminal-emulator-userland-plan.md`
- Kernel-emulator removal:
  `docs/plans/2026-07-22-002-refactor-remove-kernel-terminal-emulator-plan.md`
