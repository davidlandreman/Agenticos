---
title: "feat: desktop pointer, wait, and text-select cursor iconography"
type: feat
status: implemented
date: 2026-07-22
depth: medium
related_docs:
  - src/window/CLAUDE.md
  - src/graphics/CLAUDE.md
  - src/userland/CLAUDE.md
  - docs/window_system_design.md
  - docs/plans/2026-07-18-007-feat-userland-gui-control-maturity-and-scrollable-text-area-plan.md
---

# feat: desktop pointer, wait, and text-select cursor iconography

## Summary

Replace the current 12×12 triangle-like mouse sprite with a conventional
outlined desktop arrow and add two pointer states:

- a wait cursor for explicit application work that temporarily cannot accept
  input;
- a text-select I-beam over editable text fields and text areas.

The pointer shapes are one renderer-independent asset set used by the legacy
framebuffer overlay, retained CPU composition, and the qualified VirtIO-GPU
hardware cursor. Ring-3 applications receive a small ownership-checked
`gui_win_set_cursor` syscall because the kernel sees their content only as one
opaque `RemoteSurface` and cannot infer which client pixels are text controls.
Kernel-owned text widgets advertise the text cursor through the existing
window hit-test tree.

This plan concerns the mouse pointer. Editable controls already draw insertion
carets when focused:

- `userland/libs/gui::TextField` draws a vertical caret;
- `userland/libs/gui::TextArea` uses the shared text-edit model and draws its
  caret;
- kernel `TextInput`, `TextEditor`, `TextWindow`, and terminal widgets already
  draw focused insertion cursors.

Those caret implementations stay intact. The new `Text` pointer icon is the
missing I-beam that communicates text selection before a click.

---

## Current state and findings

### The normal pointer is an undersized filled triangle

`src/window/cursor.rs` stores a list of white pixels for one 12×12 shape and
creates its black outline by painting the four neighboring pixels. The sprite
has no distinct diagonal outline, tail, or stem, so at display scale it reads
as a triangle rather than the familiar Windows/macOS arrow.

`CursorRenderer` also assumes every cursor fits one fixed 17×17 save/restore
footprint. `bounds_at`, the legacy background buffer, retained cursor damage,
and the hardware image generator all encode that one size and hotspot.

### All renderer paths share the shape but not a shape-change contract

- Legacy saves the framebuffer under the cursor, draws it directly, then
  restores that region on the next frame.
- Retained CPU draws the cursor as the final output-surface overlay and
  recomposes the old footprint after movement.
- Strict VirGL/direct scanout creates one 64×64 VirtIO cursor resource, uploads
  the arrow only once, and subsequently issues move-only commands.

The current rendering fast paths know when the pointer position changes, but
not when its image changes at a stationary position. Adding cursor states
without changing this contract would leave stale pixels in software modes and
would never upload a second shape to the hardware cursor plane.

### Kernel hit-testing can identify kernel text widgets

`WindowManager::topmost_at` already returns the deepest visible window and its
absolute bounds. A default `Window::cursor_icon_at(local_point)` method can
therefore return `Arrow`, while kernel text widgets override it with `Text`.
Because frame chrome and content are distinct children, the title bar and
borders naturally retain the arrow.

### Ring-3 control geometry is intentionally opaque to the kernel

Every ring-3 window is a server-side `RemoteSurface`; `TextField` and
`TextArea` live inside the client pixel buffer. The kernel cannot safely
inspect those pixels or duplicate each app's widget layout. Cursor intent must
cross the GUI ABI, just like title and surface updates do.

### Scheduler blocking is not a usable busy signal

Healthy GUI applications spend most idle time in
`Ring3BlockReason::WaitingForGuiEvent`. Treating scheduler “blocked” state as
thinking would show a wait cursor whenever an application is simply waiting
for input. Busy state must be an explicit application request and must remain
visible while that application's thread is occupied.

---

## Product decisions

### PD1 — Ship three stable pointer kinds

The public v1 set is:

```rust
pub enum CursorIcon {
    Arrow = 0,
    Wait = 1,
    Text = 2,
}
```

- `Arrow` is a 16–20 px class white desktop arrow with a continuous one-pixel
  black outline, recognizable diagonal edge, and lower stem. Its hotspot is
  the top-left tip.
- `Wait` is a static outlined hourglass with visible sand at approximately
  18×18. A static icon communicates the state without creating a periodic
  compositor wakeup. An animated beachball/spinner can be added later using
  the same frame-capable sprite contract.
- `Text` is a high-contrast I-beam with top and bottom serifs. Its hotspot is
  at the center of the vertical stroke so selection begins at the expected
  character.

The assets are crisp integer-pixel sprites, not font glyphs or theme-specific
SVGs. They must look identical in all themes and rendering backends.

### PD2 — Cursor coordinates always name the hotspot

Mouse input coordinates continue to represent the actionable point. Each
sprite defines:

```rust
pub struct CursorSprite {
    pub width: u8,
    pub height: u8,
    pub hot_x: u8,
    pub hot_y: u8,
    pub pixels: &'static [CursorPixel],
}
```

Software drawing subtracts the hotspot to obtain the image origin. VirtIO
passes the same hotspot in `UpdateCursor`. Hit-testing and mouse event
coordinates therefore do not change when the icon changes.

### PD3 — Busy is explicit and window-scoped

`gui_win_set_cursor(handle, kind)` sets the requested cursor for one owned
ring-3 surface. Requests for another process's handle fail normally through
the existing ownership lookup. A new surface starts with `Arrow`, and cursor
state disappears with that surface.

The shared toolkit exposes both a low-level cursor setter and a small
busy-state helper:

```rust
window.set_cursor(CursorIcon::Text)?;
window.set_busy(true)?;
// perform bounded synchronous work
window.set_busy(false)?;
```

`Window` remembers the last hover cursor. While busy, `Wait` wins; clearing
busy restores the remembered hover cursor without requiring mouse movement.
Redundant effective-state syscalls are suppressed client-side.

Busy state is not set automatically around event waits, `select`, filesystem
access, or process scheduler transitions. Callers opt in only when the user
has initiated work and the window cannot currently accept another action.

### PD4 — Text pointer selection follows actual editable regions

Kernel widgets return `Text` from `cursor_icon_at` for their editable content.
For ring-3 controls:

- `TextField` returns a `Text` cursor hint anywhere inside its field bounds.
- `TextArea` returns `Text` inside the text viewport, but `Arrow` over its
  scrollbars.
- dialogs and applications route the hint from the control that owns the
  pointer event to their `Window`; leaving all editable controls restores
  `Arrow`.

The first move into a remote surface may be displayed as `Arrow` until the
client handles that move and submits its cursor request. The update is then
applied at the stationary pointer position. No kernel-side region
registration or mirrored widget tree is introduced.

### PD5 — Hover, not keyboard focus, selects the pointer

The I-beam appears over editable text even before the field is focused. A
focused field does not keep the I-beam after the pointer leaves it. Busy state
is the deliberate exception because it describes the whole window's current
ability to respond.

### PD6 — Shape changes are cursor-only presentation work

Changing `Arrow` to `Text` or `Wait` must not invalidate or rerasterize the
window below it:

- legacy restores the exact old saved footprint and saves/draws the new one;
- retained CPU recomposes the old footprint and overlays/presents the new
  footprint;
- direct scanout uploads the new cursor image and hotspot without composing or
  presenting the 3D scene.

Cursor-only telemetry continues to report zero window rasterization, surface
uploads, and 3D composition.

---

## Technical design

### 1. Make `cursor.rs` a typed sprite catalog

Refactor `src/window/cursor.rs` around `CursorIcon` and `CursorSprite`:

- encode deliberate black outline and white/interior pixels instead of
  manufacturing an outline from four-neighbor expansion;
- validate every sprite pixel and hotspot against its declared dimensions in
  tests;
- provide `sprite(icon)`, `bounds_at(icon, hotspot_position)`,
  `draw(icon, ...)`, and `hardware_argb_64(icon)`;
- make the legacy save buffer large enough for the maximum supported sprite,
  while saving/restoring only the active sprite's exact bounds;
- keep all dimensions below the VirtIO fixed 64×64 cursor limit.

Use a small cursor palette that supports at least transparent, black, white,
and one optional sand/accent color. Generate software `Color` and
premultiplied ARGB hardware pixels from the same pixel records so the paths
cannot drift.

The existing top-left arrow behavior remains pixel-coordinate compatible
because `Arrow` uses hotspot `(0, 0)`. The I-beam and hourglass use central
hotspots.

### 2. Track position and icon as one cursor state

Extend the compositor cursor state from position-only to:

```rust
pub struct CursorState {
    pub position: Point,
    pub icon: CursorIcon,
}
```

`WindowManager` resolves the desired icon from the deepest hovered window and
updates this state after mouse routing. It also refreshes the state immediately
when a ring-3 cursor syscall changes the currently hovered surface.

Replace the current optional “previous position” handoff with a cursor-change
record containing the previous state. Any position or icon difference enters
the cursor-only render path. Damage is the old icon's bounds plus the new
icon's bounds, clipped to screen.

Add `Window::cursor_icon_at(local_point)` with an `Arrow` default. Override it
in kernel editable widgets:

- `src/window/windows/text_input.rs`;
- `src/window/windows/text_editor.rs`;
- `src/window/windows/text.rs` / terminal content as appropriate;
- `src/window/windows/remote_surface.rs`, returning its client-requested icon.

Pointer capture continues to control event delivery only. Visual cursor
resolution uses the surface under the pointer, except that the remote surface's
explicit `Wait` remains effective everywhere inside that surface.

### 3. Support stationary image replacement in all renderers

For legacy and retained CPU, parameterize every cursor draw and bounds call by
the icon from `CursorState`. Generalize the retained cursor-only shortcut to
run for icon-only changes as well as movement.

For direct scanout, extend the VirtIO cursor API so an existing
`CursorResource` can replace its backing pixels and hotspot:

1. copy the new 64×64 ARGB image into the existing attached backing allocation;
2. issue `TRANSFER_TO_HOST_2D`;
3. issue `UPDATE_CURSOR` with the current position, resource ID, and new
   hotspot;
4. retain move-only `MOVE_CURSOR` for position changes with an unchanged icon.

The VirGL composition engine caches the uploaded `CursorIcon`. The manager
passes pixels only when that icon changes, avoiding allocation and transfer on
ordinary mouse motion. Teardown remains the existing cursor-resource teardown;
shape switches do not create/unref GPU resources.

### 4. Add the ring-3 cursor ABI

Allocate syscall **5019** as `GUI_WIN_SET_CURSOR`:

```text
gui_win_set_cursor(handle: u32, kind: u32) -> 0 | -errno
```

Kernel behavior:

- decode only `0=Arrow`, `1=Wait`, and `2=Text`; unknown values return
  `-EINVAL`;
- resolve `handle` within the caller's existing `GuiProcessState`; an unknown
  or unowned handle returns `-ENOENT`;
- update the matching `RemoteSurface` through a manager-owned method;
- if that surface is currently hovered, schedule a cursor-only frame even when
  the mouse did not move;
- do not repaint the remote surface or enqueue a GUI event.

Wire the number through `src/userland/abi.rs`, dispatch in the GUI syscall
module, mirror constants and a raw wrapper in `userland/runtime`, and expose
the typed API from `userland/libs/gui::Window`.

The GUI ABI version and `GuiEvent` layout do not change: this is an additive
syscall, not an event or structure mutation.

### 5. Add toolkit cursor hints and adopt them

Put the shared cursor enum/hint type in `gui-core` so controls can describe
pointer intent without depending on syscall code. Extend the control routing
contract with an optional cursor hint, keeping `ignored()` and `consumed()`
constructors defaulted to no hint.

Add convenience constructors/helpers so most controls require no call-site
changes. `TextField` and `TextArea` attach `Text`/`Arrow` hints based on the
pointer location described in PD4. `gui::Window` translates the shared enum to
runtime constants, applies busy precedence, and suppresses duplicate syscalls.

Adopt the hint at each shared editable-control host:

- Notepad `TextArea`;
- File Manager filter/location and rename/new-folder fields;
- Control Center search;
- common `FileDialog` location/filter/name/new-folder fields;
- desktop Run input if it uses the shared ring-3 field;
- GUI demo field and area as a focused regression fixture.

Kernel Run `TextInput` is covered by the `Window` trait override. Audit the
Links native widget bridge separately: it can call the same window cursor API
when its own hit-test reports an HTML or chrome text input, but rebuilding the
prebuilt browser is not required merely to land the shared OS/toolkit support.

Use `set_busy(true/false)` first in one bounded, user-visible operation that
already executes synchronously in a GUI event loop (for example a File Manager
directory refresh or file-dialog enumeration). Do not spread wait cursors over
operations that already return immediately or continue accepting input.

### 6. Documentation

Update:

- `src/window/CLAUDE.md` with the typed sprite set, hotspot convention,
  renderer behavior, and `Window::cursor_icon_at`;
- `src/userland/CLAUDE.md` with syscall 5019 and the rule that busy state is
  explicit rather than inferred from scheduler blocking;
- `userland/libs/gui` rustdoc with cursor hints and the busy override;
- the syscall inventory/comments that currently describe GUI syscalls only
  through 5005/5018.

---

## Implementation sequence

### Phase 1 — Sprite model and visual replacement

1. Define `CursorIcon`, sprite metadata, hotspots, palette, and the three
   sprites in `src/window/cursor.rs`.
2. Replace the fixed 17×17 assumptions in legacy drawing/save/restore with
   sprite-derived bounds.
3. Change retained CPU overlay generation and hardware ARGB generation to use
   the same sprite.
4. Land the conventional arrow as the default before adding state switching.

### Phase 2 — Cursor state and renderer transitions

1. Track icon plus position and preserve the previous state for damage.
2. Resolve kernel widget cursor intent through the window tree.
3. Generalize cursor-only rendering for stationary icon changes.
4. Add in-place VirtIO cursor image/hotspot replacement and cache the uploaded
   icon in the VirGL engine.

### Phase 3 — Ring-3 ABI and toolkit

1. Add syscall 5019 with ownership and kind validation.
2. Add runtime constants/wrapper and `gui::Window` typed cursor/busy methods.
3. Add control cursor hints for `TextField` and `TextArea`.
4. Update shared-control hosts and one explicit bounded busy operation.

### Phase 4 — Verification and documentation

1. Add sprite, hit-test, ABI, and renderer regression tests.
2. Run targeted QEMU suites, `cargo check`, and formatting.
3. Boot each renderer policy and visually inspect all three shapes and
   transitions.
4. Update subsystem context and GUI API documentation.

---

## Test plan

### Pure cursor/sprite tests

- Every sprite has nonzero dimensions, a hotspot inside its bounds, no
  out-of-bounds pixel records, and fits in 64×64.
- `bounds_at` places the declared hotspot exactly at the mouse position for all
  icons, including near negative/top-left coordinates.
- Software and hardware rasterization agree on transparent/black/white/accent
  pixels after accounting for the hardware canvas.
- The new arrow contains a continuous outline and a distinct stem; use stable
  representative-pixel assertions rather than a fragile screenshot hash.

### Window-manager/render tests

Extend `src/tests/window_manager_render.rs`:

- moving an arrow still presents old and new footprints without window
  rasterization;
- changing `Arrow → Text` at a stationary position restores/recomposes the old
  bounds and presents the new bounds;
- changing between icons of different size at a screen edge is clipped and
  leaves no stale pixels;
- a kernel text input resolves `Text`, while its enclosing frame chrome and
  desktop resolve `Arrow`;
- direct-scanout cursor-only state changes report one cursor update and zero
  3D/window/surface work.

Extend VirtIO integration coverage:

- initial cursor creation uploads image and hotspot;
- a second icon updates the existing resource rather than allocating a second
  resource;
- unchanged-icon motion uses `MOVE_CURSOR`;
- icon replacement uses `UPDATE_CURSOR` with the new hotspot.

### GUI ABI tests

Extend `src/tests/gui_userland.rs`:

- owned handle accepts each valid cursor kind;
- unknown kind returns `-EINVAL`;
- unknown/unowned handle returns `-ENOENT`;
- default remote-surface cursor is `Arrow`;
- setting the hovered surface to `Wait` schedules a cursor-only update;
- destroying the window removes its cursor state with existing GUI cleanup.

### Toolkit tests

- `TextField` hints `Text` inside and no text hint outside.
- `TextArea` hints `Text` over the viewport and `Arrow` over both scrollbars.
- busy mode overrides a text hover cursor and clearing busy restores it.
- applying the same effective cursor twice makes only one runtime request
  through a test seam/mock.

### Commands and manual smoke

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland window_render gui_userland virgl
```

Then boot `legacy`, `retained`, and strict `gpu` renderer policies and verify:

1. the default arrow is recognizable at normal display scale and its click
   hotspot remains at the tip;
2. the I-beam appears over Notepad text, dialog fields, File Manager fields,
   and kernel Run/terminal text, but not over scrollbars or frame chrome;
3. the wait icon appears immediately during the chosen synchronous operation
   and returns to the correct arrow/I-beam state afterward;
4. switching shape without moving does not leave cursor fragments;
5. rapid movement and shape transitions do not trail, flicker, or repaint
   application surfaces.

Use the actual test module names reported by `./test.sh -l` if they differ from
the descriptive filters above.

---

## Acceptance criteria

- The default mouse pointer reads as a conventional Windows/macOS-style arrow,
  not a triangle.
- `Arrow`, static `Wait`, and `Text` have explicit, tested hotspots and render
  from one canonical sprite source on legacy, retained CPU, and hardware cursor
  paths.
- Editable kernel and shared ring-3 text controls show an I-beam on hover while
  preserving their existing focused insertion carets.
- A ring-3 application can explicitly show and clear a window-scoped wait
  cursor; idle GUI-event blocking never triggers it.
- Icon changes take effect without mouse motion and create only cursor
  presentation work—no window rerasterization or 3D scene work.
- Invalid or unowned ring-3 cursor requests fail without changing another
  window's state.
- All targeted tests, `cargo check`, formatting, and three-renderer manual
  smokes pass.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| New larger sprite leaves stale pixels | Damage old and new icon-specific bounds; add stationary and edge-clipping tests. |
| Hardware hotspot differs from software semantics | Treat mouse coordinates as the hotspot everywhere and assert protocol fields in tests. |
| Shape changes allocate GPU cursor resources repeatedly | Update the existing attached resource in place and cache the uploaded icon. |
| GUI apps show wait while merely idle | Expose explicit busy state only; never infer it from scheduler block reasons. |
| I-beam gets stuck after leaving a text field | Make every pointer routing pass resolve an effective hint with `Arrow` fallback; test exit and busy restoration. |
| Ring-3 app changes another app's cursor | Reuse caller-owned handle lookup and return `-ENOENT` for foreign handles. |
| Control-response API creates broad churn | Add default constructors/helpers first, then migrate only editable-control hosts in this unit. |

---

## Out of scope

- Animated wait frames, beachball timing, or a compositor animation clock.
- Resize, hand/link, drag/drop, forbidden, precision, or custom application
  cursor images. The typed enum and sprite catalog leave room for these later.
- Cursor scaling, accessibility size settings, or HiDPI asset variants.
- Replacing existing insertion-caret drawing or adding text selection features.
- Inferring application responsiveness from scheduler state or elapsed time.
- Rebuilding the prebuilt Links browser solely for cursor adoption.
