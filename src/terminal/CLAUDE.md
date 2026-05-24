# `src/terminal/` — Terminal Subsystem

VT100/xterm-compatible terminal emulation. Sits between the window
manager (display surface + keyboard event source) and userland (process
file descriptors), owning the character grid, the ANSI/VT escape-sequence
parser, the PTY pair, and the per-pty termios + winsize state.

Designed in
`docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`.

## Key files

- `mod.rs` — module surface (`pub mod`s + test registry).
- `vte.rs` — Williams DEC state machine parser. Bytes in, `VteEvent` callbacks out via the [`Perform`] trait.
- `screen.rs` — character grid + cursor + SGR pen + scroll region + alt-screen + scrollback. Implements `vte::Perform` so a byte stream parses directly into screen mutations.
- `pty.rs` — PTY master/slave pair, per-pty `Termios` + `Winsize`, master/slave queues, registry keyed by `WindowId`.
- `caret.rs` — caret snapshot struct (`Caret`) + `blink_on_at(ms)`.
- `colors.rs` — `ColorSpec` (Default/Indexed/Rgb) + the 256-color xterm palette.
- `keys.rs` — `KeyCode` → escape-sequence byte encoding (F1–F12, PgUp/PgDn, modifier-combined arrows, bracketed paste).
- `config.rs` — compile-time constants: `SCROLLBACK_LINES = 5000`, `BLINK_INTERVAL_MS = 500`, `DEFAULT_ROWS = 24`, `DEFAULT_COLS = 80`, `TAB_WIDTH = 8`.

## Data-flow

```
ring-3 write(1, …) ──> pty.slave_write ──> master_queue
                                              │
keyboard event ──> terminal::keys::encode ──> pty.master.push_input ──> slave_queue
                                              │
                                              └─ compositor calls
                                                 TerminalWindow::prepare_for_render
                                                  ├─ takes terminal_output (drains master_queue)
                                                  ├─ feeds bytes through Vte → Screen
                                                  ├─ pushes any Screen replies (DSR, …) back to slave_queue
                                                  └─ syncs Screen viewport → TextWindow grid
                                              │
                                              └─ TextWindow::paint renders the grid
```

The Screen is the source of truth for *what's on screen*. TextWindow is
just the renderer — call `set_cell(row, col, ch, fg, bg)` from
`TerminalWindow::sync_text_window_from_screen` rebuilds the grid from
Screen each frame.

## PTY model

Each terminal `WindowId` maps to one `Arc<Mutex<PtyInner>>`. Slave handles
(`PtySlave`) are looked up via `pty::slave_for_terminal(tid)` by syscall
handlers that hold a `Process.terminal_id`. Master handles (`PtyMaster`)
are held by `TerminalWindow` (for output drain + winsize updates).

`userland::tty` and `userland::stdin` are now thin shims that resolve
through `terminal::pty::*` — they preserve the original API surface so
the rest of the kernel doesn't need to know the storage moved.

## Termios + line discipline

The pty's `Termios` is the per-pty source of truth. `ioctl(0, TCGETS)`
and `TCSETS` go through `userland::tty::snapshot/set`, which resolve to
the current process's pty's termios. Two zsh instances in two terminal
windows therefore have independent ICANON/ECHO/ISIG state.

**Line discipline is currently minimal.** TerminalWindow does its own
canonical-mode line editing (echo, backspace) and just pushes the
finished line into the slave queue on Enter. Future-work: move
ICANON + ECHO + ISIG handling into the master input pipeline so the
slave's `read` reflects POSIX line discipline regardless of host.

## Winsize + SIGWINCH

The pty's `Winsize` is updated by `window::terminal::sync_terminal_winsize`,
which the `terminal_factory` calls after registration and which
`TerminalWindow::set_bounds` calls on every resize. On actual change,
SIGWINCH (signal 28) is raised on every process whose `terminal_id`
matches via `lifecycle::raise_signal_on_terminal`.

`TIOCGWINSZ` reads the same pty.

## What's not yet here

- **Per-cell rendering attrs** — bold/italic/underline are tracked on
  `screen::Cell` (`screen::attrs::BOLD`, etc.) but `TextWindow::set_cell`
  ignores them. Rendering would need TextWindow to grow attr-aware
  per-cell state, or fold into glyph rasterization via a new bold/italic
  variant.
- **Caret blink rendering** — `caret::blink_on_at(ms)` exists; the
  compositor doesn't yet invalidate the caret cell on phase change.
  Today's caret renders as before (white 2 px bar at cursor when
  focused).
- **Mouse-wheel hardware init** — Intellimouse 4-byte packet enabling
  in `src/drivers/mouse.rs::init_ps2`. Wheel-event routing into Screen
  is wired (handle in `TerminalWindow`), but Scroll events aren't yet
  produced by the PS/2 driver. Shift+PgUp/PgDn scrolls scrollback
  unconditionally.
- **Powerline-patched font** — the current `assets/system.ttf` is
  JetBrains Mono (no Powerline glyphs at U+E0A0–E0B3). agnoster's
  separators render as `.notdef`. Swap the TTF for a Powerline-patched
  variant (DejaVu Sans Mono for Powerline is permissively licensed) and
  update `assets/system.ttf.LICENSE`.
- **DCS / Sixel / mouse tracking modes** — parser absorbs DCS and
  ignores `?1000h` / `?1006h`; vi-mouse + sixel graphics are out of
  scope.

## Cross-references

- Plan: `docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`.
- Window-system integration: `src/window/CLAUDE.md`.
- Ring-3 + syscalls: `src/userland/CLAUDE.md`.
