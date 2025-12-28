//! Double-buffered framebuffer adapter - provides smooth rendering with back buffer

use bootloader_api::info::FrameBuffer;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use crate::drivers::display::double_buffer::DoubleBufferedFrameBuffer;
use crate::window::{GraphicsDevice, Rect, ColorDepth};
use spin::Mutex;
use alloc::boxed::Box;

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
    
    /// Check if a point is within the clip rectangle
    fn is_clipped(&self, x: usize, y: usize) -> bool {
        if let Some(clip) = &self.clip_rect {
            x < clip.x as usize || 
            y < clip.y as usize ||
            x >= (clip.x + clip.width as i32) as usize ||
            y >= (clip.y + clip.height as i32) as usize
        } else {
            false
        }
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
        drop(buffer); // Release lock before setting dirty
        self.dirty = true;
    }
    
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if self.is_clipped(x, y) {
            return;
        }

        let mut buffer = self.buffer.lock();
        buffer.draw_pixel(x, y, color);
        drop(buffer); // Release lock before setting dirty
        self.dirty = true;
    }

    fn read_pixel(&self, x: usize, y: usize) -> Color {
        if self.is_clipped(x, y) || x >= self.width || y >= self.height {
            return Color::BLACK;
        }
        let buffer = self.buffer.lock();
        buffer.get_pixel(x, y)
    }
    
    fn draw_line(&mut self, x1: usize, y1: usize, x2: usize, y2: usize, color: Color) {
        // Simple Bresenham line algorithm
        let dx = (x2 as i32 - x1 as i32).abs();
        let dy = (y2 as i32 - y1 as i32).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut err = dx - dy;
        
        let mut x = x1 as i32;
        let mut y = y1 as i32;
        
        loop {
            self.draw_pixel(x as usize, y as usize, color);
            
            if x == x2 as i32 && y == y2 as i32 {
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
    
    fn draw_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        // Draw four lines
        self.draw_line(x, y, x + width - 1, y, color);
        self.draw_line(x + width - 1, y, x + width - 1, y + height - 1, color);
        self.draw_line(x + width - 1, y + height - 1, x, y + height - 1, color);
        self.draw_line(x, y + height - 1, x, y, color);
    }
    
    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        let mut buffer = self.buffer.lock();
        buffer.fill_rect(x, y, width, height, color);
        drop(buffer);
        self.dirty = true;
    }
    
    fn draw_text(&mut self, x: usize, y: usize, text: &str, font: &dyn Font, color: Color) {
        // Render text character by character
        let char_width = font.char_width();
        let char_height = font.char_height();
        let bytes_per_row = font.bytes_per_row();
        
        let mut current_x = x;
        for ch in text.chars() {
            if let Some(bitmap) = font.get_char_bitmap(ch) {
                // Draw character bitmap
                for row in 0..char_height {
                    for col in 0..char_width {
                        let byte_index = row * bytes_per_row + col / 8;
                        let bit_index = 7 - (col % 8);
                        
                        if byte_index < bitmap.len() && (bitmap[byte_index] & (1 << bit_index)) != 0 {
                            let px = current_x + col;
                            let py = y + row;
                            if !self.is_clipped(px, py) {
                                self.draw_pixel(px, py, color);
                            }
                        }
                    }
                }
            }
            current_x += char_width;
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
}