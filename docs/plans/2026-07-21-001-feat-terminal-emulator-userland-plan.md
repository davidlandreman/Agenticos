---
status: implemented
date: 2026-07-21
---

# feat: Move the terminal emulator to ring-3 (`TERMINAL.ELF`), keep the PTY + line discipline in the kernel

## Implementation status (2026-07-21)

A complete, compiling terminal migration has landed. The new
`TERMINAL.ELF` is the sole interactive terminal; the follow-up deletion plan
`2026-07-22-002-refactor-remove-kernel-terminal-emulator-plan.md` completed U6.

- **U1 — `userland/libs/vte`** (emulator port): `vte`, `screen`, `caret`,
  `colors` (RGB swap), `config`, plus new `color` + `input` modules. Compiles
  warning-clean.
- **U2 — kernel pty master fd ABI**: `FdSlot::PtyMaster`; syscalls `PTY_OPEN`
  (5013) and `PTY_SET_WINSIZE` (5014) in `src/userland/pty_syscalls.rs`;
  read/write/writev/fstat/`select`-`poll` readiness/`/proc/self/fd` all handle
  the variant; `PtyMaster` gained `read_output`/`output_ready`/`same_master`
  and an un-gated `terminal_id`. Keyed on the caller's window `surface_id`
  (plan D1 option a) — reuses the whole existing slave/termios/winsize/SIGWINCH
  registry.
- **U3 — child-on-slave**: `pty_open` binds the caller's `terminal_id`; the
  `fork`ed child inherits it, so its stdio sentinels resolve to the slave.
- **U4 — `userland/libs/termgrid`**: JetBrains Mono rasterizer at 16px + a
  `Screen`→XRGB8888 renderer (per-cell color, reverse/underline, block caret,
  theme-well default bg).
- **U5 — `userland/apps/terminal` (`TERMINAL.ELF`)**: window + `pty_open` +
  `fork`/`execve` zsh + poll loop (master fd + GUI events) + render/present.
  Registered in `apps.manifest.sh` and `bin_namespace.rs` (launch with the
  `terminal` command). Built (357 KB static ET_EXEC) and staged.

Verified: kernel + full userland workspace compile; release build + disk
images succeed; 333 kernel tests pass across the touched modules (pty, vte,
screen, caret, terminal, userland, userland_switch, gui_userland) — no
regressions.

**Runtime-validated (headless QEMU + synthesized input via the `send_input`
RPC tool):** TERMINAL.ELF launches, zsh spawns and prints its prompt,
keystrokes route focused-window → `RemoteSurface` → app → pty master → zsh,
and `ls` echoes and runs with output rendered — full round trip.

Finding during validation: **zsh runs the tty in canonical (cooked) mode, not
raw** — so D4's "rely on zsh's raw-mode self-echo" assumption was wrong. The
fix implements **canonical line discipline in the kernel pty**
(`PtyInner::push_slave_input`: echo + line buffering + backspace/`VERASE`,
CR→LF via ICRNL) on the emulator's master-write path, while the in-kernel
`TerminalWindow` and DSR/DA replies stay on a raw path
(`push_slave_raw`/`PtyMaster::push_input`). Covered by three new pty tests.

**U6 complete (2026-07-22):** the kernel emulator/window factory and desktop
spawn syscall were deleted, and the Start menu now executes
`/host/TERMINAL.ELF`. Live resize remains deferred (fixed 80×24 — `Screen` has
no resize); DSR replies written to the master while in canonical mode would be
line-buffered (rare edge case). See D4/D6 and Risks.

---

## Summary

Split the terminal the way Linux does: the kernel keeps the PTY (fd pair,
termios, winsize, line discipline); the *emulator* (VT/ANSI parser, screen
cells, scrollback, caret, key encoding, glyph rendering) moves into a ring-3
app, `TERMINAL.ELF`. The app opens the **master** end of a PTY as a file
descriptor, `fork/execve`s a shell (zsh) on the **slave** end, and renders its
own cells into a GUI pixel surface via the existing copy-blit ABI. The kernel
`TerminalWindow`/`TextWindow` emulator embedding is deleted, and the compositor
kernel thread stops being the terminal's per-frame data pump
(`src/window/compositor.rs:82`, `crate::window::process_terminal_output()`).

The emulator code is highly portable — `vte`, `screen`, `caret`, `config` have
no kernel dependencies at all; `colors` needs only an RGB-type swap; `keys`
needs the userland key-event types. Ring-3 already rasterizes the bundled
JetBrains Mono TTF (`userland/libs/gui/src/font.rs`, via `ab_glyph_rasterizer`
+ `ttf_parser`), so cell rendering is a **library port, not new graphics
infrastructure**.

**The one genuinely new ABI is the PTY master channel.** Today the master is
held by the kernel `TerminalWindow` keyed by `WindowId`; there is no
`posix_openpt`/`/dev/ptmx`/master-fd path for a ring-3 process. Exposing the
master as a read/write/`select`-able fd, and letting a child inherit the slave
as its controlling tty, is the load-bearing new work. The framing "not new ABI"
is true only of the rendering — not of the master channel.

## Problem Frame

### What exists today (verified)

- **PTY (`src/terminal/pty.rs`, 617 lines).** `PtyInner` owns `slave_queue`,
  `master_queue`, `Termios`, `Winsize`. Registered per `WindowId` in a global
  `REGISTRY` (`install_for_terminal`, `slave_for_terminal`,
  `master_for_terminal`). `PtyMaster` is held **only** by the kernel — there is
  no fd wrapping it. `PtySlave` is reached by syscall handlers via
  `Process.terminal_id` (not a real fd either — fds 0/1/2 are
  `FdSlot::Stdin/Stdout/Stderr` sentinels that resolve through
  `pty::slave_for_terminal`).
- **Emulator (`src/terminal/`, ~3.9k lines total):** `vte.rs` (835),
  `screen.rs` (1680), `keys.rs` (386), `caret.rs` (97), `colors.rs` (126),
  `config.rs` (26), `mod.rs` (99). Kernel deps (grepped):
  - `vte.rs`, `screen.rs`, `caret.rs`, `config.rs` — **none.** Pure.
  - `colors.rs` — only `crate::graphics::color::Color` (an RGB struct).
  - `keys.rs` — `crate::window::event::{KeyCode, KeyModifiers}` and
    `crate::window::keyboard::keycode_to_char`.
  - `pty.rs` — `crate::lib::arc::Arc`, `crate::window::WindowId` (**stays in
    the kernel**).
- **Kernel emulator embedding (would be gutted/deleted):**
  - `src/window/windows/terminal.rs` (579) — owns `vte`, `screen`, the PTY
    drain (`process_terminal_output`, `take_terminal_output`→`drain_output`),
    keystroke encoding (`terminal::keys::encode_keystroke`), color resolution,
    `sync_text_window_from_screen`.
  - `src/window/windows/text.rs` (665) — `TextWindow`: cell grid
    (`buffer: Vec<Vec<CharCell>>`), `set_cell`, incremental/full `paint`.
  - `src/window/terminal.rs` (154) — registry glue (`register_terminal`,
    `sync_terminal_winsize`, `write_to_terminal_id`, `take_terminal_output`,
    `invalidate_dirty_terminals`).
  - `src/window/terminal_factory.rs` — builds a `TerminalWindow` + registers a
    PTY + launches zsh with a matching `terminal_id`.
- **Per-frame pump (would be removed):** `src/window/compositor.rs::run` calls
  `invalidate_dirty_terminals()` (:77) and `process_terminal_output()` (:82)
  every tick; `WindowManager::prepare_windows_for_render`
  (`src/window/manager.rs:1872`) calls `TerminalWindow::prepare_for_render`,
  which drains the master, runs the VTE→Screen, and mirrors into the
  `TextWindow` grid. All of that leaves the kernel.
- **Userland already has the pieces the app needs:**
  - GUI ABI: `gui_win_create` (5001), `gui_win_present` copy-blit (5002),
    `gui_next_event` (5003), `gui_win_destroy` (5004), `gui_win_set_title`
    (5005), `gui_event_open` selectable event fd (5011). Wrappers in
    `userland/runtime/src/lib.rs`; events are the 32-byte `GuiEvent { kind,
    window, payload[6] }` with `GUI_EVENT_KEY/MOUSE/RESIZE/CLOSE/...`.
  - TTF rasterization in ring-3: `userland/libs/gui/src/font.rs` bakes
    `assets/system.ttf`, rasterizes ASCII via `ab_glyph_rasterizer`, caches
    glyphs, exposes monospaced `CELL_WIDTH`/`LINE_HEIGHT`.
  - `fork`/`execve`/`wait4` wrappers exist (`userland/runtime/src/lib.rs`).
  - The `select`-on-a-GUI-event-fd pattern is proven: **Links2 already folds
    the 5011 descriptor into its `select(2)` loop** alongside socket fds
    (`userland/apps/links2/README.md`). `FdSlot::GuiEvents` is already
    select/poll-integrated in the kernel (`WaitingForGuiEvent`).
  - App registration is one line in `userland/apps.manifest.sh` +
    `bin_namespace.rs`; `TASKMGR.ELF` is the closest structural template
    (text/graph rendering, `GUI_NONBLOCK` event drain, own paint loop).

### The gap

A ring-3 process cannot today (a) obtain a PTY master as an fd, (b) put a child
on the matching slave as its controlling terminal, or (c) drive winsize /
SIGWINCH from userland. Those are the new kernel surfaces this plan adds.

## Goals

1. `TERMINAL.ELF`: a ring-3 terminal emulator that opens a PTY master fd,
   spawns zsh on the slave, parses output, renders cells (full 16/256/RGB
   color, bold/underline, caret, scrollback) to a GUI surface, and encodes
   keystrokes back to the master.
2. Kernel keeps PTY + termios + winsize + line discipline (`src/terminal/pty.rs`
   stays and grows a real master-fd surface).
3. Delete the kernel `TerminalWindow`/`TextWindow` emulator; the compositor
   thread no longer pumps PTYs.
4. Reuse existing GUI + font + fork/exec/select machinery; add the minimum new
   ABI (PTY master channel) only.
5. Feature parity with today: ANSI/VT, scrollback (Shift+PgUp/PgDn), resize →
   SIGWINCH, per-pty termios (`TCGETS`/`TCSETS`), theme-aware translucent
   content well, JetBrains Mono.

## Non-Goals

- Rewriting the VT parser or Screen semantics — they port as-is.
- Kernel-side line discipline promotion (ICANON/ECHO/ISIG in the master
  pipeline). This plan ships path (a) in D4 (emulator does cooked mode, as
  `TerminalWindow` does today) to preserve parity; the kernel N_TTY promotion
  is the recommended follow-up, not part of this change.
- New graphics infrastructure. Rendering is a userland library port.
- Multiplexing/tabs, sixel, mouse-tracking modes (already out of scope).
- The kernel `print!`/RPC console path (`src/window/mod.rs:483`,
  `RPC_TERMINAL_ID` in `src/kernel.rs:236`) — keep a minimal kernel console
  sink for panics/boot diagnostics; only the *interactive* terminal moves.

## Key Technical Decisions

### D1 — PTY master as a first-class fd (the new ABI)

Add a `FdSlot::PtyMaster(Arc<PtyMasterHandle>)` and a syscall that allocates a
fresh PTY pair and returns the master fd. Two viable shapes:

- **Preferred: `/dev/ptmx`-style via a private syscall or `openat("/dev/ptmx")`.**
  A `openpt`-like call returns the master fd; the pair is keyed by a new
  `PtyId` (decouple from `WindowId`, which no longer owns terminals). `read`/
  `write`/`close`/`select`/`poll`/`ppoll` on the master route to
  `PtyInner::{drain_master_output, push_slave_input}`. Readiness integrates with
  the existing select core exactly like `FdSlot::GuiEvents` /
  `FdSlot::Socket` (`select_common`, a new `Ring3BlockReason::WaitingForPty…`
  or reuse of the descriptor-readiness sequence).
- Reuse the `slave_queue`/`master_queue` and cap logic already in `pty.rs`
  unchanged; only the *ownership/addressing* changes (fd + `PtyId` instead of
  kernel-held `PtyMaster` + `WindowId`).

The child gets the slave: extend the launch/`clone`/`execve` path so the
emulator can hand its child a `terminal_id`/`PtyId` such that the child's fds
0/1/2 resolve to *this* pair's slave. Options in ascending ABI cost:
  - (a) A `LaunchSpec.terminal_id`-style field the emulator sets when it spawns
    zsh (mirrors today's `process_service` terminal launch context) — likely
    the smallest change, since `terminal_id` plumbing already exists.
  - (b) A real `TIOCSCTTY`/slave-fd-open path (`openat` on the slave name) if we
    want POSIX-faithful controlling-tty semantics.
  Decision: start with (a) keyed on the new `PtyId`; leave (b) as a follow-up.

Winsize/SIGWINCH move to a master-side `ioctl(TIOCSWINSZ)` from the emulator
(it knows its own pixel/cell geometry on resize), which raises SIGWINCH on the
slave's process group via the existing
`lifecycle::raise_signal_on_terminal`-equivalent keyed by `PtyId`.
`TIOCGWINSZ`/`TCGETS`/`TCSETS` on the slave keep working through the same
`PtyInner`.

### D2 — New userland crate `userland/libs/vte` (the emulator port)

Move `vte.rs`, `screen.rs`, `caret.rs`, `colors.rs`, `config.rs`, `keys.rs`
into a `no_std` ring-3 library crate. Changes required:
- `colors.rs`: replace `crate::graphics::color::Color` with a local
  `Rgb`/`u32` (the app renders to XRGB8888 anyway).
- `keys.rs`: replace `crate::window::event::{KeyCode, KeyModifiers}` and
  `keycode_to_char` with the userland key representation delivered by
  `GUI_EVENT_KEY` (map raw keycode+modifiers from the `GuiEvent` payload; the
  `gui`/`gui-core` libs already decode control input). Keep the escape-sequence
  tables verbatim.
- `vte.rs`/`screen.rs`/`caret.rs`/`config.rs`: compile unchanged (they only use
  `core`/`alloc`). Port the existing unit tests into the crate's test target or
  a host-runnable harness.

Keep the kernel copy deletable: nothing in the kernel should depend on the
emulator after the move except `pty.rs` (which never did).

### D3 — Cell renderer library `userland/libs/termgrid` (extend the font layer)

Today `userland/libs/gui/src/font.rs` rasterizes **ASCII only, at 11px caption
size (cell 7×14), default foreground, `pub(crate)` glyph API**. Today's kernel
terminal renders at **16px** (`get_terminal_font`, `TERMINAL_FONT_PX=16` in
`src/graphics/fonts/core_font.rs`), so to match current cell metrics the
renderer must target the 16px face. The kernel's `TtfFont`
(`src/graphics/fonts/ttf.rs`) is a portable reference — it uses the *same*
crates (`ttf_parser` + `ab_glyph_rasterizer` + `libm`) and the same
`assets/system.ttf`, so either extend `font.rs` (larger size, full byte range,
bold variant, public API) or port `TtfFont` into the crate. A terminal needs:
arbitrary cell size, full `ColorSpec` fg/bg per cell, bold/underline (attrs
already on `screen::Cell`), caret, and scrollback viewport. Build a small
renderer that:
- Draws `Screen`'s visible rows into an XRGB8888 backbuffer, honoring
  `colors::resolve` for fg/bg and the `bg_is_default` semantic bit (so the
  Aero/Futurism translucent `#202020` content well is preserved — this is the
  same trick `sync_text_window_from_screen` uses today).
- Reuses/extends the `font.rs` glyph cache: parameterize cell size, add a bold
  variant, keep the ASCII fast path + lazy non-ASCII `BTreeMap`.
- Draws the caret (`screen.caret()`), with blink driven by the app loop
  (`caret::blink_on_at(ms)` + `clock_gettime`).
- Presents via `gui_win_present` (copy-blit). Incremental damage is a nice-to-
  have; a full-surface present per dirty frame is acceptable to start
  (`TASKMGR`/`PAINTING` already present full surfaces).

### D4 — Where cooked-mode line editing + Ctrl-C/ISIG go (the sharpest decision)

**Today canonical-mode line editing does not live in the kernel PTY — it lives
in `TerminalWindow`** (`src/window/windows/terminal.rs`): echo, backspace,
Enter-buffering, history are done in the GUI window, which then pushes the
finished line to the slave queue (`src/terminal/CLAUDE.md` "Line discipline is
currently minimal"). Deleting `TerminalWindow` forces the question of where
that logic goes. Two paths:

- **(a) Minimal parity — emulator does cooked mode (recommended first).** The
  ring-3 emulator replicates today's behavior: in ICANON it does local
  echo/backspace/line-buffer and writes completed lines to the master; in raw
  mode it writes raw keystrokes. This keeps the kernel line discipline
  unchanged. **Consequence:** ISIG (Ctrl-C→SIGINT, Ctrl-Z→SIGTSTP,
  Ctrl-\→SIGQUIT) must be generated by the emulator, which knows its child's
  pid — it calls `kill(child, SIGINT)` (or the child's pgrp). This is *not*
  faithful to Linux (a real xterm relies on kernel N_TTY) but matches current
  behavior exactly and is the smallest step.
- **(b) Linux-faithful — promote N_TTY into the kernel master pipeline.** The
  emulator writes raw bytes; the kernel PTY does ICANON/ECHO/ISIG (echo back
  out the master, deliver whole lines to the slave, generate signals on
  VINTR/VSUSP/VQUIT to the foreground process group). This is the pre-existing
  follow-up flagged in `src/terminal/CLAUDE.md`; it removes the wart and is the
  "right" endpoint, but it is a larger, independently-testable change.

Decision: ship (a) to preserve parity and unblock the split, and treat (b) as
the natural follow-up (it also unblocks proper job control). Either way, the
emulator needs a signal path to its child — verify `kill(2)` to the child pgrp
works from ring-3 (the kernel's single-user `kill` addresses any live PID).

### D5 — App structure (`userland/apps/terminal`, template = `taskmgr`)

Event loop:
1. `gui_win_create` → window; `gui_event_open(NONBLOCK|CLOEXEC)` → GUI event fd.
2. `openpt`-style syscall → master fd; `fork` + set child's slave terminal
   context + `execve` zsh with `DEFAULT_USER_ENV` (`TERM=xterm-256color`) and
   the shipped `/etc/zshrc`.
3. `select` on `{master_fd, gui_event_fd}` (add a `select`/`ppoll` wrapper to
   `userland/runtime`; kernel already supports all four).
   - master readable → read bytes → `vte.advance` into `Screen` → push
     `Screen::take_replies()` back to master (`write`) → set title from
     `take_title()` (`gui_win_set_title`) → mark dirty.
   - GUI event → key → `keys::encode_keystroke` → `write(master)`; resize →
     recompute rows/cols, `ioctl(TIOCSWINSZ)` on master, resize surface; mouse
     wheel → `Screen::scroll_view`; close → tear down, `kill` child, exit;
     theme-change → repaint with new well style.
   - dirty → render cells → `gui_win_present`.
4. On child exit (`wait4`) → exit the app (or show "[process completed]").

### D6 — Kernel deletions / rewiring

- `terminal_factory::spawn_terminal[_with_shell]` → instead launch
  `TERMINAL.ELF` via `process_service::submit` (like `spawn_taskmgr`). The
  Start-menu/`terminal` command and `guishell` `SpawnTerminal` point here.
- Delete `TerminalWindow` (`src/window/windows/terminal.rs`) and reduce
  `TextWindow` (`src/window/windows/text.rs`) to nothing, or delete it if no
  other window type uses the cell grid (audit `list.rs`, `label.rs`, etc. —
  they draw text but not via `TextWindow`'s grid).
- Delete `src/window/terminal.rs` registry glue and the compositor calls
  (`src/window/compositor.rs:77,82`), and the `prepare_for_render` terminal
  override. Keep the generic `prepare_windows_for_render` enumeration.
- `pty.rs`: rekey `REGISTRY` on `PtyId`; drop the `WindowId` coupling and the
  `terminal_id`-keyed SIGWINCH-by-window path in favor of `PtyId` + process
  group.
- `src/userland/stdin.rs` / `tty.rs` / `syscalls.rs` write paths
  (`write_to_terminal_id` at :323/:425/:3701): reroute the slave lookup from
  `WindowId`→`PtyId`; the slave-side semantics are unchanged.
- Keep a minimal kernel console sink for panics/boot text (do **not** delete the
  `src/window/mod.rs:483` console pump wholesale until a replacement panic/boot
  output path is confirmed — see Risks).

## Implementation Units

- **U1 — `userland/libs/vte` crate.** Move emulator modules; swap `colors`
  RGB type; stub `keys` types; port unit tests. Kernel still has its own copy;
  no behavior change yet. *Gate: crate builds `no_std`, all ported vte/screen
  tests pass.*
- **U2 — PTY master fd ABI.** `FdSlot::PtyMaster`, `openpt` syscall, `PtyId`
  rekey of `pty::REGISTRY`, master read/write/close + select/poll readiness,
  `TIOCSWINSZ`/`TIOCGWINSZ` on master/slave. Kernel unit tests mirroring the
  existing `pty.rs` tests, plus a select-readiness test. *Gate: a synthetic
  ring-3 fixture opens a master, spawns a child echoing on the slave, and
  `select` wakes on master-readable.*
- **U3 — Child-on-slave launch context.** Extend `LaunchSpec`/`process_service`
  so the emulator's forked child resolves fds 0/1/2 to the `PtyId`'s slave;
  SIGWINCH by process group. *Gate: zsh launched under the fixture reads input
  and writes output through the pair.*
- **U4 — `userland/libs/termgrid` renderer.** Extend `font.rs` (cell size, bold,
  color, caret); Screen→XRGB8888. *Gate: golden-image or checksum test of a
  known Screen state rendered to a buffer.*
- **U5 — `userland/apps/terminal` (`TERMINAL.ELF`).** Full event loop
  (D5), cooked-mode editing + ISIG per D4(a). Register in `apps.manifest.sh`,
  `bin_namespace.rs`, `start_menu.rs`, `commands/guishell/mod.rs`. Add the
  missing `userland/runtime` wrappers — the crate today has **no**
  `select`/`poll`/`ppoll`/`ioctl`/`pipe`/`dup` wrappers (only `read`/`write`/
  `openat`/`close`/`lseek`), so add raw `syscall*` wrappers for the ones the
  loop needs. *Gate: interactive boot — launch from zsh command / Start menu;
  run `ls`, `vi`, `htop`, resize, scroll, Ctrl-C.*
- **U6 — Rewire launchers, delete kernel emulator.** Point
  `terminal_factory`/`guishell`/Start menu at `TERMINAL.ELF`; delete
  `TerminalWindow`, `src/window/terminal.rs`, compositor pump calls; shrink
  `TextWindow`; rekey stdin/tty/syscalls write paths to `PtyId`; preserve a
  minimal panic/boot console. *Gate: full boot to desktop, multiple concurrent
  `TERMINAL.ELF` windows, theme switching, `./test.sh` green.*
- **U7 — Parity + polish.** Translucent content well per theme, bracketed
  paste, `pbcopy`/`pbpaste` from inside the app, caret blink, incremental
  present. *Gate: side-by-side with pre-change behavior.*
- **U8 — Docs.** Rewrite `src/terminal/CLAUDE.md` (now just the kernel PTY),
  add `userland/apps/terminal/README.md` and lib docs, update root `CLAUDE.md`
  terminal paragraph and the compositor/`src/window/CLAUDE.md` coupling notes.

## Risks

- **Panic/boot console.** The kernel currently renders early boot text and
  panic output through the terminal/console path. If the *interactive* emulator
  leaves ring-3, the kernel still needs a text sink for panics before/without a
  ring-3 terminal. Keep the `src/window/mod.rs:483` console pump (or a slimmer
  framebuffer text writer) alive; do not couple it to the deleted
  `TerminalWindow`. **De-risk first** (spike in U2/U6).
- **Controlling-tty faithfulness.** Option (a) in D1 reuses `terminal_id`
  plumbing and may not give full POSIX session/pgrp semantics (job control,
  `TIOCSCTTY`). zsh job control and Ctrl-Z/`fg` need SIGTSTP/SIGCONT +
  foreground pgrp; verify what today's kernel already supports and scope
  accordingly (may be an accepted parity gap, as today).
- **Startup ordering / self-hosting.** `TERMINAL.ELF` must exist and be
  launchable before the desktop offers a terminal. It builds every run (Rust,
  like `TASKMGR.ELF`), so no prebuilt/toolchain concern — but the boot path that
  auto-opens a terminal must handle launch failure gracefully.
- **Latency.** Today the compositor drains the PTY inline each frame; now it is
  a scheduled ring-3 app selecting on an fd. This is *better* for coupling but
  the app must be scheduled promptly on master-readable — verify the
  descriptor-readiness wake path drives it as tightly as `WaitingForGuiEvent`
  drives Links.
- **`TextWindow` reuse audit.** Confirm no non-terminal window depends on
  `TextWindow`'s cell grid before deleting it.

## Validation Checklist

- `./test.sh` green (kernel PTY tests rekeyed to `PtyId`; select-readiness
  test; ported vte/screen tests in the userland crate's target).
- Interactive boot: launch `TERMINAL.ELF` from Start menu and `terminal`
  command; two concurrent windows with independent termios.
- `ls`, `vi`/`vim`, `htop`/`top`, `less`, colored `git log`, `tcc` build+run.
- Resize → SIGWINCH (`vi` reflows); Shift+PgUp/PgDn scrollback; Ctrl-C / Ctrl-D.
- Theme switch (Classic/Aero/Futurism) updates the content well live.
- Panic path still prints to screen with no interactive terminal open.
- `git grep` shows no kernel references to the moved emulator modules except
  `src/terminal/pty.rs`.

## Out of Scope (Followups)

- Kernel-side canonical line discipline in the master pipeline (pre-existing).
- Full job control / session + controlling-tty (`TIOCSCTTY`) semantics.
- Tabs/splits, sixel, mouse-tracking (`?1000h`/`?1006h`), true-incremental
  damage present.
- Sharing the `termgrid`/`vte` libs with other ring-3 consumers (e.g. a
  `screen`/`tmux`-style multiplexer).

## Cross-references

- Kernel PTY: `src/terminal/pty.rs`, `src/terminal/CLAUDE.md`.
- Compositor pump: `src/window/compositor.rs`, `src/window/manager.rs:1872`,
  `src/window/windows/terminal.rs`, `src/window/windows/text.rs`.
- GUI ABI: `src/userland/gui_syscalls.rs`, `src/userland/CLAUDE.md`,
  `userland/runtime/src/lib.rs`.
- Userland templates: `userland/apps/taskmgr`, `userland/apps/links2`
  (select-on-GUI-fd), `userland/libs/gui/src/font.rs`.
- Original terminal plan: `docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`.
