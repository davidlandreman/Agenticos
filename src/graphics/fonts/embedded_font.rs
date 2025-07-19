use super::font_data::DEFAULT_8X8_FONT_DATA;

// Create a static instance of the default 8x8 font
pub static DEFAULT_8X8_FONT: Embedded8x8Font = Embedded8x8Font {
    first_char: 32,  // ASCII space
    num_chars: 95,   // space to tilde
    font_data: &DEFAULT_8X8_FONT_DATA,
};

// Embedded 8x8 font structure
#[derive(Debug)]
pub struct Embedded8x8Font {
    pub first_char: u8,
    pub num_chars: u8,
    pub font_data: &'static [[u8; 8]],
}

impl Embedded8x8Font {
    pub fn get_char_bitmap(&self, ch: char) -> Option<&[u8; 8]> {
        let char_code = ch as u8;
        if char_code < self.first_char || char_code >= self.first_char + self.num_chars {
            return None;
        }
        
        let char_index = (char_code - self.first_char) as usize;
        if char_index < self.font_data.len() {
            Some(&self.font_data[char_index])
        } else {
            None
        }
    }
}