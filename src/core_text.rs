use crate::color::Color;
use crate::frame_buffer::FrameBufferWriter;
use crate::core_font::{VFNTFont, Embedded8x8Font};
use core::str;

pub enum Font {
    Embedded8x8(&'static Embedded8x8Font),
    VFNT(&'static VFNTFont),
}


pub struct TextRenderer<'a> {
    frame_buffer: &'a mut FrameBufferWriter,
    font: Font,
    default_color: Color,
    background_color: Option<Color>,
}

impl<'a> TextRenderer<'a> {
    pub fn new(frame_buffer: &'a mut FrameBufferWriter, font: Font) -> Self {
        Self {
            frame_buffer,
            font,
            default_color: Color::new(255, 255, 255), // White
            background_color: None,
        }
    }
    
    pub fn with_default_font(frame_buffer: &'a mut FrameBufferWriter) -> Self {
        // Use the default 8x8 font from core_font.rs
        Self::new(frame_buffer, Font::Embedded8x8(&crate::core_font::DEFAULT_8X8_FONT))
    }

    pub fn set_color(&mut self, color: Color) {
        self.default_color = color;
    }

    pub fn set_background(&mut self, color: Option<Color>) {
        self.background_color = color;
    }

    pub fn draw_text(&mut self, text: &str, x: usize, y: usize) {
        self.draw_text_with_color(text, x, y, self.default_color);
    }

    pub fn draw_text_with_color(&mut self, text: &str, mut x: usize, y: usize, color: Color) {
        for ch in text.chars() {
            if ch == '\n' {
                return;
            }
            let char_width = self.draw_char(ch, x, y, color);
            x += char_width;
        }
    }

    pub fn draw_char(&mut self, ch: char, x: usize, y: usize, color: Color) -> usize {
        match &self.font {
            Font::Embedded8x8(font) => {
                if let Some(glyph) = font.get_char_bitmap(ch) {
                    // Draw background if set
                    if let Some(bg_color) = self.background_color {
                        self.frame_buffer.fill_rect(x, y, 8, 8, bg_color);
                    }
                    
                    // Draw character
                    for row in 0..8 {
                        let byte = glyph[row];
                        for col in 0..8 {
                            if (byte >> (7 - col)) & 1 == 1 {
                                self.frame_buffer.draw_pixel(x + col, y + row, color);
                            }
                        }
                    }
                    
                    8 // Return character width
                } else {
                    8 // Return default width for unsupported chars
                }
            }
            Font::VFNT(font) => {
                if let Some(bitmap) = font.get_char_bitmap(ch) {
                    let width = font.width as usize;
                    let height = font.height as usize;
                    let bytes_per_row = (width + 7) / 8;
                    
                    // Draw background if set
                    if let Some(bg_color) = self.background_color {
                        self.frame_buffer.fill_rect(x, y, width, height, bg_color);
                    }
                    
                    // Draw character
                    for row in 0..height {
                        let row_offset = row * bytes_per_row;
                        for byte_idx in 0..bytes_per_row {
                            if row_offset + byte_idx >= bitmap.len() {
                                break;
                            }
                            let byte = bitmap[row_offset + byte_idx];
                            let pixels_in_byte = if byte_idx == bytes_per_row - 1 && width % 8 != 0 {
                                width % 8
                            } else {
                                8
                            };
                            
                            for bit in 0..pixels_in_byte {
                                if (byte >> (7 - bit)) & 1 == 1 {
                                    let px = x + byte_idx * 8 + bit;
                                    if px < x + width {
                                        self.frame_buffer.draw_pixel(px, y + row, color);
                                    }
                                }
                            }
                        }
                    }
                    
                    width // Fixed width font
                } else {
                    8 // Default width for missing characters
                }
            }
        }
    }

    pub fn measure_text(&self, text: &str) -> (usize, usize) {
        let mut max_width = 0;
        let mut line_count = 1;
        let mut current_line_width = 0;
        let line_height = match &self.font {
            Font::Embedded8x8(_) => 8,
            Font::VFNT(font) => font.height as usize,
        };
        
        for ch in text.chars() {
            if ch == '\n' {
                if current_line_width > max_width {
                    max_width = current_line_width;
                }
                current_line_width = 0;
                line_count += 1;
            } else {
                let char_width = match &self.font {
                    Font::Embedded8x8(_) => 8,
                    Font::VFNT(font) => font.width as usize,
                };
                current_line_width += char_width;
            }
        }
        
        if current_line_width > max_width {
            max_width = current_line_width;
        }
        
        (max_width, line_count * line_height)
    }

    pub fn draw_multiline_text(&mut self, text: &str, x: usize, mut y: usize) {
        let line_height = match &self.font {
            Font::Embedded8x8(_) => 8,
            Font::VFNT(font) => font.height as usize,
        };
        
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_measure_text() {
        // Test would go here
    }
}