---
title: "feat: Render bitmaps via GraphicsDevice and default desktop wallpaper"
type: feat
status: active
created: 2026-05-08
---

## Summary

Wire the existing BMP parser through the modern `GraphicsDevice` trait so windows can blit images, then load a bundled wallpaper BMP from the boot disk (`/WALLPAPR.BMP`) and render it as the `DesktopWindow` background instead of a solid blue fill. The wallpaper is static — no settings UI, no live reload.

The BMP parser in `src/graphics/images/bmp.rs` already handles 1/4/8/16/24/32-bit. The legacy `core_gfx::Graphics::draw_image` already blits images to the framebuffer. The gap is at the window-system boundary: `GraphicsDevice::draw_image` is a stub that takes raw bytes with no format information and does nothing. This plan closes that gap and uses the result on `DesktopWindow`.

---

## Problem Frame

The kernel boots into a GUI desktop with a solid blue (`Color::new(0, 50, 100)`) background painted by `DesktopWindow::paint` (`src/window/windows/desktop.rs:46`). It's drab and hides the fact that the OS already has a working BMP parser and a filesystem layer that can read assets. Two pieces are blocking the obvious next step:

1. **No way to draw an image through the window system.** `GraphicsDevice::draw_image(&mut self, _x: i32, _y: i32, _data: &[u8], _width: u32, _height: u32)` (`src/window/graphics.rs:90`) is a `// TODO: Implement proper image drawing` stub. Its signature is also wrong shape — raw `&[u8]` with no pixel format means callers would have to pre-decode. The legacy `Graphics::draw_image` on `core_gfx.rs:339` and `DoubleBufferedFrameBuffer::draw_image` on `double_buffer.rs:283` use a sensible shape (`&dyn Image`), but they bypass the `GraphicsDevice` abstraction entirely — they're the direct-framebuffer hack the shell startup uses (`src/commands/shell/mod.rs:48-64`).

2. **`DesktopWindow` has no image affordance.** It only knows how to fill a rectangle.

Today the path of least resistance is to widen the experimental shell hack (write directly to the double buffer, bypassing the window tree). That entrenches the very layering problem `src/graphics/CLAUDE.md` flags as known pain (item 4: "Image loading/display" is the missing layer above drawing primitives). Instead, this plan adds the missing trait method, lets `DesktopWindow` own its wallpaper bytes, and wires loading into the existing desktop bootstrap — moving the image layer onto the right side of the boundary.

The wallpaper asset itself is unremarkable: a 24-bit BMP shipped in `assets/`, picked up automatically by `build.rs:42-65` and written to the BIOS disk image at the FAT root.

---

## Requirements

- **R1** — `GraphicsDevice` exposes a method that blits a parsed `&dyn Image` at signed `(x, y)` coordinates, honoring the active clip rect and silently dropping pixels outside the device.
- **R2** — `GraphicsDevice` exposes a scaled blit so a single wallpaper asset can fill any screen size at boot. Nearest-neighbor scaling is acceptable (no interpolation requirement).
- **R3** — `DesktopWindow` renders a bundled wallpaper image as its full-screen background when one is provided, replacing the solid-color fill.
- **R4** — `DesktopWindow` falls back to the existing solid-blue fill when no wallpaper bytes are provided or when parsing fails. The kernel must boot to a working desktop in either branch.
- **R5** — A wallpaper asset is bundled with the kernel via `assets/` and loaded once during `create_default_desktop` after the filesystem is mounted.
- **R6** — The existing shell experimental BMP path (`src/commands/shell/mod.rs:33-86`) keeps working unchanged. This plan does not refactor or remove it.
- **R7** — Existing kernel tests continue to pass; new tests cover the trait method on both adapters and the desktop-window fallback.

---

## Scope Boundaries

### In scope
- Adding `draw_image` and `draw_image_scaled` to the `GraphicsDevice` trait with default implementations.
- Adding wallpaper-bytes ownership and rendering to `DesktopWindow`.
- Bundling one BMP wallpaper asset under `assets/` with an 8.3-compliant filename.
- Loading the wallpaper from the FAT root during desktop bootstrap.
- Tests for the new trait surface and desktop fallback behavior.

### Deferred to follow-up work
- Settings UI or shell command to change the wallpaper at runtime.
- Multiple wallpapers, slideshow, per-screen wallpapers.
- Tiling / centered / "fill" / "fit" mode selection. Initial implementation stretches to screen.
- Removing or refactoring the shell-startup BMP hack in `src/commands/shell/mod.rs`.
- PNG decompression (`src/graphics/images/png.rs` is header-only today).
- Adapter-specific overrides of `draw_image` for bulk-row blitting. The default per-pixel implementation is acceptable for v1; the desktop repaints only on invalidation, not every frame.
- Alpha compositing semantics for `Bgra8888` BMPs. The existing `BmpImage::get_pixel` already discards the alpha channel; this plan inherits that behavior.

### Out of scope
- Modifying the legacy `core_gfx::Graphics::draw_image` and `DoubleBufferedFrameBuffer::draw_image` paths. They stay as-is.
- Long filename support in the FAT layer.

---

## Key Technical Decisions

### Add `draw_image` to `GraphicsDevice` rather than reusing the legacy path
The legacy `Graphics::draw_image` (on `core_gfx`) and `DoubleBufferedFrameBuffer::draw_image` (on the underlying double buffer) both take `&dyn Image` and work, but they sit *below* the `GraphicsDevice` abstraction — callers who want to use them have to bypass the window tree (which is exactly what the shell hack does today). Adding the method to the trait means windows render images the same way they render rects and text, and the rendering respects the active clip rect and double-buffered swap timing.

The new trait method shape replaces the existing stub:

```text
fn draw_image(&mut self, x: i32, y: i32, image: &dyn Image);
fn draw_image_scaled(&mut self, x: i32, y: i32, width: u32, height: u32, image: &dyn Image);
```

Both have default implementations on the trait built from `draw_pixel`, mirroring how `draw_text` is defaulted today. Adapters can override later if profiling shows the per-pixel path is too slow.

The existing `_data: &[u8], _width: u32, _height: u32` stub is removed. It has zero callers (verified by grep — the only `draw_image` call site against a `GraphicsDevice` is non-existent; all live calls go through `Graphics` or `DoubleBufferedFrameBuffer`). Removing the stub avoids a dual-method confusion.

### Stretch to full screen, no letterboxing
The wallpaper file is one BMP at one resolution; the framebuffer can be any size the bootloader negotiates. Three options for handling mismatch: (a) center + fill remainder with solid color, (b) tile, (c) stretch to fit. We pick (c) — nearest-neighbor stretch via `draw_image_scaled`. Rationale: simplest visual result, no exposed background color seam, no tiling artifacts, single well-defined code path. Aspect-ratio distortion is acceptable for a default wallpaper that the user can't change yet (R-scope: not modifiable). Re-evaluate if/when wallpaper-mode selection is added.

### `DesktopWindow` owns the BMP bytes; reparses per paint
`BmpImage<'a>` borrows from a `&'a [u8]`, so the long-lived state is the raw bytes (a `Vec<u8>`). `DesktopWindow` stores `Option<Vec<u8>>`; `paint` constructs a fresh `BmpImage` view by calling `BmpImage::from_bytes(&bytes)` each repaint. Repainting only happens on invalidation (covered in `src/window/windows/desktop.rs:51-54`), not every frame, so the parse cost is paid rarely. This avoids self-referential structs and the kernel's custom `Arc` plumbing for what is essentially a one-shot byte buffer.

### Bundle the asset, don't compile it in
Two ways to ship the wallpaper: `include_bytes!` like `assets/system.ttf` (font), or place it in `assets/` and let `build.rs` copy it into the FAT root. The font has to be `include_bytes!` because `init_fonts()` runs before the filesystem is mounted. The wallpaper does not — `create_default_desktop` runs after fs init. Loading from FAT means:
- The kernel binary stays small (the existing baked TTF is 273 KB; a baked wallpaper would add hundreds of KB to every boot). 
- The boot disk image already includes assets via `build.rs:42-65` — no build infrastructure changes.
- Swapping the wallpaper later is a file replacement, not a kernel rebuild.

Filename: **`WALLPAPR.BMP`** at the FAT root. Eight characters in the basename (the FAT12/16/32 layer is 8.3 only — see `src/fs/CLAUDE.md`). Existing assets like `agentic-banner.bmp` (13-char basename) are over the limit; `banner.bmp` and `LAND3.BMP` are not. The shell already loads `/banner.bmp`, confirming the 8.3 path works at runtime.

### Load failures fall back to solid color, not panic
File-not-found, BMP parse error, allocation failure during read — all degrade to the existing solid-blue paint. A wallpaper missing because someone forgot to drop the file in `assets/` should not prevent the desktop from coming up. Rationale aligns with `.claude/rules/panic-and-attributes.md`: never panic in boot paths when a clean fallback exists.

---

## High-Level Technical Design

```mermaid
flowchart LR
    A[assets/WALLPAPR.BMP] -->|build.rs copies| B[FAT root /WALLPAPR.BMP]
    B -->|create_default_desktop reads| C[Vec u8 bytes]
    C -->|stored on| D[DesktopWindow.wallpaper]
    D -->|paint reparses| E[BmpImage view]
    E -->|GraphicsDevice::draw_image_scaled| F[Framebuffer pixels]
```

*Directional only — exact field names and call sites are decided at implementation time.*

---

## Implementation Units

### U1. Replace `GraphicsDevice::draw_image` stub with image-aware trait methods

**Goal:** Make `GraphicsDevice` capable of blitting a parsed `&dyn Image`, with a default implementation built from `draw_pixel` so existing adapters get the new behavior for free.

**Requirements:** R1, R2

**Dependencies:** none

**Files:**
- `src/window/graphics.rs` — replace the stub `draw_image(_data: &[u8], …)` with `draw_image(image: &dyn Image)` and add `draw_image_scaled`. Default implementations live on the trait.
- `src/tests/graphics_device_image_test.rs` (new) — register via `src/tests/mod.rs`.

**Approach:**
- Import `crate::graphics::images::Image` in `src/window/graphics.rs`.
- Default `draw_image` walks `0..image.height()` × `0..image.width()`, calling `image.get_pixel(x, y)` and `self.draw_pixel(target_x, target_y, color)`. Coordinates passed as `i32` so the existing clip path in adapters handles negative origins and beyond-screen blits — same contract `draw_text` already follows.
- Default `draw_image_scaled` performs nearest-neighbor scaling: for each destination `(dx, dy)` in `0..height × 0..width`, compute `sx = dx * image.width() / width`, `sy = dy * image.height() / height` using integer arithmetic (no `f32`), call `image.get_pixel(sx, sy)`, then `self.draw_pixel`. Avoids `f32` in the kernel hot path even though `core::f32` is technically available.
- Do not add adapter-specific overrides yet. The default path is correct for both `DirectFrameBufferDevice` and `DoubleBufferedDevice` because both implement `draw_pixel` with full clipping.
- Treat `Color` returned by `get_pixel` as already-decoded RGB; the trait does not need to know the source `PixelFormat`.

**Patterns to follow:**
- The `draw_text` default implementation in the same file (uses `read_pixel`/`draw_pixel` and lets adapters clip). Mirror its structure.
- Signed-coordinate clipping established in plan `2026-05-08-004-fix-window-partial-offscreen-rendering-plan.md` — drawing methods take `i32` and the adapter clips internally.

**Test scenarios:**
- `draw_image` blits a 4×4 in-memory `Image` impl onto a stub `GraphicsDevice` and the captured pixel grid matches the source pixel-for-pixel.
- `draw_image` at negative origin (e.g., `(-2, -2)`) on an 8×8 device produces the expected clipped 6×6 region with no panic and no out-of-bounds writes.
- `draw_image` at an origin partially past the right/bottom edge clips correctly with no wrap.
- `draw_image_scaled` from a 4×4 source to a 16×16 region produces nearest-neighbor 2×2 blocks matching each source pixel.
- `draw_image_scaled` from a 16×16 source to a 4×4 region samples every fourth source pixel.
- `draw_image_scaled` with `width=0` or `height=0` is a no-op (bounds check before the loop), no panic.
- Both real adapters (`DirectFrameBufferDevice`, `DoubleBufferedDevice`) are exercised — at minimum a smoke test that constructs each adapter against a synthetic framebuffer and calls `draw_image` without panicking.

**Verification:**
- `cargo build --features test` succeeds.
- `./test.sh` exits 33 (all tests pass).
- The previously stubbed `draw_image(_data, _width, _height)` signature is gone; no caller references it (verified by `cargo check`).

---

### U2. Bundle wallpaper asset in `assets/WALLPAPR.BMP`

**Goal:** Ship a 24-bit BMP wallpaper with the kernel disk image, named within the 8.3 filename limit so the FAT layer can find it.

**Requirements:** R5

**Dependencies:** none

**Files:**
- `assets/WALLPAPR.BMP` (new, binary) — 24-bit uncompressed BMP. Source picture choice is a content decision; a clean abstract or system-themed image at a common 16:9 resolution (e.g., 1280×720) works.

**Approach:**
- Place the file in `assets/`. `build.rs:42-65` already enumerates the directory and adds each file to the BIOS image at the root path `/<filename>`. No build-script changes are needed.
- Verify on first boot that QEMU can see the file via the existing shell `ls` command (already wired) — this is part of U4 verification, not part of U2 itself.
- The basename must be uppercase 8.3 because `src/fs/CLAUDE.md` documents the FAT layer as 8.3-only. `WALLPAPR.BMP` = 8 chars + `.BMP`, valid.

**Patterns to follow:**
- The existing `assets/banner.bmp`, `assets/LAND3.BMP` files. Both are 8.3-compliant and load successfully today (the shell loads `/banner.bmp` at startup).

**Test scenarios:**
- Test expectation: none — pure asset addition. Coverage comes from U3 and U4 tests that exercise the loaded bytes.

**Verification:**
- `./build.sh -n` produces `target/bootloader/bios.img` without errors.
- Booting (manually or via the U4 verification path) and running `ls /` in the shell shows `WALLPAPR.BMP` at the root.

---

### U3. Add wallpaper field and rendering to `DesktopWindow`

**Goal:** Let `DesktopWindow` optionally hold raw BMP bytes and paint them as the background, falling back to the existing solid color when bytes are absent or parsing fails.

**Requirements:** R3, R4

**Dependencies:** U1

**Files:**
- `src/window/windows/desktop.rs` — add `wallpaper: Option<alloc::vec::Vec<u8>>` field, a `with_wallpaper(bytes)` constructor variant, and a wallpaper-aware `paint` body.
- `src/tests/desktop_window_test.rs` (new) — register via `src/tests/mod.rs`.

**Approach:**
- Field shape: `wallpaper: Option<alloc::vec::Vec<u8>>`, default `None`.
- Add a constructor: `DesktopWindow::new_with_wallpaper(id: WindowId, bounds: Rect, wallpaper_bytes: Vec<u8>)` that sets the field. Keep existing `DesktopWindow::new` unchanged so tests and any non-default-desktop callers stay simple.
- In `paint`:
  1. Early-out on `!visible() || !needs_repaint()` (same as today).
  2. If `wallpaper.is_some()`, attempt `BmpImage::from_bytes(&bytes)`. On success, call `device.draw_image_scaled(bounds.x, bounds.y, bounds.width, bounds.height, &image)`. On parse failure, fall through to the solid-fill branch.
  3. Otherwise, fall back to the existing `device.fill_rect(…, self.background_color)` path.
  4. Call `clear_needs_repaint()` once at the end regardless of branch.
- Do not pre-validate `wallpaper` bytes in the constructor — defer parse to first paint. Constructor failure is awkward; paint-time fallback is graceful.
- The `BmpImage` view is constructed and dropped within `paint`; its `'a` lifetime is bounded by the `&self.wallpaper.as_ref().unwrap()[..]` slice.

**Patterns to follow:**
- Existing `paint` early-out structure already in `src/window/windows/desktop.rs:46-67`.
- `crate::lib::arc::Arc` is the kernel's `Arc` (per `.claude/rules/no-std.md`). Not used here — we own the bytes outright — but worth noting if future iterations want to share the buffer.

**Test scenarios:**
- A `DesktopWindow` constructed via `new_with_wallpaper` with valid 24-bit BMP bytes, painted onto a recording mock `GraphicsDevice`, records exactly one `draw_image_scaled` call covering the window's bounds and zero `fill_rect` calls.
- A `DesktopWindow` constructed via `new` (no wallpaper), painted onto the mock, records zero `draw_image_scaled` calls and exactly one `fill_rect` call with `Color::new(0, 50, 100)`.
- A `DesktopWindow` constructed via `new_with_wallpaper` with deliberately malformed bytes (e.g., `vec![0xFF; 16]`) falls back to `fill_rect` and does not panic.
- After `paint` returns, `needs_repaint()` is `false` for both wallpaper and fallback branches.
- Calling `paint` twice in a row, with no intervening `invalidate`, results in only one `draw_image_scaled` call total (the early-out still fires when wallpaper is present).

**Verification:**
- `cargo check` and `cargo build --features test` succeed.
- `./test.sh` exits 33.
- Manual visual check via U4: booting the kernel shows the wallpaper image instead of solid blue.

---

### U4. Load wallpaper during `create_default_desktop`

**Goal:** Read `/WALLPAPR.BMP` from the mounted FAT root once at boot and pass the bytes into `DesktopWindow::new_with_wallpaper`. On any failure, fall back to `DesktopWindow::new` so boot still completes.

**Requirements:** R4, R5

**Dependencies:** U2, U3

**Files:**
- `src/window/mod.rs` — modify `create_default_desktop` (around line 170) to attempt the wallpaper load and pick the constructor.

**Approach:**
- Add a helper `fn load_default_wallpaper() -> Option<alloc::vec::Vec<u8>>` local to `src/window/mod.rs` (or inline) that:
  1. Calls `crate::fs::File::open_read("/WALLPAPR.BMP")`. On `Err`, returns `None` and emits a `debug_info!` log line — no panic.
  2. Allocates `vec![0u8; file.size() as usize]` and reads into it. On read error, returns `None`.
  3. Returns `Some(bytes)` on success.
- In `create_default_desktop`, replace the `DesktopWindow::new(desktop_id, …)` call with a branch on `load_default_wallpaper()`.
- Order: this runs *after* `create_default_desktop` is called from `kernel.rs`. Confirm the filesystem is already mounted at that point (it is — the shell startup at `src/commands/shell/mod.rs:33-86` already reads `/banner.bmp` and runs after `create_default_desktop`). Verify by reading `src/kernel.rs` boot-order during implementation.
- Do not retry, cache, or reload — single attempt, one boot, done.

**Patterns to follow:**
- `src/commands/shell/mod.rs:38-85` — same `File::open_read` → `read into Vec` → graceful error logging pattern.

**Test scenarios:**
- Test expectation: deferred to manual integration. The window-system bootstrap touches enough static state that an in-kernel test would essentially re-implement boot. The wallpaper-bytes flow is already covered by U3; the file-load helper is thin enough that exercising it in isolation adds little. Reconsider if this becomes a regression source.

**Verification:**
- Build and boot via `./build.sh`. The desktop background shows the wallpaper image stretched to fit the screen, with the existing terminal frame on top of it.
- Boot with `assets/WALLPAPR.BMP` temporarily renamed to a non-8.3 name (so the FAT image won't expose it). Desktop comes up with the original solid blue, kernel does not panic, serial output shows the "wallpaper not found" debug line.
- Re-rename and rebuild; wallpaper returns.

---

### U5. Update subsystem CLAUDE.md notes

**Goal:** Reflect the new image rendering capability in `src/graphics/CLAUDE.md` and `src/window/CLAUDE.md` so future work doesn't re-discover the same gap.

**Requirements:** none — documentation hygiene.

**Dependencies:** U1, U3

**Files:**
- `src/graphics/CLAUDE.md` — note that `draw_image` and `draw_image_scaled` are now first-class on `GraphicsDevice`, with a per-pixel default implementation. Update the "intended next refactor" list (item 4: image loading/display) to mark this layer as partially landed.
- `src/window/CLAUDE.md` — update the `DesktopWindow` row in the window-types table to mention optional wallpaper bytes; update the default-desktop-layout section to note that the desktop loads `/WALLPAPR.BMP` if present and falls back to solid blue otherwise.

**Approach:**
- Edits-only; no new files. Keep the prose tight — these files are loaded into agent context on every visit.

**Patterns to follow:**
- Existing tone and structure of both files.

**Test scenarios:**
- Test expectation: none — documentation only.

**Verification:**
- Both files render correctly (markdown lint mental-pass).
- A future agent reading either file would learn: BMP rendering is wired through the trait, and the desktop has a wallpaper path with graceful fallback.

---

## System-Wide Impact

| Subsystem | Change | Why it matters |
|---|---|---|
| `src/graphics/images/` | None — BMP parser used as-is. | Confirms the image layer was already shaped correctly for this use. |
| `src/window/graphics.rs` | New trait surface (`draw_image`, `draw_image_scaled`); existing stub removed. | Any future image-rendering window (icons, banners, dialogs) will use this trait method, not bypass it. |
| `src/window/windows/desktop.rs` | Optional wallpaper-bytes ownership; conditional paint branch. | Establishes the pattern other windows can copy if they want background images. |
| `src/window/mod.rs` (`create_default_desktop`) | One-shot wallpaper load. | Boot-time file read; if fs is ever moved later than desktop creation, this load needs to follow. |
| `assets/` | One new BMP file. | Picked up automatically by `build.rs`; no build-system changes. |
| `src/commands/shell/mod.rs` | Untouched. | The experimental shell BMP path still bypasses the window system. Cleanup is deferred. |
| `src/drivers/display/double_buffer.rs`, `src/graphics/core_gfx.rs` | Untouched. | Legacy `draw_image` paths stay; the shell still uses them. |

---

## Risks and Mitigations

- **Risk:** Per-pixel `draw_image_scaled` for a 1280×720 wallpaper is ~921k iterations of `get_pixel` + `draw_pixel`, each taking the adapter's mutex once per pixel. Boot might feel slow on the desktop's first paint.
  - **Mitigation:** First paint runs once at boot; subsequent renders are skipped via `needs_repaint()`. If profiling shows boot is visibly delayed, override `draw_image_scaled` on `DoubleBufferedDevice` to write straight into the back buffer with one mutex acquire per row. Defer until measured.
- **Risk:** `BmpImage::from_bytes` reparses on every `paint` call. With infrequent repaints this is negligible, but if some future code path invalidates the desktop on every frame, parse cost compounds.
  - **Mitigation:** Today nothing repaints the desktop on every frame, and the parse is a few struct-pointer reads (the data slice is borrowed, not copied). If repaint frequency increases, cache the parsed view via `Arc` or a self-referential helper.
- **Risk:** The 8.3 filename limit catches this if a wallpaper is added under a longer name (e.g., `wallpaper-default.bmp`) and the build silently drops it from the FAT image.
  - **Mitigation:** Plan calls out the constraint explicitly; U2 hard-codes `WALLPAPR.BMP`. A follow-up could add a `build.rs` warning when an asset filename violates 8.3, but that's out of scope.
- **Risk:** The `BmpImage::from_bytes` parser uses `unsafe` pointer reads on the input bytes (`*(data.as_ptr() as *const BmpFileHeader)`). A malformed file could in principle cause UB if `data` is shorter than the header, though current code does check `data.len()` first.
  - **Mitigation:** Out of scope for this plan — the parser's safety is pre-existing. Worth a follow-up audit if image parsing surface widens.
- **Risk:** Aspect-ratio distortion from stretch-to-fit looks bad if the wallpaper resolution mismatches the screen significantly.
  - **Mitigation:** Pick the bundled wallpaper at a 16:9 resolution close to the QEMU default. Distortion is acceptable for a non-modifiable default; revisit when wallpaper modes are added.

---

## Open Questions Deferred to Implementation

- Exact 8.3 filename if the implementer prefers a different one (e.g., `WLPAPER.BMP`, `BG.BMP`). The plan recommends `WALLPAPR.BMP`; the implementer can pick a different valid 8.3 name without re-planning.
- Exact wallpaper image content. A picture choice, not an architectural one.
- Whether `draw_image_scaled` belongs on the legacy `core_gfx::Graphics` and `DoubleBufferedFrameBuffer` paths in addition to `GraphicsDevice`. Today the legacy `draw_image_scaled` already exists on both; no duplication needed unless a window system caller surfaces a need.

---

## Verification

- `cargo check` clean.
- `cargo build --features test` clean.
- `./test.sh` exits 33 (all tests pass) — including the new `graphics_device_image_test` and `desktop_window_test` modules.
- `./build.sh` boots into a desktop showing the wallpaper background, with the terminal frame on top, with no serial-log panics.
- Boot with the wallpaper file removed (rename `assets/WALLPAPR.BMP` to a non-8.3 name and rebuild) shows the original solid-blue background, no panic, and a "wallpaper not found" debug line on serial.
