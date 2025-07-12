use crate::color::Color;
use crate::frame_buffer::FrameBufferWriter;
use crate::font::{BMFont, VFNTFont, Embedded8x8Font};
use core::str;

pub enum Font {
    Embedded8x8(&'static Embedded8x8Font),
    BMFont(&'static BMFont),
    VFNT(&'static VFNTFont),
}

// Include the IBM Plex font data
pub static IBM_PLEX_DATA: &[u8] = include_bytes!("../assets/ibmplex-large.fnt");

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
        // Use the default 8x8 font from font.rs
        Self::new(frame_buffer, Font::Embedded8x8(&crate::font::DEFAULT_8X8_FONT))
    }
    
    pub fn with_bmfont(frame_buffer: &'a mut FrameBufferWriter, font: &'static BMFont) -> Self {
        Self::new(frame_buffer, Font::BMFont(font))
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
            Font::BMFont(font) => {
                if let Some(char_info) = font.get_char(ch) {
                    // Draw background if set
                    if let Some(bg_color) = self.background_color {
                        self.frame_buffer.fill_rect(
                            (x as isize + char_info.xoffset as isize) as usize,
                            (y as isize + char_info.yoffset as isize) as usize,
                            char_info.width as usize,
                            char_info.height as usize,
                            bg_color
                        );
                    }
                    
                    // Draw character from texture atlas
                    let texture_width = font.scale_w as usize;
                    let bytes_per_pixel = 4; // Assuming RGBA format
                    
                    for row in 0..char_info.height {
                        for col in 0..char_info.width {
                            let src_x = char_info.x as usize + col as usize;
                            let src_y = char_info.y as usize + row as usize;
                            let src_idx = (src_y * texture_width + src_x) * bytes_per_pixel;
                            
                            if src_idx + 3 < font.texture_data.len() {
                                let alpha = font.texture_data[src_idx + 3];
                                if alpha > 128 { // Simple threshold
                                    let dst_x = (x as isize + char_info.xoffset as isize + col as isize) as usize;
                                    let dst_y = (y as isize + char_info.yoffset as isize + row as isize) as usize;
                                    self.frame_buffer.draw_pixel(dst_x, dst_y, color);
                                }
                            }
                        }
                    }
                    
                    char_info.xadvance as usize
                } else {
                    8 // Default width for missing characters
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
            Font::BMFont(font) => font.line_height as usize,
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
                    Font::BMFont(font) => {
                        font.get_char(ch)
                            .map(|info| info.xadvance as usize)
                            .unwrap_or(8)
                    }
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
            Font::BMFont(font) => font.line_height as usize,
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