//! Graphics device abstraction for the window system

use bootloader_api::info::PixelFormat;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use crate::graphics::images::Image;
use super::types::{Rect, ColorDepth};

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
    #[expect(dead_code, reason = "intentional kernel API surface")]
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
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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

    /// Flush only the supplied screen regions when the adapter supports
    /// partial presentation. The default preserves correctness for devices
    /// that only support whole-frame flushes.
    fn flush_regions(&mut self, _regions: &[Rect]) {
        self.flush();
    }

    /// Pixel byte order used by this device's underlying framebuffer.
    ///
    /// Used by windows that maintain framebuffer-native backing stores
    /// (see `WindowBuffer`) so subsequent blits become row `memcpy`. The
    /// default is `Bgr` — matches the historical fallback in
    /// `DoubleBufferedFrameBuffer::draw_pixel`'s wildcard arm and is the
    /// common QEMU/UEFI shape. Adapters with a known different format
    /// (real RGB framebuffers, headless test fakes) should override.
    fn pixel_format(&self) -> PixelFormat {
        PixelFormat::Bgr
    }

    /// Bytes occupied by each pixel in the device's framebuffer (typically
    /// 4 — three color bytes plus one padding byte). The default matches
    /// the common 32-bit framebuffer; adapters with a different backing
    /// memory layout should override.
    fn bytes_per_pixel(&self) -> usize {
        4
    }

    /// Pixel stride between consecutive rows, in pixels. May exceed `width`
    /// when the framebuffer is padded for alignment. The default mirrors
    /// the device width, which is correct for tightly-packed framebuffers.
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn stride(&self) -> usize {
        self.width()
    }

    /// Blit a `WindowBuffer` to `(x, y)`, honoring the active clip rect and
    /// device bounds. The trait default walks every pixel through
    /// `draw_pixel` (correct but slow); adapters that share their pixel
    /// format with the buffer can override for a row `memcpy`.
    ///
    /// Used by the opt-in compositor path to render windows whose
    /// `wants_backing_store()` returns true — see `Window::backing_store`.
    fn blit_buffer(&mut self, x: i32, y: i32, buffer: &WindowBuffer) {
        for by in 0..buffer.height {
            let dst_y = y + by as i32;
            let row_start = buffer.row_byte_offset(by);
            for bx in 0..buffer.width {
                let off = row_start + bx * buffer.bytes_per_pixel;
                let bytes = &buffer.pixels[off..off + 3];
                let color = match buffer.pixel_format {
                    PixelFormat::Rgb => Color::new(bytes[0], bytes[1], bytes[2]),
                    _ => Color::new(bytes[2], bytes[1], bytes[0]),
                };
                self.draw_pixel(x + bx as i32, dst_y, color);
            }
        }
    }
}

/// Framebuffer-native pixel buffer for windows that opt into the backing-
/// store compositor (see `Window::wants_backing_store`).
///
/// Pixels are stored in the same byte layout as the target framebuffer —
/// three color bytes per pixel slot in `pixel_format` order, with a
/// fourth padding byte left unwritten. Each row spans `stride_pixels *
/// bytes_per_pixel` bytes; rows are tightly packed by default
/// (`stride_pixels == width`) but the buffer can hold a wider stride
/// when needed.
///
/// Storing in framebuffer-native format means the compositor blit is a
/// straight `memcpy` per row — the price of the layout choice is paid
/// once at rasterization (`write_pixel`), not every frame.
pub struct WindowBuffer {
    /// Raw pixel bytes — `height * stride_pixels * bytes_per_pixel` long.
    pub pixels: alloc::vec::Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Pixel stride (in pixels). May exceed `width` when the source
    /// framebuffer is padded.
    pub stride_pixels: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

impl WindowBuffer {
    /// Create a tightly-packed (`stride == width`) buffer matching the
    /// given format. Pixel bytes are zero-initialized.
    pub fn new(
        width: usize,
        height: usize,
        pixel_format: PixelFormat,
        bytes_per_pixel: usize,
    ) -> Self {
        let stride_pixels = width;
        let len = height
            .saturating_mul(stride_pixels)
            .saturating_mul(bytes_per_pixel);
        WindowBuffer {
            pixels: alloc::vec![0u8; len],
            width,
            height,
            stride_pixels,
            bytes_per_pixel,
            pixel_format,
        }
    }

    /// Construct a buffer matching the device's reported pixel format. Use
    /// this from any `Window::paint_into_backing_store` impl that wants to
    /// be format-agnostic.
    pub fn for_device(width: usize, height: usize, device: &dyn GraphicsDevice) -> Self {
        Self::new(width, height, device.pixel_format(), device.bytes_per_pixel())
    }

    /// Reallocate to new dimensions if they differ from the current size.
    /// Returns `true` when reallocation happened (and pixels were zeroed).
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn resize_to(&mut self, width: usize, height: usize) -> bool {
        if self.width == width && self.height == height {
            return false;
        }
        self.width = width;
        self.height = height;
        self.stride_pixels = width;
        let len = height
            .saturating_mul(self.stride_pixels)
            .saturating_mul(self.bytes_per_pixel);
        self.pixels = alloc::vec![0u8; len];
        true
    }

    /// Total backing-store size in bytes.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn byte_len(&self) -> usize {
        self.height * self.stride_pixels * self.bytes_per_pixel
    }

    /// Byte offset of the start of row `y`. Out-of-range `y` returns 0;
    /// callers that pass in-range coordinates get a valid offset and
    /// callers that don't aren't trying to read pixels.
    #[inline]
    pub fn row_byte_offset(&self, y: usize) -> usize {
        y.saturating_mul(self.stride_pixels)
            .saturating_mul(self.bytes_per_pixel)
    }

    /// Write a single pixel using the buffer's `pixel_format`. Out-of-
    /// range coordinates are silently ignored — matches the existing
    /// `draw_pixel` clipping contract.
    pub fn write_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = self.row_byte_offset(y) + x * self.bytes_per_pixel;
        match self.pixel_format {
            PixelFormat::Rgb => {
                self.pixels[offset] = color.red;
                self.pixels[offset + 1] = color.green;
                self.pixels[offset + 2] = color.blue;
            }
            PixelFormat::Bgr => {
                self.pixels[offset] = color.blue;
                self.pixels[offset + 1] = color.green;
                self.pixels[offset + 2] = color.red;
            }
            // Mirror DoubleBufferedFrameBuffer::draw_pixel's wildcard arm
            // (treat unknown formats as BGR-shaped). Keeps the back buffer
            // and backing store byte-identical so the blit stays a
            // memcpy.
            _ => {
                self.pixels[offset] = color.blue;
                self.pixels[offset + 1] = color.green;
                self.pixels[offset + 2] = color.red;
            }
        }
    }

    /// Borrow row `y`'s bytes (length = `width * bytes_per_pixel`). Used
    /// by the compositor's blit path to feed `ptr::copy_nonoverlapping`.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn row_bytes(&self, y: usize) -> &[u8] {
        let start = self.row_byte_offset(y);
        let len = self.width * self.bytes_per_pixel;
        &self.pixels[start..start + len]
    }
}
