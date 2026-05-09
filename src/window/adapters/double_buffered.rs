//! Double-buffered framebuffer adapter - provides smooth rendering with back buffer

use bootloader_api::info::{FrameBuffer, PixelFormat};
use crate::graphics::color::Color;
use crate::graphics::images::Image;
use crate::drivers::display::double_buffer::DoubleBufferedFrameBuffer;
use crate::window::{GraphicsDevice, Rect, ColorDepth};
use crate::window::graphics::Snapshot;
use crate::window::adapters::clip::{clip_line, clip_rect, pixel_visible};
use spin::Mutex;

/// Graphics device that uses double buffering for smooth rendering
pub struct DoubleBufferedDevice {
    /// The underlying double-buffered framebuffer
    buffer: Mutex<DoubleBufferedFrameBuffer>,
    /// Current clipping rectangle
    clip_rect: Option<Rect>,
    /// Device dimensions
    width: usize,
    height: usize,
    /// Whether the buffer has been modified and needs flushing
    dirty: bool,
}

impl DoubleBufferedDevice {
    /// Create a new double-buffered device
    ///
    /// Note: This requires a pre-allocated back buffer. In the current implementation,
    /// this comes from a static 8MB buffer.
    pub fn new(framebuffer: &'static mut FrameBuffer, back_buffer: &'static mut [u8]) -> Self {
        let (width, height) = {
            let info = framebuffer.info();
            (info.width, info.height)
        };

        let buffer = DoubleBufferedFrameBuffer::new(framebuffer, back_buffer);

        DoubleBufferedDevice {
            buffer: Mutex::new(buffer),
            clip_rect: None,
            width,
            height,
            dirty: false,
        }
    }

    /// Create using the global static back buffer
    pub fn new_with_static_buffer(framebuffer: &'static mut FrameBuffer) -> Self {
        // Get the static buffer from the display module
        let back_buffer = unsafe {
            crate::drivers::display::double_buffered_text::get_static_back_buffer()
        };

        Self::new(framebuffer, back_buffer)
    }
}

impl GraphicsDevice for DoubleBufferedDevice {
    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32
    }

    fn clear(&mut self, color: Color) {
        let mut buffer = self.buffer.lock();
        buffer.clear(color);
        drop(buffer);
        self.dirty = true;
    }

    fn draw_pixel(&mut self, x: i32, y: i32, color: Color) {
        if let Some((px, py)) = pixel_visible(x, y, self.width, self.height, self.clip_rect.as_ref()) {
            self.buffer.lock().draw_pixel(px, py, color);
            self.dirty = true;
        }
    }

    fn read_pixel(&self, x: i32, y: i32) -> Color {
        match pixel_visible(x, y, self.width, self.height, self.clip_rect.as_ref()) {
            Some((px, py)) => self.buffer.lock().get_pixel(px, py),
            None => Color::BLACK,
        }
    }

    fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Color) {
        let Some(((cx1, cy1), (cx2, cy2))) =
            clip_line(x1, y1, x2, y2, self.width, self.height, self.clip_rect.as_ref())
        else {
            return;
        };

        let dx = (cx2 - cx1).abs();
        let dy = (cy2 - cy1).abs();
        let sx: i32 = if cx1 < cx2 { 1 } else { -1 };
        let sy: i32 = if cy1 < cy2 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = cx1;
        let mut y = cy1;
        let mut buffer = self.buffer.lock();
        loop {
            buffer.draw_pixel(x as usize, y as usize, color);
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
        drop(buffer);
        self.dirty = true;
    }

    fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
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
            self.buffer.lock().fill_rect(cx, cy, cw, ch, color);
            self.dirty = true;
        }
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        self.clip_rect = rect;
    }

    /// Bulk image blit. Acquires the back-buffer lock once for the entire
    /// image rather than once per pixel as the trait default would.
    fn draw_image(&mut self, x: i32, y: i32, image: &dyn Image) {
        let img_w = image.width() as u32;
        let img_h = image.height() as u32;
        if img_w == 0 || img_h == 0 {
            return;
        }

        let Some((dst_x, dst_y, dst_w, dst_h)) = clip_rect(
            x, y, img_w, img_h, self.width, self.height, self.clip_rect.as_ref(),
        ) else {
            return;
        };

        // When `x` or `y` is negative the destination start was clipped forward;
        // the offset tells us which source pixel maps to the new dst origin.
        let src_off_x = (dst_x as i64 - x as i64) as usize;
        let src_off_y = (dst_y as i64 - y as i64) as usize;

        let mut buffer = self.buffer.lock();
        for dy in 0..dst_h {
            let sy = src_off_y + dy;
            for dx in 0..dst_w {
                let sx = src_off_x + dx;
                if let Some(color) = image.get_pixel(sx, sy) {
                    buffer.draw_pixel(dst_x + dx, dst_y + dy, color);
                }
            }
        }
        drop(buffer);
        self.dirty = true;
    }

    /// Bulk scaled image blit. Same one-lock pattern as `draw_image`.
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

        let Some((dst_x, dst_y, dst_w, dst_h)) = clip_rect(
            x, y, width, height, self.width, self.height, self.clip_rect.as_ref(),
        ) else {
            return;
        };

        let h_us = height as usize;
        let w_us = width as usize;

        let mut buffer = self.buffer.lock();
        for py in 0..dst_h {
            // Map back to the unclipped destination coordinate, then to the
            // source pixel using nearest-neighbor.
            let unclipped_dy = (dst_y as i64 - y as i64) as usize + py;
            let sy = (unclipped_dy * src_h) / h_us;
            for px in 0..dst_w {
                let unclipped_dx = (dst_x as i64 - x as i64) as usize + px;
                let sx = (unclipped_dx * src_w) / w_us;
                if let Some(color) = image.get_pixel(sx, sy) {
                    buffer.draw_pixel(dst_x + px, dst_y + py, color);
                }
            }
        }
        drop(buffer);
        self.dirty = true;
    }

    fn flush(&mut self) {
        // Only swap buffers if we actually drew something
        if self.dirty {
            let mut buffer = self.buffer.lock();
            buffer.swap_buffers();
            drop(buffer);
            self.dirty = false;
        }
    }

    fn snapshot(&self) -> Option<Snapshot> {
        let buffer = self.buffer.lock();
        let (width, height, stride, bytes_per_pixel, pixel_format, pixels) =
            buffer.snapshot_bytes();
        Some(Snapshot {
            width,
            height,
            stride,
            bytes_per_pixel,
            pixel_format,
            pixels,
        })
    }

    fn pixel_format(&self) -> PixelFormat {
        self.buffer.lock().pixel_format()
    }

    fn bytes_per_pixel(&self) -> usize {
        self.buffer.lock().bytes_per_pixel()
    }

    fn stride(&self) -> usize {
        self.buffer.lock().stride()
    }
}
