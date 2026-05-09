# `src/graphics/` — Graphics and Display Subsystem

Drawing primitives, text rendering, image loading, and compositor for the framebuffer display. **Modern framebuffer** — no VGA text mode anywhere.

## Key files

- `color.rs` — RGB colors and the predefined palette.
- `core_gfx.rs` — primitives: Bresenham lines, circles, rectangles, polygons.
- `core_text.rs` — font-agnostic text rendering.
- `compositor.rs` — dirty-rectangle tracking and cursor-overlay management.
- `framebuffer.rs` — region save/restore (`SavedRegion`, `RegionCapableBuffer` trait).
- `render.rs` — `RenderTarget` abstraction for efficient row-based drawing.
- `mouse_cursor.rs` — 12×12 arrow sprite with background save/restore.
- `fonts/` — font support. `core_font.rs` defines the glyph-centric `Font` trait + `Glyph<'a>` struct (8bpp coverage). `ttf.rs` is the TTF/OTF backend (parses via `ttf-parser`, rasterizes via `ab_glyph_rasterizer`, ASCII pre-rendered into per-glyph `Box<[u8]>` slots, non-ASCII lazy via `BTreeMap`). `embedded_font.rs` is the 8x8 bitmap fallback used only on TTF parse failure. `font_data.rs` holds the embedded font's bit-packed source. The system TTF lives at `assets/system.ttf` and is `include_bytes!`-baked into the kernel; `init_fonts()` parses it once during boot, after heap init.
- `images/` — `bmp.rs` (full Windows BMP, 1/4/8/16/24/32-bit). `png.rs` (header parsing only — no decompression yet). Parsed images implement the `Image` trait and can be drawn through `GraphicsDevice::draw_image` / `draw_image_scaled` (defined in `src/window/graphics.rs`); both have per-pixel default implementations that respect the device's clip rect, so any adapter gets image rendering for free without an override.

## Double buffering

Controlled by the `USE_DOUBLE_BUFFER` flag in `src/drivers/display/display.rs` (the flag lives there, not here):

- **Enabled (default)** — 8 MiB static back buffer; smoother rendering at the cost of bulk copies.
- **Disabled** — direct framebuffer writes; lower latency for small updates, no tearing protection for full-frame work.

## Performance notes

These have been measured; treat them as constraints, not preferences:

- **Framebuffer memory is slow.** Direct pixel writes have high latency.
- **Bulk copies are fast.** `core::ptr::copy()` for swapping buffers and scrolling beats per-pixel work by an order of magnitude.
- **Scrolling = `memmove`.** Don't redraw all rows when shifting; move them in memory.
- **Static allocation** for the back buffer avoids heap fragmentation in a critical path.
- **Cursor uses direct-framebuffer.** The double-buffer path's full-frame copy is too slow for cursor latency (see `src/window/CLAUDE.md`).
- **`TextWindow` is incremental.** Dirty-cell tracking avoids redrawing all glyphs on every keystroke.

## Known architecture issues (open work)

The graphics subsystem grew organically and currently has:

- Unclear module boundaries between `display`, `graphics`, and `fonts`.
- Tight coupling between components.
- Mixed abstraction levels.
- Inconsistent naming conventions.

The intended next refactor establishes clear layers:

1. Raw framebuffer access.
2. Drawing primitives.
3. Text/font rendering.
4. Image loading/display. (BMP parsing + `GraphicsDevice` rendering landed; PNG decompression and per-adapter bulk-row blits remain.)
5. Composite operations (windows, widgets).

This is a known-pain marker — when touching this folder, prefer additions that move toward this layering rather than entrenching the current shape.

## Cross-references

- Display driver and `USE_DOUBLE_BUFFER` flag: `src/drivers/CLAUDE.md`.
- Window system uses these primitives: `src/window/CLAUDE.md`.
