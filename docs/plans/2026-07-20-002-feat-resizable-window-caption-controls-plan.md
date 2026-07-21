---
title: "feat: resizable window minimize/maximize caption controls"
type: feat
status: implemented
date: 2026-07-20
depth: medium
related_docs:
  - src/window/CLAUDE.md
  - src/commands/CLAUDE.md
  - src/userland/CLAUDE.md
  - docs/window_system_design.md
  - docs/plans/2026-07-18-001-feat-win98-classic-theme-plan.md
  - docs/plans/2026-07-18-007-feat-futurism-theme-plan.md
---

# feat: resizable window minimize/maximize caption controls

## Summary

Add real minimize and maximize/restore behavior to `FrameWindow` and paint the
corresponding caption buttons in Classic, Aero, and Futurism title bars.
Resizable frames show three functional controls — minimize, maximize/restore,
and close. Fixed-size frames remain close-only and cannot be resized from their
borders.

Minimize hides the complete frame subtree but keeps its taskbar button alive;
clicking that taskbar button restores and focuses it. Maximize fills the desktop
work area above the taskbar, sends the existing client resize event through the
normal child-reflow path, and remembers the exact normal bounds for restore.
Minimizing a maximized frame preserves its maximized placement so taskbar
restore returns it maximized.

This is window-management behavior, not an inert theme embellishment. One
caption-button layout is shared by all three painters and manager hit-testing,
and one manager transition path owns placement, visibility, focus, child
reflow, and compositor damage.

---

## Current state and findings

### Frames have close-only chrome even though resize already works

- `src/window/theme/mod.rs::FrameMetrics` describes one caption button and
  `close_button_rect()` computes its position. `FrameChrome` exposes only that
  rectangle.
- `classic.rs`, `aero.rs`, and `futurism.rs` each paint only close. The title
  clipping logic also assumes close is the leftmost caption control.
- `src/window/types.rs::HitTestResult` already reserves `MinimizeButton` and
  `MaximizeButton`, but both variants are dead code.
- `WindowManager::start_drag_if_on_title_bar` independently computes the close
  rectangle and dispatches close immediately. There is no frame placement
  state or minimize/maximize transition.
- Border resize is currently unconditional for every decorated top-level
  frame. There is no `FrameWindow` resizable capability despite the ring-3
  create syscall already reserving a flags argument.

### Visibility is sufficient for minimized rendering, but focus is not

`WindowBase::visible` already causes render walks and hit-testing to skip a
whole hidden frame subtree. That is the correct primitive for minimize. Merely
setting it false is insufficient, however: `focus_stack` can still point at a
hidden descendant, so keyboard events could continue going to the minimized
application. Minimize must remove the subtree from focus, clear active chrome,
and select the next topmost visible frame.

### The taskbar already retains the identity needed for restore

`GUIShellState::window_buttons` maps every task button to a frame ID, and
`sync_taskbar_buttons()` discovers frames by title without filtering on
visibility. A minimized frame can therefore keep its existing button with no
new taskbar model. The click action needs to change from unconditional focus to
manager-owned activation: restore first when minimized, then focus and raise.

### Maximize must use the work area, not the full screen

The taskbar is a registered top-level desktop child at the bottom of the
screen. Maximizing to raw screen dimensions would cover it. The manager can
derive a work-area rectangle from the visible taskbar's global top edge, with
the full screen as the fallback when there is no visible taskbar.

### Ring-3 compatibility requires opt-out, not opt-in, resizability

`GUI_WIN_CREATE` currently rejects every nonzero flag, while all callers pass
zero. That includes the prebuilt-managed Links browser, which makes the syscall
directly from its Rust driver. Interpreting a new `RESIZABLE` bit as required
would turn every existing or stale binary into a fixed window and would require
refreshing a large committed prebuilt for an otherwise kernel/toolkit feature.

Keep flags value zero backward-compatible and resizable. Add one
`GUI_WINDOW_FIXED_SIZE` opt-out bit; reject all unknown bits. The ordinary
toolkit `Window::new` remains resizable, while a new options constructor lets
dialogs request fixed-size behavior.

---

## Product decisions

### PD1 — Caption buttons are truthful capabilities

A resizable frame shows minimize, maximize/restore, and close. A fixed-size
frame shows close only. The manager also refuses border resize and
minimize/maximize transitions for a fixed frame, so there is no painted control
whose action cannot succeed.

The kernel Terminal and ordinary ring-3 application windows remain resizable
by default. Kernel Run/message dialogs and userland message boxes use the new
fixed-size option. The common file dialog remains resizable. Other apps keep
their current behavior unless their owner deliberately adopts fixed sizing.

### PD2 — Normal/maximized placement and minimized visibility are orthogonal

Represent frame state as two dimensions rather than one enum that loses
information:

```rust
pub enum FramePlacement {
    Normal,
    Maximized { restore_bounds: Rect },
}

pub struct FrameWindow {
    // existing fields ...
    resizable: bool,
    placement: FramePlacement,
    minimized: bool,
}
```

This supports all required transitions without reconstructing state:

| Starting state | Action | Result |
|---|---|---|
| Normal | Maximize | Save normal bounds; fill work area |
| Maximized | Maximize/restore | Restore saved normal bounds |
| Normal | Minimize | Hide; retain normal bounds |
| Maximized | Minimize | Hide; retain maximized placement and restore bounds |
| Minimized | Taskbar activation | Show in retained placement, raise, focus |

Destroying a minimized frame uses the existing recursive teardown and taskbar
sync removes its button normally.

### PD3 — Maximize uses one manager geometry transaction

`WindowManager` owns public frame actions such as `minimize_frame`,
`toggle_maximize_frame`, and `activate_frame`. A shared internal bounds helper:

1. snapshots the old outer bounds;
2. changes the frame bounds/state;
3. calls `update_children_for_resized_window` once;
4. lets `RemoteSurface::set_bounds` emit the existing client-size resize event;
5. marks the old/new union dirty (or a full repaint for visibility/z-order
   changes).

Minimize never changes bounds and never emits a resize event. Restore from
minimize therefore does not make clients reallocate a same-sized canvas.

### PD4 — A maximized frame is immovable until restored

While maximized, border resize and title-bar drag do not start. The restore
caption control remains available. Drag-to-restore, edge snapping, and
double-click-to-maximize are useful later interactions but are not required for
the first state implementation.

### PD5 — Taskbar activation restores but does not toggle-minimize

Clicking a minimized frame's taskbar button restores, raises, and focuses it.
Clicking a visible frame's button raises and focuses it. It does not minimize an
already active window; minimize remains an explicit caption action in this
unit. This keeps the taskbar change narrowly tied to reachability of minimized
windows.

### PD6 — Theme changes preserve both placement invariants

For a normal frame, the existing live-theme rule remains: preserve client
dimensions while decoration metrics change. For a maximized frame, keep the
current outer bounds equal to the work area and convert only its stored normal
restore bounds so restoring after a theme change preserves the normal client
size. The same rule applies while a maximized frame is minimized.

---

## Technical design

### Caption-button geometry

Replace the close-only geometry contract with shared typed layout:

```rust
pub struct CaptionButtonLayout {
    pub minimize: Option<Rect>,
    pub maximize: Option<Rect>,
    pub close: Rect,
}

pub fn caption_button_layout(
    bounds: Rect,
    metrics: FrameMetrics,
    resizable: bool,
) -> CaptionButtonLayout;
```

Extend `FrameMetrics` with the spacing needed to place equal-height controls
from right to left. Keep the current close rectangle bit-identical in all
themes; place maximize immediately to its left and minimize to the left of
maximize. Preserve `close_button_rect()` as a compatibility wrapper over the
new layout until all call sites and tests use the typed contract.

The complete three-button footprint must fit the supported minimum in every
theme. Give neutral buttons a separate width metric when necessary (Futurism's
close pill can remain wider), and use saturating arithmetic. Express the
minimum in client-width terms across all built-in themes, then derive the
current outer minimum from active decoration metrics. `GUI_WIN_CREATE` rejects
a smaller resizable client with `-EINVAL`, and border resize uses the same
helper. A live theme change can therefore preserve client width without making
the new title-bar controls overlap or escape the frame.

`FrameChrome` receives the layout plus whether the maximize slot should paint
the maximize or restore glyph. Each theme then:

- paints minimize/maximize with its neutral caption surface and close with its
  existing destructive surface;
- uses theme-appropriate line glyphs (underscore, square, overlapping restore
  squares, and close X);
- clips the title at the left edge of the leftmost visible caption button;
- keeps current active/inactive frame colors and shadow/overlay behavior.

No new hover/pressed animation is required. Caption actions keep the current
close-button activation model and run on button-down.

### Frame capability and placement API

Add immutable `Window`/`FrameWindow` queries needed by hit-testing and taskbar
activation rather than continuing to use `window_title().is_some()` as the
only frame-policy proxy. The manager must be able to distinguish:

- decorated frame vs other titled/panel windows;
- resizable vs fixed-size;
- normal vs maximized;
- minimized vs visible.

Construction remains backward compatible:

- `FrameWindow::new` creates a resizable normal frame;
- a builder/setter or explicit options constructor creates a fixed frame;
- changing resizability after maximize/minimize is not supported in v1.

### Hit-testing and transition dispatch

In `start_drag_if_on_title_bar`:

1. find the topmost visible top-level frame as today;
2. allow border hits only when the frame is resizable and normal;
3. compute one local `CaptionButtonLayout` and test close, maximize, minimize;
4. dispatch caption actions through the manager transition methods;
5. allow title drag only in normal placement;
6. keep client focus and close-request delivery unchanged.

The same layout object used by the painter defines every clickable pixel.
There are no duplicated offsets in `manager.rs`.

### Focus and visibility during minimize

Minimize performs these steps as one manager operation:

1. cancel pointer capture/drag/resize if it targets the frame subtree;
2. clear focus for every subtree member and remove those IDs from
   `focus_stack`;
3. set the frame invisible and mark a full repaint so exposed siblings and
   backdrop effects are recomposed;
4. walk desktop children from top to bottom, skipping the taskbar, popups,
   hidden windows, and the minimized frame;
5. focus the first focusable descendant of the next visible frame, or leave
   focus empty when none exists.

Activation from the taskbar reverses visibility, raises the frame through the
existing z-order path, and focuses its first focusable descendant. This also
ensures ring-3 `FocusChange` events describe the actual visible focus state.

### Work-area maximize

Add `WindowManager::desktop_work_area()`:

- start with `Rect::new(0, 0, screen_width, screen_height)`;
- if the registered taskbar exists and is visible, resolve its global bounds;
- for the current bottom-docked layout, cap work-area height at the taskbar's
  top edge;
- use full screen when the taskbar is absent, hidden, malformed, or outside
  the screen.

Maximize sets the frame's logical outer bounds to that rectangle. Retained
shadow gutters may extend beyond the screen and are clipped normally. The
taskbar remains visible and top-level sibling ordering remains unchanged.

### Ring-3 create flags and toolkit options

Define the same flag constant in `src/userland/gui.rs` and
`userland/runtime`:

```rust
pub const GUI_WINDOW_FIXED_SIZE: u64 = 1 << 0;
```

`gui_win_create_handler` accepts only this bit and calls
`frame.set_resizable(flags & GUI_WINDOW_FIXED_SIZE == 0)`. Zero remains the
resizable default for all current binaries, including the committed Links
ELF.

In `userland/libs/gui`:

```rust
pub struct WindowOptions {
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self { Self { resizable: true } }
}

Window::new(...)                         // default options
Window::new_with_options(..., options)   // explicit fixed-size request
```

Use fixed options for common message boxes (and the fixed-size color picker if
its existing layout is kept fixed); leave file dialogs and normal applications
on the default. No GUI event ABI version bump is needed: maximize/restore reuse
`GUI_EVENT_RESIZE`, and minimize is server-side visibility/focus state.

---

## Implementation steps

### 1. Introduce frame capability/state and shared caption layout

Files:

- `src/window/windows/frame.rs`
- `src/window/types.rs`
- `src/window/theme/mod.rs`
- `src/window/mod.rs`

Add `FramePlacement`, resizable/minimized state, frame queries, caption-button
layout, maximize-vs-restore chrome state, and the new metric spacing. Keep the
existing close rectangle stable. Update theme application so normal and stored
restore bounds preserve client dimensions correctly.

### 2. Paint functional controls in all frame themes

Files:

- `src/window/theme/classic.rs`
- `src/window/theme/aero.rs`
- `src/window/theme/futurism.rs`

Add neutral minimize/maximize surfaces and glyphs, switch maximize to restore
when appropriate, and derive caption clipping from the leftmost visible
control. Keep close styling and all non-caption frame pixels unchanged.

### 3. Add manager placement, visibility, focus, and work-area transitions

Files:

- `src/window/manager.rs`
- `src/window/types.rs`

Implement minimize, maximize/restore, taskbar activation, next-visible-frame
focus, work-area calculation, interaction cancellation, and shared resize/
damage/reflow helpers. Wire `MinimizeButton` and `MaximizeButton` hit results.
Gate border resize and drag by frame capability/placement.

### 4. Make taskbar restore minimized frames

File:

- `src/commands/guishell/mod.rs`

Replace `PendingAction::FocusWindow` with an activation action that delegates
to `WindowManager::activate_frame`. Continue listing invisible titled frames so
their taskbar buttons remain present. Do not duplicate state or bounds in
`GUIShellState`.

### 5. Expose backward-compatible fixed-size creation

Files:

- `src/userland/gui.rs`
- `src/userland/gui_syscalls.rs`
- `userland/runtime/src/lib.rs`
- `userland/libs/gui/src/lib.rs`
- `userland/libs/dialogs/src/message_box.rs`
- `userland/libs/dialogs/src/color_picker.rs` (if kept fixed)
- `src/window/dialogs/run.rs`
- `src/window/dialogs/message_box.rs`

Accept only `GUI_WINDOW_FIXED_SIZE`, preserve zero as resizable, add toolkit
window options, and mark fixed dialogs close-only. The Links driver and its
committed prebuilt remain unchanged because its zero flag continues to mean
resizable.

### 6. Add regression coverage and update live documentation

Files:

- `src/tests/window_theme.rs`
- `src/tests/window_manager_render.rs`
- `src/tests/gui_userland.rs`
- `src/tests/taskbar_tests.rs` if taskbar-specific helpers are introduced
- `src/window/CLAUDE.md`
- `src/commands/CLAUDE.md`
- `src/userland/CLAUDE.md`
- `docs/window_system_design.md`
- `CLAUDE.md`

Document functional caption controls, frame state, work-area maximize,
taskbar restore, and the fixed-size create flag. Update the window-system
status so border resizing and minimize/maximize are no longer listed as
future work.

---

## Test plan

### Caption layout and painting

In `window_theme`:

- the close rectangle remains exactly its current value for Classic, Aero,
  and Futurism;
- resizable layout returns three non-overlapping buttons in right-to-left
  minimize/maximize/close order and keeps all rectangles inside the title bar;
- fixed layout returns close only;
- narrow frames use saturating/clipped title layout and never underflow;
- every theme's three-button footprint fits the enforced resizable minimum;
- each theme paints visible minimize and maximize glyph pixels;
- maximized chrome paints a restore glyph instead of the maximize square;
- title text does not enter any caption button rectangle;
- existing border, alpha, shadow, rounded-corner, and close-button regression
  assertions remain valid.

### Manager state transitions

In `window_manager_render`:

- normal -> maximize fills exactly the area above the taskbar;
- maximize -> restore returns the exact original outer bounds;
- maximize and restore each reflow the content once and expose the expected
  client dimensions;
- minimizing hides the subtree, removes hidden focus, focuses the next visible
  frame, and leaves the frame registered;
- activating a minimized frame restores visibility, raises it, and focuses its
  content;
- minimize while maximized followed by taskbar activation remains maximized;
- a subsequent restore still returns the original normal bounds;
- fixed frames do not enter resize/minimize/maximize states and expose only
  close hit geometry;
- maximized frames do not drag or border-resize;
- old/new bounds and visibility transitions repaint all exposed pixels without
  leaving retained surfaces or shadows behind;
- destroying a minimized frame clears state and remains safe.

### GUI ABI compatibility

In `gui_userland`:

- flags zero creates a resizable frame;
- `GUI_WINDOW_FIXED_SIZE` creates a fixed frame;
- unknown flag bits return `-EINVAL`;
- maximize/restore produces ordinary `GUI_EVENT_RESIZE` payloads with client,
  not frame, dimensions;
- minimize produces focus loss but no resize event;
- create/destroy and process-cleanup tests pass for both visible and minimized
  records.

### Manual matrix

Boot each renderer/theme combination that exercises distinct paint paths:

1. Classic + legacy.
2. Classic + retained CPU.
3. Aero + retained CPU.
4. Futurism + retained CPU.
5. Futurism + strict VirGL when the qualified host is available.

For Terminal and at least one ring-3 app, verify caption targets, client reflow,
taskbar restore, focus handoff, exact restore bounds, repeated max/restore, and
close from normal/maximized state. Verify Run/message dialogs remain close-only
and non-resizable.

---

## Verification

1. `cargo fmt --check`
2. `cargo check`
3. `cargo clippy`
4. `cargo check --manifest-path userland/Cargo.toml`
5. `./test.sh window_theme window_manager_render gui_userland taskbar`
6. `./test.sh`
7. Run the manual renderer/theme matrix above.

Expected QEMU test success is exit code 33. A filter matching zero tests is a
failure (exit code 35), so use the registered module names exactly.

---

## Risks and mitigations

### R1 — Hidden focus continues receiving keyboard input

Visibility alone does not edit `focus_stack`. Minimize explicitly removes the
entire subtree, emits focus loss through normal `set_focus`, and selects a
visible replacement. Tests send keyboard/focus events after minimize.

### R2 — Painter and hit targets drift

All themes and manager hit-testing consume `caption_button_layout`; no theme or
manager owns private caption offsets. Exact-geometry tests pin every theme.

### R3 — Theme switch corrupts maximize restore bounds

Treat current maximized work-area bounds and saved normal bounds separately.
Theme transition leaves the former fixed and reframes the latter using old/new
decoration metrics. Test Classic -> Aero -> Futurism while maximized and while
minimized-maximized, then restore.

### R4 — Maximize covers the taskbar

Use the registered visible taskbar's global top edge as the work-area bottom.
Test both a normal bottom taskbar and no-taskbar fallback.

### R5 — Existing binaries unexpectedly become fixed-size

Flags zero remains resizable. Fixed size is an explicit opt-out, and unknown
bits still fail closed. The committed Links binary needs no refresh.

### R6 — Resize event/resource churn

Only placement transitions change bounds. Minimize/activation never resize.
Maximize/restore call the existing child-reflow path once, allowing ring-3 CPU
surfaces and VirGL clients to follow their established resize handling.

---

## Out of scope and follow-ups

- Hover/pressed caption animations and pointer capture for caption controls.
- Double-click title bar to maximize/restore.
- Drag a maximized title bar to restore under the pointer.
- Aero Snap, edge tiling, multi-monitor work areas, movable taskbars, or virtual
  desktops.
- Keyboard window-management shortcuts such as Alt+F4 or Meta+Arrow.
- Minimize/maximize animations or live thumbnail previews.
- Taskbar click-to-minimize for the already active frame.
- App-controlled min/max capability changes after window creation.
- Persisting window placement across process restarts.
