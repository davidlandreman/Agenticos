//! Double-buffered framebuffer adapter - provides smooth rendering with back buffer

use bootloader_api::info::FrameBuffer;
use crate::graphics::color::Color;
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
}
