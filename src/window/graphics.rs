//! Graphics device abstraction for the window system

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::Font;
use super::types::{Rect, ColorDepth};

/// Abstract interface for graphics rendering
/// 
/// Note: All implementations ultimately write to the single physical framebuffer
/// provided by the bootloader. Different implementations may add buffering or
/// other features, but they all share the same underlying hardware.
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
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color);

    /// Read a pixel at the given position
    fn read_pixel(&self, x: usize, y: usize) -> Color;
    
    /// Draw a line between two points
    fn draw_line(&mut self, x1: usize, y1: usize, x2: usize, y2: usize, color: Color);
    
    /// Draw a rectangle outline
    fn draw_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color);
    
    /// Fill a rectangle with a color
    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color);
    
    /// Draw text at a position. `(x, y)` is the top-left of the text cell;
    /// the baseline is `y + font.ascent()`.
    ///
    /// The default implementation walks `text` glyph-by-glyph, blending 8bpp
    /// coverage against existing framebuffer contents via `read_pixel` /
    /// `draw_pixel`. Adapters that can do faster bulk blits may override.
    fn draw_text(&mut self, x: usize, y: usize, text: &str, font: &dyn Font, color: Color) {
        let baseline = y as i32 + font.ascent() as i32;
        let mut pen_x = x as i32;
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
                if dst_y < 0 {
                    continue;
                }
                for col in 0..width {
                    let dst_x = glyph_x + col;
                    if dst_x < 0 {
                        continue;
                    }
                    let alpha = glyph.coverage[(row * width + col) as usize];
                    if alpha == 0 {
                        continue;
                    }
                    let dst_x = dst_x as usize;
                    let dst_y = dst_y as usize;
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
    
    /// Draw an image (for future use)
    fn draw_image(&mut self, _x: usize, _y: usize, _data: &[u8], _width: usize, _height: usize) {
        // Default implementation for now
        // TODO: Implement proper image drawing
    }
    
    /// Set the clipping rectangle for drawing operations
    fn set_clip_rect(&mut self, rect: Option<Rect>);
    
    /// Flush any pending operations (for double-buffered implementations)
    fn flush(&mut self);
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