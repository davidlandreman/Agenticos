---
title: "fix: Render windows correctly when partially off-screen"
type: fix
status: completed
created: 2026-05-08
---

## Summary

Migrate the `GraphicsDevice` trait's drawing primitives from `usize` pixel coordinates to signed `i32`, then implement proper signed-coordinate clipping in both framebuffer adapters so windows positioned at negative or beyond-screen origins render with the off-screen portion correctly clipped — no wrap-around, no distortion. Add a small drag-time clamp so the title bar always stays grabbable for recovery.

---

## Problem Frame

Dragging a window past any screen edge today causes the off-screen portion to "wrap" and reappear on the opposite side, with arbitrary pixel corruption across unrelated regions. The root cause is at the trait boundary: window-painters cast `i32` bounds (`bounds.x`, `bounds.y`) to `usize` before calling `GraphicsDevice` primitives, which all take `usize`. When `bounds.x` is negative — which the drag handler in `src/window/manager.rs::handle_dragging` happily produces (no clamping) — the `as usize` cast wraps `-10` into `2^64 - 10`. From that wrapped value:

- `DirectFrameBufferDevice::is_clipped` and its `DoubleBufferedDevice` twin compare wrapped pixel coords against a wrapped `clip.x as usize`, defeating clip checks
- `fill_rect` and `draw_pixel` paths in `src/drivers/display/frame_buffer.rs` compute `byte_offset = y * stride + x` over wrapped operands, landing pixel writes at unrelated framebuffer offsets
- The Bresenham implementation in both adapters truncates back to `i32` mid-loop, producing geometry that wraps further

Visually the user sees: drag a window left, garbage appears on the right; drag down, garbage at the top. The user wants partial off-screen drag to work — windows visibly clipped at the edge, not blocked from going there.

The fix is at the trait boundary, not at the call sites. Pushing every caller to clamp before drawing leaves the unsigned trap waiting for the next caller; widening the trait to signed coordinates and clipping inside the adapters removes the precondition entirely.

---

## Requirements

- **R1** — Windows whose bounds extend past any screen edge render with the off-screen portion clipped, without producing wrap-around pixels or out-of-bounds framebuffer writes
- **R2** — Dragging a window leaves at least the title bar grabbable, regardless of which edge it is dragged toward
- **R3** — All existing kernel tests continue to pass; new tests cover negative-origin and beyond-screen drawing through both framebuffer adapters
- **R4** — No regressions in cursor rendering at or near screen edges
- **R5** — The double-buffered and direct-framebuffer adapters behave equivalently with respect to clipping (they currently share the bug; they must share the fix)

---

## Key Technical Decisions

### Migrate `GraphicsDevice` drawing-primitive coordinates to `i32`

Method signatures for `draw_pixel`, `draw_line`, `draw_rect`, `fill_rect`, `draw_text`, `draw_image`, and `read_pixel` change from `usize` x/y to `i32` x/y. Width/height parameters become `u32` to match `Rect`. `width()` / `height()` device-query methods stay `usize` (they are never negative and downstream framebuffer code expects them).

This is the cleanest fix: it makes the trait's contract honest. Callers already work in signed space (`Rect::x`, `Point::x` are `i32`). The current `usize` interface forced every caller to perform a lossy cast. Centralizing the negative-coordinate handling in the adapters removes the trap.

### Clip in signed space, then convert at the framebuffer-write boundary

Adapters intersect `(x, y, width, height)` against both the active `clip_rect` and the device's own dimensions in `i32` arithmetic, producing a clipped rect that is guaranteed non-negative and within bounds. Only after clipping does the adapter cast to `usize` and call the underlying `FrameBufferWriter`. The lower-level driver (`src/drivers/display/frame_buffer.rs`) keeps its `usize` interface unchanged.

### Adapter-level Bresenham rewrite

`draw_line` is rewritten to operate in `i32` end-to-end and use the per-pixel clip path (or, for an in-bounds segment, fall through unclipped). The current implementation casts `usize → i32 → usize`, which compounds the wrap when endpoints are negative.

### Drag-time guard, lenient by design

`handle_dragging` clamps `new_x` / `new_y` so a strip of the title bar (full title-bar height × ~`MIN_TITLEBAR_VISIBLE` pixels wide, e.g. 80 px) remains within the screen rect. This is a recovery affordance, not the fix — partial off-screen is fully supported. The clamp is intentionally lenient: edges and most of the body can leave the screen.

### Resize path inherits drag-time clamp shape

The `Resizing` arm of `handle_dragging` already enforces `MIN_WINDOW_WIDTH/HEIGHT` via `Rect::resize_edge`. We extend it with a screen-edge clamp only where a resize could push the window's title bar fully off-screen (e.g., dragging the top edge below the screen). Resize is otherwise unchanged.

---

## High-Level Technical Design

*This sketch communicates the intended boundary; it is directional guidance for review, not implementation specification.*

```
                   ┌────────────────────────────────────────────┐
                   │  Window painters (frame.rs, text.rs, …)    │
                   │  Pass bounds.x: i32 directly — no casts.   │
                   └────────────────────┬───────────────────────┘
                                        │ i32 coords (may be negative)
                                        ▼
                   ┌────────────────────────────────────────────┐
                   │  GraphicsDevice trait (i32 surface)        │
                   └────────────────────┬───────────────────────┘
                                        │
                                        ▼
                   ┌────────────────────────────────────────────┐
                   │  Adapter (Direct or DoubleBuffered)        │
                   │   1. intersect with clip_rect (i32)        │
                   │   2. intersect with device bounds (i32)    │
                   │   3. if empty → skip                       │
                   │   4. cast clipped rect to usize            │
                   └────────────────────┬───────────────────────┘
                                        │ usize coords (in-bounds)
                                        ▼
                   ┌────────────────────────────────────────────┐
                   │  FrameBufferWriter (usize, unchanged)      │
                   └────────────────────────────────────────────┘
```

Clipping logic the adapter reuses across primitives:

```
fn clip(x: i32, y: i32, w: u32, h: u32) -> Option<(usize, usize, usize, usize)>:
    let rect = Rect::new(x, y, w, h)
    let bounded = rect ∩ device_bounds ∩ active_clip_rect    // signed math
    if bounded is empty: None
    else: Some(bounded as usize tuple)
```

---

## Implementation Units

### U1. Migrate `GraphicsDevice` trait surface to `i32` coordinates

**Goal**: Change the trait's drawing-primitive method signatures so callers can pass `i32` (and `u32` width/height) directly. No behavior change yet — adapters keep their current clipping logic with the cast moved inside the adapter, so this unit lands as a type-system migration that compiles green.

**Requirements**: R1 (precondition for the fix)

**Dependencies**: none

**Files**:
- `src/window/graphics.rs` — trait definition
- `src/window/adapters/direct_framebuffer.rs` — signature updates only; clipping logic still in `usize` for now (cast x/y at top of each method)
- `src/window/adapters/double_buffered.rs` — same shape as direct adapter
- (no test file — covered by U6)

**Approach**:
- Change `draw_pixel`, `draw_line`, `draw_rect`, `fill_rect`, `draw_text`, `read_pixel`, `draw_image` to take `x: i32, y: i32` (and `width: u32, height: u32` where applicable)
- Inside each adapter method, do the temporary `if x < 0 || y < 0 { return; }` early-return plus `x as usize` cast at the entry of each primitive — this preserves current behavior for non-negative inputs while letting the trait migrate
- Document on the trait: "Coordinates are absolute pixel positions in device space and may be negative or beyond device bounds; adapters clip"

**Patterns to follow**: existing trait shape in `src/window/graphics.rs`; existing `Rect`/`Point` types use `i32` x/y already

**Test scenarios**:
- Test expectation: none -- pure signature change with behavior preserved by entry-time casts; behavioral coverage lives in U2/U3/U6

**Verification**: `cargo check` passes; `./test.sh` passes; the OS still boots into the desktop and the terminal frame paints normally at on-screen positions

---

### U2. Implement signed-coordinate clipping in `DirectFrameBufferDevice`

**Goal**: Replace the entry-time guards from U1 with proper signed-space clipping inside the adapter. Negative or beyond-bounds inputs produce correctly clipped output rather than no-op or wrap.

**Requirements**: R1, R5

**Dependencies**: U1

**Files**:
- `src/window/adapters/direct_framebuffer.rs`
- `src/tests/window_clipping.rs` (new)
- `src/tests/mod.rs` (register the new module)

**Approach**:
- Introduce a private helper that intersects an input rect with both the device bounds and the active clip rect in `i32` space, returning `Option<(usize, usize, usize, usize)>` for the clipped rect
- Rewrite `fill_rect` and `draw_rect` to use the helper, then call the underlying writer with the resulting `usize` rect (or skip on `None`)
- Rewrite `draw_pixel` to test the single-pixel case via the helper
- Rewrite `draw_line` (Bresenham) to operate in `i32` throughout and either pre-clip endpoints (Cohen–Sutherland-style) or use per-pixel clipping; correctness > optimal clipping strategy
- Rewrite `draw_text` to compute each glyph's coordinates in `i32`, then defer to the now-clipping `draw_pixel`
- `set_clip_rect` may store the rect as-is; the helper handles negative origin
- Remove the temporary U1 entry-time casts; all coordinate handling now lives in the helper

**Patterns to follow**: existing `Rect` arithmetic in `src/window/types.rs` and `WindowBuffer::mark_dirty` in `src/window/graphics.rs:81-93` (rect-union math in `i32`)

**Test scenarios** (new file `src/tests/window_clipping.rs`):
- A wrapper test device that records all `FrameBufferWriter`-bound writes verifies no out-of-bounds pixel writes when callers pass negative coordinates. (Implementation note: the simplest path is to call methods on a real `DirectFrameBufferDevice` constructed against a small synthetic framebuffer; keep the test infrastructure local to this module.)
- `fill_rect(-50, -50, 100, 100)` on a 200x200 device draws only the bottom-right 50x50 portion (top-left clipped)
- `fill_rect(150, 150, 100, 100)` on a 200x200 device draws only the top-left 50x50 portion (bottom-right clipped)
- `fill_rect(-1000, -1000, 100, 100)` produces zero pixel writes (fully off-screen)
- `fill_rect(0, 0, u32::MAX, u32::MAX)` clips to device bounds (overflow safety)
- `draw_pixel(-1, 0)` is a no-op; `draw_pixel(0, 0)` writes; `draw_pixel(width, 0)` is a no-op
- `draw_line` from `(-50, -50)` to `(250, 250)` on a 200x200 device produces only in-bounds pixels along the line
- `draw_line` from `(-100, 50)` to `(-50, 60)` (entirely off-screen) produces zero writes
- With a clip rect of `Rect::new(10, 10, 50, 50)`: `fill_rect(0, 0, 200, 200)` writes only the 50x50 region inside the clip
- With a clip rect of `Rect::new(-20, -20, 50, 50)` (negative origin clip): `fill_rect(0, 0, 200, 200)` writes the 30x30 region from `(0,0)` to `(30,30)` — i.e., the clip's negative-origin portion is correctly excluded
- `draw_text` at `(-100, 100)` with text wider than 100 px clips the leading characters and renders only the tail visible portion; no pixel writes outside device bounds

**Verification**: new tests pass under `./test.sh`; OS boots; dragging the terminal window past the left/right/top/bottom edges shows the window correctly clipped, no wrap, no garbage

---

### U3. Mirror the fix in `DoubleBufferedDevice`

**Goal**: Apply the same clipping helper and Bresenham rewrite to the double-buffered adapter.

**Requirements**: R1, R5

**Dependencies**: U1, U2 (lift the helper shape from U2)

**Files**:
- `src/window/adapters/double_buffered.rs`
- `src/tests/window_clipping.rs` (extend with double-buffered cases)

**Approach**:
- Extract the clipping helper from U2 (or duplicate as a private module-local helper — the two adapters could share via a small free function in `src/window/adapters/mod.rs`, decided during implementation; either is acceptable)
- Apply the same primitive rewrites
- Preserve the `dirty` flag behavior — a fully clipped-away call must not set `dirty = true` (subtle: today it does, because `fill_rect` always sets dirty regardless)

**Patterns to follow**: U2

**Test scenarios** (extend `src/tests/window_clipping.rs`):
- All U2 scenarios re-run against `DoubleBufferedDevice`, asserting parity
- A fully-clipped-away `fill_rect` does not set the dirty flag (verify via `flush()` not swapping buffers when nothing was actually drawn — match this against the existing "Only swap buffers if we actually drew something" comment at `src/window/adapters/double_buffered.rs:186-193`)

**Verification**: new tests pass; OS booted with the double-buffered adapter (toggle `USE_DOUBLE_BUFFER` per `src/drivers/CLAUDE.md`) shows the same correct clipping behavior

---

### U4. Drop `as usize` casts in window painters

**Goal**: Remove the now-unnecessary `bounds.x as usize` casts across the window-implementation files. Callers pass `i32` directly; arithmetic stays in `i32`.

**Requirements**: R1 (closes the loop on caller-side wrap)

**Dependencies**: U1

**Files** (per Grep against `src/window/`):
- `src/window/windows/frame.rs` — title bar, borders, close button
- `src/window/windows/text.rs` — text grid rendering
- `src/window/windows/text_input.rs`
- `src/window/windows/text_editor.rs`
- `src/window/windows/desktop.rs`
- `src/window/windows/container.rs`
- `src/window/windows/button.rs`
- `src/window/windows/label.rs`
- `src/window/windows/list.rs`
- `src/window/windows/multi_column_list.rs`
- `src/window/windows/menu.rs`
- `src/window/windows/menu_bar.rs`
- `src/window/windows/menu_bar_popup.rs`
- `src/window/windows/taskbar.rs`
- `src/window/cursor.rs` — cursor save/restore (review: cursor is small and centered, but the code still casts; replace casts with `i32` math even if the cursor never goes negative today)
- `src/window/manager.rs` — `render_window_tree_with_offset_propagate` already uses `i32` for the `bounds` it passes to `set_clip_rect`; verify and adjust any leftover casts

**Approach**:
- Each call site replaces `something as usize` with the `i32` value passed straight through. Where width/height arithmetic mixes `i32` and `u32` (e.g., `bounds.width - 2 * border as u32`), keep widths as `u32` and casts only at the seam between widths and positions
- For positions computed via `bounds.x as usize + offset`, change to `bounds.x + offset as i32` (or compute the offset in `i32` to begin with)
- Where today's code computes `bounds.x as usize + bounds.width as usize - border` (e.g., right-edge close-button placement in `frame.rs:90-101`), refactor to `bounds.x + bounds.width as i32 - border as i32`

**Patterns to follow**: `Rect::resize_edge` in `src/window/types.rs` already mixes `i32` positions with `u32` sizes cleanly

**Test scenarios**:
- Test expectation: no new tests in this unit -- behavior is exercised by U2/U3 clipping tests and the visual verification below. Adding per-window tests would duplicate the per-adapter coverage
- Boot verification: launch the OS, exercise every visible window type (terminal, taskbar if active, any menu) with no visual regression at on-screen positions

**Verification**: `cargo check` clean; `./test.sh` passes; OS boots; manual test: drag the terminal frame so half hangs off each edge in turn — title text, close button, borders, and inner text content all clip cleanly with no wrap

---

### U5. Add drag-time title-bar visibility clamp

**Goal**: Ensure the user can always drag the window back, even after the rendering fix permits arbitrary off-screen positions.

**Requirements**: R2

**Dependencies**: independent of U1–U4 (could land first, but more useful after the render fix is in)

**Files**:
- `src/window/manager.rs` — `handle_dragging`, `Resizing` arm
- `src/window/types.rs` — add `MIN_TITLEBAR_VISIBLE` constant near `MIN_WINDOW_WIDTH` / `MIN_WINDOW_HEIGHT`

**Approach**:
- Define `MIN_TITLEBAR_VISIBLE: u32 = 80` (or similar) — the minimum horizontal title-bar strip that must remain inside the screen rect
- In the `Dragging` arm, after computing `new_x`/`new_y`, clamp:
  - `new_x` so that `new_x + window.width >= MIN_TITLEBAR_VISIBLE` (left side) and `new_x <= screen_width - MIN_TITLEBAR_VISIBLE` (right side)
  - `new_y` so the title bar's vertical band stays within `[0, screen_height - title_bar_height]`
- Pull screen dimensions from `self.graphics_device.width()/height()`, not from a hardcoded constant
- Apply the same vertical clamp in the `Resizing` arm only for edges that move the title bar (top edge primarily)
- The title bar height is currently a `FrameWindow` constant (`24`); for the manager-level clamp, use the standard 24 px directly (or expose it via a `Window` trait method in a follow-up — out of scope here)

**Patterns to follow**: existing `MIN_WINDOW_WIDTH`/`MIN_WINDOW_HEIGHT` and `Rect::resize_edge` in `src/window/types.rs`

**Test scenarios** (add to `src/tests/window_clipping.rs` or a new `src/tests/window_drag.rs`):
- Drag clamp: starting from `(100, 50)` on an 800x600 device, dragging by `(-1000, 0)` lands at `(-(width - MIN_TITLEBAR_VISIBLE), 50)` — title bar's right edge sits at `MIN_TITLEBAR_VISIBLE` from the left
- Drag clamp: dragging by `(+1000, 0)` lands at `(800 - MIN_TITLEBAR_VISIBLE, 50)`
- Drag clamp: dragging up by `1000` clamps `new_y` to `0` (title bar at the top, fully visible)
- Drag clamp: dragging down by `1000` lands so the title bar's bottom edge sits at `screen_height` minus title-bar height — title bar fully visible at the bottom
- Drag clamp does not block partial off-screen: dragging by `(-50, 0)` on a window starting at `(100, 50)` lands exactly at `(50, 50)` (no clamp interference within the lenient zone)

**Execution note**: since this unit changes interactive behavior with no new render-path coverage of its own, write the clamp tests first against the current `handle_dragging` shape so the test surface is settled before edits

**Verification**: new tests pass; manual test: drag the terminal window aggressively in each direction — it can go partially off-screen but the title bar stays grabbable; release and re-grab the title bar to drag the window back

---

### U6. Cross-cutting verification: cursor at edges and combined drag scenarios

**Goal**: Confirm the cursor renderer behaves correctly when the cursor crosses screen edges (it does its own framebuffer save/restore via `src/window/cursor.rs`), and add an integration-shaped test that exercises a window dragged across an edge.

**Requirements**: R3, R4

**Dependencies**: U2, U3, U4, U5

**Files**:
- `src/window/cursor.rs` — review only; if `as usize` casts on cursor x/y exist, replace per U4 conventions
- `src/tests/window_clipping.rs` — extend with cursor-edge cases

**Approach**:
- Trace `CursorRenderer::draw` and `restore_background` paths; mouse position comes through as `i32` from input drivers but may be cast to `usize` for save/restore. Apply the same clip-at-the-edge treatment used in U2 if any unsafe cast remains
- Confirm visually that a cursor at `(width - 1, height - 1)` does not produce a one-pixel wrap onto the next row (current behavior under examination)

**Patterns to follow**: U2 clipping helper

**Test scenarios**:
- Cursor save/restore at `(0, 0)`, `(width - 1, 0)`, `(0, height - 1)`, `(width - 1, height - 1)` produces no out-of-bounds reads or writes
- Cursor save/restore at `(width - 6, height - 6)` (cursor sprite straddles the right/bottom edge) writes only in-bounds pixels and the background is correctly restored after movement
- Combined: with the terminal frame dragged so its left edge sits at `x = -200`, the cursor moving over the visible portion of the title bar correctly redraws with the background restoration intact

**Verification**: new tests pass; manual test: move the cursor to all four screen corners and across edges; drag a window past an edge while the cursor is over it — both render correctly, no smearing, no wrap

---

## Scope Boundaries

**In scope**:
- `GraphicsDevice` trait surface change to `i32` coordinates
- Clipping logic in `DirectFrameBufferDevice` and `DoubleBufferedDevice`
- Caller-side cleanup in `src/window/windows/*` and `src/window/cursor.rs`
- Drag-time title-bar visibility clamp
- Test coverage for the above

**Out of scope (deliberately excluded)**:
- Refactoring the underlying `FrameBufferWriter` (`src/drivers/display/frame_buffer.rs`) — keeps its `usize` interface
- Changing `Rect`/`Point` numeric types (already signed where it matters)
- Multi-monitor or virtual-screen layouts
- Window snapping, magnetism, maximize/minimize, off-screen previews
- Performance work on the clipped paths beyond what the rewrite naturally produces
- Generalizing title-bar height into a `Window` trait method (the manager uses the constant 24 px today; revisit if/when other frame styles emerge)
- A `GraphicsDevice` capability that reports its clip rect intersected with bounds, or other API expansions

### Deferred to Follow-Up Work

- Running `/ce-compound` to capture the i32-vs-usize-coordinate convention as the first entry in `docs/solutions/` once this lands
- Sharing the clipping helper between adapters via a common module if the duplication-during-implementation in U3 justifies it (decision deferred to implementation)
- Auditing other drawing surfaces (`graphics::compositor`) for the same cast pattern — this plan covers the surfaces that touch the bug; a broader sweep is its own work

---

## Risks & Mitigations

- **Trait migration ripple**: changing `GraphicsDevice` signatures touches every painter. Mitigation: U1 lands the type change with behavior preserved (entry-time casts), so it compiles green before any clipping logic moves. U4 cleans up casts incrementally.
- **Bresenham regression**: rewriting the line algorithm in `i32` can introduce off-by-one artifacts. Mitigation: cover endpoint cases in tests (entirely in-bounds, entirely out-of-bounds, crossing a single edge, crossing two edges).
- **Double-buffered dirty-flag drift**: today the dirty flag is set unconditionally in `fill_rect`. Skipping it when fully clipped is a correctness improvement but could mask an unrelated repaint bug. Mitigation: preserve current behavior when *any* pixel is drawn; only skip when the clipped rect is empty.
- **Drag-clamp UX surprise**: a too-aggressive clamp would feel like the window is "snapping back." Mitigation: `MIN_TITLEBAR_VISIBLE` is a small fraction of typical window width; manual test confirms partial off-screen feels natural before merging.

---

## Verification Strategy

- `cargo check` and `cargo clippy` clean after each unit
- `./test.sh` passes after each unit (new tests added per U2/U3/U5/U6)
- Manual end-to-end: boot the OS, drag the default terminal window past each of the four screen edges. Expected: window clips smoothly, no wrap, no garbage. Title bar always remains grabbable. Re-drag from any clamped position works.
- Manual cursor verification at the four corners and along edges with no smearing
