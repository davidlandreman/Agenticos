# `src/graphics/` — Graphics and Display Subsystem

Drawing primitives, text rendering, image loading, and compositor for the framebuffer display. **Modern framebuffer** — no VGA text mode anywhere.

## Key files

- `color.rs` — RGB colors and the predefined palette.
- `compositor.rs` — dirty-rectangle tracking and cursor-overlay management.
- `surface.rs` — retained canonical premultiplied ARGB8888 surfaces, local
  damage merging, and explicit allocation budgeting.
- `scene.rs` — backend-neutral ordered layers, opacity, 16.16 transforms, and
  reserved effect metadata.
- `composition/cpu.rs` — pixel-correct premultiplied source-over reference
  compositor and runtime fallback, including the three-pass backdrop blur used
  by Aero glass layers. Optional render telemetry separates surface raster,
  upload, composition, blur, fence, and presentation cycle buckets.
- `composition/virgl.rs` — qualified accelerated compositor. It keeps one
  bounded host texture per live retained surface, uploads only transactional
  surface-local damage after the first frame, and gives each cached texture one
  persistent sampler view. The framebuffer surface, shaders, vertex elements,
  blend/depth/rasterizer/sampler state, and a geometrically growing vertex
  resource survive across frames and are torn down before their context.
  Two fixed render-target/sampler scratch textures and bounded separable TGSI
  shader variants implement the CPU reference's three-box backdrop blur for
  effect-expanded damage. Engine construction qualifies the exact copy,
  ping-pong, multi-sampler combine, transparent-discard, and readback path.
  Each clipped output-damage rectangle is cleared by a scissored transparent
  overwrite quad, then only intersecting textured layers are drawn with
  premultiplied source-over. The result is fenced and directly scanned out from
  the host texture. Layer opacity is draw state rather than cached pixel data,
  so movement/focus/opacity-only frames reuse textures without upload or GPU
  object churn. Qualified VirGL supports Aero's radius-4 blurred glass without
  ordinary-frame readback; unsupported effect radii fail composition instead
  of silently rendering sharp glass.
- `present/` — scanout boundary. The boot-framebuffer presenter converts only
  damaged pixels; VirtIO-GPU 2D presentation is owned by `src/drivers/`.
- `fonts/` — font support. `core_font.rs` defines the glyph-centric `Font` trait + `Glyph<'a>` struct (8bpp coverage). `ttf.rs` is the TTF/OTF backend (parses via `ttf-parser`, rasterizes via `ab_glyph_rasterizer`, ASCII pre-rendered into per-glyph `Box<[u8]>` slots, non-ASCII lazy via `BTreeMap`). `embedded_font.rs` is the 8x8 bitmap fallback used only on TTF parse failure. `font_data.rs` holds the embedded font's bit-packed source. The system TTF lives at `assets/system.ttf` and is `include_bytes!`-baked into the kernel; `init_fonts()` parses it once during boot, after heap init.
- `images/` — `bmp.rs` supports full Windows BMP (1/4/8/16/24/32-bit). Parsed images implement the `Image` trait and can be drawn through `GraphicsDevice::draw_image` / `draw_image_scaled` (defined in `src/window/graphics.rs`); both have per-pixel default implementations that respect the device's clip rect, so any adapter gets image rendering for free without an override.

## Double buffering

Controlled by the `USE_DOUBLE_BUFFER` flag in `src/drivers/display/display.rs` (the flag lives there, not here):

- **Enabled (default)** — 8 MiB static back buffer; smoother rendering at the cost of bulk copies.
- **Disabled** — direct framebuffer writes; lower latency for small updates, no tearing protection for full-frame work.

This buffer is part of the `legacy` renderer only. The `retained` renderer owns
canonical output storage and does not use `WindowBuffer` as its surface format.
Do not call VirtIO-GPU 2D composition acceleration: it only transfers the CPU-
composed output. The VirGL engine is the accelerated composition path and owns
its VirtIO device for its entire lifetime.

## Performance notes

These have been measured; treat them as constraints, not preferences:

- **Framebuffer memory is slow.** Direct pixel writes have high latency.
- **Bulk copies are fast.** `core::ptr::copy()` for swapping buffers and scrolling beats per-pixel work by an order of magnitude.
- **Scrolling = `memmove`.** Don't redraw all rows when shifting; move them in memory.
- **Static allocation** for the back buffer avoids heap fragmentation in a critical path.
- **Legacy cursor uses direct-framebuffer.** The double-buffer path's full-frame copy is too slow for cursor latency (see `src/window/CLAUDE.md`).
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
4. Image loading/display. (BMP parsing + `GraphicsDevice` rendering landed; additional formats and per-adapter bulk-row blits remain.)
5. Composite operations (windows, widgets).

This is a known-pain marker — when touching this folder, prefer additions that move toward this layering rather than entrenching the current shape.

## Cross-references

- Display driver and `USE_DOUBLE_BUFFER` flag: `src/drivers/CLAUDE.md`.
- Window system uses these primitives: `src/window/CLAUDE.md`.
