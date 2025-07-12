use crate::color::Color;
use crate::frame_buffer::FrameBufferWriter;
use core::str;

#[derive(Debug, Clone, Copy)]
pub struct FontInfo {
    pub width: usize,
    pub height: usize,
    pub first_char: u8,
    pub num_chars: usize,
}

pub struct BitmapFont {
    pub info: FontInfo,
    pub data: &'static [u8],
}

impl BitmapFont {
    pub const fn from_raw(data: &'static [u8], width: usize, height: usize) -> Self {
        Self {
            info: FontInfo {
                width,
                height,
                first_char: 0,
                num_chars: 256,
            },
            data,
        }
    }
    
    pub fn from_fnt(data: &'static [u8]) -> Option<Self> {
        // Basic FNT format: starts with header containing font info
        // For now, we'll use a simple format:
        // 2 bytes: width
        // 2 bytes: height  
        // 1 byte: first character code
        // 1 byte: number of characters
        // Rest: font bitmap data
        
        if data.len() < 6 {
            return None;
        }
        
        let width = (data[0] as usize) | ((data[1] as usize) << 8);
        let height = (data[2] as usize) | ((data[3] as usize) << 8);
        let first_char = data[4];
        let num_chars = data[5] as usize;
        
        let bytes_per_char = (width + 7) / 8 * height;
        let expected_size = 6 + num_chars * bytes_per_char;
        
        if data.len() < expected_size {
            return None;
        }
        
        Some(Self {
            info: FontInfo {
                width,
                height,
                first_char,
                num_chars,
            },
            data: &data[6..],
        })
    }
    
    pub fn get_glyph(&self, ch: char) -> Option<&[u8]> {
        let char_code = ch as u8;
        if char_code < self.info.first_char || 
           char_code >= self.info.first_char + self.info.num_chars as u8 {
            return None;
        }
        
        let char_index = (char_code - self.info.first_char) as usize;
        let bytes_per_row = (self.info.width + 7) / 8;
        let bytes_per_char = bytes_per_row * self.info.height;
        let offset = char_index * bytes_per_char;
        
        if offset + bytes_per_char <= self.data.len() {
            Some(&self.data[offset..offset + bytes_per_char])
        } else {
            None
        }
    }
}

// Default 8x16 font embedded in the kernel
pub static DEFAULT_FONT_DATA: &[u8] = include_bytes!("../assets/default_font.bin");
pub static DEFAULT_FONT: BitmapFont = BitmapFont::from_raw(DEFAULT_FONT_DATA, 8, 16);

pub struct TextRenderer<'a> {
    frame_buffer: &'a mut FrameBufferWriter,
    font: &'a BitmapFont,
    default_color: Color,
    background_color: Option<Color>,
}

impl<'a> TextRenderer<'a> {
    pub fn new(frame_buffer: &'a mut FrameBufferWriter, font: &'a BitmapFont) -> Self {
        Self {
            frame_buffer,
            font,
            default_color: Color::new(255, 255, 255), // White
            background_color: None,
        }
    }
    
    pub fn with_default_font(frame_buffer: &'a mut FrameBufferWriter) -> Self {
        Self::new(frame_buffer, &DEFAULT_FONT)
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
            self.draw_char(ch, x, y, color);
            x += self.font.info.width;
        }
    }

    pub fn draw_char(&mut self, ch: char, x: usize, y: usize, color: Color) {
        let glyph = match self.font.get_glyph(ch) {
            Some(g) => g,
            None => return,
        };

        let width = self.font.info.width;
        let height = self.font.info.height;
        let bytes_per_row = (width + 7) / 8;

        // Draw background if set
        if let Some(bg_color) = self.background_color {
            self.frame_buffer.fill_rect(x, y, width, height, bg_color);
        }

        // Draw character
        for row in 0..height {
            let row_offset = row * bytes_per_row;
            for byte_idx in 0..bytes_per_row {
                if row_offset + byte_idx >= glyph.len() {
                    break;
                }
                let byte = glyph[row_offset + byte_idx];
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
    }

    pub fn measure_text(&self, text: &str) -> (usize, usize) {
        let mut max_width = 0;
        let mut line_count = 1;
        let mut current_line_width = 0;
        
        for ch in text.chars() {
            if ch == '\n' {
                if current_line_width > max_width {
                    max_width = current_line_width;
                }
                current_line_width = 0;
                line_count += 1;
            } else {
                current_line_width += self.font.info.width;
            }
        }
        
        if current_line_width > max_width {
            max_width = current_line_width;
        }
        
        (max_width, line_count * self.font.info.height)
    }

    pub fn draw_multiline_text(&mut self, text: &str, x: usize, mut y: usize) {
        for line in text.split('\n') {
            self.draw_text(line, x, y);
            y += self.font.info.height;
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

// Helper function to create a simple bitmap font in memory
pub fn create_default_font_data() -> [u8; 256 * 16] {
    let mut font = [0u8; 256 * 16];
    
    // Basic ASCII characters - just a few examples
    // You would normally load this from a proper .fnt file
    
    // Space (0x20)
    // All zeros (blank)
    
    // A (0x41)
    font[0x41 * 16 + 2] = 0b00111100;
    font[0x41 * 16 + 3] = 0b01100110;
    font[0x41 * 16 + 4] = 0b11000011;
    font[0x41 * 16 + 5] = 0b11000011;
    font[0x41 * 16 + 6] = 0b11111111;
    font[0x41 * 16 + 7] = 0b11000011;
    font[0x41 * 16 + 8] = 0b11000011;
    font[0x41 * 16 + 9] = 0b11000011;
    
    // H (0x48)  
    font[0x48 * 16 + 2] = 0b11000011;
    font[0x48 * 16 + 3] = 0b11000011;
    font[0x48 * 16 + 4] = 0b11000011;
    font[0x48 * 16 + 5] = 0b11111111;
    font[0x48 * 16 + 6] = 0b11000011;
    font[0x48 * 16 + 7] = 0b11000011;
    font[0x48 * 16 + 8] = 0b11000011;
    font[0x48 * 16 + 9] = 0b11000011;
    
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_measure_text() {
        // Test would go here
    }
}