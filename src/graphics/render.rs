//! Render target abstraction for efficient drawing operations.
//!
//! This module provides the RenderTarget trait which is the primary
//! interface for drawing operations. It emphasizes:
//! - Row-based operations for efficiency
//! - Unified text rendering
//! - Clipping support

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::{Font, Glyph};
use crate::window::types::Rect;

/// A target surface that can be rendered to.
///
/// This trait provides optimized drawing primitives that work
/// in terms of rows rather than individual pixels where possible.
pub trait RenderTarget {
    /// Get the width of the render target in pixels.
    fn width(&self) -> usize;

    /// Get the height of the render target in pixels.
    fn height(&self) -> usize;

    /// Draw a single pixel at (x, y).
    ///
    /// This is the primitive operation - prefer bulk operations when possible.
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color);

    /// Fill a horizontal span with a color (optimized).
    ///
    /// Default implementation uses draw_pixel, but implementations should
    /// override this with row-based slice operations for efficiency.
    fn fill_span(&mut self, x: usize, y: usize, width: usize, color: Color) {
        let x_end = (x + width).min(self.width());
        for px in x..x_end {
            self.draw_pixel(px, y, color);
        }
    }

    /// Fill a rectangle with a solid color.
    ///
    /// Uses fill_span for row-based efficiency.
    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        let y_end = (y + height).min(self.height());
        for py in y..y_end {
            self.fill_span(x, py, width, color);
        }
    }

    /// Draw a rectangle outline.
    fn draw_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        if width == 0 || height == 0 {
            return;
        }

        // Top and bottom edges
        self.fill_span(x, y, width, color);
        if height > 1 {
            self.fill_span(x, y + height - 1, width, color);
        }

        // Left and right edges (excluding corners already drawn)
        for py in y + 1..y + height - 1 {
            self.draw_pixel(x, py, color);
            if width > 1 {
                self.draw_pixel(x + width - 1, py, color);
            }
        }
    }

    /// Draw a line between two points using Bresenham's algorithm.
    fn draw_line(&mut self, x1: usize, y1: usize, x2: usize, y2: usize, color: Color) {
        let dx = (x2 as i32 - x1 as i32).abs();
        let dy = (y2 as i32 - y1 as i32).abs();
        let sx = if x1 < x2 { 1i32 } else { -1i32 };
        let sy = if y1 < y2 { 1i32 } else { -1i32 };
        let mut err = dx - dy;

        let mut x = x1 as i32;
        let mut y = y1 as i32;

        loop {
            if x >= 0 && y >= 0 {
                self.draw_pixel(x as usize, y as usize, color);
            }

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

    /// Clear the entire target with a color.
    fn clear(&mut self, color: Color) {
        let width = self.width();
        let height = self.height();
        self.fill_rect(0, 0, width, height, color);
    }

    /// Blit an 8bpp coverage glyph onto this target at `(x, y)` (bitmap
    /// top-left). This trait has no `read_pixel`, so partial alpha is
    /// thresholded at 128 — adequate for the legacy text path that uses it.
    fn blit_glyph(&mut self, x: i32, y: i32, glyph: &Glyph, color: Color) {
        if glyph.coverage.is_empty() || glyph.width == 0 || glyph.height == 0 {
            return;
        }
        let width = glyph.width as i32;
        let height = glyph.height as i32;
        for row in 0..height {
            let dst_y = y + row;
            if dst_y < 0 {
                continue;
            }
            for col in 0..width {
                let dst_x = x + col;
                if dst_x < 0 {
                    continue;
                }
                let alpha = glyph.coverage[(row * width + col) as usize];
                if alpha < 128 {
                    continue;
                }
                let dst_x = dst_x as usize;
                let dst_y = dst_y as usize;
                if dst_x < self.width() && dst_y < self.height() {
                    self.draw_pixel(dst_x, dst_y, color);
                }
            }
        }
    }

    /// Draw text at a position using the specified font. `(x, y)` is the
    /// top-left of the text cell; the baseline is `y + font.ascent()`.
    fn draw_text(&mut self, x: usize, y: usize, text: &str, font: &dyn Font, color: Color) {
        let baseline = y as i32 + font.ascent() as i32;
        let mut pen_x = x as i32;
        for ch in text.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let Some(glyph) = font.glyph(ch) else { continue };
            self.blit_glyph(pen_x + glyph.x_offset, baseline + glyph.y_offset, &glyph, color);
            pen_x += glyph.advance as i32;
        }
    }

    /// Draw text with background color (fills each cell first).
    fn draw_text_with_bg(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        font: &dyn Font,
        fg_color: Color,
        bg_color: Color,
    ) {
        let cell_width = font.cell_width() as usize;
        let line_height = font.line_height() as usize;
        let baseline = y as i32 + font.ascent() as i32;
        let mut pen_x = x as i32;
        for ch in text.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            self.fill_rect(pen_x.max(0) as usize, y, cell_width, line_height, bg_color);
            let Some(glyph) = font.glyph(ch) else {
                pen_x += cell_width as i32;
                continue;
            };
            self.blit_glyph(pen_x + glyph.x_offset, baseline + glyph.y_offset, &glyph, fg_color);
            pen_x += glyph.advance as i32;
        }
    }

    /// Scroll a region up by the specified number of pixels.
    ///
    /// The bottom portion is filled with the clear color.
    fn scroll_up(&mut self, region: Rect, pixels: usize, clear_color: Color);
}

/// Context for painting within a specific window.
///
/// Provides coordinate translation and clipping for window-local drawing.
pub struct PaintContext<'a> {
    /// The underlying render target
    target: &'a mut dyn RenderTarget,
    /// Window's bounds in global coordinates
    bounds: Rect,
    /// Optional clip region (intersection of window and dirty region)
    clip: Option<Rect>,
    /// Offset for coordinate translation (window's global position)
    offset_x: i32,
    offset_y: i32,
}

impl<'a> PaintContext<'a> {
    /// Create a new paint context for a window.
    pub fn new(target: &'a mut dyn RenderTarget, bounds: Rect) -> Self {
        PaintContext {
            target,
            offset_x: bounds.x,
            offset_y: bounds.y,
            bounds,
            clip: None,
        }
    }

    /// Create a paint context with a specific clip region.
    pub fn with_clip(target: &'a mut dyn RenderTarget, bounds: Rect, clip: Rect) -> Self {
        PaintContext {
            target,
            offset_x: bounds.x,
            offset_y: bounds.y,
            bounds,
            clip: Some(clip),
        }
    }

    /// Get the window bounds.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Get window width.
    pub fn width(&self) -> u32 {
        self.bounds.width
    }

    /// Get window height.
    pub fn height(&self) -> u32 {
        self.bounds.height
    }

    /// Check if a local rectangle needs painting (intersects with clip region).
    pub fn needs_paint(&self, local_rect: Rect) -> bool {
        let global_rect = Rect::new(
            local_rect.x + self.offset_x,
            local_rect.y + self.offset_y,
            local_rect.width,
            local_rect.height,
        );

        match &self.clip {
            Some(clip) => global_rect.intersects(clip),
            None => true,
        }
    }

    /// Convert local coordinates to global and check bounds.
    fn to_global(&self, local_x: i32, local_y: i32) -> Option<(usize, usize)> {
        let global_x = local_x + self.offset_x;
        let global_y = local_y + self.offset_y;

        // Check if within bounds
        if global_x < 0 || global_y < 0 {
            return None;
        }

        let gx = global_x as usize;
        let gy = global_y as usize;

        if gx >= self.target.width() || gy >= self.target.height() {
            return None;
        }

        // Check clip region
        if let Some(clip) = &self.clip {
            if global_x < clip.x
                || global_x >= clip.right()
                || global_y < clip.y
                || global_y >= clip.bottom()
            {
                return None;
            }
        }

        Some((gx, gy))
    }

    /// Draw a pixel at local coordinates.
    pub fn draw_pixel(&mut self, x: i32, y: i32, color: Color) {
        if let Some((gx, gy)) = self.to_global(x, y) {
            self.target.draw_pixel(gx, gy, color);
        }
    }

    /// Fill a rectangle at local coordinates.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        // Calculate global coordinates
        let global_x = x + self.offset_x;
        let global_y = y + self.offset_y;

        // Clamp to window bounds
        let x1 = global_x.max(self.bounds.x).max(0);
        let y1 = global_y.max(self.bounds.y).max(0);
        let x2 = (global_x + width as i32).min(self.bounds.right());
        let y2 = (global_y + height as i32).min(self.bounds.bottom());

        // Apply clip region if present
        let (x1, y1, x2, y2) = if let Some(clip) = &self.clip {
            (
                x1.max(clip.x),
                y1.max(clip.y),
                x2.min(clip.right()),
                y2.min(clip.bottom()),
            )
        } else {
            (x1, y1, x2, y2)
        };

        if x2 <= x1 || y2 <= y1 {
            return;
        }

        self.target.fill_rect(
            x1 as usize,
            y1 as usize,
            (x2 - x1) as usize,
            (y2 - y1) as usize,
            color,
        );
    }

    /// Draw text at local coordinates.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, font: &dyn Font, color: Color) {
        if let Some((gx, gy)) = self.to_global(x, y) {
            self.target.draw_text(gx, gy, text, font, color);
        }
    }

    /// Draw text with background at local coordinates.
    pub fn draw_text_with_bg(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        font: &dyn Font,
        fg_color: Color,
        bg_color: Color,
    ) {
        if let Some((gx, gy)) = self.to_global(x, y) {
            self.target.draw_text_with_bg(gx, gy, text, font, fg_color, bg_color);
        }
    }
}
