---
title: Terminal overhaul — ANSI/VT parser, PTY pair, Screen, scrollback, caret
date: 2026-05-24
related:
  - docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md
  - src/terminal/CLAUDE.md
tags: [terminal, pty, ansi, vt100, scrollback]
---

# Terminal overhaul — ANSI/VT parser, PTY pair, Screen, scrollback, caret

## What we shipped

A new `src/terminal/` subsystem replacing the byte-pass-through terminal
the kernel had before. End state:

- **VT100 / xterm parser** via Williams' DEC state machine
  (`src/terminal/vte.rs`). Handles CSI, OSC, ESC dispatch with private
  markers (`?`/`>`/`<`/`=`), up to 32 numeric params, two intermediates,
  full UTF-8 reassembly for the print path.
- **Screen** (`src/terminal/screen.rs`): character grid + cursor +
  SGR pen + scroll region + DSR replies + DECSCUSR cursor shape +
  delayed-wrap-at-last-column. Implements `vte::Perform` so bytes parse
  directly into screen mutations.
- **Alt-screen buffer** + **5000-line scrollback ring**
  (`Screen::enter_alt_screen` / `scroll_view`). Vi swaps in via
  `?1049h`, edits, swaps back via `?1049l`, and the user's scrollback
  is intact.
- **PTY pair** (`src/terminal/pty.rs`). One `Arc<Mutex<PtyInner>>` per
  `WindowId` holds slave_queue (input) + master_queue (output) +
  `Termios` + `Winsize`. Replaces the three previously-independent
  surfaces: `userland::stdin`'s WindowId-keyed queues,
  `userland::tty`'s global `static TERMIOS`, and
  `window::terminal`'s per-WindowId output `Vec<String>` buffers.
- **Per-pty termios + winsize**. Two zsh processes in two terminal
  windows now have independent ICANON/ECHO state; `TIOCGWINSZ` returns
  the hosting `TerminalWindow`'s actual grid; `SIGWINCH` raised on
  resize via `lifecycle::raise_signal_on_terminal`.
- **Key encoding** (`src/terminal/keys.rs`): F1–F12, PgUp/PgDn, Insert,
  Shift/Ctrl/Alt+arrow combinations using xterm's modifier-code
  convention. Bracketed-paste helper for when clipboard paste lands.
- **TextWindow → Screen renderer**. TerminalWindow owns a `Vte` +
  `Screen` and re-syncs the TextWindow grid from the Screen each
  `prepare_for_render`. Local echoes (canonical-mode typing, backspace,
  enter) flow through the Vte parser into the Screen so the source of
  truth stays consistent.
- **TERM=xterm-256color + COLORTERM=truecolor** in zsh's env so modern
  programs unlock their color paths.

239 unit tests across the new modules (`vte`, `screen`, `caret`,
`colors`, `keys`, `pty`, plus `terminal::tests`). Existing kernel
regression suite (basic / memory / heap / arc / fonts /
window_clipping / compositor / window_manager_render / desktop_window
/ window_buffer) continues to pass.

## What's not yet wired (tracked as follow-ups)

1. **Per-cell rendering attrs.** `screen::Cell.attrs` carries
   bold/italic/underline/reverse, but `TextWindow::set_cell` ignores
   them. Wire requires either a per-cell attr in TextWindow's grid or
   a glyph-rasterizer variant selector. Cell state is correct; only
   the renderer drops it.

2. **Caret blink.** `caret::blink_on_at(ms)` exists and is unit-tested;
   the compositor doesn't yet invalidate the caret cell on phase
   change. Existing caret rendering (2 px bar at cursor when focused)
   is unchanged.

3. **Mouse-wheel hardware init.** Wheel-event routing into Screen is
   done (`TerminalWindow::handle_event` matches
   `MouseEventType::Scroll`). The PS/2 driver in `src/drivers/mouse.rs`
   doesn't yet do the Intellimouse "knock sequence" to switch to
   4-byte packets; VirtIO tablet wheel events are also unplumbed.
   Shift+PgUp / Shift+PgDn scroll the view unconditionally.

4. **Full line discipline in the master.** TerminalWindow still does
   its own canonical-mode echo/backspace before pushing the line into
   the slave queue. The proper home for ICANON + ECHO + ISIG is the
   master input pipeline; future-work.

5. **Screen resize on TerminalWindow resize.** When the window grows
   or shrinks, the `Screen` inside `TerminalWindow` keeps its initial
   dimensions until the next `Screen::new`. TextWindow renders
   `min(screen_rows, textwindow_rows)`, so the worst case is blank
   rows at the bottom; on SIGWINCH the running app redraws. A
   `Screen::resize` helper would reflow more gracefully.

## Lessons

- **Delayed-wrap at last column matters.** Without it, after printing
  N chars on an N-wide row, the cursor "appears" wrapped to col 0 of
  the next row — and DSR cursor-position queries report the wrong
  cell, which breaks zsh's tab-completion redraw. xterm sticks the
  cursor at the right margin until the next printable arrives; we do
  the same. Tests cover this (`screen::tests::test_delayed_wrap_at_last_col`).
- **LF clears the pending-wrap flag.** Tests caught this — any
  row-changing movement (LF, IND, CUP) retires the sticky-right-margin
  state. Easy bug to write the first time.
- **BCE (background-color erase) is required for SGR correctness.**
  `clear` with a colored prompt paints the screen with the prompt's
  bg color in modern terminals. Cells erased by `EL`/`ED` inherit the
  current `bg`, not the default. Tested under `test_bce_erase_uses_current_bg`.
- **Williams' state machine is the right shape.** Eight states cover
  every flavor of escape we need. Recovery is automatic — any ESC
  mid-sequence restarts cleanly. Around 300 lines plus the per-state
  byte-class tables.
- **Per-pty state lookup model survives.** Before this PR, the three
  separate registries (stdin queues / terminal buffers / global
  termios) each duplicated some flavor of "keyed by `WindowId` or
  `Process.terminal_id`." Consolidating into one
  `Arc<Mutex<PtyInner>>` per `WindowId` removed the duplication and
  the dispatch fan-out without changing the lookup pattern.

## Test surface

- `terminal` — color resolution + palette + config sanity (8 tests).
- `vte` — parser dispatches (16 tests).
- `screen` — cursor, SGR, erase, insert/delete, scroll region, save/
  restore, DEC private modes, DSR, alt-screen swap, scrollback view
  (47 tests).
- `caret` — `Caret` struct + blink (4 tests).
- `pty` — Termios/Winsize, master/slave, registry, queue caps,
  per-pty independence (9 tests).
- `keys` — full encoding table including modifier-combined arrows and
  bracketed paste (14 tests).
