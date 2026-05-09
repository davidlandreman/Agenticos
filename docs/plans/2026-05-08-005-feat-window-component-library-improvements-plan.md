---
title: Window Component Library Improvements & File-Manager Readiness
type: feat
status: active
date: 2026-05-08
origin: docs/brainstorms/2026-05-08-window-components-improvements-requirements.md
---

# Window Component Library Improvements & File-Manager Readiness

## Summary

Implement three foundational abstractions (layout primitives, a single-child `ScrollView` wrapper, a unified `Selection` model) and seven net-new components (`TreeView`, `Splitter`, `Toolbar`, `StatusBar`, `PathBar`, `IconView`, `ProgressBar`) in `src/window/windows/`, then migrate `List`, `MultiColumnList`, `TextEditor`, and the three existing dialogs onto the new foundations. Delivered in five phases (Foundations → Trait cleanup → Migrations → Net-new → Hygiene), with one event-types extension (U16 — `MouseEvent.modifiers` and `MouseEventType::Scroll { delta_x, delta_y }`) sequenced before the migration phase to give multi-select and scroll-wheel handling a clean data source. Mouse-wheel routing splits into U4a (in-scope event routing) and U4b (contingent driver Scroll-emission follow-up). The Window-trait delegation boilerplate is collapsed into trait default methods routed through new `base()` / `base_mut()` accessors — no new macro.

---

## Problem Frame

`src/window/windows/` accumulated 17 components but consumers (notably `src/window/dialogs/file_open.rs`) hand-compute pixel offsets for every child, `List` and `MultiColumnList` reimplement the same scroll/scrollbar/selection logic, and the selection model can't express the multi-select semantics File Manager needs. The next planned app — a File Manager / Finder / Explorer — also needs widgets that don't exist (TreeView, Splitter, IconView, PathBar, Toolbar, StatusBar, ProgressBar). See origin for full pain narrative.

---

## Requirements

- R1. `VBox`, `HBox`, `Padding`, `Spacer`, and a `Fill` modifier are available as passive layout containers that compute child bounds from their own bounds.
- R2. Layout containers propagate resize: when a container's bounds change, layout-managed children's bounds are recomputed without caller intervention.
- R3. `src/window/dialogs/file_open.rs`, `src/window/dialogs/file_save.rs`, and `src/window/dialogs/message_box.rs` are migrated off hand-computed pixel offsets onto layout primitives.
- R4. Layout containers respect minimum-size hints from children when supplied; truncation policy is per-widget.
- R5. A `ScrollView` wrapper takes a single content window, draws a scrollbar when content is larger than viewport, and translates child paint and event coordinates by the current scroll offset.
- R6. Mouse-wheel events scroll the topmost `ScrollView` ancestor of the hit window.
- R7. `List`, `MultiColumnList`, and `TextEditor` are migrated to the shared `ScrollView`, removing in-widget scrollbar drawing and scroll-offset bookkeeping.
- R8. `ScrollView` supports vertical scrolling by default; horizontal scrolling is opt-in.
- R9. A shared `Selection` type replaces the current `Option<usize>` selection state in `List` and `MultiColumnList` and covers none, single, multi (set of indices), and contiguous range.
- R10. Mouse and keyboard selection semantics (shift-click range extension, ctrl-click toggle, arrow keys, shift+arrow extension) live in one shared place, not duplicated per widget.
- R11. Selection callbacks fire with the full `Selection` value, not just an index.
- R12. Single-select-only consumers can opt out of multi-select via a one-line widget configuration.
- R13. `TreeView` provides hierarchical lists with expand/collapse nodes, the unified `Selection` model, and keyboard nav (arrows; Left to collapse / move to parent; Right to expand / move to first child); sits on top of `ScrollView`.
- R14. `Splitter` is a two-pane container (vertical or horizontal) with a draggable divider and minimum-pane-size constraints; works with any two child windows.
- R15. `Toolbar` is a horizontal strip of `Button`s composed via `HBox` with consistent spacing.
- R16. `StatusBar` is a thin horizontal strip composed of `Label`s via `HBox`.
- R17. `PathBar` is a clickable breadcrumb bar; clicking a segment fires a callback with the truncated path; collapses to "..." overflow when needed.
- R18. `IconView` is a Finder-style grid of icon+label tiles using the unified `Selection` model and `ScrollView`; supports configurable tile size.
- R19. `ProgressBar` is a determinate progress widget with current/total values and an optional label, embeddable inside dialogs (e.g., a future Copy Progress dialog) and inside `StatusBar`.
- R20. Window-trait delegation boilerplate (the ~25-line `impl Window` blocks delegating to `WindowBase`) is eliminated by moving pure delegation into `Window` trait default methods, requiring widgets to expose `base()` / `base_mut()` accessors.
- R21. Color and styling defaults across components are reconciled so default-styled widgets in the same window look consistent.

**Origin actors:** none (origin omitted Actors — single-user kernel).
**Origin flows:** none (origin omitted Key Flows — pure component-library work).
**Origin acceptance examples:** AE1 (covers R1, R2, R3 — `file_open` migration), AE2 (covers R5, R7 — scroll wiring), AE3 (covers R9, R10 — multi-select semantics), AE4 (covers R13 — TreeView keyboard nav), AE5 (covers R14 — Splitter constraints).

**AE coverage layering.** AE3 is verified across two layers — the `Selection` model layer (U1, exercising the click/arrow/extend helpers in isolation) and the widget integration layer (U6, exercising the same semantics through `List` and `MultiColumnList` mouse and keyboard events). AE2 is verified at the `ScrollView` layer (U3, scroll-wheel and scrollbar behavior) and at the integration layer (U6, a 1000-row list scrolling through ScrollView). The two-layer split is intentional: model-layer tests pin behavior; integration tests pin wiring.

---

## Scope Boundaries

- Action / Command abstraction (cross-cutting menubar / toolbar / context-menu binding) — out of plan, deferred to post-File-Manager.
- Drag-and-drop between widgets — out of plan.
- Cross-app clipboard — out of plan.
- Full theming system (color schemes, dark mode, runtime switching) — out of plan; only default reconciliation per R21.
- Dialog-system overhaul (modal stacking, builder API, multi-modal) — out of plan.
- Accessibility primitives (screen reader hooks, high-contrast themes, focus-ring rendering) — out of plan.
- Animation, transitions, opacity blending — out of plan.
- Custom per-widget fonts — out of plan; keep using `core_font::get_default_font()`.
- Building the File Manager itself — this plan delivers the components File Manager will consume; the app is a separate plan.
- Replacing or rewriting the `mouse.rs` driver — only verify and wire what the driver already exposes.

---

## Context & Research

### Relevant Code and Patterns

- `src/window/mod.rs:36` — `Window` trait. Already uses default methods (`set_bounds_no_invalidate` ships a default impl). Default methods are the natural delegation-cleanup mechanism.
- `src/window/windows/base.rs` — `WindowBase` struct. The shared base every widget composes; new `base()` / `base_mut()` accessors will return references to it.
- `src/window/windows/list.rs` — canonical single-column list. Reference for selection state, scrollbar drawing, keyboard nav. Stub at line 338 for `MouseEventType::Scroll`.
- `src/window/windows/multi_column_list.rs` — canonical multi-column list. Already exposes `on_right_click(usize, Point)` with global position for context-menu placement; that contract is preserved.
- `src/window/windows/text_editor.rs` — multi-line editor. Caches `visible_cols` / `visible_rows` from initial bounds; migration must restore correct behavior under resize.
- `src/window/windows/frame.rs` — `FrameWindow::content_area()` returns the inner content rectangle; the dialog-migration unit relies on this for the layout-container root.
- `src/window/dialogs/file_open.rs` — canonical example of hand-computed offsets the layout primitives replace; a clean before/after comparison case.
- `src/window/manager.rs:338` — `route_mouse_event`; the entry point where mouse-wheel routing to `ScrollView` ancestors will live.
- `src/window/manager.rs:393` — `topmost_at` hit-test; the function whose result the new scroll routing will walk upward from.
- `src/window/event.rs` — `Event::Resize(ResizeEvent)` already exists; layout containers consume it via `handle_event`.
- `src/window/event.rs:91-96` — `MouseEventType::Scroll` exists in the type but is not currently special-routed.
- `src/drivers/mouse.rs` — current mouse driver. Verify whether wheel deltas reach `MouseEventType::Scroll` events (deferred-to-implementation question).
- `src/window/windows/menu.rs`, `src/window/windows/menu_bar.rs` — existing patterns for popup-style widgets and item lists with hover state; informative for `PathBar` and `Toolbar` styling.
- `src/window/types.rs:336` — `HitTestResult` for resize edges; the `Splitter` divider drag reuses this enum's spirit (drag-on-edge).
- `src/tests/CLAUDE.md` — test framework conventions; new tests live in `src/tests/`.
- `.claude/rules/no-std.md` — `no_std` discipline; no `std::*` imports, custom `Arc` from `crate::lib::arc::Arc`.
- `.claude/rules/testing-flow.md` — kernel test exit codes (33 pass, 35 fail); QEMU-based integration tests.

### Institutional Learnings

- `docs/solutions/` is currently empty — no prior learnings apply.
- `src/graphics/CLAUDE.md` notes "Scrolling = `memmove`. Don't redraw all rows when shifting; move them in memory." — relevant if `ScrollView` ever optimizes scroll-by-delta; deferred to implementation as a perf optimization, not a planning decision.

### External References

- None gathered. Local patterns are dense and consistent; external research would not improve this plan.

---

## Key Technical Decisions

- **Default-method delegation over macro or wrapper for R20.** The `Window` trait already uses default methods; adding `fn base(&self) -> &WindowBase` and `fn base_mut(&mut self) -> &mut WindowBase` as required methods lets every other method default to a one-line delegation. Rejected: a `delegate_window!` macro (extra concept to learn, harder to step through in a debugger) and a separate `Widget` wrapper struct (bigger refactor, churns every widget call site).
- **Single-child `ScrollView<W: Window + ?Sized>` over multi-child.** Each migration target has one logical scrollable content area. Multi-child (e.g., header-stays-visible patterns) can layer on later via composition (`VBox { fixed-header, ScrollView { body } }`); committing to it now would speculate about consumers we don't yet have.
- **Mouse-wheel routing in `WindowManager::route_mouse_event`.** When the hit window is identified by `topmost_at`, walk up the parent chain looking for the **nearest enclosing** `ScrollView` ancestor (innermost match wins) and deliver the `Scroll` event there. Falls through to standard delivery if none found. Centralizes the scroll contract in the dispatcher rather than asking every widget to opt in.
- **`ScrollView` discriminator: bool method + manager-side downcast, not a typed accessor.** `Window` gains `fn is_scroll_view(&self) -> bool { false }` (overridden by `ScrollView` to return `true`); `WindowManager` performs the downcast through its existing window registry only when routing a `Scroll` event. Avoids coupling every widget's `impl Window` to the concrete `ScrollView` type and leaves room for future scrollable types without per-method trait expansion.
- **`MouseEventType::Scroll` carries a delta.** Variant becomes `Scroll { delta_x: i32, delta_y: i32 }`. One unit on each axis represents one wheel notch (scaled by widget-side step constants). Driver layer populates from real wheel input; tests construct synthetic events with explicit deltas. Promoted out of Open Questions because every scroll-handling widget depends on it.
- **`MouseEvent` carries keyboard modifiers.** `MouseEvent` gains `modifiers: KeyModifiers`. The input pipeline fuses keyboard-modifier state with each emitted mouse event so that `MouseEventType::ButtonDown / ButtonUp` carry the modifier set at click time. Required for shift-click range selection and ctrl-click toggle (R10, AE3); without it the Selection model has no data source for `ClickMods`. Promoted out of Open Questions and U6's footnote into a first-class plan unit (U16) sequenced before U6.
- **Layout-container resize via `set_bounds` override, not `Event::Resize` dispatch.** Layout containers (`VBox`, `HBox`, `Padding`, `Splitter`) override `set_bounds` on the `Window` trait to call a private `relayout(&mut self)` that walks `WindowBase::children` and writes each child's bounds via `set_bounds` (not `set_bounds_no_invalidate`). No `Event::Resize` synthesis or manager-level resize dispatch needed. Self-contained inside the layout containers; keeps U2's blast radius inside the layout module.
- **`WindowManager` exposes a `with_window_mut(WindowId, FnMut(&mut dyn Window))` accessor.** Required for layout containers to write child bounds without owning typed children. Manager already owns every window via the registry; the accessor formalizes the existing lookup that other dispatch paths (e.g., `bring_to_front`, `focus_window`) already perform internally.
- **Children's bounds writes use `set_bounds` (not `set_bounds_no_invalidate`).** `set_bounds_no_invalidate` is reserved for the existing render-time-transform pattern in `manager.rs::render_window_tree_with_offset`. Layout-driven and drag-driven bounds writes (Splitter divider drag, `VBox`/`HBox` relayout) use `set_bounds` so the child's `needs_repaint` flag flips and it actually repaints next frame.
- **`Event::EnsureVisible(Rect)` event variant for child-to-ScrollView coupling.** A new `Event` variant: a child emits it (e.g., `TextEditor` on cursor move, `TreeView` on selection move via keyboard, `IconView` on selection move) and the enclosing `ScrollView`'s `handle_event` consumes it to adjust scroll offset. Avoids typed parent references through trait objects and gives `TextEditor`'s `ensure_cursor_visible` a clean home. Updates the `Event` enum — see "Unchanged invariants" change below.
- **`Selection` enum in `src/window/selection.rs` (top-level under `src/window/`).** It is a model, not a widget — placed alongside `event.rs` and `keyboard.rs` rather than inside `windows/`. Variants: `None`, `Single(usize)`, `Multi(BTreeSet<usize>)`, `Range { anchor: usize, end: usize }`. `BTreeMap`/`BTreeSet` from `alloc` is the `no_std`-compatible choice; no `HashMap`.
- **Layout primitives in `src/window/windows/layout/` subdirectory.** Layout containers implement `Window` and live among other window-implementing types. Subdirectory groups them without polluting the top-level `windows/` listing.
- **`ScrollView` lives at `src/window/windows/scroll_view.rs`** (a window/widget, not a layout primitive — it has its own paint logic for the scrollbar).
- **Phasing chosen for blast radius isolation.** Foundations (U1-U4) and net-new components (U9-U14) are net-additive — they don't touch existing widget call sites. Trait cleanup (U5) and migrations (U6-U8) are the only phases that change existing code paths. Putting Foundations first lets the trait-cleanup unit be authored against a known-good new trait shape.
- **Toolbar + StatusBar combined into one unit (U11).** Both are thin compositions over `HBox` with one decorated child type (Button vs. Label). Splitting them would create two near-identical units with no useful boundary.
- **`ProgressBar` separate (U14).** Has its own paint logic (filled-vs-empty bar split by current/total ratio); not a composition.
- **Test posture: standard unit tests under `src/tests/` plus boot-test the desktop after each batch of trait-delegation rewrites in U5.** The desktop's `FrameWindow + TerminalWindow` is the practical regression check that the trait-cleanup unit didn't break the implicit Window contract. Tests use the existing in-kernel framework (`./test.sh`).
- **Selection multi-select opt-out (R12) as a single configuration call.** Default is single-select; widgets call `set_selection_mode(SelectionMode::Multi)` to opt in. Avoids breaking single-select consumers like the existing `file_open` dialog usage.
- **`Splitter` orientation set at construction, not toggled at runtime.** No consumer needs runtime orientation changes; static orientation simplifies layout math and minimum-size enforcement.
- **`PathBar` segment overflow uses leading "..." with rightmost segments visible** (Finder/Explorer convention). Right side is the most relevant context (current dir).

---

## Open Questions

### Resolved During Planning

- **Layout container shape — do they own children or just position them?** Resolved: layout containers are `Window`-implementing parents that own their children via the existing `WindowBase::children` mechanism. They override `set_bounds` to call a private `relayout` that walks children and writes each child's bounds via `WindowManager::with_window_mut`. No new ownership model needed.
- **Selection storage — `Vec<bool>` or `BTreeSet<usize>` for `Multi`?** Resolved: `BTreeSet<usize>`. Sparse selection across thousands of items is the realistic File Manager case; `Vec<bool>` wastes memory.
- **Where does `Selection` live — `src/window/selection.rs` or `src/window/windows/selection.rs`?** Resolved: top-level `src/window/selection.rs`. It is reused by widgets across `windows/` and is a model, not a widget itself.
- **Layout primitives directory — `src/window/layout/` or `src/window/windows/layout/`?** Resolved: under `windows/` since they implement `Window`.
- **`MouseEventType::Scroll` payload shape.** Resolved: `Scroll { delta_x: i32, delta_y: i32 }`. See Key Technical Decisions.
- **Mouse modifier state on click events.** Resolved: `MouseEvent` gains `modifiers: KeyModifiers`; sequenced as a dedicated unit (U16) before U6.
- **Layout container relayout-on-resize mechanism.** Resolved: `set_bounds` override + private `relayout`, not `Event::Resize` dispatch.
- **`ScrollView` discriminator shape.** Resolved: bool `is_scroll_view()` + manager-side downcast, not a typed accessor.

### Deferred to Implementation

- **Does the current `mouse.rs` driver actually emit `MouseEventType::Scroll` events end-to-end?** The `MouseEventType` enum has the variant; `mouse_old.rs` has IntelliMouse 4-byte protocol code; the active `mouse.rs` does not. **U4 is split into U4a (event-routing inside the manager — testable with synthetic events, in-scope) and U4b (driver Scroll emission — contingent follow-up that may rehabilitate `mouse_old.rs` wheel handling into `mouse.rs` and add VirtIO scroll plumbing).** R6 acceptance moves to U4b. The driver-state verification happens at U4b kickoff; if the work is large, U4b ships in a separate plan and this plan delivers ScrollView with synthetic-event coverage only.
- **Exact `Selection` API surface (helper methods like `is_selected`, `iter`, `len`, `extend_to`).** Resolved at U1 implementation time once concrete callsites exist.
- **`VBox`/`HBox` weight / `Fill` semantics** — whether `Fill` is a child marker or a method on the container. Pick the simpler shape at U2 implementation against the file_open migration target.
- **`Splitter` divider hit-test thickness.** Pick a value (e.g., 4-6 pixels) that's draggable without being visually obtrusive at U10 implementation.
- **`TreeView` node-data trait** — whether nodes implement a trait (`TreeNode`) or are stored as concrete data. Decide at U9 against File Manager's filesystem-traversal needs.
- **`IconView` icon source** — whether icons come from a new icon-loading API in `src/graphics/` or use placeholder glyphs from the system font for v1. Decide at U13.
- **Color reconciliation values for R21** (e.g., should `List` default to `Color::WHITE` or to the `Container`-style 240/240/240?). Decide at U15 by surveying current callsites and picking the value that minimizes per-callsite overrides.
- **Borrow-checker viability of default-method delegation in U5.** Default methods routing through `self.base_mut()` force a full reborrow of `self`, which may conflict with widget code that simultaneously holds a borrow on a widget-specific field. **Before committing to U5's delegation refactor across all 17 widgets, prototype the conversion on `MultiColumnList` (the most event-heavy non-trivial widget) end-to-end and report whether borrow accommodation in `handle_event` materially offsets the line-count savings.** If borrow conflicts are pervasive, reconsider the rejected `delegate_window!` macro alternative — direct field accessors avoid the reborrow tax.

---

## Output Structure

```
src/window/
├── mod.rs                            (modify: U5 — Window trait default methods; modify: U15 — palette comment block)
├── selection.rs                      (NEW — U1)
├── event.rs                          (modify: U16 — MouseEvent.modifiers + Scroll{delta_x, delta_y} + Event::EnsureVisible variant)
├── manager.rs                        (modify: U2 — with_window_mut accessor; modify: U4a — scroll routing; modify: U16 — preserve new fields in route_mouse_event)
└── windows/
    ├── base.rs                       (modify: U5 — base()/base_mut() accessor pattern in Window trait still uses this)
    ├── scroll_view.rs                (NEW — U3)
    ├── layout/
    │   ├── mod.rs                    (NEW — U2)
    │   ├── vbox.rs                   (NEW — U2)
    │   ├── hbox.rs                   (NEW — U2)
    │   ├── padding.rs                (NEW — U2)
    │   └── spacer.rs                 (NEW — U2: Spacer + Fill)
    ├── tree_view.rs                  (NEW — U9)
    ├── splitter.rs                   (NEW — U10)
    ├── toolbar.rs                    (NEW — U11)
    ├── status_bar.rs                 (NEW — U11)
    ├── path_bar.rs                   (NEW — U12)
    ├── icon_view.rs                  (NEW — U13)
    ├── progress_bar.rs               (NEW — U14)
    ├── list.rs                       (modify: U6 [trait cleanup folded in], U16)
    ├── multi_column_list.rs          (modify: U6 [trait cleanup folded in], U16)
    ├── text_editor.rs                (modify: U5, U7, U16)
    ├── text_input.rs                 (modify: U5, U16)
    ├── label.rs                      (modify: U5)
    ├── button.rs                     (modify: U5, U11 [set_enabled + greyed-out paint state], U16)
    ├── menu.rs                       (modify: U5, U16)
    ├── menu_bar.rs                   (modify: U5, U16)
    ├── menu_bar_popup.rs             (modify: U5, U16)
    ├── taskbar.rs                    (modify: U5, U16)
    ├── container.rs                  (modify: U5)
    ├── frame.rs                      (modify: U5 [keeps custom set_focus/has_focus], U16)
    ├── desktop.rs                    (modify: U5)
    ├── text.rs                       (modify: U5)
    ├── terminal.rs                   (modify: U5)
    ├── dialog.rs                     (no change)
    └── mod.rs                        (modify: U2-U14 — add new module exports)
src/window/dialogs/
├── file_open.rs                      (modify: U6 [callback signature], U8 [layout migration])
├── file_save.rs                      (modify: U6, U8)
└── message_box.rs                    (modify: U8)
src/input/
├── mod.rs                            (modify: U16 — fuse keyboard modifiers into mouse events; modify: U4b [contingent] — emit Scroll events)
└── mouse_driver.rs                   (modify: U16, U4b [contingent])
src/drivers/
├── mouse.rs                          (modify: U4b [contingent] — IntelliMouse 4-byte protocol from mouse_old.rs)
└── virtio/input.rs                   (modify: U4b [contingent] — VirtIO scroll handling)
src/tests/
├── selection_tests.rs                (NEW — U1)
├── layout_tests.rs                   (NEW — U2)
├── scroll_view_tests.rs              (NEW — U3, U4a)
├── trait_delegation_tests.rs         (NEW — U5)
├── list_migration_tests.rs           (NEW — U6)
├── text_editor_migration_tests.rs    (NEW — U7)
├── tree_view_tests.rs                (NEW — U9)
├── splitter_tests.rs                 (NEW — U10)
├── toolbar_status_tests.rs           (NEW — U11)
├── path_bar_tests.rs                 (NEW — U12)
├── icon_view_tests.rs                (NEW — U13)
├── progress_bar_tests.rs             (NEW — U14)
├── mouse_event_extension_tests.rs    (NEW — U16)
└── mod.rs                            (modify: register new test modules)
```

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### Window trait shape after U5

```
trait Window: Send {
    // Required (each widget supplies):
    fn base(&self) -> &WindowBase;
    fn base_mut(&mut self) -> &mut WindowBase;
    fn paint(&mut self, device: &mut dyn GraphicsDevice);
    fn handle_event(&mut self, event: Event) -> EventResult;
    fn can_focus(&self) -> bool { false }   // default: not focusable

    // Defaulted (delegate to base):
    fn id(&self) -> WindowId             { self.base().id() }
    fn bounds(&self) -> Rect             { self.base().bounds() }
    fn visible(&self) -> bool            { self.base().visible() }
    fn parent(&self) -> Option<WindowId> { self.base().parent() }
    fn children(&self) -> &[WindowId]    { self.base().children() }
    fn needs_repaint(&self) -> bool      { self.base().needs_repaint() }
    fn has_focus(&self) -> bool          { self.base().has_focus() }
    fn set_bounds(&mut self, b: Rect)    { self.base_mut().set_bounds(b) }
    fn set_visible(&mut self, v: bool)   { self.base_mut().set_visible(v) }
    fn set_parent(&mut self, p: Option<WindowId>) { self.base_mut().set_parent(p) }
    fn add_child(&mut self, c: WindowId) { self.base_mut().add_child(c) }
    fn remove_child(&mut self, c: WindowId) { self.base_mut().remove_child(c) }
    fn invalidate(&mut self)             { self.base_mut().invalidate() }
    fn set_focus(&mut self, f: bool)     { self.base_mut().set_focus(f) }
    fn set_bounds_no_invalidate(&mut self, b: Rect) { self.base_mut().set_bounds_no_invalidate(b) }
    fn window_title(&self) -> Option<&str> { None }
}
```

### Selection model (U1)

```
enum Selection {
    None,
    Single(usize),
    Multi(BTreeSet<usize>),
    Range { anchor: usize, end: usize },  // inclusive both ends
}

enum SelectionMode { Single, Multi }

// Helpers (illustrative names):
//   is_selected(idx) -> bool
//   contains_anchor() -> Option<usize>   // for shift-extend
//   click(idx, mods)                     // applies mouse-click semantics given modifiers
//   move_arrow(direction, mods, item_count)
```

### Mouse-wheel routing flow (U4a)

```mermaid
flowchart TD
    A["MouseEventType::Scroll { delta_x, delta_y } arrives"] --> B[route_mouse_event]
    B --> C[topmost_at finds hit window H]
    C --> D{Walk H -> parent -> ... up tree, calling is_scroll_view on each}
    D --> E{Found ScrollView ancestor (innermost match)?}
    E -- yes --> F[Deliver Scroll to that ScrollView with payload preserved]
    E -- no --> G[Deliver Scroll to H normally]
    F --> H[ScrollView updates scroll_x/scroll_y, invalidates]
    G --> I[Widget either ignores or handles per its rules]
```

### Layout container resize flow (U2)

```
Parent bounds change (drag, resize, or initial layout)
  → Caller invokes set_bounds on the container (typically via
    WindowManager::with_window_mut from another layout container,
    or directly when the container is at the top of a content tree)
  → Container's set_bounds override:
      1. updates WindowBase via self.base_mut().set_bounds(new_bounds)
      2. calls self.relayout() — walks WindowBase::children, computes
         each child's new bounds from sizing hints
      3. for each child: WindowManager::with_window_mut(child_id,
         |w| w.set_bounds(child_bounds))   ← uses set_bounds, NOT
         set_bounds_no_invalidate, so children invalidate and repaint
  → No Event::Resize synthesis or manager-level resize-dispatch path
```

### File Manager target composition (informational)

```
FrameWindow ("Files")
└── VBox
    ├── MenuBar
    ├── Toolbar
    ├── PathBar
    ├── Splitter (horizontal)
    │   ├── ScrollView { TreeView }       (sidebar)
    │   └── ScrollView { IconView | MultiColumnList }   (main pane)
    └── StatusBar
```

This is the consumer that motivates each new component; it is not built in this plan.

---

## Implementation Units

### U1. Add the `Selection` model

**Goal:** Introduce a shared `Selection` type and `SelectionMode` enum that subsequent list-shaped widgets will use. No widget integration in this unit — purely additive.

**Requirements:** R9, R10, R12

**Dependencies:** None

**Files:**
- Create: `src/window/selection.rs`
- Modify: `src/window/mod.rs` (add `pub mod selection;` and re-export `Selection`, `SelectionMode`)
- Test: `src/tests/selection_tests.rs`
- Modify: `src/tests/mod.rs` (register new test module)

**Approach:**
- Define `Selection` enum: `None`, `Single(usize)`, `Multi(BTreeSet<usize>)`, `Range { anchor, end }`.
- Define `SelectionMode` enum: `Single`, `Multi`. Widgets store this and use it to decide click semantics.
- Helper methods: `is_selected`, `len`, `iter`, `clear`, `click(idx, ClickMods)`, `move_to(idx)`, `extend_to(idx)`, `arrow(direction, item_count, ClickMods)`.
- `ClickMods` is a small struct mirroring `KeyModifiers` for mouse-modifier semantics (shift, ctrl).
- Range normalization helper (so `Range { anchor: 5, end: 2 }` iterates 2..=5 correctly).

**Patterns to follow:**
- `src/window/event.rs` for module-level `pub use` style and enum shapes.
- `src/window/types.rs` for small-helper style on enum types.
- `.claude/rules/no-std.md`: use `alloc::collections::BTreeSet`, never `std::collections::HashSet`.

**Test scenarios:**
- Happy path: `Single(3)` returns `is_selected(3) == true`, `is_selected(4) == false`.
- Happy path: `Multi({1, 4, 7})` returns true for each member, false otherwise.
- Happy path: **Covers AE3 (model layer).** Click on `B` with no mods on an empty selection sets `Single(B)`. Shift-click on `D` extends to `Range { anchor: B, end: D }`. Ctrl-click on `A` after the range yields `Multi({A} ∪ {B..=D})`. Plain click on `C` collapses to `Single(C)`.
- Edge case: `Range { anchor: 5, end: 2 }` iterates indices `2, 3, 4, 5`.
- Edge case: arrow-down at the last index does not advance past `item_count - 1`; arrow-up at index 0 does not go negative.
- Edge case: clearing a `Multi` selection produces `None`.
- Edge case: `SelectionMode::Single` rejects multi/range click attempts and falls back to `Single`.
- Error path: out-of-range index in `is_selected` returns `false` (never panics).

**Verification:**
- `./test.sh` passes with the new test module registered.
- `Selection` is referenced only by future units; no existing widget changes in this commit.

---

### U2. Layout primitives (`VBox`, `HBox`, `Padding`, `Spacer`, `Fill`)

**Goal:** Add passive layout containers under `src/window/windows/layout/` that compute child bounds from the container's own bounds and propagate resize to children.

**Requirements:** R1, R2, R4

**Dependencies:** None

**Files:**
- Create: `src/window/windows/layout/mod.rs`
- Create: `src/window/windows/layout/vbox.rs`
- Create: `src/window/windows/layout/hbox.rs`
- Create: `src/window/windows/layout/padding.rs`
- Create: `src/window/windows/layout/spacer.rs` (contains `Spacer` window + `Fill` weight type)
- Modify: `src/window/windows/mod.rs` (add `pub mod layout;` and re-exports)
- Modify: `src/window/manager.rs` (add `with_window_mut(WindowId, FnMut(&mut dyn Window))` accessor used by layout containers)
- Test: `src/tests/layout_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- Each container is a `Window`-implementing struct backed by `WindowBase`.
- `VBox` and `HBox`: store an ordered list of child `WindowId`s plus per-child sizing hints (`Fixed(u32)`, `Fill(weight)`, `MinContent`).
- **Relayout is triggered by overriding `set_bounds`** — when the container's bounds change, the override calls a private `relayout(&mut self)` that walks `WindowBase::children` and writes each child's new bounds. No `Event::Resize` synthesis or manager-level resize-dispatch path is needed (per Key Technical Decisions).
- Initial `add_child(_, hint)` calls also trigger `relayout` so children get correct bounds immediately.
- `Padding` wraps a single child and shrinks the child's bounds by `(top, right, bottom, left)`.
- `Spacer` is a fixed-size empty window for explicit gaps; `Fill` is a sizing hint, not a struct.
- **Layout writes child bounds via `WindowManager::with_window_mut(child_id, |w| w.set_bounds(new_bounds))`** — the new accessor formalizes the lookup the manager already does internally for routing/focus paths. Layout containers use `set_bounds` (not `set_bounds_no_invalidate`) so children's `needs_repaint` flag flips correctly. Container does not own child storage beyond `WindowBase::children`.
- Minimum-size hints from children: layout containers respect them when total fixed + minimum exceeds available — children at minimum, surplus distributed by `Fill` weights, overflow truncates the last `Fill` child.

**Execution note:** Begin by porting one section of `src/window/dialogs/file_open.rs` to `VBox + Padding` as a smoke test before adding `HBox` polish — confirms the API shape against a real consumer.

**Technical design:**

```
VBox::new(bounds)
    .add_child(toolbar_id, SizeHint::Fixed(32))
    .add_child(splitter_id, SizeHint::Fill(1))
    .add_child(statusbar_id, SizeHint::Fixed(20))
```

**Patterns to follow:**
- `src/window/windows/container.rs` for the minimal `Window` impl shape (background fill + delegation).
- `src/window/windows/frame.rs` for content-area computation under decoration.
- `src/window/types.rs:Rect::resize_edge` for the kind of math the layout containers do internally.

**Test scenarios:**
- Happy path: `VBox` with three `Fill(1)` children of equal weight in a 300-tall container produces children of height 100 each.
- Happy path: `HBox` with one `Fixed(50)` and one `Fill(1)` child in a 200-wide container produces widths 50 and 150.
- Happy path: `Padding(top=10, right=10, bottom=10, left=10)` around a child in a 100x100 container yields a child rect of 80x80 at (10, 10).
- Happy path: **Covers AE1 (partial).** Resizing the container with three children causes each child's bounds to recompute without caller intervention (verify `set_bounds` was called on each child).
- Edge case: `VBox` with zero children does not panic.
- Edge case: `HBox` with all `Fixed` children whose sum exceeds container width — last child is clipped, no panic.
- Edge case: `Padding` with insets larger than the container produces a zero-sized child rect (not negative).
- Edge case: Mixed weight `Fill(2)` and `Fill(1)` in a 30-pixel `VBox` distribute as 20 and 10 (rounding consistent).
- Integration: container with a `Spacer` between two children leaves the gap empty in paint output (sampled pixels fall through to parent background).

**Verification:**
- A scratch test in `layout_tests.rs` constructs the layout from the file_open dialog manually and verifies all child bounds match the current hand-computed values to within ±1 pixel.
- `./test.sh` passes.
- No existing widget behavior changes (additive unit).

---

### U3. `ScrollView` wrapper

**Goal:** Add a single-child `ScrollView` that wraps any content window, draws a scrollbar, and translates child paint and event coordinates by the current scroll offset.

**Requirements:** R5, R8

**Dependencies:** U16 (uses the new `MouseEventType::Scroll { delta_x, delta_y }` shape and the `Event::EnsureVisible(Rect)` variant). U1 is not required (`ScrollView` doesn't use `Selection`); independent of U2.

**Files:**
- Create: `src/window/windows/scroll_view.rs`
- Modify: `src/window/windows/mod.rs` (export `ScrollView`)
- Test: `src/tests/scroll_view_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- `ScrollView { base: WindowBase, content: WindowId, scroll_x: i32, scroll_y: i32, h_scroll_enabled: bool, v_scroll_enabled: bool, content_size: (u32, u32), thumb_grab: Option<i32>, bg_color: Color, ... }`. `is_scroll_view()` returns `true` (overrides the default).
- Content size is supplied by the caller (via `set_content_size(w, h)`) — `ScrollView` does not introspect the child's natural size. This avoids requiring a "preferred size" method on `Window`.
- `paint` draws scrollbars when `content_size > viewport`, then sets a clip rect for the viewport, then translates the child's bounds to `(viewport.x - scroll_x, viewport.y - scroll_y, content_w, content_h)` via `set_bounds_no_invalidate` before painting the child, then restores. (This use of `set_bounds_no_invalidate` is the legitimate render-time-transform pattern — distinct from layout-driven bounds writes which use `set_bounds`.)
- `handle_event` for `MouseEventType::Scroll { delta_x, delta_y }` applies `delta_y` to `scroll_y` (and `delta_x` to `scroll_x` when `h_scroll_enabled`), clamps to `[0, content_size - viewport]`, invalidates. One unit of delta is multiplied by a `wheel_step` constant (default ~3 lines worth of pixels).
- `handle_event` for `Event::EnsureVisible(rect)` adjusts scroll offsets so the requested rect is visible inside the viewport; called by content widgets (`TextEditor` on cursor move, `TreeView`/`IconView` on selection move via keyboard).
- **Scrollbar drag — explicit grab-offset state.** On `ButtonDown` over the thumb, store `thumb_grab = Some(thumb_top - mouse.y)` (a `i32` offset). On `Move` while `thumb_grab` is `Some(grab)`, compute `new_thumb_top = mouse.y + grab`, convert to `scroll_y` via the proportional formula (`scroll_y = (new_thumb_top - track_top) * (content_h - viewport_h) / (track_h - thumb_h)`), clamp, invalidate. On `ButtonUp`, set `thumb_grab = None`. Mirrors the Splitter drag-state pattern (U10).
- Default vertical-only; horizontal off until `set_horizontal_enabled(true)`.

**Technical design:** The clip-rect approach reuses the existing `GraphicsDevice::set_clip_rect` already used by the manager during render. The translation-via-`set_bounds_no_invalidate` mirrors the trick `manager.rs::render_window_tree_with_offset` already uses for parent-child transforms.

**Patterns to follow:**
- `src/window/manager.rs` `render_window_tree_with_offset` for the "temporarily transform child bounds during render" pattern.
- `src/window/windows/list.rs:295-312` for current scrollbar drawing math (track + thumb + thumb-position formula). Lift it into `ScrollView`.
- `src/window/graphics.rs` `GraphicsDevice::set_clip_rect` for viewport clipping.

**Test scenarios:**
- Happy path: **Covers AE2 (ScrollView layer).** Viewport 100 tall, content 300 tall, scroll_y starts at 0, scrollbar thumb height proportional (~33% of track); a scroll-wheel event delivered to ScrollView updates scroll_y and the scrollbar thumb tracks.
- Happy path: scroll-wheel down event increases `scroll_y` by the configured step, clamped to `content_height - viewport_height`.
- Happy path: scroll-wheel up at scroll_y=0 leaves it at 0 (no negative).
- Happy path: scroll-wheel down at the bottom is a no-op.
- Edge case: content size smaller than viewport — no scrollbar drawn, scroll events have no effect.
- Edge case: content size exactly equals viewport — no scrollbar drawn.
- Edge case: horizontal scrolling disabled by default — h-wheel events ignored.
- Integration: child's painted output appears clipped to viewport (samples outside viewport are unchanged from background).
- Integration: child receives mouse events with positions adjusted by scroll offset (verify via a probe child that records its received events).

**Verification:**
- `./test.sh` passes.
- A scratch test composes `ScrollView { Container of size 100x300 }` in a 100x100 viewport and verifies scroll-wheel adjusts visible content.

---

### U4a. Route `MouseEventType::Scroll` to nearest enclosing `ScrollView`

**Goal:** When a `MouseEventType::Scroll { delta_x, delta_y }` event arrives at the manager, route it to the **nearest enclosing** `ScrollView` ancestor of the hit window (innermost match wins). Testable end-to-end with synthetic events; does not depend on the driver actually emitting Scroll today.

**Requirements:** Partially R6 — R6 acceptance fully lands when U4b ships and real wheel events reach the manager. U4a delivers the routing half.

**Dependencies:** U3, U16

**Files:**
- Modify: `src/window/manager.rs` (extend `route_mouse_event`)
- Test: extend `src/tests/scroll_view_tests.rs` (synthetic-event routing tests)

**Approach:**
- In `route_mouse_event`, after `topmost_at` returns the hit window, branch on event type. For `MouseEventType::Scroll`, walk the parent chain from the hit window upward via the manager's window registry, looking for a window where `is_scroll_view()` returns `true`. The first match (innermost) receives the event.
- Add `fn is_scroll_view(&self) -> bool { false }` as a default method on the `Window` trait. `ScrollView` overrides to return `true`. The bool discriminator + manager-side downcast (via the existing window registry, casting `&mut dyn Window` to `&mut ScrollView` only at the routing site) avoids coupling every widget's `impl Window` to the concrete `ScrollView` type.
- If no `ScrollView` ancestor exists, fall through to standard delivery to the hit window.
- The translated event payload preserves `delta_x`/`delta_y` from the source — the routing layer adjusts no scroll values, only the recipient.

**Patterns to follow:**
- `src/window/manager.rs:393` `topmost_at` for parent-walk style.
- `src/window/windows/menu_bar.rs` and `menu_bar_popup.rs` for examples of `Window` trait additions used as discriminators.

**Test scenarios:**
- Happy path: synthetic Scroll event with `delta_y = -3` over a child of a `ScrollView` is delivered to that `ScrollView`'s `handle_event` with `delta_y = -3` preserved.
- Happy path: Scroll event over a window with no `ScrollView` ancestor reaches the hit window normally (no special routing).
- Edge case: nested `ScrollView`s — the **innermost** (nearest enclosing) wins.
- Edge case: Scroll event over the desktop with no children at the cursor point is dropped without panic.
- Integration: harness with `Frame > ScrollView > List` and a synthetic Scroll event over the list — ScrollView's scroll offset moves; List's `handle_event` is not called.

**Verification:**
- `./test.sh` passes (synthetic-event routing tests cover U4a's whole scope).
- Manual boot in QEMU: terminal still receives keyboard / button events normally; nothing routes to a non-existent ScrollView ancestor.

---

### U4b. (Contingent follow-up) Driver Scroll-event emission

**Goal:** Make the active mouse driver actually emit `MouseEventType::Scroll { delta_x, delta_y }` events end-to-end, completing R6.

**Requirements:** R6 (final acceptance).

**Dependencies:** U4a, U16. **Contingent on driver-state verification at U4b kickoff.** May ship in a separate plan if the work is large.

**Files (provisional — verified at kickoff):**
- Modify: `src/drivers/mouse.rs` (rehabilitate IntelliMouse 4-byte protocol code currently in `mouse_old.rs`, including device-ID-based wheel-support detection and 4-byte packet handling)
- Modify: `src/drivers/virtio/input.rs` (add scroll-axis handling for the VirtIO tablet path, if applicable)
- Modify: `src/input/mod.rs` and `src/input/mouse_driver.rs` (translate driver-level wheel deltas into `MouseEventType::Scroll { delta_x, delta_y }` events)
- Test: extend `src/tests/scroll_view_tests.rs` with end-to-end coverage if feasible at the in-kernel test layer; otherwise rely on QEMU manual verification documented in the commit message.

**Approach (provisional):**
- At U4b kickoff, read `src/drivers/mouse.rs`, `src/drivers/virtio/input.rs`, and the `src/input/` pipeline to confirm what's missing. The plan currently expects: PS/2 wheel detection + 4-byte packet support is missing in `mouse.rs` (present in `mouse_old.rs`), and VirtIO scroll handling is absent.
- Decide at kickoff: if the work is small (~1 day of kernel-level driver code), include in U4b; if large (~multi-day with PS/2 protocol negotiation, VirtIO descriptor changes), spin into a separate plan and ship this plan with U4a only.
- Keep the delta scaling consistent: one wheel notch ≈ 1 unit of `delta_y`. Driver may emit fractional ticks if hardware supports it; widget-side `wheel_step` constants apply the visual scaling.

**Patterns to follow:**
- `src/drivers/mouse_old.rs` IntelliMouse code as the starting reference for PS/2 4-byte protocol.

**Test scenarios:**
- Manual: scroll the mouse wheel over a `ScrollView`-wrapped widget in QEMU and observe scroll offset changes.
- Synthetic-event tests from U4a continue to pass; this unit only adds the real-event source.

**Verification:**
- Manual QEMU verification documented in the commit message.
- If U4b ships in a separate plan, the original plan ships with R6 marked as "routing in U4a; emission deferred to U4b in [link to follow-up plan]."

---

### U5. Window-trait delegation cleanup via default methods

**Goal:** Eliminate the ~25-line `impl Window` delegation block in every widget by moving pure delegation into `Window` trait default methods, requiring widgets to implement only `base()`, `base_mut()`, `paint`, `handle_event`, and `can_focus` (where it differs from the default).

**Requirements:** R20

**Dependencies:** U1, U2, U3 must complete before U5 lands so the trait change is applied to all current and new widgets in one pass.

**Files:**
- Modify: `src/window/mod.rs` (`Window` trait — promote delegation methods to default impls; add `base()` / `base_mut()` required methods)
- Modify: every file under `src/window/windows/` (remove now-redundant delegation blocks; add `base()` / `base_mut()` accessors). Specifically: `base.rs`, `container.rs`, `desktop.rs`, `frame.rs` (with `set_focus` / `has_focus` kept as custom overrides), `text.rs`, `terminal.rs`, `label.rs`, `button.rs`, `text_input.rs`, `text_editor.rs`, `menu.rs`, `menu_bar.rs`, `menu_bar_popup.rs`, `taskbar.rs`, `scroll_view.rs` (from U3), all of `layout/*.rs` (from U2). **`list.rs` and `multi_column_list.rs` are migrated as part of U6** (folded in there to avoid leaving the file half-migrated between commits).
- Test: `src/tests/trait_delegation_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- **Pre-flight prototype.** Before starting batches, prototype the conversion on `MultiColumnList` end-to-end (it is the most event-heavy non-trivial widget) and confirm that borrow-checker accommodation in `handle_event` does not pervasively offset the line-count savings. If conflicts are pervasive, stop and fall back to the rejected `delegate_window!` macro alternative — direct field accessors avoid the reborrow tax. (See Open Questions / Deferred to Implementation.)
- **Pre-flight audit.** For every existing widget, scan the current `impl Window` block and confirm each delegation method is verbatim (no widget intentionally overrides `invalidate` to also clear a cache, `set_bounds` to also resize a buffer, etc.). Document widgets where a method must remain a custom override after cleanup. Known exceptions: `FrameWindow::set_focus` / `has_focus` — these write the frame-local `active` field that drives blue/grey title-bar coloring; they must NOT be replaced with the default `WindowBase` delegation. The audit pass catches any other intentional non-delegation we discover.
- Land in batches by widget category to keep diffs reviewable and to make boot-test verification incremental:
  1. Trait change + `Container` + `Desktop` (simplest widgets) → boot-test desktop loads.
  2. `Frame` + `Label` + `Button` (next simplest) → boot-test windowed terminal still renders. (FrameWindow keeps custom `set_focus` / `has_focus`.)
  3. `Text` + `Terminal` (interactive) → boot-test typing in terminal still works.
  4. `TextInput` + `TextEditor` (focus-bearing).
  5. `Menu` + `MenuBar` + `MenuBarPopup` + `Taskbar` (chrome).
  6. `ScrollView` + layout primitives (new from U2/U3).
- **Note:** `List` + `MultiColumnList` delegation cleanup is folded into U6 (selection/scroll migration) since U6 is already restructuring those files heavily — a separate U5 batch would leave the file in a half-migrated state between commits. U6's diff covers both the trait-cleanup and the migration in one pass.
- Each batch is a separate commit; after each, run `./test.sh` and boot in QEMU to confirm regression-free.

**Execution note:** Boot-test the desktop after each batch. The kernel test suite catches behavioral regressions at the test layer; boot-test catches regressions in the implicit Window contract (e.g., a missed delegation, a method that needs a non-default override).
**Widgets requiring custom override after cleanup (initial list — refine via the pre-flight audit):**
- `FrameWindow.set_focus(bool)` and `FrameWindow.has_focus()` — write/read frame-local `active` field that drives title-bar chrome coloring; do NOT delegate to `WindowBase` defaults.
- Any others surfaced by the audit pass before batch 1.

**Technical design:** See "Window trait shape after U5" in the High-Level Technical Design section above.

**Patterns to follow:**
- The existing default method `set_bounds_no_invalidate` (in `src/window/mod.rs`) for the default-method idiom.
- The existing `WindowBase` accessor pattern visible inside every current widget's `impl Window` block.

**Test scenarios:**
- Happy path: a widget's `id()`, `bounds()`, `visible()`, `parent()`, `children()` return values consistent with what the previous (manual delegation) code returned for the same `WindowBase` state.
- Happy path: a widget that overrides `can_focus()` returns the overridden value, not the default `false`.
- Happy path: a widget that overrides `paint` and `handle_event` runs its custom logic; defaults handle the rest.
- Edge case: invalidating a widget triggers the default `invalidate()` path correctly (sets `WindowBase::needs_repaint`).
- Edge case: `set_bounds_no_invalidate` (which already had a default) still works — verify no behavioral change.
- Integration: full desktop boot (`FrameWindow` containing `TerminalWindow`) renders identically to pre-cleanup.

**Verification:**
- `./test.sh` passes after each batch.
- QEMU boot after each batch: desktop renders, terminal accepts input, mouse moves cursor.
- Total `impl Window` line count across `src/window/windows/` drops by an order of magnitude (rough target: ~350+ lines removed).

---

### U6. Migrate `List` and `MultiColumnList` to `Selection` + `ScrollView`

**Goal:** Replace `List`'s and `MultiColumnList`'s in-widget scroll/scrollbar/selection logic with the shared `Selection` model, embed each list inside `ScrollView` for scrolling, wire shift-click/ctrl-click/arrow/shift+arrow semantics, AND apply the U5 trait-delegation cleanup to both files (folded in here so the files don't sit half-migrated between commits).

**Requirements:** R7, R9, R10, R11, R12, plus the R20 trait-cleanup portion for these two files

**Dependencies:** U1, U3, U5, U16

**Files:**
- Modify: `src/window/windows/list.rs`
- Modify: `src/window/windows/multi_column_list.rs`
- Modify: `src/window/dialogs/file_open.rs` (callback signature changes — `on_select(usize)` → `on_select(&Selection)`)
- Modify: `src/window/dialogs/file_save.rs` (same)
- Test: `src/tests/list_migration_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- Order: `List` first (single-column, simpler), then `MultiColumnList` (apply same pattern + column rendering on top).
- **Apply trait-delegation cleanup as the same diff** — remove the manual `impl Window` blocks and add `base()` / `base_mut()` accessors per U5's pattern. This folds the U5 batch-5 work into U6 because separating them would leave the file half-migrated between commits.
- Remove `selected_index: Option<usize>` from both widgets; replace with `selection: Selection` and `selection_mode: SelectionMode` (default `Single` to preserve existing single-select callsites).
- Remove `scroll_offset: usize` and per-widget scrollbar drawing; lists no longer draw scrollbars themselves. Callers wrap in `ScrollView` to get scrolling.
- Migrate `on_select(usize)` callback to `on_select(&Selection)`. For backward-compat ergonomics, keep `selected()` returning `Option<usize>` for `Single` mode.
- Wire keyboard handling through `Selection::arrow(direction, count, mods)`.
- Wire mouse handling: `Selection::click(idx, ClickMods)`. `ClickMods` is derived from `mouse_event.modifiers` (the field added in U16).
- Preserve `MultiColumnList::on_right_click(usize, Point)` contract — selection still moves to the right-clicked row, callback still receives global position.

**Technical design:** Width and height of the list's internal content is `item_count * item_height` (plus header for MCL); this is the value passed to the wrapping `ScrollView::set_content_size`.

**Patterns to follow:**
- Existing `List::handle_event` mouse/keyboard arms — same shape, just replacing the `selected_index` mutation with `Selection::click` / `Selection::arrow`.
- `MultiColumnList`'s right-click handling — preserve verbatim, including global-position semantics.

**Test scenarios:**
- Happy path: selecting an item via mouse click in single-select mode produces `Selection::Single(i)`.
- Happy path: **Covers AE3 (integration layer).** In multi-select mode: shift-click extends to range, ctrl-click toggles, plain click collapses to single (full sequence) — exercised through `List` mouse-event handling, verifying the model semantics from U1 reach the user-facing widget correctly.
- Happy path: arrow-down moves selection to next item; shift+arrow-down extends a range.
- Happy path: list with 1000 items wrapped in `ScrollView` of 200 height scrolls correctly via mouse wheel; clicking a visible row selects it correctly (coordinate translation works through ScrollView).
- Edge case: clicking past the last row in a list with 5 items in a tall viewport leaves selection unchanged.
- Edge case: arrow-down at the last item with no shift modifier does not advance.
- Edge case: switching `SelectionMode::Multi` to `Single` after a multi-selection collapses to the first selected index.
- Integration: file_open dialog still selects files correctly via the new callback signature; double-click semantics preserved (currently not implemented but should not regress).
- Integration: `MultiColumnList::on_right_click` still fires with the right-clicked row index and global position even when wrapped in `ScrollView`.
- **Covers AE2 (integration layer).** A 1000-row `MultiColumnList` in a 20-row viewport: scrollbar tracks correctly via `ScrollView`; the list widget contains no scrollbar drawing code — verifying the migration removed in-widget scroll machinery and the wrapping ScrollView delivers correct behavior end-to-end.

**Verification:**
- `./test.sh` passes.
- `file_open` dialog still functional under QEMU boot — open file, click a row, dialog responds.
- `grep` of `list.rs` and `multi_column_list.rs` for "scrollbar" finds zero matches outside comments.

---

### U7. Migrate `TextEditor` to `ScrollView`

**Goal:** Replace `TextEditor`'s internal `scroll_x` / `scroll_y` and viewport-cell caching with `ScrollView` wrapping. Cursor remains in TextEditor; viewport math is delegated.

**Requirements:** R7

**Dependencies:** U3, U5

**Files:**
- Modify: `src/window/windows/text_editor.rs`
- Modify: `src/commands/notepad/mod.rs` (if it constructs `TextEditor` directly — wrap in `ScrollView`)
- Test: `src/tests/text_editor_migration_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- Remove `scroll_x`, `scroll_y`, `visible_cols`, `visible_rows` from `TextEditor`; let it paint its full content rect (size = `lines.iter().map(|l| l.len()).max() * char_width × lines.len() * char_height`).
- Notepad app (and any other consumer) wraps `TextEditor` in a `ScrollView`.
- Cursor visibility on edit: when cursor moves, `TextEditor` emits `Event::EnsureVisible(cursor_rect)` upward through the standard event-propagation path (returns `EventResult::Propagate`). The enclosing `ScrollView` consumes this event in its `handle_event` (per U3) and adjusts scroll offset to bring the cursor into view. This avoids any need for `TextEditor` to hold a typed reference to its parent.
- Mouse coordinates in `TextEditor::handle_event` already arrive in local widget coords (post-scroll-translation, courtesy of `ScrollView`), so cursor-from-click math simplifies (no manual scroll-offset addition).

**Patterns to follow:**
- U6 list migration for the "remove internal scroll state, rely on ScrollView" pattern.
- Existing `TextEditor::ensure_cursor_visible` logic for the cursor-into-view recipe — port to emitting `Event::EnsureVisible` instead of mutating local scroll state.

**Test scenarios:**
- Happy path: typing text into a small `TextEditor` wrapped in `ScrollView` causes the viewport to scroll once the cursor reaches the edge.
- Happy path: clicking at a position in a scrolled `TextEditor` places the cursor at the correct logical row/column (not the visual one).
- Happy path: arrow-down at the last visible line scrolls the viewport down by one line.
- Edge case: empty `TextEditor` (no content) renders without panic; ScrollView shows no scrollbar.
- Edge case: a single very long line overflows the viewport horizontally — h-scroll enabled scrolls it, h-scroll disabled clips it.
- Integration: notepad app in QEMU still loads, edits, and saves files (file persistence is out of scope, just visual editing).

**Verification:**
- `./test.sh` passes.
- Notepad in QEMU: typing, cursor movement, scroll-on-cursor-motion all work.

---

### U8. Migrate dialogs (`file_open`, `file_save`, `message_box`) to layout primitives

**Goal:** Replace hand-computed pixel offsets in the three dialogs with `VBox` / `HBox` / `Padding`. Net effect: zero `content_area.x + N` arithmetic in dialog code; dialogs auto-relayout if their frame is resized.

**Requirements:** R3

**Dependencies:** U2, U6 (selection callback signature change touches `file_open`/`file_save`)

**Files:**
- Modify: `src/window/dialogs/file_open.rs`
- Modify: `src/window/dialogs/file_save.rs`
- Modify: `src/window/dialogs/message_box.rs`

**Approach:**
- Replace each dialog's manual layout block with a `VBox` rooted at the frame's `content_area()`, populated with `Padding`-wrapped children.
- Buttons sit in an `HBox` aligned to the bottom (use `Spacer` with `Fill(1)` weight before the OK/Cancel buttons to right-align).
- Verify acceptance examples still pass:
  - `file_open` lists files and double-click (or row-click + Open button) opens.
  - `file_save` accepts a path and saves.
  - `message_box` shows a message + OK/Cancel and returns the right `DialogResult`.

**Test scenarios:**
- Happy path: **Covers AE1.** `file_open` source contains zero `content_area.x + N` arithmetic; opening the dialog shows the same visual layout as today.
- Happy path: shrinking the dialog frame in code (`set_bounds`) causes path label, list, and buttons to relayout without dialog-side handling.
- Happy path: `file_save` "Save" button still enables when filename is non-empty.
- Edge case: a dialog with very long file names doesn't visually overflow into the buttons row.
- Integration: `dialog::get_dialog_result` still returns the correct `DialogResult` after each dialog flow.

**Verification:**
- `./test.sh` passes.
- Each dialog opens correctly in QEMU and produces the same `DialogResult` flow as today.
- `git diff` shows the file_open net line count is meaningfully smaller (target: roughly half the layout-related ceremony, per Success Criterion).

---

### U9. `TreeView`

**Goal:** Hierarchical-list widget with expand/collapse, the unified `Selection` model, keyboard nav, and `ScrollView` integration.

**Requirements:** R13

**Dependencies:** U1, U3, U5

**Files:**
- Create: `src/window/windows/tree_view.rs`
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/tree_view_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- Internal node model: a flat `Vec<TreeNode>` with `depth: usize`, `expanded: bool`, `has_children: bool`, `label: String`. Rendered as a flat list with indentation by `depth * indent_px`.
- `TreeView` exposes `add_node(parent: Option<NodeId>, label) -> NodeId`, `expand(NodeId)`, `collapse(NodeId)`, `set_root_paths(...)` (for File Manager, the consumer maps filesystem entries to nodes).
- Visible-row computation walks the flat list, skipping subtrees of collapsed nodes.
- Selection uses `Selection::Single` by default. Multi-select opt-in via `set_selection_mode(Multi)`.
- Keyboard:
  - Up/Down: move selection through visible rows.
  - Right on collapsed: expand. Right on expanded leaf or already-expanded: move to first child.
  - Left on expanded: collapse. Left on collapsed/leaf: move to parent.
  - Enter: fire `on_activate(NodeId)` callback.
- Paint: each visible row is `[indent][disclosure-triangle ▶/▼][label]`. **Pinned constants:** `INDENT_PX = 16` per depth level; disclosure-triangle hit-zone = `16 × 16` square at `x = depth * INDENT_PX` (the row-relative x for hit-testing the triangle). Label hit-zone is `[x = depth * INDENT_PX + 16, x = row_width)`. Disclosure triangle rendered as a small filled triangle via `draw_line` calls inside the 16×16 cell.
- Hit-test: `handle_event` for `MouseEventType::ButtonDown` checks the cursor's row-local x. If `x < depth * INDENT_PX + 16`, treat as a click on the disclosure triangle (toggle expand/collapse, do not change selection). Otherwise, treat as a row label click (select the row, no toggle).

**Technical design:** A flat `Vec<TreeNode>` plus a `visible_rows: Vec<usize>` cache (indices into the flat vec, recomputed on expand/collapse) is simpler and faster for paint than a recursive tree walk.

**Patterns to follow:**
- `src/window/windows/list.rs` for selection + paint flow under `ScrollView`.
- `src/window/windows/menu.rs` for hover/click-row-detection style.

**Test scenarios:**
- Happy path: **Covers AE4.** Right on a collapsed node expands it; Left on an expanded node collapses it; Left on a collapsed node moves selection to parent.
- Happy path: clicking a disclosure triangle expands/collapses without changing selection.
- Happy path: clicking a row label selects the row.
- Edge case: tree with a single node (no children) — Right on it does nothing; Left does nothing.
- Edge case: collapsing a node with the selection inside it moves selection to the collapsed parent.
- Edge case: deeply nested tree (5+ levels) renders with correct indentation and doesn't overflow horizontally (relies on h-scroll if needed).
- Integration: `ScrollView { TreeView }` of 100 nodes scrolls correctly; clicking a scrolled-into-view node selects the right node.

**Verification:**
- `./test.sh` passes.
- A scratch demo composes a small tree in QEMU and verifies expand/collapse + selection visually.

---

### U10. `Splitter`

**Goal:** Two-pane container (vertical or horizontal) with a draggable divider and minimum-pane-size enforcement.

**Requirements:** R14

**Dependencies:** U2 (consistent layout semantics), U5

**Files:**
- Create: `src/window/windows/splitter.rs`
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/splitter_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- `Splitter::new_horizontal(bounds)` and `Splitter::new_vertical(bounds)` (orientation static at construction).
- API: `set_first(WindowId, min_size: u32)`, `set_second(WindowId, min_size: u32)`, `set_divider_position(u32)`.
- Divider is a 4-pixel-wide draggable strip painted between panes.
- Mouse interaction: cursor over divider sets `pressed` on `ButtonDown`; `Move` while pressed updates `divider_position`, clamped by both panes' minimum sizes; `ButtonUp` releases.
- On `set_bounds` (Splitter resized): keep divider at its current relative ratio of available space (`new_position = old_ratio * new_total`), respecting minimums.
- **Children are positioned via `set_bounds`** (NOT `set_bounds_no_invalidate`) — Splitter owns layout like `VBox`/`HBox`. Divider drag is a permanent bounds change, so children must invalidate to repaint correctly. Layout containers reserve `set_bounds_no_invalidate` for the render-time-transform pattern in `manager.rs`. Splitter uses the same `WindowManager::with_window_mut` accessor as the layout primitives in U2.

**Technical design:** Conceptually similar to a special-cased `HBox` / `VBox` with two children and a draggable seam.

**Patterns to follow:**
- `src/window/types.rs` `clamp_drag_x`/`clamp_drag_y` for the drag-clamping idiom.
- `src/window/windows/frame.rs` drag-state handling for window dragging — same pattern with mouse-down/move/up.

**Test scenarios:**
- Happy path: vertical splitter with two panes splits container 50/50 by default.
- Happy path: dragging the divider right grows the left pane and shrinks the right.
- Happy path: **Covers AE5.** Both panes have a 200px minimum; dragging past the minimum stops at the minimum and does not occlude either pane.
- Edge case: container too small to honor both minimums — divider centers, both panes at minimum, content overflows (layout doesn't enforce truncation).
- Edge case: resizing the splitter container preserves divider ratio.
- Integration: `Splitter { TreeView, IconView }` (the File Manager target composition) renders both panes correctly.

**Verification:**
- `./test.sh` passes.
- A scratch demo in QEMU drags the divider and confirms minimums hold.

---

### U11. `Toolbar` and `StatusBar`

**Goal:** Two thin compositions over `HBox`: `Toolbar` for icon-or-text command buttons, `StatusBar` for status text with consistent padding.

**Requirements:** R15, R16

**Dependencies:** U2

**Files:**
- Create: `src/window/windows/toolbar.rs`
- Create: `src/window/windows/status_bar.rs`
- Modify: `src/window/windows/button.rs` (extend with `set_enabled(bool)` and a greyed-out paint state)
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/toolbar_status_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- **Extend `Button` with a disabled state.** Add `enabled: bool` field (default `true`), `set_enabled(bool)`, and a greyed-out paint state (lighter background, mid-grey label). When `enabled == false`, `handle_event` ignores `ButtonDown` / `ButtonUp` so the click callback never fires. Required because Toolbar consumers (File Manager Back/Forward/Up) need contextual disable.
- `Toolbar`: `HBox`-backed; `add_button(label, on_click)` constructs a `Button` with consistent size and padding, appends as fixed-size child, and returns the button's `WindowId` so callers can later toggle its enabled state. `add_separator()` adds a small spacer + 1-pixel vertical line.
- `StatusBar`: `HBox`-backed with bottom-anchored 20-pixel-tall row; `add_section(label, weight)` adds a `Label` with the given weight. Default style: light grey background, dark text.
- Both expose `bounds()`-based positioning that consumers layer into a parent `VBox` via `Fixed(N)` weight.

**Patterns to follow:**
- `src/window/windows/menu_bar.rs` for the "horizontal strip with sections" style.
- `src/window/windows/taskbar.rs` for the "bottom-anchored bar" style.

**Test scenarios:**
- Happy path: `Toolbar` with 3 buttons renders them left-aligned with consistent spacing.
- Happy path: clicking a toolbar button fires its `on_click` callback.
- Happy path: a `Button` with `set_enabled(false)` paints in the greyed-out state and ignores `ButtonDown` / `ButtonUp` events; the click callback does not fire.
- Happy path: re-enabling a previously-disabled button restores the normal paint state and click handling.
- Happy path: `StatusBar` with one `Label` fills the full width.
- Happy path: `StatusBar` with two `Label`s of weight 1 each splits the width 50/50.
- Edge case: empty `Toolbar` paints just the background.
- Edge case: `StatusBar` text wider than its section truncates (no overflow into adjacent sections).
- Integration: `VBox { Toolbar(fixed 32), Splitter(fill 1), StatusBar(fixed 20) }` lays out as expected when the container is 600 tall.

**Verification:**
- `./test.sh` passes.

---

### U12. `PathBar`

**Goal:** Clickable breadcrumb bar where each path segment is a separate clickable region.

**Requirements:** R17

**Dependencies:** U5

**Files:**
- Create: `src/window/windows/path_bar.rs`
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/path_bar_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- `PathBar::new(bounds)` + `set_path("/host/Documents/Projects")`.
- Internally: split path on `/`, compute display widths per segment (segment text + `>` separator), determine which segments fit. If overflow, replace leading segments with "..." and keep the rightmost `n` that fit.
- Hit-test on click: segment-x boundaries determined from layout; clicking a segment fires `on_segment_click(truncated_path)` where `truncated_path` is the path up through (and including) the clicked segment.
- **Overflow "..." segment is inert for v1** — visual indicator only, not clickable, no callback. (A future overflow-menu reveal can be added if PathBar overflow becomes common in practice; not in this plan.)
- Paint: each segment as text with hover state (background highlight on mouse-over). The "..." token paints with the same text color but never enters the hover state.

**Patterns to follow:**
- `src/window/windows/menu_bar.rs` for "horizontal segments with hit-testing and hover".

**Test scenarios:**
- Happy path: `set_path("/a/b/c")` and clicking `b` fires callback with `"/a/b"`.
- Happy path: hovering over a segment highlights it.
- Edge case: `set_path("/")` — single segment "/", clickable, callback receives "/".
- Edge case: very long path that overflows — leading segments collapse to "...", rightmost segments remain clickable; clicking the "..." token does not fire the callback (inert).
- Edge case: empty path — paints background only, no clickable segments.
- Edge case: path with trailing slash (`"/a/b/"`) treated identically to `"/a/b"`.

**Verification:**
- `./test.sh` passes.

---

### U13. `IconView`

**Goal:** Finder-style grid of icon+label tiles with the unified `Selection` model and `ScrollView` integration.

**Requirements:** R18

**Dependencies:** U1, U3, U5

**Files:**
- Create: `src/window/windows/icon_view.rs`
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/icon_view_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- `IconView::new(bounds)`, `set_tile_size(width, height)`, `add_tile(label, icon: Option<&[u8]>)` (icon bytes are placeholder for v1 — see Open Questions; rendered as a colored square if absent).
- Layout: tiles flow left-to-right in rows, wrapping to the next row when the next tile would exceed the viewport width.
- Selection uses `Selection`. Multi-select keyboard nav: arrow keys move by tile; shift+arrow extends; ctrl-click/shift-click for mouse.
- Click-detection: convert mouse position to tile index via `(y / tile_h) * tiles_per_row + (x / tile_w)`.
- **Arrow-key boundary behavior — clamp, no wrap-around** (consistent with TreeView and `Selection::arrow`):
  - Arrow-right at the very last tile (last row, last column): no movement.
  - Arrow-right at the end of a non-last row: move to first tile of next row.
  - Arrow-left at the first tile (row 0, column 0): no movement.
  - Arrow-left at the start of a non-first row: move to last tile of previous row.
  - Arrow-down at the last row: no movement (clamps even if a column shorter than `tiles_per_row` exists in the last row).
  - Arrow-up at the first row: no movement.
- Rubber-band selection (drag-to-multi-select) is **deferred** — not in this unit. Document explicitly.

**Patterns to follow:**
- `src/window/windows/list.rs` post-U6 for `Selection` integration.
- `src/window/windows/multi_column_list.rs` for hover/click row math, adapted for 2D.

**Test scenarios:**
- Happy path: 10 tiles in a 5-tile-wide viewport render in 2 rows.
- Happy path: clicking tile (row 1, col 2) selects index 7 (0-indexed: row*tiles_per_row + col).
- Happy path: shift-click extends selection.
- Happy path: arrow-right at end of row wraps to first tile of next row.
- Happy path: arrow-down moves down one row (same column).
- Edge case: arrow-right at the very last tile clamps (no movement).
- Edge case: arrow-left at the very first tile clamps (no movement).
- Edge case: arrow-down at the last row clamps even if the last row has fewer tiles than `tiles_per_row`.
- Edge case: arrow-up at the first row clamps.
- Edge case: viewport narrower than one tile — single column, vertical-only.
- Edge case: zero tiles — paints background.
- Integration: 100 tiles in `ScrollView` — scroll wheel moves the viewport, click-position math adjusts for scroll correctly.

**Verification:**
- `./test.sh` passes.

---

### U14. `ProgressBar`

**Goal:** Determinate progress widget with current/total values and an optional label.

**Requirements:** R19

**Dependencies:** U5

**Files:**
- Create: `src/window/windows/progress_bar.rs`
- Modify: `src/window/windows/mod.rs`
- Test: `src/tests/progress_bar_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- `ProgressBar::new(bounds)` + `set_progress(current: u64, total: u64)` + `set_label(Option<String>)`.
- Paint: outline rect + filled inner rect of width `(current / total) * inner_width`. Optional label overlay (centered) e.g., "47% — Copying file_42.txt".
- No interaction; ignores all events.
- **Composition is caller-driven.** ProgressBar exposes only `bounds()`-based positioning; embedding inside a dialog (e.g., a future Copy Progress dialog) or inside `StatusBar` is done by adding the ProgressBar as a child of the relevant container (e.g., `VBox::add_child(progress_id, SizeHint::Fixed(20))`). No special embedding API is required.

**Patterns to follow:**
- `src/window/windows/list.rs` scrollbar-thumb paint math (proportional fill).

**Test scenarios:**
- Happy path: `set_progress(50, 100)` paints fill at half the inner width.
- Edge case: `set_progress(0, 100)` paints empty bar with outline only.
- Edge case: `set_progress(100, 100)` paints full bar.
- Edge case: `set_progress(50, 0)` — divide-by-zero — treated as empty (no panic).
- Edge case: `current > total` clamps to 100% (no overflow).

**Verification:**
- `./test.sh` passes.

---

### U15. Reconcile color and styling defaults

**Goal:** Walk all components and adjust default `bg_color`, `text_color`, `border_color` so default-styled widgets in the same window look consistent. Out of scope: a configurable theming system.

**Requirements:** R21

**Dependencies:** U1-U14 (touches all components, easier when they're all migrated)

**Files:**
- Modify: every widget's `Default::default()`-equivalent constructor color values across `src/window/windows/`.

**Approach:**
- Survey current defaults (already done during planning — see Problem Frame): `Container` 240/240/240, `List` `Color::WHITE`, `Frame` chrome 0/100/200, etc.
- Pick a small palette that matches the "Windows-95-ish" aesthetic the project leans toward:
  - Window chrome: 0/100/200 (active), 100/100/100 (inactive) — keep as-is.
  - Content background: 240/240/240 (consistent across `Container`, `List`, `MultiColumnList`, `TreeView`, `IconView`).
  - Borders: 100/100/100.
  - Highlight (selection): 0/120/215.
  - Text: BLACK on light backgrounds, WHITE on selection highlight.
- Apply across all widgets. Verify dialogs and apps still look coherent in QEMU.
- Document the palette as a comment block at the top of `src/window/mod.rs` adjacent to existing constants. **Do NOT introduce a new `src/window/style.rs` module** — R21 reconciles defaults; a configurable theming system is explicitly out of scope, and a documentation-only module that no widget imports adds maintenance overhead without consumer value.

---

### U16. Extend `MouseEvent`, `MouseEventType::Scroll`, and `Event` for the new payloads

**Goal:** Three Event-type changes landed together:
1. Add `modifiers: KeyModifiers` to the `MouseEvent` struct.
2. Change `MouseEventType::Scroll` to `Scroll { delta_x: i32, delta_y: i32 }`.
3. Add an `Event::EnsureVisible(Rect)` variant for child-to-`ScrollView` upward-event coupling (consumed by `TextEditor`, `TreeView`, `IconView`).

Update every site that constructs or pattern-matches `MouseEvent` / `Event` — input pipeline, manager dispatch, and every widget's `handle_event` arms — so the changes compile cleanly. This is a foundational event-type change that U3, U4a, U6, and U7 all depend on; it's sequenced before U3 and otherwise independent of U5's trait work.

**Requirements:** Foundation for R5/R8 (Scroll delta payload), R10 (mouse modifier semantics in selection), R7 (TextEditor migration via `EnsureVisible`).

**Dependencies:** None (touches event types and call sites, not the Window trait or widget structure). Sequenced before U6, U3's scroll-event tests, and U4a.

**Files:**
- Modify: `src/window/event.rs` — add `modifiers: KeyModifiers` field to `MouseEvent`; change `MouseEventType::Scroll` to `Scroll { delta_x: i32, delta_y: i32 }`; add `Event::EnsureVisible(Rect)` variant
- Modify: `src/input/mod.rs` — fuse `KeyboardState` modifier state into emitted mouse events
- Modify: `src/input/mouse_driver.rs` — pass through new fields when constructing `MouseEvent`
- Modify: `src/window/manager.rs` — `route_mouse_event` and the local-coord recompute preserve `modifiers` and the new `Scroll` payload shape
- Modify: every widget under `src/window/windows/` whose `handle_event` pattern-matches on `MouseEvent` or `MouseEventType::Scroll` — at minimum `list.rs`, `multi_column_list.rs`, `text_editor.rs`, `text_input.rs`, `button.rs`, `menu.rs`, `menu_bar.rs`, `menu_bar_popup.rs`, `taskbar.rs`, `frame.rs`. Most call sites only need to recognize the new payload shape; widget logic in this unit is purely "compile-cleanly with the new types," not behavior changes.
- Test: `src/tests/mouse_event_extension_tests.rs`
- Modify: `src/tests/mod.rs`

**Approach:**
- Change the `Scroll` variant first (smaller blast radius — only the few sites that pattern-match `Scroll` need updating; most match `Move` / `ButtonDown` / `ButtonUp`).
- Then add the `modifiers` field. The field's default value is `KeyModifiers::default()` (all false) — call sites that don't yet care can pass it as a default. The input pipeline tracks keyboard modifier state (already maintained for keyboard events) and snapshots it into every emitted mouse event.
- Manager-side `route_mouse_event` constructs translated mouse events for child delivery; copy `modifiers` and the full `Scroll` payload through unchanged.
- No widget behavior changes in this unit — widgets that will read `modifiers` (post-U6) just receive the field with zeros until U6 wires it through. Test coverage in this unit is "compile cleanly + carry the new payloads through dispatch."

**Patterns to follow:**
- Existing `KeyboardEvent.modifiers: KeyModifiers` field for the modifier-snapshotting style (`src/window/event.rs:64`).
- Existing `MouseEvent` callsites for the construction / pattern-match shape.

**Test scenarios:**
- Happy path: a synthetic `Scroll { delta_x: 0, delta_y: -3 }` is constructed and pattern-matched cleanly.
- Happy path: a synthetic `MouseEvent` with `modifiers.shift = true` flows through `WindowManager::route_mouse_event` to a target window's `handle_event` with `modifiers.shift == true` preserved.
- Happy path: the input pipeline emits a mouse event with `modifiers` reflecting the `KeyboardState` at the time of the event.
- Edge case: a mouse event constructed with `KeyModifiers::default()` (no modifiers) carries `shift == false`, `ctrl == false`, `alt == false`, `meta == false`.
- Edge case: a `Scroll { delta_x: i32::MAX, delta_y: i32::MIN }` event does not panic when constructed or pattern-matched (boundary value test).
- Integration: every existing mouse-handling widget continues to work — no widget regresses on click/move/button-up handling because of the struct change. Verified by `./test.sh` passing and QEMU boot showing the desktop, terminal input, and cursor movement function normally.

**Verification:**
- `cargo check --features test` and `./test.sh` both pass after the change.
- QEMU boot in normal and test mode continues to show the desktop, terminal input, mouse cursor, and clickable frame chrome.
- `grep` for `MouseEventType::Scroll` shows every match using the new `Scroll { delta_x, delta_y }` form.

**Test scenarios:**

Test expectation: none — pure visual adjustment with no behavioral change. Verification is QEMU boot + visual review.

**Verification:**
- QEMU boot: desktop, terminal, all dialogs, all new components in scratch demos look consistent.
- No widget shows a visually jarring color clash with its neighbors at default settings.

---

## System-Wide Impact

- **Interaction graph.** `WindowManager::route_mouse_event` gains a Scroll-routing branch (U4a). The input pipeline (`src/input/mod.rs`) gains a modifier-state fuse for mouse events (U16). `WindowManager` exposes a new `with_window_mut` accessor used by layout containers for child positioning (U2). Existing `route_*` paths for keyboard, button, and move events otherwise unchanged. `Window` trait method count drops (U5) — every existing widget's `impl Window` block changes shape but external behavior is preserved.
- **Error propagation.** No new panics introduced. Out-of-range indices in `Selection`, divide-by-zero in `ProgressBar`, oversized `Padding` insets, and zero-size containers all produce safe no-op behavior. Validates against the kernel-context "panic in interrupt context is fatal" rule from `.claude/rules/no-std.md` (none of this code runs in interrupt context).
- **State lifecycle risks.** None of the new components own persistent state outside `WindowBase`. `Selection` stores only indices, not item data. `ScrollView` tracks scroll offset. No cache-coherency or duplicate-state concerns.
- **API surface parity.** `List::on_select(usize)` → `List::on_select(&Selection)` is a breaking change for callers — but the only caller of `List` is the `file_open`/`file_save` dialogs and they are migrated in U8. `MultiColumnList::on_right_click(usize, Point)` is preserved verbatim. `TextEditor` no longer takes responsibility for scroll math — callers must wrap in `ScrollView`; the only caller is `notepad`, migrated in U7. **`MouseEvent` struct gains a `modifiers: KeyModifiers` field** (U16) — every site constructing or pattern-matching `MouseEvent` is updated; this is enumerated in U16's blast-radius scope. **`MouseEventType::Scroll` becomes `Scroll { delta_x: i32, delta_y: i32 }`** (U16, sequenced together with the modifiers extension since both touch the same event-type module). **`Button` gains `set_enabled(bool)` and a greyed-out paint state** (U11) for Toolbar contextual disable.
- **Integration coverage.** Boot-test the desktop after each U5 batch. After U6, exercise file_open dialog. After U7, exercise notepad. After U13, prototype the File Manager skeleton in a scratch test (do not commit; just verify component composition works end-to-end).
- **Unchanged invariants.**
  - `WindowBase` struct stays — new code composes on top.
  - `WindowId`, `Rect`, `Point`, `ColorDepth` types stay.
  - `Event` enum gains one new variant: `EnsureVisible(Rect)` (used by `TextEditor` / `TreeView` / `IconView` to request scroll-into-view from an enclosing `ScrollView`). All existing variants and their payloads stay.
  - `MultiColumnList::on_right_click` signature stays.
  - `dialog::DialogResult` and the single-slot dialog state machinery stay.
  - The desktop's boot composition (`DesktopWindow → FrameWindow → TerminalWindow`) is preserved; U5 changes the trait wiring but not the visual output.
  - `core_font::get_default_font()` remains the universal text source — no per-widget custom fonts.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| U5 (trait cleanup) breaks the implicit `Window` contract for some widget — silent regression | Land in batches; boot-test desktop after each batch; per-widget audit before U5 starts to confirm each widget's existing delegation is verbatim (no widgets that intentionally override delegation methods to add side effects). Explicit `trait_delegation_tests.rs` exercising each widget category. |
| U5 default-method delegation creates pervasive borrow-checker conflicts in `handle_event` bodies that hold simultaneous borrows on widget fields | Prototype the conversion on `MultiColumnList` (event-heavy) before committing to all 17 widgets; if conflicts are pervasive, fall back to the rejected `delegate_window!` macro alternative. Captured as a Deferred-to-Implementation item. |
| U4b mouse-wheel driver work expands beyond initial estimate (PS/2 IntelliMouse 4-byte protocol + VirtIO scroll plumbing) | U4a delivers in-scope event-routing tested with synthetic events; U4b is contingent and may ship in a separate plan if driver work is non-trivial. R6 acceptance moves to U4b. |
| Selection-callback signature change breaks consumers we don't know about | `grep` for `on_select(` callers before U6 lands; the known callers are dialog files. |
| `TreeView` performance with deep trees during expand/collapse if implemented as a recursive walk | Use the flat `Vec<TreeNode>` + `visible_rows: Vec<usize>` cache approach (documented in U9). |
| File-Manager-driven design pulls in too much speculatively (e.g., `IconView` rubber-band selection) | Explicit "rubber-band deferred" note in U13; revisit when File Manager actually exposes the need. |
| Color reconciliation in U15 changes user-visible appearance of existing apps in unwanted ways | Land U15 last; visual review in QEMU; revert specific defaults if they regress an existing app's look. |
| Unit count (16, plus contingent U4b) implies a long-running plan branch with merge risk | Each unit is independently committable; batches in U5 can be PR'd separately; nothing in this plan requires a single-mega-commit. |

---

## Documentation / Operational Notes

- **Update `src/window/CLAUDE.md`** at the end of the plan: add new components to the Window-types table; mention the `selection.rs` model; note the `layout/` subdirectory; remove the line claiming `windows/` only contains the older set (the existing CLAUDE.md is already stale — this plan corrects that).
- **Optionally add `src/window/windows/CLAUDE.md`** if cumulative context across the new windows benefits from a folder file (not gated by U15 anymore — U15 no longer adds a `style.rs` module).
- **No external docs** affected — this is internal kernel work.
- **No rollout/monitoring** — kernel boots clean or doesn't.

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-05-08-window-components-improvements-requirements.md`
- Window trait & event types: `src/window/mod.rs`, `src/window/event.rs`, `src/window/types.rs`
- Existing widgets: `src/window/windows/*.rs`
- Existing dialogs: `src/window/dialogs/*.rs`
- Kernel rules: `.claude/rules/no-std.md`, `.claude/rules/panic-and-attributes.md`, `.claude/rules/testing-flow.md`
- Test framework: `src/tests/CLAUDE.md`
- Design docs: `docs/window_system_design.md`, `docs/shell_window_integration.md`
