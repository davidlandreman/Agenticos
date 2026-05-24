---
title: "feat: Full-featured terminal — ANSI/VT parser, PTY, scrollback, caret"
status: active
created: 2026-05-24
plan_type: feat
depth: deep
related_docs:
  - CLAUDE.md (Current Limitations: Constant Window Repainting; Command System; Mouse Integration)
  - src/window/CLAUDE.md (TextWindow grid; TerminalWindow input handling; compositor as scheduled kernel thread)
  - src/userland/CLAUDE.md (per-process address space, fd_table; stdin/tty modules)
  - src/userland/tty.rs (global Termios singleton; Winsize hardcoded 80x24)
  - src/userland/stdin.rs (per-WindowId stdin queues — replaced by PTY)
  - src/window/windows/terminal.rs (TerminalWindow + encode_keystroke_for_raw_mode)
  - src/window/windows/text.rs (TextWindow grid + paint + caret rendering)
  - docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md (Phase C raw-mode termios bring-up)
  - docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md (U11 per-process termios already on the radar)
---

# feat: Full-featured terminal — ANSI/VT parser, PTY, scrollback, caret

## Summary

Replace the current line-editor-with-pass-through terminal with a real VT100/xterm-compatible terminal capable of hosting TUI applications (vi/vim, less, htop, nano, agnoster-themed zsh with tab completion). The work touches three independent surfaces: an ANSI/VT escape-sequence parser (currently absent — escape bytes pass through `TextWindow::write_char` as literal characters), a proper PTY pair replacing the WindowId-keyed stdin singleton + global termios, and a correct caret (blink, hide/show, shape, erase-before-redraw). Shipped in a single PR because the three surfaces are deeply intertwined — the parser owns the screen, the PTY owns termios+winsize, and the caret position is owned by the screen the parser writes into.

Validated end-to-end by: BusyBox `vi` runs (open/edit/save), agnoster prompt renders with Powerline glyphs, zsh tab completion redraws correctly, scrollback survives across full-screen TUIs via the alternate-screen buffer, mouse wheel scrolls history.

A follow-up PR adds static `vim` and any theme/font polish discovered along the way.

---

## Problem Frame

**Output side is broken at the foundation.** Today every byte that ring-3 writes to stdout is delivered to `TextWindow::write_char` (`src/window/windows/text.rs:128`) which treats it as a literal glyph. `\x1B` (ESC) renders as an unprintable cell or is silently dropped; `\x1B[2J` (clear screen), `\x1B[1;1H` (cursor home), `\x1B[?1049h` (alt screen), `\x1B[6n` (cursor position report), `\x1B[31m` (red FG) all pass through uninterpreted. agnoster looks like garbage, vi cannot draw, zsh tab completion paints stale prompts.

**Termios is a global singleton** (`src/userland/tty.rs:97` — `static TERMIOS: Mutex<Termios>`). With multi-ring-3 already shipped (`docs/plans/2026-05-16-005-...`), two zsh instances in two terminal windows fight over `ICANON`/`ECHO`. U11 of the multi-ring-3 plan already flags this; this plan resolves it.

**Winsize is hardcoded 80×24** (`tty.rs:127`). TerminalWindow grids vary with the FrameWindow size, but `TIOCGWINSZ` always returns 80×24, so zsh wraps wrong. No SIGWINCH is ever raised when the user resizes.

**stdin routing bypasses termios processing.** `src/userland/stdin.rs` keys queues by `WindowId`. The kernel reads termios flags out-of-band to decide canonical vs raw, but the *data* never passes through a termios input pipeline — no `ICRNL`, no echo-from-driver, no line discipline. It works for raw-mode zsh because zsh paints its own echo; canonical-mode line editing happens in `TerminalWindow` rather than in a kernel TTY layer. There is no concept of a master/slave fd pair; the kernel cannot give a separate process its own PTY.

**Caret rendering is incrementally broken** (`src/window/windows/text.rs:445–456, 515–527`):
- Drawn as a 2 px white bar at `(cursor_x, cursor_y)` only when `has_focus()`.
- No erase of the previous caret cell. The dirty-rect machinery happens to cover the cursor cell during typing because `dirty_rect_hint` expands to include it, but `set_cursor_position` does not push to `dirty_cells` — so cursor-only movement (which any ANSI parser will trigger constantly) leaves stale carets in incremental paint.
- No blink. No shape (block/underline/bar). No hide-cursor support (`\x1B[?25l`).
- In raw mode the kernel still draws a caret at the textwindow's internal `(cursor_x, cursor_y)` while zsh sends ANSI cursor moves the kernel ignores — the visible caret diverges from where zsh thinks the cursor is.

**Scrollback is lossy.** `TextWindow::scroll_up` (`text.rs:197`) calls `Vec::remove(0)` and drops the line on the floor. There is no scrollback buffer; PgUp does nothing. vi cannot save+restore the screen because there's no alt-screen buffer either.

**Input encoding is partial.** `encode_keystroke_for_raw_mode` (`terminal.rs:379`) covers arrows, Home/End, Delete, Tab, Escape, Ctrl-letter. Missing for full TUI: F1–F12, PgUp/PgDn, Insert, Shift+arrow, Meta+key, bracketed paste. Mouse wheel events are not surfaced by `src/input/` at all.

**Font lacks Powerline glyphs.** agnoster uses the Powerline private-use-area codepoints (U+E0A0–U+E0B3). The current font has no glyphs there, so agnoster's prompt separators render as `.notdef` boxes even after the parser is correct.

---

## Goals

- ANSI/VT100/xterm escape sequences are interpreted on the output side. Vi/vim, less, htop, nano, ncurses-based apps render correctly.
- Per-process termios. Each ring-3 process has its own `Termios` struct on its `Process`; two zsh instances in two terminals can be in different modes without interference.
- Per-process PTY pair (master/slave). The slave fd is installed as fd 0/1/2 at process install. The master is owned by the TerminalWindow and is what the keyboard writes to and the screen reads from.
- Correct `TIOCGWINSZ` — winsize derived from the focused TerminalWindow's grid. `SIGWINCH` raised on grid change.
- 24-bit truecolor SGR (`\x1B[38;2;r;g;b m`), 256-color indexed (`\x1B[38;5;n m`), and 16-color basic. Bold, reverse, underline, italic (if font supports).
- Alternate screen buffer (`\x1B[?1049h/l`). Vi enters, edits, exits, and the user's scrollback is intact.
- Scrollback buffer (5000 lines configurable via compile-time const, in a ring). PgUp/PgDn keys + mouse wheel scroll the view; any new output snaps back to the bottom. Disabled while alt-screen is active.
- Caret: blink (toggle ~500 ms), hide via `\x1B[?25l`, shape via DECSCUSR (`\x1B[N q`: 1/2 block, 3/4 underline, 5/6 bar). Erase-old-position on every move.
- Full TUI input: F1–F12, PgUp/PgDn, Insert, Shift+arrow, Ctrl+arrow, Meta+key. Bracketed paste (`\x1B[?2004h/l`).
- Mouse wheel events surfaced from `src/input/` and wired to scrollback.
- Powerline-patched font shipped so agnoster glyphs render.
- `TERM=xterm-256color` in env passed to zsh (so terminfo selection picks a reasonable curses entry).
- BusyBox `vi` runs end-to-end. agnoster prompt renders correctly. zsh tab completion paints without artifacts.

## Non-Goals

- **Full xterm compatibility.** We pick a tractable subset (CSI + relevant DEC private + minimal OSC). Sixel graphics, mouse-mode tracking (`?1000h/?1006h`), bidi, RTL, double-width CJK rendering: out.
- **`vim` (full Bram-vim) build.** Follow-up PR. BusyBox vi is the in-PR proof of life.
- **`tmux` / `screen` / job control.** PTY enables them eventually but they require setsid/setpgid/process-group plumbing not in this plan.
- **Terminal multiplexing inside the window.** One window = one PTY = one foreground process group.
- **Reflowing scrollback on resize.** On grid change we re-anchor the bottom; old lines stay at their original column width. (xterm gets this wrong too.)
- **Asynchronous PTY discipline.** All termios canonical-mode buffering happens on the master side synchronously when bytes are pushed in. No background timer for VTIME.
- **Cleaning up `src/window/console.rs`** (the kernel `print!` ring buffer). It keeps working as today — when ring-3 isn't holding the terminal, kernel prints still appear.

---

## Key Technical Decisions

### A new top-level `src/terminal/` subsystem

The terminal is its own subsystem, not a feature of the window manager or of userland. The window manager owns the *display surface* and the *keyboard event source*; userland owns the *process file descriptors*; the terminal sits between them and owns the *character grid + parser + PTY*.

```
src/terminal/
  mod.rs        # public API: PtyPair, Terminal::new, attach to TerminalWindow
  vte.rs        # Williams DEC state machine — bytes in, parsed events out
  screen.rs     # Grid (primary + alt), attributes, scrollback ring, scroll regions, cursor
  pty.rs        # PtyPair = (PtyMaster, PtySlave), termios, winsize
  caret.rs      # blink phase, shape, visible flag
  colors.rs     # 16/256/truecolor palette + Color resolution
  config.rs     # SCROLLBACK_LINES = 5000, BLINK_INTERVAL_MS = 500, etc.
  keys.rs       # KeyCode → escape sequence encoding (replaces encode_keystroke_for_raw_mode)
```

**Why a separate subsystem:** the parser, grid, and PTY have no dependencies on rendering or event loops — they're pure data transforms. Isolating them lets us unit-test each escape sequence and each termios mode without spinning up a window. Adding them to `src/window/` couples grid semantics to GraphicsDevice; adding them to `src/userland/` couples PTY to syscalls. The current intertwined state is exactly the design we're escaping.

**Alternative considered:** put the parser inside `TerminalWindow` as a field. Rejected — `TerminalWindow` is already the dirtiest file (event handling + output drain + history + paint coordination); piling parser state on top would re-create the rats-nest we're cleaning up.

### `TextWindow` shrinks to a renderer

After the refactor, `TextWindow` does not own a `Vec<Vec<CharCell>>`. It owns a reference (or `Arc`) to a `terminal::Screen` and its `paint()` method walks the screen's current visible viewport. Cursor position, attributes, scroll state — all read from `Screen`. `TerminalWindow` instantiates one `terminal::Terminal` per window; the terminal owns its `Screen` and exposes it for rendering.

**Why:** today `TextWindow` is the grid's owner *and* the renderer *and* the API surface used by `print!`. After the split, only kernel `print!`-direct writes touch a degenerate code path; everything else flows through `Terminal::write_output` → vte parser → screen.

**Migration:** `src/window/console.rs`'s output ring keeps existing as the kernel-side print mechanism. When no PTY is attached (boot / pre-zsh), TextWindow falls back to consuming console output directly via a thin shim that writes through the parser anyway — so even kernel prints get ANSI handling for free.

### PTY pair with kernel-side master

`PtyPair` = a master end (owned by `TerminalWindow` via its `Terminal`) and a slave end (an fd installed on the ring-3 `Process`'s `fd_table` as 0/1/2). Today's `src/userland/stdin.rs` queues go away — they're replaced by:

- **Master → slave (input direction):** keyboard event arrives at TerminalWindow, gets encoded via `terminal::keys::encode_keystroke`, written to the master. The PTY input pipeline applies termios (`ICRNL`, signal generation for `VINTR`/`VQUIT`/`VSUSP`, `ICANON` line buffering, `ECHO` echo into the screen). Bytes that pass through become readable on the slave fd; ring-3 `read(0, ...)` drains them.
- **Slave → master (output direction):** ring-3 `write(1, ...)` pushes bytes into the slave's output queue. The master drains them, feeds them to the vte parser, the parser updates the Screen. Drained on every compositor tick via `Terminal::drain_output`.

Termios + winsize live on the `PtyMaster`. `tcgetattr(0)` / `tcsetattr(0)` syscalls walk the slave fd → its master → reads/writes the master's termios. `TIOCGWINSZ` reads the master's winsize, which the TerminalWindow updates on every `set_bounds`.

**Why not just per-process termios on `Process`:** the PTY abstraction is what tmux/screen/job-control will need; we're paying the cost once. It also gives us a clean place to put the input-side line discipline (ECHO, ICRNL, ISIG signal generation) that today is scattered between `TerminalWindow` and the kernel.

**Lock discipline:** `PtyMaster` is an `Arc<Mutex<PtyMasterInner>>`. The slave end holds the same Arc. Keyboard ISR path → `TerminalWindow::handle_event` → `master.write_keystroke` takes the lock briefly. Ring-3 syscalls → slave fd's read/write methods → take the lock briefly. The vte parser runs under the lock during `Terminal::drain_output` (compositor thread). No lock crosses a yield point.

### The vte parser is the DEC state machine, written from scratch

Paul Williams' DEC state diagram is the de facto reference for VT500-series parsing — `vte`, `libvte`, `alacritty`'s parser, and basically every modern terminal use it. We reimplement a stripped-down version (`no_std` + no deps). States: GROUND, ESCAPE, ESCAPE_INTERMEDIATE, CSI_ENTRY, CSI_PARAM, CSI_INTERMEDIATE, CSI_IGNORE, OSC_STRING, DCS_*, SOS_PM_APC_STRING. The parser emits events:

```rust
enum VteEvent {
    Print(char),
    Execute(u8),             // C0 controls: BS, TAB, LF, CR, BEL
    CsiDispatch { params: &[u16], intermediates: &[u8], final_byte: u8 },
    OscDispatch { params: &[&[u8]] },
    EscDispatch { intermediates: &[u8], final_byte: u8 },
}
```

`screen::Screen` consumes events and mutates itself. The supported CSI sequences (initial set — `Final` byte / meaning):
- `A B C D` — cursor up/down/right/left N
- `E F` — cursor next/prev line N
- `G` — cursor to column N
- `H f` — cursor to (row, col); 1-indexed
- `J` — erase in display (0=below, 1=above, 2=all, 3=all+scrollback)
- `K` — erase in line (0=right, 1=left, 2=all)
- `L M` — insert/delete lines
- `P` — delete characters
- `S T` — scroll up/down N
- `X` — erase N characters
- `d` — cursor to row N
- `h l` — set/reset mode (private with `?`: 7=autowrap, 25=cursor-visible, 1049=alt-screen, 2004=bracketed-paste; non-private: 4=insert mode)
- `m` — SGR (the long one; see colors.rs)
- `n` — DSR (`6n` → reply `\x1B[r;c R` on master input queue)
- `r` — set scroll region
- `s u` — save/restore cursor (xterm flavor; also `\x1B7`/`\x1B8`)
- ` q` (intermediate space) — DECSCUSR cursor shape

Plus ESC dispatches `7 8 D E H M` and OSC `0;… BEL` (window title, stub to FrameWindow title) and `4;n;rgb:rr/gg/bb BEL` (set palette — defer to follow-up if not free).

**Why not pull in the `vte` crate:** it's `no_std` but pulls a couple of small deps and we'd need to vendor it anyway through our custom-toolchain build path. Williams' state machine is ~300 lines of straightforward code. Unit-test coverage is the bigger investment than the parser itself.

### Caret is a screen property, blink is a compositor concern

The Screen owns: `cursor_row`, `cursor_col`, `cursor_visible`, `cursor_shape`. Per-frame, the compositor reads a monotonic millisecond clock (`crate::arch::time::ticks_ms()` or equivalent) and computes `blink_on = (ms / BLINK_INTERVAL_MS) % 2 == 0`. TextWindow's paint draws the caret only when `cursor_visible && (blink_on || not_blinking_state)` and erases the previous position when it changes.

Erase discipline: Screen tracks `prev_cursor_row, prev_cursor_col`. On any cursor move, both prev and new cells are added to `dirty_cells`. After paint, prev := new.

Blink-driven repaint: the compositor invalidates the focused TerminalWindow's caret cell on every blink phase change (~ every 500 ms). This adds one invalidation per second which is well under the repaint budget.

**Hide-in-raw-mode:** when ICANON is off (zsh's zle) the kernel-side caret stays off by default; zsh paints its own. The screen still tracks logical cursor (the ANSI parser drives it) and the caret renders based on `cursor_visible` from `\x1B[?25l/h`. Zsh emits `\x1B[?25h` whenever it wants the caret shown — this lights up the screen-tracked caret in the right spot.

### Mouse wheel through the existing input pipeline

`src/input/` already routes PS/2 mouse events. The hardware emits intellimouse-style wheel data when enabled; we extend the PS/2 mouse init to enable wheel mode (the standard "knock sequence" — set sample rate 200/100/80 then read device ID, expect 3 or 4). `MouseEvent` grows a `wheel_delta: i8` field. `TerminalWindow::handle_event` routes wheel events to `Screen::scroll_view(delta)`. Disabled when alt-screen is active or when xterm mouse-tracking modes are on (deferred).

### Font: ship a Powerline-patched TTF

Use a permissively-licensed Powerline-patched font (e.g., DejaVu Sans Mono for Powerline, Bitstream license). Drop it under `assets/fonts/` and update `src/graphics/fonts/core_font.rs` to load it as the default. Cell metrics will shift slightly; the impact is one re-test of the boot UI. Optional fallback: keep the old font available behind a feature flag for diff-testing rendering.

---

## Implementation Units

Single PR, but ordered units inside it so each commit is reviewable and the test suite stays green after each.

### U1 — `src/terminal/` skeleton + colors + config

Create the module, define `Color` resolution for 16/256/truecolor, `SCROLLBACK_LINES` and friends. No behavior change; just empty pluggable surfaces. Add a smoke test that constructs a `Screen` and asserts its initial state.

### U2 — vte parser + unit tests

Implement the DEC state machine. Extensive unit tests: one per CSI final byte we support, OSC parsing, parameter overflow, escape-in-escape recovery, BS/LF/CR/BEL execution. The parser exists in isolation; no rendering integration yet.

### U3 — `Screen` (primary buffer + cursor + SGR + erase + scroll region)

Implement `Screen` consuming `VteEvent`s. Primary buffer only at this unit. Cursor movement, SGR attribute tracking, erase-in-display/erase-in-line, insert/delete lines/chars, scroll region. Save/restore cursor. Unit tests drive bytes through `vte → Screen` and assert grid state.

### U4 — Scrollback ring + alt-screen buffer

Add scrollback as a `VecDeque<Row>` with capacity `SCROLLBACK_LINES`. Lines scrolled off the top of the primary buffer go in; alt-screen does not consume scrollback. `\x1B[?1049h` swaps to alt-screen (saves cursor, clears alt); `\x1B[?1049l` swaps back. View offset (`scroll_view(delta)`) for PgUp/PgDn and mouse wheel.

### U5 — Caret state + blink

`screen::Cursor` grows visibility, shape, prev-position tracking. Erase-old discipline on move. Blink phase computed in compositor and threaded through to TextWindow paint.

### U6 — PTY pair + per-process termios + winsize

`pty.rs`: `PtyMaster`, `PtySlave`, `PtyPair`. Master owns `Termios` and `Winsize`. Slave fd implements read/write via the master's queues. Move `Termios` off the global `tty.rs` singleton onto the master; `tcgetattr(0)`/`tcsetattr(0)`/`TIOCGWINSZ` resolve through the slave fd. Master input pipeline applies `ICRNL`, `ICANON` line buffering, `ECHO` (writes into the screen's input echo path), `ISIG` (raises SIGINT/SIGQUIT/SIGTSTP on `c_cc[VINTR/VQUIT/VSUSP]`).

This unit deletes `src/userland/stdin.rs` and `src/userland/tty.rs`'s `static TERMIOS` global. `src/window/terminal.rs`'s per-WindowId output buffers also go (output flows slave→master→screen directly). `tty.rs` may keep its `Termios` *struct* + constants as a shared definition imported by `terminal::pty`.

`TerminalWindow` owns a `Terminal` which owns the `PtyPair`. When a ring-3 process is spawned for that terminal, the slave fd is installed as 0/1/2. SIGWINCH raised in `Window::set_bounds` when the grid dimensions change.

### U7 — Key encoding + bracketed paste + new key types

`terminal::keys::encode_keystroke(KeyCode, Modifiers, KeyboardMode) → bytes`. Covers F1–F12 (xterm sequences `\x1BOP..\x1B[24~`), PgUp/PgDn (`\x1B[5~/6~`), Insert (`\x1B[2~`), Shift+arrow / Ctrl+arrow (CSI 1;mod_letter), Meta-prefixed. Bracketed paste: when `\x1B[?2004h` is set, future text input is wrapped with `\x1B[200~ … \x1B[201~`.

### U8 — Mouse wheel in PS/2 driver + scrollback wiring

Enable intellimouse wheel mode in `src/drivers/` PS/2 mouse init. Plumb `wheel_delta` through `MouseEvent`. `TerminalWindow::handle_event` consumes wheel events when focused and dispatches to `Screen::scroll_view`.

### U9 — TextWindow renderer refactor

`TextWindow` no longer owns a buffer. It holds an `Arc<Mutex<Screen>>` (or borrow) and renders the current visible viewport. Existing `prepare_for_render` is replaced with `terminal.drain_output()` (parser consumes pending slave-output bytes). `dirty_rect_hint` reads from `Screen::dirty_rect()`. Caret rendering moves into TextWindow's paint, reading caret state from Screen + blink phase from compositor.

`src/window/console.rs` continues to exist; when no PTY is attached (boot scenario) TextWindow consumes its output via the vte parser directly (so kernel prints get color too). Once zsh spawns and the PTY attaches, console output keeps going through the same path — kernel can still `debug_info!` and it lands in the terminal alongside zsh output, which matches today's behavior.

### U10 — Font swap + TERM env

Add the Powerline-patched TTF to `assets/`, update `core_font.rs` to load it. Verify boot screen still fits. Set `TERM=xterm-256color` in the env passed to zsh by the binary loader.

### U11 — End-to-end validation

Boot, open terminal, run:
- `echo $TERM` → `xterm-256color`
- `ls --color=always /` → colored output
- `vi /tmp/test.txt`, type text, `:wq` → file written, terminal redraws cleanly back to prompt, scrollback intact
- `agnoster` zsh prompt (via `zsh -c '...source agnoster...'` or `.zshrc`) → Powerline glyphs render
- Type a partial command, Tab → completion redraws prompt correctly
- `for i in {1..200}; do echo $i; done` then PgUp → scroll back through output
- Two terminals side by side, vi in one, zsh in the other → independent termios, no interference
- `stty -a` in zsh → reports the terminal's real winsize (not 80×24)

### U12 — Docs

- `src/terminal/CLAUDE.md` describing the subsystem.
- Update `CLAUDE.md` directory index.
- Update `src/window/CLAUDE.md`'s TextWindow entry (it's now a renderer, not a grid owner).
- Update `src/userland/CLAUDE.md`'s tty/stdin entries (replaced by pty).
- Add a learning under `docs/solutions/learnings/` capturing any non-obvious bugs found during bring-up (e.g., interaction between alt-screen and scrollback, blink-induced full-repaints if any).

---

## Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| PTY refactor regresses zsh boot | High | U6 lands behind U1–U5; U6's first commit is the type-only refactor (no behavior change). Run the full zsh-interactive test suite after each U-step. |
| Parser bug freezes terminal under hostile input | Medium | Williams' state machine has well-known recovery transitions (any ESC mid-sequence aborts to ESCAPE). Unit tests cover malformed input. Parser doesn't allocate during steady-state print path. |
| Font swap breaks boot UI layout | Low | Compare-test boot screen before/after; if cell metrics shift, pad FrameWindow contents to absorb it, or pick a font with identical metrics. |
| Mouse wheel init not handled by some QEMU PS/2 backends | Low | Wheel init protocol is well-supported by QEMU's `i8042` device; if it fails on a config, fall back to no-wheel and log. Shift+PgUp/PgDn always works. |
| Blink invalidation causes too many full repaints | Medium | Blink invalidates only the caret cell via `dirty_rect_hint` returning a single-cell rect. If a full-repaint regression shows up, gate blink behind focus + non-typing-burst heuristic. |
| Per-process termios interaction with U11 of multi-ring-3 plan | Medium | U6 of this plan effectively *is* the multi-ring-3 U11. Coordinate by closing U11 in the multi-ring-3 plan as superseded. |
| `console.rs` integration: kernel `print!` racing with ring-3 stdout | Low | Both feed the same vte parser through the screen; the parser is sequential under the screen's lock. Interleaving may look ugly but won't corrupt state. |
| Powerline glyph licensing | Low | DejaVu Sans Mono for Powerline is Bitstream Vera license (very permissive). Note in `assets/fonts/README.md`. |
| Scrollback memory at 5000 lines × (e.g.) 200 cols × ~8 bytes/cell ≈ 8 MiB per terminal | Low | Acceptable on 128 MiB system. Compile-time const lets us tune down. |

---

## Out of Scope (Followups)

- Static `vim` ELF build under `userland/prebuilt/`. Separate PR.
- xterm mouse tracking modes (`?1000h`, `?1006h`) for vim/htop mouse support.
- `tmux` / `screen` (needs setsid/setpgid/process-group plumbing).
- Sixel / Kitty graphics protocols.
- Reflow on resize.
- True line discipline `VTIME` (interbyte timer).
- Per-terminal scrollback configurable at runtime (compile-time is the in-scope answer).

---

## Validation Checklist

- [ ] All existing kernel tests pass (`./test.sh`).
- [ ] New `src/terminal/` modules have unit tests for every CSI/OSC/ESC dispatch implemented.
- [ ] Boot, type into terminal, output renders correctly (no regression from today's behavior).
- [ ] BusyBox `vi /tmp/test.txt` works through full edit cycle.
- [ ] agnoster theme renders with Powerline separators.
- [ ] zsh tab completion: type `ec`, Tab, see `echo` complete; no stale prompt fragments.
- [ ] Scrollback: 200 lines of output, PgUp scrolls, PgDn returns, mouse wheel works.
- [ ] Two terminals, independent termios: vi in one, prompt in the other.
- [ ] `stty size` reports real grid dimensions; resizing the FrameWindow updates them.
- [ ] No memory leak across 10× spawn/exit of vi.
- [ ] Caret: blinks at ~500 ms, disappears on `tput civis`, returns on `tput cnorm`, shape changes via `\x1B[4 q` (underline).
