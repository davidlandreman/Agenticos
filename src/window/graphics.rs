//! Graphics device abstraction for the window system

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use crate::graphics::images::Image;
use super::types::{Rect, ColorDepth};

/// Owned snapshot of the device's current pixels plus enough metadata to
/// reconstruct an image on the host. Returned by [`GraphicsDevice::snapshot`].
pub struct Snapshot {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    /// `"rgb"`, `"bgr"`, or `"u8"` — matches the bootloader's `PixelFormat`.
    pub pixel_format: &'static str,
    pub pixels: alloc::vec::Vec<u8>,
}

/// Abstract interface for graphics rendering.
///
/// All implementations ultimately write to the single physical framebuffer
/// provided by the bootloader. Different implementations may add buffering or
/// other features, but they all share the same underlying hardware.
///
/// **Coordinate contract.** Drawing primitives accept signed `i32` positions
/// that may be negative or beyond the device's pixel grid; widths and heights
/// are unsigned. Callers do not need to clamp — the adapter clips against its
/// own dimensions and the active `clip_rect` and silently drops pixels that
/// fall outside the visible region. `width()` and `height()` device queries
/// remain `usize` because they are always non-negative.
pub trait GraphicsDevice: Send {
    /// Get the width of the device in pixels
    fn width(&self) -> usize;

    /// Get the height of the device in pixels
    fn height(&self) -> usize;

    /// Get the color depth of the device
    fn color_depth(&self) -> ColorDepth;

    /// Clear the entire device with a color
    fn clear(&mut self, color: Color);

    /// Draw a single pixel
    fn draw_pixel(&mut self, x: i32, y: i32, color: Color);

    /// Read a pixel at the given position. Returns `Color::BLACK` when the
    /// position is outside the device or active clip rect.
    fn read_pixel(&self, x: i32, y: i32) -> Color;

    /// Draw a line between two points
    fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Color);

    /// Draw a rectangle outline
    fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color);

    /// Fill a rectangle with a color
    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color);

    /// Draw text at a position. `(x, y)` is the top-left of the text cell;
    /// the baseline is `y + font.ascent()`.
    ///
    /// The default implementation walks `text` glyph-by-glyph, blending 8bpp
    /// coverage against existing framebuffer contents via `read_pixel` /
    /// `draw_pixel`. Both primitives clip against the device bounds and the
    /// active clip rect, so the loop emits raw `i32` coordinates without
    /// pre-checking. Adapters that can do faster bulk blits may override.
    fn draw_text(&mut self, x: i32, y: i32, text: &str, font: &dyn Font, color: Color) {
        let baseline = y + font.ascent() as i32;
        let mut pen_x = x;
        for ch in text.chars() {
            if ch == '\n' {
                break;
            }
            let Some(glyph) = font.glyph(ch) else { continue };
            let glyph_x = pen_x + glyph.x_offset;
            let glyph_y = baseline + glyph.y_offset;
            let width = glyph.width as i32;
            let height = glyph.height as i32;
            for row in 0..height {
                let dst_y = glyph_y + row;
                for col in 0..width {
                    let dst_x = glyph_x + col;
                    let alpha = glyph.coverage[(row * width + col) as usize];
                    if alpha == 0 {
                        continue;
                    }
                    if alpha == 0xFF {
                        self.draw_pixel(dst_x, dst_y, color);
                    } else {
                        let bg = self.read_pixel(dst_x, dst_y);
                        self.draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
                    }
                }
            }
            pen_x += glyph.advance as i32;
        }
    }


    /// Blit a parsed image at `(x, y)` at its native resolution.
    ///
    /// The default implementation walks every source pixel and forwards it to
    /// `draw_pixel`, which clips against the device bounds and the active clip
    /// rect. Adapters that can do bulk row blits may override.
    fn draw_image(&mut self, x: i32, y: i32, image: &dyn Image) {
        let height = image.height();
        let width = image.width();
        for img_y in 0..height {
            let dst_y = y + img_y as i32;
            for img_x in 0..width {
                if let Some(color) = image.get_pixel(img_x, img_y) {
                    self.draw_pixel(x + img_x as i32, dst_y, color);
                }
            }
        }
    }

    /// Blit a parsed image at `(x, y)` scaled to `width × height` using
    /// nearest-neighbor sampling. Coordinates and clipping follow the same
    /// contract as `draw_pixel`.
    fn draw_image_scaled(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        image: &dyn Image,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let src_w = image.width();
        let src_h = image.height();
        if src_w == 0 || src_h == 0 {
            return;
        }
        for dy in 0..height {
            let sy = (dy as usize * src_h) / height as usize;
            let dst_y = y + dy as i32;
            for dx in 0..width {
                let sx = (dx as usize * src_w) / width as usize;
                if let Some(color) = image.get_pixel(sx, sy) {
                    self.draw_pixel(x + dx as i32, dst_y, color);
                }
            }
        }
    }

    /// Set the clipping rectangle for drawing operations
    fn set_clip_rect(&mut self, rect: Option<Rect>);

    /// Flush any pending operations (for double-buffered implementations)
    fn flush(&mut self);

    /// Snapshot the device's current pixels into an owned buffer. Default
    /// returns `None`; adapters that back the framebuffer override this.
    /// Used by the `screenshot` tool.
    fn snapshot(&self) -> Option<Snapshot> {
        None
    }
}

/// Window buffer for per-window rendering
pub struct WindowBuffer {
    /// RGBA pixel data
    pub pixels: alloc::vec::Vec<u32>,
    /// Buffer width
    pub width: usize,
    /// Buffer height
    pub height: usize,
    /// Dirty region that needs redrawing
    pub dirty_region: Option<Rect>,
}

impl WindowBuffer {
    /// Create a new window buffer
    pub fn new(width: usize, height: usize) -> Self {
        let pixels = alloc::vec![0u32; width * height];
        WindowBuffer {
            pixels,
            width,
            height,
            dirty_region: None,
        }
    }

    /// Mark a region as dirty
    pub fn mark_dirty(&mut self, rect: Rect) {
        self.dirty_region = match self.dirty_region {
            None => Some(rect),
            Some(existing) => {
                // Expand dirty region to include new rect
                let x1 = existing.x.min(rect.x);
                let y1 = existing.y.min(rect.y);
                let x2 = (existing.x + existing.width as i32).max(rect.x + rect.width as i32);
                let y2 = (existing.y + existing.height as i32).max(rect.y + rect.height as i32);
                Some(Rect::new(x1, y1, (x2 - x1) as u32, (y2 - y1) as u32))
            }
        };
    }

    /// Clear the dirty region
    pub fn clear_dirty(&mut self) {
        self.dirty_region = None;
    }
}
