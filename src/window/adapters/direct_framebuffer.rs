//! Direct framebuffer adapter - writes directly to physical display memory

use bootloader_api::info::FrameBuffer;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use crate::drivers::display::frame_buffer::FrameBufferWriter;
use crate::window::{GraphicsDevice, Rect, ColorDepth};
use spin::Mutex;
use alloc::boxed::Box;

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
    
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if self.is_clipped(x, y) {
            return;
        }
        
        let mut writer = self.writer.lock();
        writer.draw_pixel(x, y, color);
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
        let mut writer = self.writer.lock();
        writer.fill_rect(x, y, width, height, color);
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
        // Direct framebuffer doesn't need flushing
    }
}