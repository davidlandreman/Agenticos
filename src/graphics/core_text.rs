//! Font-agnostic text rendering against a `FrameBufferWriter`.
//!
//! Glyphs are 8bpp coverage; we blend each pixel with the underlying
//! framebuffer color (or with a caller-supplied background when set).

use super::color::Color;
use super::fonts::core_font::{FontRef, Glyph};
use crate::drivers::display::frame_buffer::FrameBufferWriter;

pub struct TextRenderer<'a> {
    frame_buffer: &'a mut FrameBufferWriter,
    font: FontRef,
    default_color: Color,
    /// Optional fill color for the cell behind each glyph. When `None`, the
    /// existing framebuffer contents are used as the blend background.
    background_color: Option<Color>,
}

impl<'a> TextRenderer<'a> {
    pub fn new(frame_buffer: &'a mut FrameBufferWriter, font: FontRef) -> Self {
        Self {
            frame_buffer,
            font,
            default_color: Color::WHITE,
            background_color: None,
        }
    }

    pub fn with_default_font(frame_buffer: &'a mut FrameBufferWriter) -> Self {
        Self::new(frame_buffer, super::fonts::core_font::get_default_font())
    }

    pub fn set_color(&mut self, color: Color) {
        self.default_color = color;
    }

    pub fn set_background(&mut self, color: Option<Color>) {
        self.background_color = color;
    }

    /// Draw text with `(x, y)` interpreted as the top-left of the text cell.
    /// The baseline is `y + ascent`.
    pub fn draw_text(&mut self, text: &str, x: usize, y: usize) {
        self.draw_text_with_color(text, x, y, self.default_color);
    }

    pub fn draw_text_with_color(&mut self, text: &str, mut x: usize, y: usize, color: Color) {
        let baseline = y as i32 + self.font.ascent() as i32;
        for ch in text.chars() {
            if ch == '\n' {
                return;
            }
            let advance = self.draw_char(ch, x as i32, baseline, color);
            x = (x as i32 + advance).max(0) as usize;
        }
    }

    /// Render a single glyph, returning its pixel advance.
    pub fn draw_char(&mut self, ch: char, pen_x: i32, baseline_y: i32, color: Color) -> i32 {
        let Some(glyph) = self.font.glyph(ch) else {
            return self.font.cell_width() as i32;
        };

        if let Some(bg) = self.background_color {
            // Fill the full cell behind the glyph (top of cell to top + line_height).
            let cell_top = (baseline_y - self.font.ascent() as i32).max(0) as usize;
            let cell_x = pen_x.max(0) as usize;
            self.frame_buffer.fill_rect(
                cell_x,
                cell_top,
                self.font.cell_width() as usize,
                self.font.line_height() as usize,
                bg,
            );
        }

        blit_glyph(self.frame_buffer, pen_x + glyph.x_offset, baseline_y + glyph.y_offset, &glyph, color);
        glyph.advance as i32
    }

    pub fn measure_text(&self, text: &str) -> (usize, usize) {
        let mut max_width: usize = 0;
        let mut current: usize = 0;
        let mut lines: usize = 1;
        for ch in text.chars() {
            if ch == '\n' {
                if current > max_width {
                    max_width = current;
                }
                current = 0;
                lines += 1;
            } else if let Some(g) = self.font.glyph(ch) {
                current += g.advance as usize;
            } else {
                current += self.font.cell_width() as usize;
            }
        }
        if current > max_width {
            max_width = current;
        }
        (max_width, lines * self.font.line_height() as usize)
    }

    pub fn draw_multiline_text(&mut self, text: &str, x: usize, mut y: usize) {
        let line_height = self.font.line_height() as usize;
        for line in text.split('\n') {
            self.draw_text(line, x, y);
            y += line_height;
        }
    }

    pub fn draw_text_centered(&mut self, text: &str, center_x: usize, center_y: usize) {
        let (width, height) = self.measure_text(text);
        let x = center_x.saturating_sub(width / 2);
        let y = center_y.saturating_sub(height / 2);
        self.draw_multiline_text(text, x, y);
    }

    pub fn draw_text_right_aligned(&mut self, text: &str, right_x: usize, y: usize) {
        let (width, _) = self.measure_text(text);
        let x = right_x.saturating_sub(width);
        self.draw_text(text, x, y);
    }
}

/// Blit an 8bpp coverage glyph onto a `FrameBufferWriter` at `(x, y)`
/// (bitmap top-left). Pixels with alpha 0 are skipped, alpha 255 writes the
/// foreground directly, and partial coverage blends with whatever the
/// framebuffer already shows.
fn blit_glyph(fb: &mut FrameBufferWriter, x: i32, y: i32, glyph: &Glyph, color: Color) {
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
            if alpha == 0 {
                continue;
            }
            let dst_x = dst_x as usize;
            let dst_y = dst_y as usize;
            if alpha == 0xFF {
                fb.draw_pixel(dst_x, dst_y, color);
            } else {
                let bg = fb.get_pixel(dst_x, dst_y);
                fb.draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
            }
        }
    }
}

