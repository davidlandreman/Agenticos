//! Direct framebuffer adapter - writes directly to physical display memory

use bootloader_api::info::FrameBuffer;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use crate::drivers::display::frame_buffer::FrameBufferWriter;
use crate::window::{GraphicsDevice, Rect, ColorDepth};
use crate::window::adapters::clip::{clip_line, clip_rect, pixel_visible};
use spin::Mutex;

/// Graphics device that writes directly to the physical framebuffer
/// This is the simplest implementation with no buffering
pub struct DirectFrameBufferDevice {
    /// The underlying framebuffer writer
    writer: Mutex<FrameBufferWriter>,
    /// Current clipping rectangle
    clip_rect: Option<Rect>,
    /// Device dimensions
    width: usize,
    height: usize,
}

impl DirectFrameBufferDevice {
    /// Create a new direct framebuffer device
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let (width, height) = {
            let info = framebuffer.info();
            (info.width, info.height)
        };

        let writer = FrameBufferWriter::new(framebuffer);

        DirectFrameBufferDevice {
            writer: Mutex::new(writer),
            clip_rect: None,
            width,
            height,
        }
    }
}

impl GraphicsDevice for DirectFrameBufferDevice {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32 // Most modern framebuffers are 32-bit
    }

    fn clear(&mut self, color: Color) {
        let mut writer = self.writer.lock();
        writer.clear(color);
    }

    fn draw_pixel(&mut self, x: i32, y: i32, color: Color) {
        if let Some((px, py)) = pixel_visible(x, y, self.width, self.height, self.clip_rect.as_ref()) {
            self.writer.lock().draw_pixel(px, py, color);
        }
    }

    fn read_pixel(&self, x: i32, y: i32) -> Color {
        match pixel_visible(x, y, self.width, self.height, self.clip_rect.as_ref()) {
            Some((px, py)) => self.writer.lock().get_pixel(px, py),
            None => Color::BLACK,
        }
    }

    fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Color) {
        let Some(((cx1, cy1), (cx2, cy2))) =
            clip_line(x1, y1, x2, y2, self.width, self.height, self.clip_rect.as_ref())
        else {
            return;
        };

        // Bresenham over the clipped endpoints. The clip guarantees both
        // endpoints sit inside `[0, width) × [0, height)`, so per-pixel writes
        // are unconditionally in-range.
        let dx = (cx2 - cx1).abs();
        let dy = (cy2 - cy1).abs();
        let sx: i32 = if cx1 < cx2 { 1 } else { -1 };
        let sy: i32 = if cy1 < cy2 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = cx1;
        let mut y = cy1;
        let mut writer = self.writer.lock();
        loop {
            writer.draw_pixel(x as usize, y as usize, color);
            if x == cx2 && y == cy2 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
        // Four edges as lines; the line clipper handles partial visibility.
        let right = x + width as i32 - 1;
        let bottom = y + height as i32 - 1;
        self.draw_line(x, y, right, y, color);
        self.draw_line(right, y, right, bottom, color);
        self.draw_line(right, bottom, x, bottom, color);
        self.draw_line(x, bottom, x, y, color);
    }

    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if let Some((cx, cy, cw, ch)) =
            clip_rect(x, y, width, height, self.width, self.height, self.clip_rect.as_ref())
        {
            self.writer.lock().fill_rect(cx, cy, cw, ch, color);
        }
    }

    fn draw_text(&mut self, x: i32, y: i32, text: &str, font: &dyn Font, color: Color) {
        let char_width = font.char_width();
        let char_height = font.char_height();
        let bytes_per_row = font.bytes_per_row();

        let mut current_x = x;
        for ch in text.chars() {
            // Per-glyph pre-clip: skip glyphs whose bounding box is fully off
            // the visible region; for fully-inside glyphs, write pixels
            // unconditionally; for partial glyphs, fall through to per-pixel
            // visibility checks.
            let glyph_box = clip_rect(
                current_x,
                y,
                char_width as u32,
                char_height as u32,
                self.width,
                self.height,
                self.clip_rect.as_ref(),
            );
            if glyph_box.is_none() {
                current_x += char_width as i32;
                continue;
            }
            let fully_inside = glyph_box
                .map(|(_, _, cw, ch)| cw == char_width && ch == char_height)
                .unwrap_or(false);

            if let Some(bitmap) = font.get_char_bitmap(ch) {
                let mut writer = self.writer.lock();
                for row in 0..char_height {
                    for col in 0..char_width {
                        let byte_index = row * bytes_per_row + col / 8;
                        let bit_index = 7 - (col % 8);
                        if byte_index < bitmap.len() && (bitmap[byte_index] & (1 << bit_index)) != 0 {
                            let px = current_x + col as i32;
                            let py = y + row as i32;
                            if fully_inside {
                                writer.draw_pixel(px as usize, py as usize, color);
                            } else if let Some((ux, uy)) = pixel_visible(
                                px,
                                py,
                                self.width,
                                self.height,
                                self.clip_rect.as_ref(),
                            ) {
                                writer.draw_pixel(ux, uy, color);
                            }
                        }
                    }
                }
            }
            current_x += char_width as i32;
        }
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        self.clip_rect = rect;
    }

    fn flush(&mut self) {
        // Direct framebuffer doesn't need flushing
    }
}
