use crate::color::Color;
use crate::frame_buffer::FrameBufferWriter;
use crate::core_font::FontRef;
use core::str;

pub struct TextRenderer<'a> {
    frame_buffer: &'a mut FrameBufferWriter,
    font: FontRef,
    default_color: Color,
    background_color: Option<Color>,
}

impl<'a> TextRenderer<'a> {
    pub fn new(frame_buffer: &'a mut FrameBufferWriter, font: FontRef) -> Self {
        Self {
            frame_buffer,
            font,
            default_color: Color::new(255, 255, 255), // White
            background_color: None,
        }
    }
    
    pub fn with_default_font(frame_buffer: &'a mut FrameBufferWriter) -> Self {
        // Use the default 8x8 font from core_font.rs
        Self::new(frame_buffer, crate::core_font::get_default_font())
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
        if let Some(bitmap) = self.font.get_char_bitmap(ch) {
            let width = self.font.char_width();
            let height = self.font.char_height();
            let bytes_per_row = self.font.bytes_per_row();
            
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
            
            width // Return character width
        } else {
            // Default width for missing characters
            self.font.char_width()
        }
    }

    pub fn measure_text(&self, text: &str) -> (usize, usize) {
        let mut max_width = 0;
        let mut line_count = 1;
        let mut current_line_width = 0;
        let line_height = self.font.char_height();
        
        for ch in text.chars() {
            if ch == '\n' {
                if current_line_width > max_width {
                    max_width = current_line_width;
                }
                current_line_width = 0;
                line_count += 1;
            } else {
                current_line_width += self.font.char_width();
            }
        }
        
        if current_line_width > max_width {
            max_width = current_line_width;
        }
        
        (max_width, line_count * line_height)
    }

    pub fn draw_multiline_text(&mut self, text: &str, x: usize, mut y: usize) {
        let line_height = self.font.char_height();
        
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