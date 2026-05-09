---
date: 2026-05-09
topic: rendering-perf-compositor
---

# Rendering Performance: Hybrid Compositor with Opt-in Backing Stores

## Summary

Refactor the window rendering pipeline so dirty tracking actually clips drawing, then introduce per-window backing stores on an opt-in basis with the desktop wallpaper as the first opt-in. Drag, resize, and z-order changes for opted-in windows become bitmap translates rather than re-renders.

---

## Problem Frame

After the wallpaper PR (#11) landed, full-screen redraws feel pronouncedly slow — perceived ~5fps when a window redraws over the desktop. The worst case is dragging or resizing a window: the dragged window leaves a visible lag/trail behind the cursor. Cursor movement alone over the wallpaper is also choppier than expected.

Three properties of the current pipeline combine to cause this:

1. The default `GraphicsDevice::draw_image_scaled` walks every destination pixel and calls `draw_pixel`, which on the double-buffered adapter takes a `Mutex` lock on the back buffer per pixel. A 1024×768 wallpaper blit is ~786k locks per repaint.
2. The renderer sets the clip rect to *window bounds*, not to the intersection of window bounds and dirty regions, and calls `paint()` unconditionally on every visible window each frame. Dirty tracking exists but is not load-bearing for actual painting — only for the cursor save/restore region.
3. Drag and resize call `mark_full_repaint` on every cursor tick, which forces the entire screen — including the full wallpaper — to repaint at drag rate.

The combination means that even a cursor jiggle re-blits the wallpaper end-to-end, and dragging a window does the same per drag tick. The window-dirty-tracking infrastructure is in place but does not gate the work that actually costs.

---

## Key Flows

- F1. **Cursor moves only, no window content changed**
  - **Trigger:** mouse delta with no other state change
  - **Steps:**
    1. Compositor marks small old/new cursor padding rects dirty.
    2. Renderer wakes (dirty non-empty).
    3. Renderer skips windows whose bounds don't intersect any dirty rect and that are not invalidated.
    4. For windows that do intersect, renderer sets clip to (window bounds ∩ dirty union) and calls `paint()`.
    5. Cursor save/restore runs as today against the framebuffer.
  - **Outcome:** No window's `paint()` runs; cursor moves are framebuffer save/restore only.
  - **Covered by:** R3, R4, R5

- F2. **Dragging a frame window over the wallpaper**
  - **Trigger:** left button held while title bar drags
  - **Actors:** dragged frame window, desktop window (opted-in backing store)
  - **Steps:**
    1. Manager updates the frame's bounds for the new mouse position.
    2. Manager marks (old frame bounds ∪ new frame bounds) dirty — not the full screen.
    3. Compositor blits the desktop's cached backing store over the dirty regions (no re-rasterization).
    4. Renderer paints the dragged frame at its new position, clipped to the dirty intersection.
    5. Children of the dragged frame paint clipped to the dirty intersection.
  - **Outcome:** Drag updates only the strip exposed by the move plus the new frame position; wallpaper is never re-rasterized during a drag.
  - **Covered by:** R6, R7, R8

- F3. **Frame window paints with opted-in backing store (future)**
  - **Trigger:** an opted-in window's content changed (`needs_repaint()` true)
  - **Steps:**
    1. Window paints into its own backing buffer instead of directly into the back buffer.
    2. Compositor blits the backing buffer to the back buffer at the window's current bounds, clipped to dirty + visible region.
  - **Outcome:** Rendering is decoupled from positional changes; only content changes pay re-render cost.
  - **Covered by:** R9, R10, R11

---

## Requirements

**Cross-cutting tactical fixes (land first, independent of compositor variant)**

- R1. The double-buffered graphics adapter overrides `draw_image` and `draw_image_scaled` with a bulk row-oriented blit path that takes the back-buffer lock once per row (or once per call) rather than per pixel.
- R2. The double-buffered adapter's `fill_rect` and other bulk primitives must not take the back-buffer lock per pixel; one lock per call.
- R3. The window renderer skips calling `paint()` on a visible window whose absolute bounds do not intersect any dirty rect *and* whose `needs_repaint()` is false. Windows that meet either condition are still painted.
- R4. When the renderer calls `paint()` on a window, the active clip rect is the intersection of the window's absolute bounds with the union of dirty rectangles, not the window's bounds alone.
- R5. Drag and resize stop calling `mark_full_repaint`. They mark the union of (old window bounds, new window bounds) dirty, plus any newly-exposed area from sibling/desktop background.

**Hybrid opt-in compositor (Variant 3)**

- R6. Windows declare via a trait method (or equivalent capability flag) whether they want a backing store. Default is "no" — existing windows continue direct-to-back-buffer painting unchanged.
- R7. The desktop / wallpaper window opts in. On wallpaper load it pre-rasterizes its scaled bitmap into a backing buffer once; subsequent paints are bulk row blits from that buffer to the back buffer for the dirty-rect intersection.
- R8. The compositor recognizes the opt-in flag during the frame walk: opted-in windows are blitted from their backing store; non-opted windows go through the existing direct-paint path with R3/R4 clipping applied.
- R9. For an opted-in window, position changes (drag) and z-order changes do **not** invalidate the backing store. Only content changes (`invalidate()` triggered by the window's own state) re-render into the backing store.
- R10. For an opted-in window, the backing store is allocated and rasterized lazily on first paint after invalidation; it is not pre-allocated for every window.
- R11. When an opted-in window resizes, its backing store is replaced/regenerated. The resize path is correct (no stale-content artifacts), even if not optimal.

**Cursor and existing behavior preservation**

- R12. Cursor save/restore continues to use the direct-framebuffer adapter as today; the cursor is not part of the compositor backing-store flow.
- R13. `TextWindow`'s existing dirty-cell tracking continues unchanged. It is a non-opted window in the new model.
- R14. Modal dialogs, popup menus, and the menu-bar popup flow continue to work; their first-paint and close paths may still force a full repaint where necessary, but the steady-state behavior benefits from R3/R4.

---

## Acceptance Examples

- AE1. **Covers R1, R3, R4, R5.** Given the kernel is booted with the wallpaper visible and a terminal frame at (100, 50). When the user drags the title bar 200 pixels to the right over 200ms, no individual frame's paint pass re-blits the entire wallpaper; only the strip between old and new bounds plus the new frame area is repainted.
- AE2. **Covers R3, R4, R12.** Given the kernel is at idle desktop with the cursor visible. When the user moves the mouse with no buttons pressed and no window content changing, no window's `paint()` method is called for the duration of the move; only cursor save/restore runs.
- AE3. **Covers R7, R9.** Given the desktop has finished pre-rasterizing the wallpaper into its backing store. When the wallpaper-display state has not changed (no resolution change, no wallpaper swap), every subsequent desktop paint reads only from the cached backing store; the BMP parse / scale path runs zero additional times.
- AE4. **Covers R6, R8, R13.** Given a `TextWindow` that has not opted in to a backing store. When `needs_repaint()` is true, the window is painted directly to the back buffer via the existing path with the dirty-rect-aware clip from R4 applied.
- AE5. **Covers R11.** Given an opted-in window with a 400×300 backing store. When the window is resized to 600×400, the window paints correctly (no stale pixels, no garbage at the new edges) on the next frame.

---

## Success Criteria

- Dragging the terminal frame across the wallpaper feels smooth — no visible trail/lag of the dragged window behind the cursor at typical mouse speeds.
- Cursor movement over the wallpaper does not perceptibly stutter when no other state is changing.
- The wallpaper PR no longer feels like a regression vs. the pre-wallpaper solid-blue desktop.
- A downstream implementer can trace each cross-cutting fix (R1–R5) to a specific function in `src/window/adapters/double_buffered.rs` and `src/window/manager.rs` without re-discovering the architecture.
- Adding a future expensive window (e.g., an image viewer) requires only flipping the opt-in flag and writing into a backing store — no compositor-level rework.

---

## Scope Boundaries

- **Variant 2 (per-window backing stores for *all* windows)** is out. The hybrid model is the chosen target; full Variant 2 is a possible future migration path but not part of this work.
- **Translucency, drop shadows, anti-aliased text spilling outside window bounds** — out. The compositor assumes opaque rectangular windows.
- **GPU acceleration / VirtIO-GPU / hardware blit** — out. CPU-only rendering throughout.
- **Animations, transitions, fade effects** — out.
- **Per-window snapshot or screenshot APIs** — out, even though the backing store could enable them later.
- **Sub-pixel or fractional wallpaper scaling** — out. Nearest-neighbor scaling stays.
- **Multi-screen support, resolution changes at runtime** — out.
- **The full layering refactor noted in `src/graphics/CLAUDE.md`** — this work moves toward it (clearer separation of compositor vs. drawing primitives) but does not aim to complete it. Module-boundary cleanup beyond what the compositor change requires is deferred.
- **`TextWindow` opting in to a backing store** — out. The existing dirty-cell tracking already covers TextWindow's hot path; adding a backing store would duplicate work and burn ~3 MiB.
- **PNG decompression** — out (orthogonal to this work, mentioned only because PNG is referenced in the graphics CLAUDE.md).

---

## Key Decisions

- **Variant 3 (opt-in hybrid) chosen over Variant 1 (cached static layer only) and Variant 2 (per-window backing stores everywhere).** Variant 1 leaves dragging cost on the table; Variant 2 is the right end-state but pays refactor cost on every window today, most of which paint cheaply. Variant 3 captures the drag-as-translate win where it matters (the wallpaper) and is a clean migration path to Variant 2 if the rest of the system grows expensive.
- **Cross-cutting fixes (R1–R5) ship first, as their own discrete change.** They are independently valuable, expected to substantially improve perceived performance on their own, and are prerequisites for the compositor variant to deliver its real benefit (without dirty-rect-aware clipping, the backing store is a partial fix at best).
- **The desktop is the only initial opt-in.** No other window type is migrated as part of this work. Future opt-ins are a per-window flip, not a compositor change.
- **Cursor stays on the direct-framebuffer adapter.** The compositor does not own the cursor. Cursor save/restore continues against the physical framebuffer.

---

## Dependencies / Assumptions

- `~3 MiB` per opted-in full-screen-sized backing store (1024×768×4 bytes). With one initial opt-in (desktop), this fits comfortably in the 100 MiB heap. Each future opt-in adds one window-bounds-sized buffer.
- The double-buffered adapter's existing back buffer (8 MiB static) and `swap_buffers` bulk copy remain unchanged. Backing stores live on the heap, not in the static back buffer.
- The `GraphicsDevice` trait surface is sufficient for "draw into a backing store" — implementing the trait against an in-memory buffer is straightforward and does not require trait changes. To be confirmed during planning.
- Cascade invalidation (`manager.rs:cascade_invalidation`) continues to be needed for non-opted overlapping windows; opted-in windows benefit from the compositor's blit-time clipping instead.

---

## Outstanding Questions

### Resolve Before Planning

(None — both open questions are technical and answered better during planning.)

### Deferred to Planning

- *[Affects R10, R11][Technical]* When an opted-in window resizes, do we (a) keep its backing store sized to the maximum screen dimensions and clip on use, or (b) reallocate/re-rasterize on each resize? Tradeoff: ~3 MiB always-pinned vs. resize-tick stutter. May depend on per-window characteristics (the desktop's wallpaper is a clear (a) candidate; future windows may not be).
- *[Affects Success Criteria][Needs research]* A measurable acceptance proxy beyond "feels smooth." Candidates: wall-clock time per render frame while dragging over wallpaper (target < 33ms); count of `paint()` calls per cursor-only tick (target: 0 for non-dirty windows). To be picked during planning, ideally with an instrumentation hook added under a `cfg(feature = "test")` flag so it can be measured without polluting release builds.
- *[Affects R6][Technical]* Trait shape for the opt-in flag — plain `Window::wants_backing_store(&self) -> bool`, or a richer capability that returns the buffer? Likely the former; resolve in planning.
