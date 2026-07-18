---
title: "feat: mature ring-3 GUI controls with interactive scrolling and reusable text editing"
type: feat
status: implemented
date: 2026-07-18
---

# feat: mature ring-3 GUI controls with interactive scrolling and reusable text editing

## Implementation outcome

Implemented on 2026-07-18. The ring-3 toolkit now exposes typed control input,
shared scroll/text models, interactive horizontal and vertical scrollbars, a
reusable multiline `TextArea`, and a reusable `Slider`. Buttons and text fields
have stateful typed interaction while retaining compatibility methods.

Notepad now uses the shared `TextArea` with independent automatic scrollbars.
Toolkit lists, Task Manager, File Manager, and the common file dialog use the
shared scrollbar behavior; ColorPicker uses the shared sliders; GUIDEMO
exercises the new controls.

Automated verification completed with `./userland/test-gui-core.sh`, the full
userland release check, kernel checks with and without the `test` feature, and
`./build.sh -n`. Interactive Classic/Aero QEMU smoke testing remains a manual
acceptance activity.

## Summary

Level up `userland/libs/gui` from a useful collection of drawing helpers into a
small, coherent control library while preserving its retained, manually
positioned architecture. The first tranche focuses on controls with real
consumers and on the largest current usability gap: scrollbars are passive
painted gutters rather than controls, and Notepad's multiline editor is private
application code with wheel-only vertical scrolling.

This plan delivers:

- typed control input and shared geometry/response conventions;
- stateful, release-to-activate buttons and selection-capable text fields;
- reusable horizontal and vertical `Scrollbar` controls with
  `Never` / `Auto` / `Always` policies, arrows, page tracks, draggable thumbs,
  wheel support, clamping, and theme-aware rendering;
- a reusable `TextArea` with caret/selection editing and optional vertical and
  horizontal scrollbars;
- migrations of Notepad, the toolkit list controls, File Manager/common file
  dialog browser surfaces, and ColorPicker's sliders;
- host-runnable tests for pure control models plus an expanded GUIDEMO control
  gallery and a two-theme QEMU acceptance matrix.

This is deliberately a control-maturity plan, not a new widget tree or layout
engine. Existing apps continue to own layout, focus, event routing, and the
window event loop.

---

## Problem frame

The ring-3 toolkit has become the shared UI foundation for Notepad, File
Manager, Control Center, Task Manager, Calc, GUIDEMO, and the common dialogs,
but its control behavior is inconsistent:

1. `userland/libs/gui/src/lib.rs` is a 1,575-line module containing canvas,
   window, event, filesystem, and control code. Adding richer control state in
   place will make the public surface harder to understand and test.
2. `ListView` and `ColumnListView` each implement their own row offset, clamp,
   wheel, and six-pixel scrollbar-gutter math. Their gutters are visual only:
   the thumb cannot be dragged, the track cannot be clicked, and the gutter is
   still treated as row hit area.
3. `userland/apps/notepad/src/main.rs` owns a separate 200-line `Editor` with a
   byte-index cursor, selection, vertical `first_line`, rendering, and text
   coordinate helpers. It has no visible scrollbar and no horizontal viewport,
   so long lines are inaccessible after they leave the right edge.
4. File Manager and `FileDialog` each own another `scroll: usize` and parallel
   details/grid wheel, page, selection-visibility, and clamp logic. Neither
   surface exposes a draggable scrollbar.
5. `Button` only paints a caller-selected state and hit-tests a point. Apps
   decide independently whether mouse-down or mouse-up activates it and must
   track hover/pressed state themselves. Disabled is a paint state, not an
   interaction guarantee.
6. `TextField` has caret editing and private horizontal viewport state but no
   selection or mouse drag. Notepad independently implements the missing
   selection model.
7. ColorPicker hand-builds three sliders, while the toolkit has no `Slider`.
8. Apps repeatedly decode raw `GuiEvent.payload` indexes. In particular, wheel
   deltas are signed values transported in `u32`, and mouse button/modifier bits
   share `payload[2]`. That is easy to get subtly wrong in every consumer.
9. There is no automated toolkit test harness. Existing verification is the
   userland release build plus manual QEMU use.

The kernel-side widget library already demonstrates the useful separation:
`ScrollView` owns clamped viewport behavior and `TextEditor` owns text editing.
Ring 3 should adopt the separation and interaction semantics, but not copy the
kernel window-tree implementation: ring-3 controls paint into one app-owned
`Canvas` and receive app-routed events.

## Product experience

### Notepad

- An empty or short document uses the entire editor well; `Auto` scrollbars do
  not reserve empty gutters.
- When lines exceed the viewport vertically, a themed vertical scrollbar
  appears. When an unwrapped line exceeds it horizontally, a horizontal bar
  appears. If one bar creates overflow on the other axis, both appear and the
  bottom-right corner is filled correctly.
- The mouse wheel, arrow buttons, track clicks, and thumb dragging all move the
  same clamped viewport. Page Up/Down and caret movement keep the caret visible.
- Dragging in text extends the selection. Dragging outside the text viewport
  continues selection and auto-scrolls at a bounded rate; dragging a scrollbar
  owns the gesture until mouse-up and never changes the text selection.
- Resizing recomputes the viewport, preserves the nearest valid scroll offset,
  and removes bars immediately when content fits.
- The status strip shows `Ln N, Col M` alongside path/modified state, proving
  that Notepad consumes the reusable editor model rather than retaining its own
  cursor bookkeeping.

### Lists and browser surfaces

- `ListView`, `ColumnListView`, Task Manager tables, File Manager, and the
  common FileDialog use the same scrollbar interaction and visual language.
- Row/tile hit-testing stops at the content viewport; clicking a scrollbar
  never selects or activates an item underneath it.
- Keyboard selection and refreshes still keep the selected item visible.
- Details and grid views may calculate different page sizes, but they feed one
  shared scroll model rather than duplicating clamp/offset arithmetic.

### General controls

- Buttons activate on left-button release inside the same enabled control that
  received the press. Moving away cancels the hot visual; moving back before
  release restores it.
- Text fields support click-drag selection, Shift+arrow extension, Home/End,
  and Ctrl+A while preserving automatic horizontal caret visibility.
- ColorPicker uses a library slider with drag and keyboard behavior.
- Classic and Aero change geometry only where necessary for their visual
  treatment; control content/layout does not jump when the theme changes.

## Requirements

### R1 — Modular, typed control foundation

- **R1.1.** Split controls out of `gui/src/lib.rs` into focused modules while
  preserving root re-exports such as `gui::Button`, `gui::TextField`, and
  `gui::ListView`. `Canvas`, `Window`, event-loop helpers, directory helpers,
  and legacy color constants remain source-compatible.
- **R1.2.** Add a small shared `Rect` type with containment/inset/intersection
  helpers. Controls stop carrying bespoke point-in-four-fields expressions.
- **R1.3.** Decode `runtime::GuiEvent` into typed `KeyInput` and `PointerInput`
  values. Decoding covers pressed/released, key code, character, Shift/Ctrl/Alt,
  pointer buttons, move/down/up, signed `delta_x`/`delta_y`, and timestamps.
  Malformed/non-input events return `None`; controls never index raw payloads.
- **R1.4.** Establish a small response contract containing at least `consumed`
  and `repaint`, with control-specific actions (`Activated`, `Changed`,
  `SelectionChanged`, and so on). Do not force heterogeneous controls behind a
  single trait or erase their action types.
- **R1.5.** Focus remains app-owned. Pointer capture is control-local state:
  after a thumb, slider, button, or text-selection drag begins, that control
  consumes subsequent move/up inputs until release/cancel.
- **R1.6.** Old convenience entry points (`hit`, `click`, `scroll`, current
  constructors) remain as thin compatibility wrappers where doing so is not
  ambiguous. All in-tree clients migrate to typed input in this plan.

### R2 — Shared, interactive scrolling

- **R2.1.** Add a pure `ScrollState`/axis model with content extent, viewport
  extent, offset, line step, and page step. Every mutation clamps to
  `0..=content.saturating_sub(viewport)`; zero-sized and tiny viewports never
  divide by zero or produce negative geometry.
- **R2.2.** Add `Axis::{Horizontal, Vertical}` and
  `ScrollbarPolicy::{Never, Auto, Always}`. `Auto` reserves space only on
  overflow; `Always` remains visible but disabled when there is no range;
  `Never` exposes wheel/programmatic scrolling only when the host explicitly
  elects to retain it.
- **R2.3.** `Scrollbar` supports themed decrement/increment buttons, a track,
  proportional minimum-sized thumb, hover/pressed/disabled states, thumb drag,
  line stepping, page stepping, and wheel input. Track clicks page toward the
  click and do not teleport the thumb.
- **R2.4.** A two-axis viewport layout helper resolves bar interdependence to a
  stable result: adding a vertical bar can cause horizontal overflow and vice
  versa. It returns the content viewport, both optional bar bounds, and the
  corner bounds from one calculation used by draw and hit-test.
- **R2.5.** Programmatic `ensure_visible(start, end)` minimally changes offset
  to reveal a row, tile, caret, or selection edge. Oversized targets align their
  leading edge.
- **R2.6.** Theme palettes expose scrollbar track, thumb, arrow, hover,
  pressed, disabled, and corner colors through drawing helpers. Classic uses
  bevels consistent with existing buttons; Aero uses its existing neutral
  track/thumb palette and blue hover feedback. Consumers do not paint raw
  scrollbar colors.
- **R2.7.** Scroll position is model state, not derived from thumb pixels;
  resize/theme/repaint round trips therefore do not accumulate rounding drift.

### R3 — Shared text-editing model and upgraded `TextField`

- **R3.1.** Add a no_std text-edit model shared by `TextField` and `TextArea`:
  UTF-8-boundary-safe caret, optional selection anchor, insert/delete,
  Backspace/Delete, Shift extension, Home/End, Ctrl+A, and query APIs for text,
  selection, and changed state.
- **R3.2.** Multiline mode maintains line-start byte indexes. Rendering and
  hit-testing begin at the first visible line instead of rescanning the entire
  document on every frame. A full line-index rebuild after a mutation is
  acceptable for v1; incremental reindexing is a measured follow-up.
- **R3.3.** `TextField` adopts the shared model without changing its visual
  dimensions or basic constructor. It adds mouse selection, keyboard selection,
  Ctrl+A, and consistent typed change/repaint responses.
- **R3.4.** Password masking, validation, IME/composition, clipboard, undo/redo,
  and Unicode grapheme/word segmentation are not silently approximated. The
  existing Unicode-scalar (`char`) boundary contract remains explicit.

### R4 — Reusable multiline `TextArea`

- **R4.1.** Add a public `TextArea` owning text-edit state, content metrics,
  caret, selection, viewport, and optional scrollbars. The default is editable,
  unwrapped text with vertical `Auto` and horizontal `Auto`; callers can set
  either policy independently and can opt into read-only mode.
- **R4.2.** Support insertion, newline, configurable spaces-per-Tab, selection
  deletion, all arrow directions, Home/End, Page Up/Down, Ctrl+Home/End, and
  Ctrl+A. Vertical movement preserves a preferred visual column across short
  lines until horizontal movement/editing changes it.
- **R4.3.** Keyboard edits, navigation, click placement, drag selection,
  viewport-edge auto-scroll, wheel input, scrollbar interaction, resize, and
  `set_text` all converge through one caret/selection/scroll state machine.
- **R4.4.** `draw` clips text, selection, and caret to the computed content
  viewport and never paints beneath scrollbar gutters. Only visible lines and
  visible character columns are submitted to `Canvas`.
- **R4.5.** Expose `text`, `set_text`, `caret`, `selection`, `line_col`,
  `is_modified`, `set_modified`, `set_bounds`, `handle_input`, and `draw` APIs.
  Apps can own persistence/dirty prompts without reaching into scroll fields.
- **R4.6.** Word wrapping is not part of v1. The options type reserves an
  explicit wrap mode so a later wrapped layout cannot conflict with horizontal
  scrollbar semantics.

### R5 — Existing control behavior upgrades

- **R5.1.** `Button` owns enabled/hot/pressed interaction state, activates on
  release, cancels cleanly, supports Enter/Space when focused, and uses
  `ButtonState` only as its rendered result. Existing explicitly styled Calc
  keypad buttons remain custom because their color semantics are app-specific.
- **R5.2.** `ListView` and `ColumnListView` use the shared vertical scrolling
  model and interactive `Scrollbar`; remove their duplicate gutter/thumb math.
  Their existing selection/action enums and key-stable Task Manager selection
  remain intact.
- **R5.3.** `ColumnListView` completes the theme migration: field, header,
  divider, selection, text, and scrollbar colors come from `gui::theme`, not
  legacy fixed constants.
- **R5.4.** Add a horizontal `Slider` with min/max/value/step, pointer drag,
  click-to-step, arrows/Home/End, enabled state, and theme-aware rendering.
  Migrate the three ColorPicker channels to it without changing RGB behavior.
- **R5.5.** `TabBar` gains typed pointer input and Left/Right/Home/End keyboard
  selection while preserving its current construction and active-index model.
- **R5.6.** Menu command modeling, multi-menu bars, combo boxes, check boxes,
  radio buttons, tree views, and general layout/focus traversal are follow-up
  tranches, not placeholder controls in this one.

### R6 — Consumer migrations

- **R6.1.** Replace Notepad's private `Editor`, `line_col_at`, and
  `index_for_line_col` with `gui::TextArea`. Notepad retains file I/O, menus,
  dialogs, path, and dirty-close policy; text editing and scrolling move wholly
  into the library.
- **R6.2.** Notepad uses vertical/horizontal `Auto`, makes both long documents
  and long lines reachable, updates `Ln/Col`, and marks dirty only for a
  `TextArea` content-change action—not navigation or scrolling.
- **R6.3.** GUIDEMO becomes the control reference page: Button, TextField,
  TextArea with enough content to show both bars, ListView, TabBar, Slider, and
  toggles for each scrollbar policy. It demonstrates input routing, not just
  static painting.
- **R6.4.** Task Manager routes typed pointer/key inputs into
  `ColumnListView`; app code no longer manually decodes wheel payloads.
- **R6.5.** File Manager and `FileDialog` retain their specialized details/grid
  renderers but replace duplicated `scroll` clamp/wheel/page math with the
  shared scroll model and scrollbar. Shared presentation/geometry belongs in
  `gui::file_ui`; selection and filesystem policy stay with each consumer.
- **R6.6.** Dialog buttons migrate to the release-to-activate Button contract.
  Modal input suppression and result types do not change.

### R7 — Testing, documentation, and acceptance

- **R7.1.** Put pure geometry, scroll, and text-edit models in a dependency-free
  `userland/libs/gui-core` (`#![no_std]`) crate consumed and re-exported by
  `gui`. This avoids pulling the x86 syscall runtime into host tests.
- **R7.2.** Add host tests for clamp/extents, two-axis overflow resolution,
  proportional thumb geometry, drag mapping, page/line steps, tiny bounds,
  UTF-8 boundary safety, line indexing, selection deletion, preferred-column
  movement, and caret visibility.
- **R7.3.** Add a repository script that runs `gui-core` tests on the host
  outside the repo's forced `x86_64-unknown-none` Cargo configuration, using the
  pinned toolchain from `rust-toolchain.toml`. The command is documented next to
  `./build.sh -n` and becomes the fast control-logic gate.
- **R7.4.** The normal release userland workspace build remains the compile/link
  gate for actual Canvas/theme/runtime adapters.
- **R7.5.** Update `userland/README.md` with the control catalog, typed input
  pattern, scrollbar policy behavior, `TextField` versus `TextArea`, and
  GUIDEMO reference instructions. Update root `CLAUDE.md` only after the
  implementation is complete.

## Scope boundaries

### Included

- Ring-3 `gui` and `dialogs` libraries and their current in-tree consumers.
- Pure model extraction needed for meaningful host tests.
- Visual parity under both Classic and Aero, including live theme changes.
- Horizontal and vertical scrolling inside a single app-owned pixel canvas.

### Deferred

- Kernel widget rewrites or a shared kernel/ring-3 widget crate. The render and
  ownership models are intentionally different.
- New GUI syscalls, server-side control widgets, shared-memory surfaces, damage
  rectangles, or compositor changes.
- A widget tree, automatic layout engine, global focus manager, tab-order
  traversal, command/action registry, or accessibility tree.
- Word wrapping, rich text, syntax highlighting, multiple carets, clipboard,
  undo/redo, IME, bidi layout, Unicode grapheme/word segmentation, and very
  large-file paging.
- Touch kinetic scrolling, animation, overlay/autohide scrollbars, and
  OS-level pointer capture.
- Speculative controls with no current consumer. Checkbox/radio/combo/tree
  controls should arrive with the app that needs and validates them.

## High-level technical design

### Dependency shape

```text
userland/libs/gui-core                 no_std, no runtime, host-testable
  geometry + typed model inputs
  scroll state/geometry
  text edit + line index
          |
          v
userland/libs/gui                      no_std, depends on runtime + gui-core
  Canvas / Window / ABI decoding
  theme drawing
  Button / TextField / TextArea / Scrollbar / Slider / lists / tabs
          |
          +---------------------+
          v                     v
userland/libs/dialogs       userland/apps/*
```

`gui-core` owns no colors, pixels, syscalls, windows, or filesystem calls. This
keeps behavioral tests honest and prevents a second UI library from forming:
applications continue to depend on `gui`, which re-exports the useful model
types.

### Input and response flow

```text
runtime::GuiEvent
  -> gui::input::decode(event)
  -> KeyInput | PointerInput
  -> focused control and/or hit control.handle_input(...)
  -> ControlResponse { consumed, repaint, action }
  -> app mutates domain state only for the typed action
  -> app presents once when repaint/domain changes require it
```

Theme, resize, close, focus, and settings events remain app/window events. A
control never destroys a window or calls `present` itself.

### Scroll geometry and gesture ownership

The host supplies content extents and outer bounds. One layout calculation
produces the content viewport and bars. Draw and input use that same immutable
geometry for the frame, avoiding the current mismatch where a visual gutter is
still selectable content.

On left down, the ordered hit regions are thumb, decrement button, increment
button, track-before-thumb, track-after-thumb, then content. Thumb/slider/text
drag records gesture ownership. Mouse move/up is routed to the owner even when
the pointer leaves its bounds. Resize, content replacement, focus loss, or
explicit cancel terminates capture safely.

### Directional API sketch

This is review guidance, not a signature lock:

```rust
let mut editor = TextArea::new(
    Rect::new(0, MenuBar::HEIGHT as i32, width, height),
    TextAreaOptions::default()
        .vertical_scrollbar(ScrollbarPolicy::Auto)
        .horizontal_scrollbar(ScrollbarPolicy::Auto),
);

if let Some(input) = gui::decode_control_input(&event) {
    let response = editor.handle_input(input, focused);
    if matches!(response.action, Some(TextAreaAction::Changed)) {
        dirty = true;
    }
    if response.repaint {
        render();
    }
}

editor.draw(window.canvas_mut(), focused);
```

The implementation may separate key and pointer methods if that gives clearer
borrows, but raw payload arrays must not leak back into consumer control code.

## Implementation units

### U1 — Pure control models and host-test gate

Create `gui-core`, shared rectangle/input model types, scroll state/geometry,
text edit/line index, focused unit tests, and the host test script. Add it to the
userland workspace and make `gui` depend on/re-export it. This unit has no
visual or app behavior change and gates the rest of the plan.

### U2 — Toolkit modularization and typed input

Split controls into modules behind compatible root re-exports; add runtime
event decoding and response conventions. Migrate one small reference path in
GUIDEMO to validate the API before applying it broadly.

### U3 — Button, TextField, TabBar, and Slider maturity

Add stateful pointer/keyboard behavior, connect theme drawing, migrate dialog
buttons and text fields, and replace ColorPicker's private slider code. Preserve
Calc's intentionally custom keypad painting.

### U4 — Scrollbar rendering and list adoption

Implement themed `Scrollbar`, two-axis viewport layout, and pointer gestures.
Migrate `ListView` and `ColumnListView`, then update GUIDEMO and Task Manager.
This is the first full vertical-scroll acceptance gate.

### U5 — TextArea and Notepad migration

Build `TextArea` over the shared editor/scroll models. Delete Notepad's private
editor and coordinate helpers; add Auto bars and line/column status. Verify
large multiline files, long lines, selection, resize, save, dirty prompts, and
live theme changes.

### U6 — File browser scroll convergence

Move shared details/grid scroll presentation into `gui::file_ui`; migrate File
Manager and FileDialog together so their intentionally parallel browser UX
cannot drift. Verify selection stability, details/grid switches, directory
refreshes, and dialogs resized near their minimum dimensions.

### U7 — Documentation and full acceptance

Finish GUIDEMO's control gallery, update docs, run model tests and all build
gates, complete the Classic/Aero manual matrix, and record implementation
outcome/status in this plan.

## Expected file map

### New

```text
userland/libs/gui-core/Cargo.toml
userland/libs/gui-core/src/lib.rs
userland/libs/gui-core/src/geometry.rs
userland/libs/gui-core/src/scroll.rs
userland/libs/gui-core/src/text_edit.rs
userland/libs/gui/src/input.rs
userland/libs/gui/src/scrollbar.rs
userland/libs/gui/src/text_area.rs
userland/libs/gui/src/slider.rs
userland/test-gui-core.sh
```

Existing `Button`, `TextField`, list, and tab code may move to their own module
files during U2; retain root re-exports so this is not visible to callers.

### Modified

```text
userland/Cargo.toml
userland/libs/gui/Cargo.toml
userland/libs/gui/src/lib.rs
userland/libs/gui/src/theme.rs
userland/libs/gui/src/file_ui.rs
userland/libs/dialogs/src/{message_box,color_picker,file_dialog}.rs
userland/apps/{guidemo,notepad,taskmgr,fileman}/src/main.rs
userland/README.md
CLAUDE.md                         implementation outcome only
```

No kernel, syscall ABI, compositor, or window-manager files should change.

## Verification

### Automated

1. `./userland/test-gui-core.sh`
2. `cargo check --manifest-path userland/Cargo.toml --release`
3. `./build.sh -n`
4. `cargo check`
5. `cargo check --features test`
6. Focused kernel GUI ABI/event tests only if consumer migrations expose an
   existing regression; this plan should add no kernel behavior.

The model tests include boundary tables for no overflow, exact fit, one-unit
overflow, both-axis feedback, extreme content lengths, minimum thumbs, drag at
both track ends, viewport growth/shrink, empty text, trailing newline, multi-byte
UTF-8, selection in both directions, and preferred-column movement.

### Manual QEMU acceptance

Run under both Classic and Aero, then switch theme live while apps remain open:

1. GUIDEMO exercises Button press-cancel-release, disabled state, TextField
   selection, Slider mouse/keyboard, list wheel/track/thumb, and all scrollbar
   policies.
2. Notepad opens short text (no bars), a many-line file (vertical), one long
   line (horizontal), and a document needing both. Exercise wheel, buttons,
   track, thumb, Page keys, caret visibility, drag selection, resize, save, and
   unsaved-close behavior.
3. Task Manager scrolls long process/socket lists without losing key-stable
   selection during refresh.
4. File Manager and FileDialog scroll in details and grid views; scrollbar
   clicks never select entries; switching view and navigating directories
   clamps correctly.
5. ColorPicker changes each channel through click, drag, arrows, Home, and End;
   OK/Cancel semantics remain unchanged.
6. Repeated drags followed by focus loss, modal open/close, and window resize do
   not leave any control visually pressed or captured.

## Key technical decisions

### KTD1 — Mature the existing retained model

Apps already compose controls successfully by owning structs, positioning them,
drawing into a `Canvas`, and routing events. A widget tree/layout rewrite would
delay the concrete usability gains and entangle every app. This plan makes the
current model consistent and reusable first.

### KTD2 — One scroll model, composable visual controls

Lists, text, and browser grids have different content units, but share extent,
viewport, offset, clamp, page, ensure-visible, and thumb mapping. Those rules
live once. Each consumer remains responsible for mapping its rows/columns/pixels
onto that state.

### KTD3 — Text model shared by single-line and multiline controls

Selection/caret mutation must not fork again. `TextField` and `TextArea` share
the byte-boundary-safe edit state; multiline indexing/navigation is layered on
top. File I/O and dirty policy remain application concerns.

### KTD4 — Host-test pure behavior, QEMU-test integration

The syscall-bound `gui` crate is awkward to execute on the macOS host because
the repo targets `x86_64-unknown-none` and `runtime` contains x86 syscall asm.
A small dependency-free model crate gives fast deterministic tests without fake
syscalls. Canvas/theme/runtime integration remains covered by release builds and
the persistent GUIDEMO reference client.

### KTD5 — No kernel ABI work

GUI ABI v1 already carries mouse move/down/up, signed wheel deltas, buttons,
modifiers, timestamps, keys, focus, resize, close, and theme notifications.
Control-local capture is sufficient because the app receives events for its
window. This plan decodes and uses the existing information.

## Risks and mitigations

### RK-1 — A broad API cleanup churns every app at once

Keep root re-exports and constructor compatibility, validate the typed API in
GUIDEMO first, then migrate by control family. Each implementation unit ends in
a complete userland release build.

### RK-2 — Two scrollbar axes oscillate during layout

Use a bounded stable calculation over the four possible bar-visibility states,
with table tests for exact-fit and cross-axis overflow. Draw and hit-test share
the returned geometry.

### RK-3 — Pointer capture sticks after focus/modal/resize changes

Every stateful control supports explicit cancel; apps broadcast cancel on focus
loss and before modal takeover. Resize/content replacement also ends invalid
drags. GUIDEMO exposes captured/pressed state visually for manual verification.

### RK-4 — TextArea becomes a full editor project

Hold the line at plain UTF-8-scalar text, cached line starts, selection, caret,
and scrolling. Word wrap, clipboard, undo, grapheme segmentation, and large-file
paging are explicit follow-ups.

### RK-5 — Browser migration changes selection semantics

Migrate File Manager and FileDialog together, but change only their offset and
bar interaction. Keep existing key-based selection, double-click timing,
filtering, operation policy, and details/grid item geometry. Use the current
manual file-dialog acceptance matrix as a regression gate.

### RK-6 — Classic/Aero scrollbar geometry drifts

Keep interaction geometry theme-invariant and theme only the pixels inside it.
Central theme draw helpers and the GUIDEMO two-theme pass prevent per-consumer
variants.

## Follow-up control roadmap

After this plan lands, add controls only with concrete consumers:

- command/action model plus multi-menu `MenuBar` and keyboard accelerators;
- checkbox/radio/combo controls with the first settings pages that need them;
- layout containers and focus traversal when manual positioning becomes the
  dominant app cost;
- clipboard and undo/redo after a versioned GUI clipboard contract exists;
- wrapped text and grapheme-aware navigation as a dedicated text-layout plan;
- virtualized collections if profiling shows large directories/process tables
  are dominated by model/render work.

## Origin

Created from the request to level up the controls available to ring-3 GUI apps,
with scrollbars on multiline textboxes such as Notepad as the motivating
example. Repository exploration found passive duplicated list gutters, four
independent scroll-offset implementations, a private Notepad editor, raw event
payload decoding in consumers, custom ColorPicker sliders, and no host-runnable
control tests. The plan prioritizes shared behavior with existing consumers over
a speculative catalog of new widgets.
