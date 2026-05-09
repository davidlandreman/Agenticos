---
date: 2026-05-08
topic: window-components-improvements
---

# Window Components Library: Improvements & File-Manager Readiness

## Summary

Extract three high-leverage abstractions from the existing `src/window/windows/` library — layout primitives, a shared scrollable container, and a unified selection model — and add the net-new components (TreeView, Splitter, Toolbar, StatusBar, PathBar, IconView, ProgressBar) that an upcoming File Manager / Finder / Explorer app will require. Defer an Action/Command abstraction and any dedicated Window-trait macro pass.

---

## Problem Frame

The `windows/` folder has accumulated 17 components — `Container`, `Frame`, `Desktop`, `Text`, `Terminal`, `Label`, `Button`, `TextInput`, `TextEditor`, `List`, `MultiColumnList`, `Menu`, `MenuBar`, `MenuBarPopup`, `Taskbar`, `Dialog` (state), plus `WindowBase` — and three dialog consumers (`file_open`, `file_save`, `message_box`).

In practice, three pain points have surfaced:

1. **Consumers do their own pixel math.** `file_open.rs` is the canonical example: every child window is positioned via hand-computed offsets inside the frame's `content_area()`. The same arithmetic recurs across dialogs and apps. Resize-on-parent-change is impossible without re-running that arithmetic by hand.
2. **`List` and `MultiColumnList` reimplement the same primitives.** Both own selection state, scroll offset, scrollbar drawing, and arrow-key navigation. `TextEditor` has its own copy of scroll math. Future `TreeView` and `IconView` would extend the duplication.
3. **The selection model can't express what File Manager needs.** Both list widgets store `Option<usize>` — single-select only. File Manager users expect shift-click ranges and ctrl-click toggles for multi-select.

The next planned app is a File Manager. It needs widgets that don't exist (TreeView for the sidebar, Splitter for the sidebar/main divide, IconView for grid mode, PathBar for the address area, Toolbar/StatusBar for chrome, ProgressBar for copy progress) and selection semantics no current widget supports. Building it without first extracting shared abstractions would either lock in another wave of duplication or block on hand-rolled custom widgets per app.

---

## Requirements

**Layout primitives (Pillar 1)**

- R1. Provide passive layout containers — at minimum `VBox`, `HBox`, `Padding`, `Spacer`, and a flex/`Fill` modifier — that compose other windows and compute child bounds from their own bounds.
- R2. Layout containers propagate resize: when the container's bounds change, layout-managed children's bounds are recomputed without caller intervention.
- R3. Existing dialogs (`file_open`, `file_save`, `message_box`) are migrated to use layout primitives instead of hand-computed pixel offsets, so the new system is regression-tested against current callsites.
- R4. Layout containers respect minimum-size hints from children when supplied. When content exceeds available space, the layout itself does not enforce truncation policy — that is per-widget (clip vs. ellipsize vs. scroll).

**Shared scrollable container (Pillar 2a)**

- R5. Provide a `ScrollView`-style wrapper that takes a content window, draws a scrollbar when the content is larger than the viewport, and translates child paint and event coordinates by the current scroll offset.
- R6. Mouse-wheel events scroll the topmost ScrollView under the cursor. (Today `List::handle_event` has a stubbed `MouseEventType::Scroll` arm — wire it through.)
- R7. `List`, `MultiColumnList`, and `TextEditor` migrate to the shared ScrollView, removing in-widget scrollbar drawing and scroll-offset bookkeeping.
- R8. ScrollView supports vertical scrolling by default; horizontal scrolling is opt-in.

**Unified selection model (Pillar 2b)**

- R9. A shared `Selection` type covers: none, single index, multi (set of indices), and contiguous range. It replaces the current `Option<usize>` in `List` and `MultiColumnList` and is reused by all later list-shaped widgets.
- R10. Mouse and keyboard selection semantics are shared: shift-click extends a range from the anchor; ctrl/cmd-click toggles individual items; arrow keys move the selection; shift+arrow extends. These behaviors live in one place, not in each widget.
- R11. Selection callbacks fire with the full `Selection` (not just an index), so consumers can react to range and multi-select changes.
- R12. Single-select-only consumers remain ergonomic: opting out of multi-select is a one-line configuration on the widget, not boilerplate at the callsite.

**Net-new components (File-Manager-driven)**

- R13. `TreeView` — hierarchical list with expand/collapse nodes, the unified Selection model, and keyboard nav (arrows; Left to collapse / move to parent; Right to expand / move to first child). Sits on top of ScrollView.
- R14. `Splitter` — two-pane container (vertical or horizontal split) with a draggable divider and minimum-pane-size constraints. Works with any two child windows.
- R15. `Toolbar` — horizontal strip of icon-or-text buttons hosting frequent commands. Thin composition of `Button` + `HBox` with consistent spacing/styling.
- R16. `StatusBar` — thin horizontal strip at the bottom of a frame for status text and counts. Thin composition of `Label` + `HBox`.
- R17. `PathBar` (breadcrumb bar) — clickable path-segment widget. Clicking a segment fires a callback with the truncated path; collapses into a "..." overflow when the path is too long for the available width.
- R18. `IconView` — Finder-style grid of icon+label tiles. Uses the unified Selection model and ScrollView. Supports configurable tile size.
- R19. `ProgressBar` — determinate progress widget with current/total values and an optional label. Embeddable inside dialogs (e.g., a future Copy Progress dialog) and inside `StatusBar`.

**Cleanup riding along**

- R20. Window-trait delegation boilerplate (the ~25-line `impl Window` block that delegates verbatim to `WindowBase` in every widget) is eliminated. Mechanism — macro, default trait methods, or a thin composition wrapper — is a planning-time decision; the requirement is that the boilerplate goes away as part of the same diffs that integrate widgets with the new layout system.
- R21. Color and styling defaults are reconciled across components so default-styled widgets in the same window look consistent (e.g., a default `Container` background should not clash with a default `List` background). Out of scope: a configurable theming system.

---

## Acceptance Examples

- AE1. **Covers R1, R2, R3.** Given the current `file_open` dialog, when migrated to layout primitives, the dialog source contains zero hand-computed child offsets (no `content_area.x + 10`-style arithmetic); resizing the frame causes the path label, file list, and buttons to relayout without any code in the dialog reacting to the resize.
- AE2. **Covers R5, R7.** Given a `MultiColumnList` with 1000 rows in a viewport that fits 20, when scrolled via mouse wheel, the scrollbar thumb tracks correctly and the list widget itself contains no scrollbar drawing code.
- AE3. **Covers R9, R10.** Given a multi-select-enabled `List` with items `[A, B, C, D, E]` and selection on `B`, when the user shift-clicks `D`, the selection becomes the contiguous range `B..=D`; when the user then ctrl-clicks `A`, `A` is added to the selection without clearing the range; when the user clicks `C` without modifiers, the selection collapses to single `C`.
- AE4. **Covers R13.** Given a `TreeView` displaying `/host` with collapsed subdirectories, when the user presses Right on a collapsed node, the node expands; pressing Left on an expanded node collapses it; pressing Left on a collapsed (or leaf) node moves selection to the parent.
- AE5. **Covers R14.** Given a `Splitter` with `TreeView` left and `IconView` right and a 200-pixel minimum on each pane, when the user drags the divider past the minimum, the divider stops at the minimum and does not occlude either pane.

---

## Success Criteria

- The File Manager can be assembled primarily by composing existing components: only File-Manager-specific business logic (filesystem traversal, copy/delete operations, path handling) lives in the app code. No new layout math, no new selection logic, no new scrollbar drawing.
- A new dialog comparable to `file_open` is written in materially fewer lines than today (target: roughly half the non-callback ceremony). A reader unfamiliar with the codebase can read a dialog source top-to-bottom and understand the visual structure without consulting `Rect` arithmetic.
- `List` and `MultiColumnList` no longer reimplement scrolling, scrollbar drawing, or selection state. Both delegate to the shared core.
- A widget file's `impl Window` block is short enough to read without scrolling — the widget's interesting code is no longer buried under trait delegation.
- `ce-plan` can take this document and design implementation without inventing new product behavior, selection semantics, or which components File Manager needs.

---

## Scope Boundaries

- **Action / Command abstraction** (cross-cutting menubar / toolbar / context-menu / shortcut binding to one named action). Deferred until the File Manager exists and reveals what an `Action` actually needs to be — premature design risks the wrong shape.
- **Drag-and-drop between widgets** (e.g., dragging a file from one IconView to another). Deferred. File Manager v1 can use cut/paste via right-click menu.
- **Clipboard system** (cross-app copy/paste of arbitrary data). Out of scope; clipboard is its own subsystem.
- **Full theming system** (color schemes, dark mode, runtime theme switching). Out of scope. R21 reconciles defaults only.
- **Dialog-system overhaul** (modal stacking, dialog-builder API, multi-modal at once). Out of scope; the current single-slot global dialog state is sufficient for File Manager v1.
- **Accessibility primitives** (screen reader hooks, high-contrast themes, focus-ring rendering). Out of scope at the current kernel maturity.
- **Animation, transitions, opacity blending.** Out of scope.
- **Custom per-widget fonts.** Out of scope; keep using the system default font from `core_font::get_default_font()`.

---

## Key Decisions

- **Locked-in approach: Pillar 1 (layout) + slice of Pillar 2 (ScrollView + Selection model), plus the File-Manager-driven net-new components.** Rationale: layout is the universal cleanup that compounds with everything else; ScrollView and the Selection model are the two specific abstractions File Manager *forces* into existence via TreeView and IconView. Committing to a full `ItemsView`-style data-model framework now risks abstracting against fewer than three concrete consumers.
- **Defer Action / Command abstraction (Approach C from brainstorm).** Rationale: best designed against the real File Manager command surface (Open, Delete, Copy, Paste, Rename, New Folder, Refresh, …). Designing it now without a real consumer would commit to the wrong shape.
- **Skip a dedicated Window-trait macro pass.** Rationale: ride-along with layout integration. Every widget is already being touched to plumb into the new layout / ScrollView / Selection systems; folding delegation cleanup into the same diffs avoids a no-op churn pass.
- **Specify all seven net-new components now, even though only Tree/Splitter/IconView are strictly required for the File Manager sidebar+main split.** Rationale: each is small once layout, scroll, and selection are in place. Locking the API surface in one design pass prevents per-component drift later when StatusBar etc. are added in a hurry.
- **`WindowBase` stays as the foundation.** New layout primitives compose on top of `WindowBase`, not under it.

---

## Dependencies / Assumptions

- The heap allocator is initialized before any of these components are constructed. This is already true for the current widget set.
- The kernel is single-threaded today, so selection and scroll state mutation does not need to be reentrant-safe across event loops. If multitasking is added later, this assumption must be revisited.
- Mouse-wheel events exist in the input pipeline at least at the type level: `MouseEventType::Scroll` is referenced by `List::handle_event`. Whether the input pipeline actually emits Scroll events end-to-end (PS/2 vs VirtIO tablet) needs verification during planning — this is flagged as an unverified assumption against `src/input/` and `src/window/event.rs`.
- The existing `MultiColumnList::on_right_click` already returns global mouse position, which is sufficient for context-menu placement; the planned components don't need to extend that contract.

---

## Visual Reference: planned File Manager decomposition

```
FrameWindow ("Files")
└── VBox (content)
    ├── MenuBar               (R-existing)
    ├── Toolbar               (R15)   ── Back, Forward, Up, Refresh, View-Mode buttons
    ├── PathBar               (R17)   ── /  > host  > Documents  > Projects
    ├── Splitter (horizontal) (R14)
    │   ├── ScrollView (R5)
    │   │   └── TreeView      (R13)   ── sidebar: filesystem tree
    │   └── ScrollView (R5)
    │       └── IconView      (R18)   ── main pane (or MultiColumnList in details mode)
    └── StatusBar             (R16)   ── "12 items, 3 selected, 4.2 MB"
```

This decomposition shows why each new component earns its place and how the three pillars (layout, scroll, selection) underpin them.

---

## Outstanding Questions

### Resolve Before Planning

- None — scope is locked.

### Deferred to Planning

- [Affects R5][Technical] Whether `ScrollView` wraps a single child or hosts a list of children laid out in a virtual rect. Single-child is simpler; multi-child enables headers-stay-visible patterns (header + scrolling body in one viewport). Plan-time decision.
- [Affects R6][Technical] Where mouse-wheel-to-ScrollView routing lives — in `WindowManager`'s hit-testing path, or in event propagation through the window tree. Dependent on existing dispatch architecture in `manager.rs`.
- [Affects R20][Technical] Macro vs. default methods on the `Window` trait vs. composition wrapper for delegation cleanup. Each has trade-offs for inspectability and trait-object compatibility.
- [Affects R6][Needs research] Confirm that the input pipeline actually emits `MouseEventType::Scroll` events end-to-end. The stub in `List::handle_event` may be aspirational. Verify `src/input/` and `src/window/event.rs` during planning.
- [Affects R10][Needs research] Whether `keycode_to_char` and the modifier representation in `KeyboardEvent::modifiers` carry enough info today to implement shift+arrow / ctrl+click selection extension cleanly, or whether the event types need modest extension.
- [Affects R13, R18][Technical] Icon source strategy for `TreeView` and `IconView` — whether to extend `src/graphics/` with an icon-loading API or use placeholder glyphs from the existing font for v1. Plan-time call.
