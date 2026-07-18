---
title: "feat: userland common-dialogs library (file open/save, message box, color picker) + notepad adoption"
type: feat
status: complete
date: 2026-07-18
---

# feat: userland common-dialogs library (file open/save, message box, color picker) + notepad adoption

## Summary

Build the shared ring-3 dialog layer that KTD8 of the GUI-platform plan promised ("dialogs are userland, not kernel syscalls") but that Phase 3 only delivered as notepad-private code. Concretely:

- **Grow `userland/libs/gui` with three reusable widgets** — `Button`, `TextField`, `ListView` — joining `MenuBar` as the toolkit's retained widgets.
- **Add a new workspace crate `userland/libs/dialogs`** exposing four common dialogs as modal windows: `FileDialog` (Open and Save modes), `MessageBox` (Ok / OkCancel / YesNo), and `ColorPicker`.
- **Port notepad onto the library**, deleting its hand-rolled `Modal` enum, `render_modal`, `handle_modal`, and path helpers (~250 of its 846 lines), and upgrading its dialog UX in the process (scrollable file list, `..` navigation, clickable buttons, real filename field).
- **Extend `guidemo`** to exercise `ColorPicker` and `MessageBox`, preserving its role as the minimal reference client for each toolkit capability.

No kernel changes anywhere: dialogs are ordinary ring-3 windows built from the existing four GUI syscalls plus `getdents64`/`fstat`.

---

## Problem Frame

The ring-3 GUI platform plan (`2026-07-18-001`) deliberately kept the kernel ABI at four syscalls and pushed dialogs to userland (KTD8). Phase 3 implemented that policy the cheapest way possible: notepad hand-rolls its modals inline in `userland/apps/notepad/src/main.rs`:

- `enum Modal { Path {..}, Message {..} }` + `render_modal` + `handle_modal` (`main.rs:203-229`, `578-722`) — raw `Canvas` calls and hardcoded hit-test math.
- The path dialog shows at most 24 entries in two fixed columns (`entries.iter().take(24)`, column split at `x >= 274`), cannot scroll, has no `..` entry, no buttons (keyboard-only "Enter confirms - Esc cancels"), and its "text field" is append/pop only.
- The message dialog is two `draw_text` calls with Enter/Esc semantics explained in the message text itself.
- Path helpers (`parent_directory`, `directory_for_input`, `join_path`, `main.rs:775-806`) are app-private.

Meanwhile the *kernel* has a full retained-mode dialog suite (`src/window/dialogs/`: `message_box.rs`, `file_open.rs`, `file_save.rs`, built on `Button`/`Label`/`VBox`/`ListView` widgets in `src/window/windows/`) — but it is ring-0, coupled to `with_window_manager`, and unreachable from ring 3. And **no color picker exists anywhere in the system** — kernel `painting` uses four hardcoded `Color` constants.

The next wave of work makes this gap expensive: migrating `painting`, `calc`, `tasks`, and `explorer` to ring 3 (declared follow-ups of the platform plan) needs file dialogs (explorer, painting save), message boxes (all of them), and a color picker (painting). Without a shared library, each migration re-hand-rolls notepad's modals — the exact divergence disease the userland build unification just cured on the build side.

Foundations already in place, which this plan composes rather than extends:

- `gui::{Canvas, Window, MenuBar, DirEntry, list_dir, c_path}` (`userland/libs/gui/src/lib.rs`, 326 lines) and `runtime`'s syscall stubs + `GuiEvent` + allocator.
- Multi-window-per-process event routing: every `GuiEvent` carries its window handle, so an app with a dialog open just compares `event.window` (notepad's `run()` loop already does this).
- Writable overlay `/` and `/data`, `-EROFS` on `/host` — Save dialogs surface real errors today.

One stale artifact discovered while scoping: the `userland/Cargo.toml` release-profile comment still claims a "64 KiB ceiling" for output ELFs. `ZSH.ELF` is 1.5 MB and `HELLOCPP.ELF` 8.7 MB, staged and running; notepad is 29 KB. The ceiling is dead — fix the comment while touching the workspace manifest.

---

## Requirements

### R1 — Toolkit widgets in `userland/libs/gui`

- **R1.1 `Button`**: label, rect (x, y, w, h), `draw(&self, canvas, hot)` and `hit(&self, x, y) -> bool`. Visual: filled panel, border, centered 8×8 text; a `hot`/default variant using `COLOR_HIGHLIGHT`. No focus traversal — mouse + accelerator keys only.
- **R1.2 `TextField`**: single-line editable field owning `text: String` + byte-index caret. Handles printable insert, Backspace, Delete, Left/Right/Home/End (reuse notepad's `previous_boundary`/`next_boundary`, which move into `gui` as `pub` text helpers). `draw` renders box, text (clipped, scrolled so the caret is visible), and caret line. `click(x)` places the caret. No selection in v1.
- **R1.3 `ListView`**: scrollable single-column list over `Vec<String>`-like rows with `first_row` offset, `selected: Option<usize>`, row height constant. Handles: wheel scroll (`GUI_MOUSE_SCROLL`), click-to-select, Up/Down/PageUp/PageDown/Home/End selection movement, and **activation** = Enter on a selection or a second click on the already-selected row. (No double-click: `GuiEvent` carries no timestamp, and adding one is kernel ABI churn this plan avoids.) `draw` renders visible rows with the selected row inverted, plus a minimal scrollbar gutter when rows overflow.
- **R1.4** Widgets are plain structs the caller positions manually (no layout engine, matching `MenuBar`'s style). No speculative widgets beyond these three — demand comes from the four dialogs.

### R2 — The `dialogs` crate

- **R2.1** New workspace member `userland/libs/dialogs` (crate name `dialogs`), `no_std` + `alloc`, depending on `gui` + `runtime`. No manifest row (it is a library, not an app); no build.rs.
- **R2.2 Retained-mode modal API.** Each dialog is a struct that owns its own `gui::Window` (created in the constructor, destroyed on drop) and exposes:
  ```rust
  fn window_handle(&self) -> u32;
  fn handle_event(&mut self, event: &runtime::GuiEvent) -> DialogStatus<T>;
  // enum DialogStatus<T> { Pending, Done(Option<T>) }   // None = cancelled
  ```
  The dialog renders itself internally (constructor and after each handled event). The host app keeps running its own event loop, routes events whose `event.window` matches to `handle_event`, and drops the dialog on `Done`. Rationale in KTD2.
- **R2.3 `Modal` convenience enum** wrapping the four dialog types with a unified `window_handle()` and `handle_event -> DialogStatus<ModalOutcome>` (`ModalOutcome::{Path(String), Choice(MessageChoice), Color(u32)}`), so single-modal apps like notepad hold one `Option<Modal>` field and one dispatch arm.
- **R2.4** Dialogs handle their own `GUI_EVENT_RESIZE` (re-layout to new canvas size) and `GUI_EVENT_CLOSE` (→ `Done(None)`). Esc always cancels.
- **R2.5** Modality is app-side discipline, as today: the kernel does not block input to other windows, so hosts must ignore key/mouse events for their main window while a modal is open (notepad already does; document the pattern in the crate's rustdoc).

### R3 — `FileDialog`

- **R3.1** Constructors `FileDialog::open(start_dir: &str)` and `FileDialog::save(suggested_path: &str)`; result type `String` (absolute path).
- **R3.2** Layout: current-directory label, `ListView` of entries from `gui::list_dir` with a synthetic `..` first row (except at `/`), directories rendered with the `[DIR]` prefix, a `TextField` (Open: shows the selected name / typed path; Save: the filename), and `Open`/`Save` + `Cancel` buttons.
- **R3.3** Navigation: activating a directory row (Enter / second click) descends and re-lists; activating `..` ascends; activating a file row in Open mode confirms it; selecting a file in Save mode copies its name into the field. Directory listing failures show inline ("(cannot list directory)") rather than dying.
- **R3.4** Confirm resolves the final path: if the field content starts with `/` it is taken verbatim (power-user escape hatch, preserves today's type-a-full-path flow); otherwise it is joined to the current directory. Notepad's `parent_directory`/`directory_for_input`/`join_path` move into `dialogs` as `pub` path utilities.
- **R3.5** No overwrite-confirmation in Save v1 (matches current behavior; see Deferred).
- **R3.6** Window size ~560×380, min-size clamped in re-layout; entry counts beyond the visible page scroll instead of truncating (kills the 24-entry cap).

### R4 — `MessageBox`

- **R4.1** `MessageBox::new(title, text, Buttons)` with `Buttons::{Ok, OkCancel, YesNo}`; result `MessageChoice::{Ok, Cancel, Yes, No}` (cancel path also reachable via Esc/Close → `Done(None)`).
- **R4.2** Text wraps to the window width at 8 px per char (simple greedy word wrap; `\n` respected); window height derives from line count, clamped to a sane range.
- **R4.3** Keyboard: Enter activates the affirmative button, Esc the negative/cancel one. Mouse: real `Button` widgets.
- **R4.4** Convenience constructors: `MessageBox::error(text)`, `MessageBox::info(text)`, `MessageBox::confirm(title, text)` (YesNo).

### R5 — `ColorPicker`

- **R5.1** `ColorPicker::new(initial: u32)` → result `u32` in the compositor's XRGB8888 (`0x00RRGGBB`) format.
- **R5.2** UI: a fixed swatch grid (~8×5 curated palette), three R/G/B slider bars (click/drag sets the channel), a large preview swatch showing the current value alongside its `RRGGBB` hex text, and `OK`/`Cancel` buttons. Clicking a swatch loads it into the sliders; sliders allow arbitrary colors.
- **R5.3** Mouse drag on sliders uses `GUI_MOUSE_MOVE` with the button held (`GUI_MOUSE_DOWN` sets an active-slider flag, `GUI_MOUSE_UP` clears it) — first in-tree consumer of move-with-state, worth having in a reference dialog.

### R6 — Notepad adoption

- **R6.1** Delete notepad's `Modal`, `PathMode`, `render_modal`, `handle_modal`, `open_path_dialog`, `show_error` internals and the private path helpers; replace with `modal: Option<dialogs::Modal>` plus one routing arm in `run()` and small `on_modal_done` glue mapping outcomes to `load_from`/`save_to`/exit.
- **R6.2** Flows: File→Open / Ctrl-O → `FileDialog::open("/host/")`; File→Save As / Ctrl-Shift-S (and Save with no path) → `FileDialog::save(current or "/UNTITLED.TXT")`; unsaved-changes exit prompt → `MessageBox::confirm("Unsaved Changes", ...)` where Yes exits, No/Esc returns; I/O and UTF-8 errors → `MessageBox::error`.
- **R6.3** Behavior preserved: main window continues to process its own Resize/Close/Focus events while a modal is open and ignores key/mouse; Save to `/host` still surfaces `-EROFS` (now via `MessageBox::error`).
- **R6.4** Net effect on `main.rs`: editor logic untouched; the file shrinks and no longer contains any pixel-level dialog code.

### R7 — Reference-client and hygiene updates

- **R7.1** `guidemo` gains: `c` opens `ColorPicker` (result sets the background), `m` opens a `MessageBox::confirm` demo (Yes exits the app, No dismisses). This keeps every dialog type exercised by an in-tree app even though notepad has no color feature (KTD5).
- **R7.2** Fix the stale "64 KiB ceiling" comment in `userland/Cargo.toml`'s profile block (dead constraint; state the real contract: static, non-PIE, ET_EXEC, loader-acceptable).
- **R7.3** Docs: `userland/README.md` layout tree gains `libs/dialogs/` and an "adding a dialog / using dialogs" paragraph; root `CLAUDE.md` current-state sentence mentions the shared dialog library; this plan's status flips on completion.

---

## Scope Boundaries

### Outside scope

- **Kernel changes of any kind.** No new syscalls, no event timestamp field, no kernel-enforced modality, no z-order pinning of dialog windows. Dialogs are plain sibling windows.
- **Migrating `painting`/`calc`/`tasks`/`explorer`.** This plan builds the layer they will consume; the migrations stay separate follow-up plans.
- **Touching `src/window/dialogs/` (kernel-side).** It keeps serving the remaining kernel apps; it retires with the last of them, not here.
- **A layout engine, focus traversal (Tab), text selection in `TextField`, font sizes.** Widgets stay manually-positioned structs in the `MenuBar` idiom.
- **Multi-menu `MenuBar` + a notepad "Format → Text Color" feature.** Would let notepad itself exercise `ColorPicker`, but requires generalizing `MenuBar` (single hardcoded menu today) and inventing a notepad feature; guidemo covers the picker instead. Revisit when the painting migration needs multi-menu anyway.

### Deferred to follow-up

- **Overwrite confirmation in Save mode** (stat target, internal confirm phase) — natural once dialog-composition patterns settle.
- **Double-click activation** — blocked on an event timestamp; the second-click-on-selected idiom stands in.
- **Directory creation from the Save dialog** (`mkdir` exists in `runtime`; needs FAT mkdir on `/data`, currently deferred kernel-side).
- **Wiring `ColorPicker` into a real feature** (painting migration is the customer).

---

## High-Level Technical Design

### Crate layering

```
runtime            syscalls, GuiEvent, allocator          (unchanged)
  └── gui          Window, Canvas, MenuBar,
                   + Button, TextField, ListView          (R1: widgets live here)
        └── dialogs FileDialog, MessageBox, ColorPicker,
                    Modal, DialogStatus, path utils        (R2-R5: new crate)
              └── apps: notepad, guidemo                   (consumers)
```

Widgets go in `gui` because they are general toolkit pieces (a future settings window wants a `Button` without dialogs); `dialogs` stays purely compositional. Apps needing no dialogs don't pay for the crate (LTO strips it anyway, but the dependency graph stays honest).

### Why retained-mode, not a blocking `open_file() -> Option<String>` call

Each process has **one** GUI event queue shared by all its windows. A nested event loop inside the library would receive main-window events (Resize, Close, FocusChange) mid-modal and would have to either drop them (main window breaks) or call back into the app (reinventing the outer loop, inverted). Notepad's existing structure — one loop, dispatch by `event.window`, main window stays live-but-inert during a modal — is the correct shape for this ABI, so the library formalizes it: `handle_event` + `DialogStatus`, host keeps the loop. A blocking convenience wrapper can be layered later if an app with no main window (pure launcher) wants one.

### Host integration pattern (notepad after R6)

```rust
// event loop core
let event = gui::next_event()?;
if event.window == self.window.handle() {
    self.handle_main(event)                       // unchanged
} else if let Some(modal) = self.modal.as_mut() {
    if event.window == modal.window_handle() {
        if let DialogStatus::Done(outcome) = modal.handle_event(&event) {
            self.modal = None;                    // Window dropped → destroyed
            self.on_modal_done(outcome);          // load/save/exit/nothing
            self.render();
        }
    }
}
```

`on_modal_done` needs to know *why* the dialog was open (Open vs Save-As vs exit-confirm); notepad keeps a small `ModalPurpose` enum next to the `Option<Modal>` rather than the library guessing.

### FileDialog anatomy (560×380)

```
┌ Open File ──────────────────────────────┐
│ Directory: /host                        │
│ ┌─────────────────────────────────────┐ │
│ │ ..                                  │ │  ListView, scrollable,
│ │ [DIR] FONTS                         │ │  sel = inverted row
│ │ NOTEPAD.ELF                         │ │
│ │ README.TXT                          │ │
│ └─────────────────────────────────────┘ │
│ Name: [README.TXT________________]      │  TextField
│                    [ Open ] [ Cancel ]  │  Buttons
└─────────────────────────────────────────┘
```

State: `current_dir: String`, `entries: Vec<DirEntry>` (with the synthetic `..`), `list: ListView`, `name: TextField`, `mode`. Activation of a dir row mutates `current_dir` + re-lists; confirm resolves via R3.4 and returns `Done(Some(path))`.

### Size and testing reality

Notepad is 29 KB today with LTO + `opt-level=z`; the widgets and three dialogs are a few KB of monomorphized code — no size concern (the 64 KiB comment is stale, R7.2). There is no automated userland test harness (kernel `./test.sh` tests ring-0 code; userland verification is manual per platform-plan precedent), so correctness rides on: pure helpers (path utils, wrap, boundary fns) being small and shared rather than duplicated, guidemo as the per-dialog smoke, and the U6 checklist.

---

## Implementation Units

### U1. Toolkit widgets in `gui`
`Button`, `TextField`, `ListView` + text-boundary helpers promoted from notepad (R1). Verify: `cargo build --release` in `userland/`; widgets exercised by U2-U4 consumers (no standalone demo).

### U2. `dialogs` crate scaffold + `MessageBox`
New crate, `DialogStatus`, `Modal` enum, `MessageBox` with wrap + buttons (R2, R4). Verify: temporary guidemo hook (`m` key) shows/dismisses it; Enter/Esc/Close/mouse all resolve correctly; main window keeps repainting on resize during the modal.

### U3. `FileDialog`
Open/Save modes, `..` navigation, scrolling list, path resolution, moved path utils (R3). Verify via notepad in U5 plus manual navigation of `/`, `/host`, `/data`, a >1-page directory, and a failing `list_dir`.

### U4. `ColorPicker`
Swatch grid, RGB sliders with drag, preview + hex, OK/Cancel (R5). Verify via guidemo `c` key: pick → background changes; drag each slider; cancel leaves background unchanged.

### U5. Notepad + guidemo adoption
Notepad modal replacement (R6), guidemo `c`/`m` bindings (R7.1). Verify: full smoke — Open from `/host`, edit, Save to `/` (then `sync` + reboot persistence), Save to `/host` shows `-EROFS` error box, dirty-close prompts Yes/No both ways, Ctrl-O/S/Shift-S, resize main window while a dialog is open.

### U6. Docs + hygiene
`userland/README.md`, root `CLAUDE.md` state sentence, stale 64 KiB comment (R7.2, R7.3), plan status. Verify: `./build.sh -n` and `./test.sh --skip-userland` still green.

Sequencing: U1 → U2 → {U3, U4 in parallel} → U5 → U6. Each unit lands buildable; notepad keeps its hand-rolled dialogs until U5 flips it atomically.

---

## Key Technical Decisions

### KTD1. Widgets in `gui`, dialogs in a new `libs/dialogs` crate
Layering: primitives + widgets (toolkit) vs. modal compositions (policy). Matches the kernel-side split (`src/window/windows/` widgets vs `src/window/dialogs/`) and keeps `gui` reusable by apps that never open a dialog.

### KTD2. Retained-mode `handle_event`/`DialogStatus` API, not blocking calls
Forced by the single per-process event queue: the host loop must keep seeing its own window's Resize/Close/Focus events during a modal. Formalizes the pattern notepad already proved. (Design section has the full argument.)

### KTD3. Dialogs are separate top-level windows, not in-canvas overlays
Matches current notepad behavior, gets kernel decorations/drag/focus for free, and costs nothing. In-canvas overlays would re-implement chrome and hit-testing inside every app window.

### KTD4. App-side modality only
The kernel has no modal concept and adding one is ABI churn. The host-ignores-input-while-modal pattern is two lines per app and already exists in notepad.

### KTD5. `ColorPicker` validated in guidemo, not notepad
Notepad has no color feature and inventing one drags in multi-menu `MenuBar` work. guidemo is explicitly the reference client for platform capabilities; the painting migration is the picker's real customer.

### KTD6. Second-click activation instead of double-click
`GuiEvent` has no timestamp and this plan takes no kernel changes. Enter-activates + click-again-activates covers the UX; a timestamp field is a compatible future ABI extension (versioned struct).

## Risks and Mitigations

### RK-1. Focus on dialog creation
If the kernel does not focus a newly created frame, keyboard input keeps flowing to the main window (which ignores it) and the dialog feels dead. Window cascade/focus behavior worked for notepad's current modals, so this should hold; U2's smoke explicitly checks type-into-fresh-dialog. If broken, that's a kernel bug to fix in `gui_win_create`'s registration path, not a library workaround.

### RK-2. Widget scope creep
TextField-with-selection, focus rings, layout containers all beckon. Mitigation: R1.4 pins the widget set to what the four dialogs consume; anything more waits for a concrete app demand (same demand-driven rule as platform-plan R2.4).

### RK-3. Notepad behavior regressions in the swap
The modal rewrite touches its event loop. Mitigation: editor code untouched; U5's checklist covers every current flow including the `-EROFS` path and dirty-exit both ways; the old behavior is fully specified by today's `handle_modal` for comparison.

### RK-4. Slider drag exposes event-queue coalescing artifacts
Mouse-move coalescing (kernel queue) may drop intermediate positions during fast drags. Sliders derive value from absolute cursor x, not deltas, so coalescing only reduces intermediate repaints — correctness unaffected.

## System-Wide Impact

- **Userland workspace**: +1 crate (`libs/dialogs`), `gui` grows ~3 widgets; all future ring-3 GUI apps get dialogs for free — the migration cost of `painting`/`explorer` drops accordingly.
- **No kernel surface change**: ABI stays at syscalls 5001-5004; no new tests in `src/tests/`.
- **Notepad** shrinks by its entire inline dialog layer while gaining scrolling/`..`/buttons — first proof that the platform's "second app is cheap" promise extends to dialog UX.
- **guidemo** grows two keybindings but stays a single-file reference.
- **Docs**: `userland/README.md`, root `CLAUDE.md` current-state line, one stale-comment fix.

## Open Questions

- Should `Modal` (the convenience enum, R2.3) live in `dialogs` or be app-local? Planned: in `dialogs`, since every single-modal host wants the same four-way wrapper; revisit if apps need heterogeneous extra modals.
- `TextField` caret movement set (R1.2): Left/Right/Home/End planned; is Delete-at-caret worth it in v1? (Trivial either way — decide in U1.)
- Curated swatch palette contents (R5.2): pick ~40 sensible colors during U4; consider mirroring the kernel `Color` constants for continuity.
- Whether `FileDialog::open` should filter/deprioritize non-UTF-8-openable files — planned no (notepad reports cleanly via `MessageBox::error`).

## Origin

Requested 2026-07-18: "implement a shared library which exposes common dialogs (open, save, color picker, messagebox) and update notepad to use them." Scoped against the completed ring-3 GUI platform plan (`2026-07-18-001`), whose KTD8 established dialogs-as-userland and whose Phase 3 shipped notepad with app-private modals. Exploration confirmed: no color picker exists anywhere in the tree; kernel dialogs (`src/window/dialogs/`) are ring-0-only; notepad + guidemo are the only ring-3 GUI clients today.
