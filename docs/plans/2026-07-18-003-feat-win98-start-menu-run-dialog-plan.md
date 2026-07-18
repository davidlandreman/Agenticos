---
title: "feat: Windows 98 Start menu with Programs fly-out and working Run dialog"
type: feat
status: completed
date: 2026-07-18
---

# Windows 98 Start menu with Programs fly-out and working Run dialog

## Summary

Replace the current flat four-row Start popup with a Windows 95/98-style Start
menu:

```text
+----------------------------+------------------+
| A | Programs             > | Terminal         |
| g | Documents              | Notepad          |
| e | Settings               | Painting         |
| n | Run...                  | Calc             |
| t |-------------------------+------------------+
| i | Shut Down...            |
| c |                         |
| O |                         |
| S |                         |
+-----------------------------+
```

The left strip is a blue vertical banner with the word `AgenticOS` rotated 90
degrees, matching the classic Windows 95/98 Start-menu treatment. `Programs`
opens a fly-out containing the four applications that are currently flat Start
items. `Documents` and `Settings` are visible disabled placeholders. `Run...`
opens a real modal dialog and executes the submitted command through the bundled
zsh. `Shut Down...` is the last row and has a separator above it.

This is a Start-menu improvement, not a general rewrite of the taskbar or menu
system. The existing taskbar window, Start button, window buttons, and generic
`MenuWindow` context menus remain in place.

## Current state

- `src/commands/guishell/mod.rs::show_start_menu` creates a generic
  `MenuWindow` with four flat actions: Terminal, Notepad, Painting, and Calc.
  Its height is derived from a local hardcoded item count.
- `src/window/windows/menu.rs` only supports uniformly sized text rows. It has
  no separator, disabled-row, sidebar, or submenu representation. The Tasks
  app also uses this widget for its context menu, so changing its behavior to
  be Start-specific would risk a regression elsewhere.
- `WindowManager` tracks one active popup and closes it when a mouse-down lands
  outside that popup's bounds. A multi-window submenu would therefore require
  teaching the manager about popup groups and union hit regions.
- Kernel widgets already provide `FrameWindow`, `ContainerWindow`, `Label`,
  `TextInput`, and `Button`, and the manager already provides modal routing.
  These are sufficient for a native Run dialog; no GUI syscall or ring-3 app is
  needed.
- `terminal_factory` already knows how to launch `/host/ZSH.ELF` with the
  system PATH/environment. Passing `[ZSH_HOST_PATH, "-c", command]` gives the
  Run dialog shell parsing, quoting, arguments, and `/bin` lookup without
  adding a second command parser.
- AgenticOS has filesystem sync support but no production halt/power-off path.
  BusyBox explicitly disables `halt`/`poweroff`, and `build.sh` launches QEMU
  with `-no-shutdown`. This plan therefore adds the requested Shut Down menu
  affordance but does not pretend that an abrupt emulator exit is a clean OS
  shutdown.

## Product decisions

### PD1 — one dedicated Start-menu window, not nested popup windows

Add `StartMenuWindow`, a Start-specific widget that paints the root menu and,
when Programs is open, its fly-out in one dynamically widened window. From the
window manager's perspective it remains the one active popup, so existing
outside-click dismissal works for the union of the root and fly-out.

This keeps classic Start-menu policy out of `MenuWindow`, avoids popup-group
state in `WindowManager`, and leaves Tasks' context menu unchanged.

### PD2 — Documents and Settings are disabled placeholders

The request calls these entries empty. Render both labels in the disabled
embossed-grey style, give them no hover highlight or callback, and do not open
empty fly-outs. They remain explicit typed rows so a later feature can replace
either with a submenu without changing Start-menu geometry.

### PD3 — Run commands use zsh as the one command-language implementation

The text entered in Run is passed as one unchanged `argv[2]` string to
`zsh -c`; it is never interpolated into a kernel-built command string. This
supports commands such as `notepad`, `notepad /data/note.txt`, quoted paths,
and ordinary shell syntax while reusing the same `PATH=/bin:/host` resolution
as the terminal.

The launch has no terminal attached, matching the visual behavior expected for
GUI applications. Stdout/stderr from console-only commands continues to serial;
the Run dialog is for launching programs, while Terminal remains the UI for
interactive/console output. A command-not-found or other non-zero zsh exit is
reported in a kernel message box.

### PD4 — Shut Down is safe and explicit in this feature

Selecting `Shut Down...` closes Start and displays an informational dialog that
clean shutdown is not available yet. Do not write directly to QEMU's test-exit
port: that bypasses filesystem/process shutdown, produces a test-oriented host
exit code, and would silently encode an emulator-specific platform policy in a
taskbar change.

A real shutdown follow-up must define process termination, `vfs_sync_all`, FAT
clean-bit handling, interrupt/device quiescing, ACPI S5 (or a documented
QEMU-only fallback), and whether interactive `build.sh` should retain
`-no-shutdown`.

## Requirements

### R1 — Start-menu model and classic layout

- **R1.1.** Add `src/window/windows/start_menu.rs` and export
  `StartMenuWindow`, `StartMenuItem`, and `StartMenuAction` from
  `src/window/windows/mod.rs`.
- **R1.2.** Use a typed item model rather than positional integer meaning:
  action row, submenu row, disabled placeholder, and separator. Program rows
  carry `StartMenuAction::{Terminal, Notepad, Painting, Calc}`; root actions
  carry `Run` and `ShutDown`.
- **R1.3.** Root order is exactly:
  1. `Programs` (fly-out indicator)
  2. `Documents` (disabled)
  3. `Settings` (disabled)
  4. `Run...`
  5. separator
  6. `Shut Down...`
- **R1.4.** Programs order is exactly: `Terminal`, `Notepad`, `Painting`,
  `Calc`. These are removed from the root; no duplicate flat launch rows remain.
- **R1.5.** Derive root/fly-out bounds and hit regions from the item model.
  Do not retain a hardcoded `menu_items` count in guishell. Root action rows
  are a roomy 32 px high, Programs fly-out rows remain 24 px high, the
  separator consumes 8 px, and a 2 px classic border surrounds each panel.
- **R1.6.** Root menu width includes a 28 px banner plus a content panel wide
  enough for `Shut Down...` and the Programs arrow. The fly-out begins against
  the right edge of the root panel and aligns with the Programs row. Clamp the
  combined window to the screen's right/top edge so it remains usable on small
  displays.

### R2 — Windows 95/98 visual treatment

- **R2.1.** Paint both panels with classic ButtonFace (`#C0C0C0`) and raised
  light/dark bevel edges. Use navy (`#000080`) with white text for hover and
  selection, matching the existing classic frame palette.
- **R2.2.** Paint separators as the standard two-line inset rule: dark shadow
  on top and white highlight immediately below, inset from the content panel's
  left/right edges.
- **R2.3.** Paint disabled placeholders in shadow/highlight embossed text and
  suppress their hover/activation behavior.
- **R2.4.** Paint a blue banner down the full left side of the root panel. Add
  a small local glyph renderer that rotates the existing font coverage 90
  degrees counter-clockwise and draws `AgenticOS` bottom-to-top in white. It
  must blend partial glyph coverage through `read_pixel`/`draw_pixel`, so the
  label renders consistently on both legacy and retained renderers. Do not add
  a new font asset or pre-rendered bitmap.
- **R2.5.** Draw a small right-pointing pixel triangle for Programs. Do not add
  speculative icons for the rows; iconography is outside this pass.

### R3 — Programs fly-out and interaction

- **R3.1.** Hovering or clicking Programs opens the fly-out by updating
  `StartMenuWindow` state and widening its bounds. Moving to another root row
  closes it; moving between Programs and the fly-out keeps it open.
- **R3.2.** Mouse-down/up on an enabled leaf triggers the typed selection
  callback once. Separators and disabled rows never become selections.
- **R3.3.** Keep the root and fly-out painted and hit-tested inside the same
  window, so `WindowManager::active_menu`, click-outside dismissal, z-order,
  and guishell's stale-menu cleanup keep their existing semantics.
- **R3.4.** Selecting a program queues the existing deferred spawn action.
  Guishell closes the full Start window before launching the application, as
  it does today.

### R4 — modal Run dialog

- **R4.1.** Add `src/window/dialogs/run.rs`, exported as
  `open_run_dialog()` plus a non-blocking `poll_run_dialog()`. Only one Run
  dialog may exist, and opening it while another kernel modal is active fails
  cleanly instead of replacing global dialog state.
- **R4.2.** The centered classic frame is titled `Run` and contains:
  - explanatory text (`Type the name of a program or command, and AgenticOS
    will open it for you.`),
  - a single-line `TextInput`, initially empty and focused,
  - `OK` and `Cancel` buttons.
  Browse/history/autocomplete are outside v1.
- **R4.3.** Extend the kernel `TextInput` with the minimal reusable APIs this
  dialog needs: `text()`, `set_max_length`, `on_change`, `on_submit`, and
  `on_cancel`. Enter invokes submit and Esc invokes cancel in the focused input
  (keyboard routing does not otherwise bubble ignored keys to the frame). Cap
  Run input at 256 UTF-8 bytes and never slice in the middle of a code point.
- **R4.4.** The dialog tracks the current input outside the window-manager
  registry through a small mutex-protected state. This avoids downcasting and
  avoids recursively locking the window manager from a Button callback.
- **R4.5.** Clicking OK or pressing Enter with non-whitespace input returns the
  original command string. Empty/whitespace-only submission leaves the dialog
  open. Cancel, Esc, or the frame close button returns cancellation.
- **R4.6.** Polling owns cleanup: clear manager modality, destroy the complete
  frame subtree, clear dialog state, and return exactly one completion event.
  The guishell process continues taskbar synchronization while the dialog is
  open; do not block it in a modal spin loop.

### R5 — command launch path

- **R5.1.** Add `terminal_factory::spawn_run_command(command: String)` using a
  kernel wrapper process without a terminal association. It calls
  `launch_user_binary(ZSH_HOST_PATH, &[ZSH_HOST_PATH, "-c", command],
  &TERMINAL_SHELL_ENV)` and owns the command string for the child lifetime.
- **R5.2.** Preserve the command as one argv element. Do not tokenize it in
  guishell, concatenate shell quoting, special-case application names, or
  duplicate `/bin` namespace lookup.
- **R5.3.** Log the process exit kind/code. On launch failure or non-zero exit,
  open a concise error message box naming the submitted command and exit code;
  normal exit produces no extra window.
- **R5.4.** Add `PendingAction::OpenRunDialog`. Processing it first closes the
  Start menu, then opens the modal. Each guishell poll checks for Run-dialog
  completion and calls `spawn_run_command` only for a submitted value.

### R6 — Shut Down and placeholder actions

- **R6.1.** `Documents` and `Settings` have no pending actions and cannot
  overwrite an already queued guishell action.
- **R6.2.** `StartMenuAction::ShutDown` maps to a deferred guishell action that
  closes Start and calls the existing kernel message-box infrastructure with a
  clear `Shutdown is not available yet` explanation.
- **R6.3.** The Shut Down separator is part of the typed menu model, not a
  manually drawn y-coordinate, and is included in height/hit-test calculation.
- **R6.4.** Do not modify QEMU flags, the test-exit device, ACPI, process
  lifecycle, filesystem shutdown, or BusyBox configuration in this feature.

### R7 — documentation and code hygiene

- **R7.1.** Update `src/commands/CLAUDE.md` launch-path notes to describe the
  Programs fly-out and Run command path.
- **R7.2.** Update `src/window/CLAUDE.md` to list `StartMenuWindow` and the
  classic Start-menu/banner behavior.
- **R7.3.** Update the root `CLAUDE.md` current-state paragraph so it no longer
  describes the four applications as flat Start entries.
- **R7.4.** Flip this plan to `completed` when the implementation and boot
  verification land.

## Implementation units

### U1 — Start-menu widget and renderer

Files:

- Add `src/window/windows/start_menu.rs`
- Modify `src/window/windows/mod.rs`
- Add `src/tests/start_menu_tests.rs`
- Modify `src/tests/mod.rs`

Implement the typed row model, layout calculation, root/fly-out state machine,
classic painting, rotated banner text, and event-to-action callback. Keep every
calculation local to the widget so guishell only supplies actions and screen
placement.

### U2 — Run dialog and TextInput completion hooks

Files:

- Add `src/window/dialogs/run.rs`
- Modify `src/window/dialogs/mod.rs`
- Modify `src/window/windows/text_input.rs`
- Extend `src/tests/start_menu_tests.rs` or add focused widget tests

Implement the non-blocking modal lifecycle, focused input, OK/Cancel/Enter/Esc
behavior, UTF-8-safe length cap, and cleanup. Keep input/result state separate
from the manager registry to preserve the no-downcast pattern used by the
window system.

### U3 — command execution helper

Files:

- Modify `src/window/terminal_factory.rs`
- Add unit coverage in the closest terminal/userland test module

Extract a small shared zsh-launch helper if needed so interactive terminals and
Run commands share the binary path and environment but supply different argv
and terminal associations. Test argv construction as pure data; leave the
existing booted zsh launch regression intact.

### U4 — guishell integration

Files:

- Modify `src/commands/guishell/mod.rs`

Replace `MenuWindow` construction with `StartMenuWindow`, map typed selections
to existing spawn actions plus Run and Shut Down, poll Run completion, and
remove the hardcoded four-item height calculation. Preserve deferred callbacks,
active-menu registration, stale-window reconciliation, and close-before-launch
ordering.

### U5 — docs and end-to-end verification

Files:

- Modify `CLAUDE.md`
- Modify `src/commands/CLAUDE.md`
- Modify `src/window/CLAUDE.md`
- Modify this plan status

Run automated coverage, then boot both rendering paths and exercise every row,
fly-out boundary, dialog exit path, and representative Run command.

## Tests

### Automated

Add a `start_menu` test topic (or equivalently named focused topic) covering:

1. Root row order, calculated height, and the separator's shorter extent.
2. Root-only bounds versus Programs-open combined bounds.
3. Programs hover opens the fly-out; moving into the fly-out preserves it;
   moving to another root row closes it.
4. Program rows emit the correct typed action exactly once.
5. Documents, Settings, and the separator emit no action.
6. Shut Down remains the last row and has a separator immediately above it.
7. Key pixels for classic bevel, blue banner, hover fill, separator shadow and
   highlight, disabled text, and at least one rotated-label foreground pixel.
8. `TextInput` change callback, UTF-8-safe max length, Enter submit callback,
   and Esc cancel callback.
9. Run-dialog empty submission does not complete; OK/Enter returns the exact
   input; Cancel/close returns no command; cleanup clears modal state.
10. Run launch argv is exactly `[ZSH_HOST_PATH, "-c", command]`, including a
    command containing spaces and quotes as one element.

Commands:

```sh
cargo fmt --check
cargo check
cargo clippy
./test.sh start_menu
./test.sh window_theme
./test.sh userland
./test.sh
```

### Manual QEMU acceptance

Boot both paths:

```sh
AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=legacy ./build.sh
AGENTICOS_THEME=classic AGENTICOS_COMPOSITOR=retained ./build.sh
```

Verify:

- Start opens directly above the taskbar with the blue vertical `AgenticOS`
  banner and no clipping.
- Programs opens to the right; moving across the panel boundary does not close
  it; Terminal, Notepad, Painting, and Calc each launch once and close Start.
- Documents and Settings are visibly disabled and inert.
- A click outside either visible portion closes the whole Start menu.
- Run opens centered with its text field focused. Mouse OK and keyboard Enter
  both launch `notepad`; `notepad /data/example.txt` preserves its argument;
  an invalid command reports an error; Cancel, Esc, and frame close launch
  nothing.
- Shut Down appears after a separator at the bottom and opens the explicit
  unavailable message rather than exiting or hanging the VM.
- Taskbar window buttons continue to appear, resize, focus their windows, and
  remain usable after repeatedly opening/closing Start and Run.

## Risks and mitigations

- **Dynamic popup bounds can desynchronize outside-click checks.** Change the
  `StartMenuWindow` bounds in the same state transition that opens/closes the
  fly-out, invalidate immediately, and test `WindowManager::get_global_bounds`
  through manual boundary clicks.
- **Callbacks run while the window manager is locked.** Callbacks may only
  update widget-local state, a small dialog-state mutex, or queue a guishell
  action. They must never call `with_window_manager` recursively.
- **The dialog state is global.** Refuse Run when another modal is active and
  make cleanup idempotent for Cancel, frame destruction, and external window
  teardown.
- **Run can execute shell syntax by design.** This OS has no user accounts or
  privilege separation; Run has the same authority as the existing root zsh.
  Passing the text as one `-c` argv avoids accidental *kernel-side* quoting
  changes while retaining expected shell behavior.
- **Rotated TTF coverage is easy to offset incorrectly.** Use font ascent,
  glyph offsets, and advance widths rather than byte length, and protect the
  geometry with an embedded-font pixel test plus visual checks under both
  renderers.

## Out of scope / follow-ups

- Real ACPI/QEMU power-off and a complete clean-shutdown sequence.
- Contents or submenus for Documents and Settings.
- Start-menu icons, keyboard navigation, access-key underlines, search,
  recently used programs, drag/drop, and user-customizable entries.
- A general hierarchical popup-group abstraction for every `MenuWindow`.
- A full Win98 taskbar pass (clock/notification area, pressed Start state,
  quick launch, task-button styling) or desktop icon/theme changes.
- Run history, autocomplete, Browse, working-directory selection, or captured
  console output.
