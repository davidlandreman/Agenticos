use core::str;
use crate::font_data::DEFAULT_8X8_FONT_DATA;

// Create a static instance of the default 8x8 font
pub static DEFAULT_8X8_FONT: Embedded8x8Font = Embedded8x8Font {
    first_char: 32,  // ASCII space
    num_chars: 95,   // space to tilde
    font_data: &DEFAULT_8X8_FONT_DATA,
};

#[derive(Debug, Clone, Copy)]
pub struct CharInfo {
    pub id: u16,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub xoffset: i16,
    pub yoffset: i16,
    pub xadvance: i16,
    pub page: u8,
}

#[derive(Debug)]
pub struct BMFont {
    pub line_height: u16,
    pub base: u16,
    pub scale_w: u16,
    pub scale_h: u16,
    pub chars: [Option<CharInfo>; 256],
    pub char_count: usize,
    pub texture_data: &'static [u8],
}

// VFNT format structure for embedded bitmap fonts
#[derive(Debug)]
pub struct VFNTFont {
    pub width: u8,
    pub height: u8,
    pub first_char: u8,
    pub num_chars: u8,
    pub bitmap_data: &'static [u8],
}

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

impl VFNTFont {
    pub fn from_vfnt_data(data: &'static [u8]) -> Option<Self> {
        use crate::debug_info;
        
        // Check for VFNT header
        if data.len() < 12 || &data[0..4] != b"VFNT" {
            debug_info!("VFNT: Invalid header or too small");
            return None;
        }
        
        // Check version string "0002"
        if &data[4..8] != b"0002" {
            debug_info!("VFNT: Wrong version");
            return None;
        }
        
        // Parse font properties at offset 8
        let width = data[8];
        let height = data[9];
        
        debug_info!("VFNT: Font dimensions: {}x{}", width, height);
        
        // The VFNT0002 format appears to have the bitmap data starting early in the file
        // and character range metadata at the end. Let's read from both locations.
        
        // First, try to read character range from the expected location at end of file
        const CHAR_RANGE_OFFSET: usize = 0x1918;
        let (first_char, num_chars) = if data.len() >= CHAR_RANGE_OFFSET + 8 {
            // Debug: print bytes at this offset
            debug_info!("VFNT: Bytes at 0x{:x}: {:02x} {:02x} {:02x} {:02x} | {:02x} {:02x} {:02x} {:02x}",
                       CHAR_RANGE_OFFSET,
                       data[CHAR_RANGE_OFFSET], data[CHAR_RANGE_OFFSET + 1],
                       data[CHAR_RANGE_OFFSET + 2], data[CHAR_RANGE_OFFSET + 3],
                       data[CHAR_RANGE_OFFSET + 4], data[CHAR_RANGE_OFFSET + 5],
                       data[CHAR_RANGE_OFFSET + 6], data[CHAR_RANGE_OFFSET + 7]);
            
            let fc = u32::from_be_bytes([
                data[CHAR_RANGE_OFFSET],
                data[CHAR_RANGE_OFFSET + 1],
                data[CHAR_RANGE_OFFSET + 2],
                data[CHAR_RANGE_OFFSET + 3],
            ]) as u8;
            
            let nc = u32::from_be_bytes([
                data[CHAR_RANGE_OFFSET + 4],
                data[CHAR_RANGE_OFFSET + 5],
                data[CHAR_RANGE_OFFSET + 6],
                data[CHAR_RANGE_OFFSET + 7],
            ]) as u8;
            
            debug_info!("VFNT: Parsed character range: first_char={}, num_chars={}", fc, nc);
            
            // If we get 0,0 it means the data isn't where we expected
            if fc == 0 && nc == 0 {
                debug_info!("VFNT: Invalid range (0,0), using default ASCII range");
                (32, 94)
            } else {
                (fc, nc)
            }
        } else {
            // Fallback: assume standard ASCII printable range
            debug_info!("VFNT: File too small for metadata, using default ASCII range (32-126)");
            (32, 94)
        };
        
        debug_info!("VFNT: Character range: first_char={}, num_chars={}", first_char, num_chars);
        
        // Looking at the file structure more carefully:
        // After examining various offsets, the bitmap data appears to be in the middle
        // The data before 0x1918 seems to contain the actual font bitmaps
        // Let's try a different approach - scan for the start of recognizable character patterns
        // Based on the hex dumps, offset 0x40 shows some structured data
        let bitmap_offset = 0x40;
        
        // Calculate expected data size
        let bytes_per_char = ((width as usize + 7) / 8) * height as usize;
        let bitmap_size = num_chars as usize * bytes_per_char;
        
        debug_info!("VFNT: Bytes per char: {}, total bitmap size: {}", bytes_per_char, bitmap_size);
        debug_info!("VFNT: Bitmap offset: 0x{:x}, file size: {}", bitmap_offset, data.len());
        
        // The character range info at 0x1918 might be metadata at the end
        // Let's ensure we have enough bitmap data from the start
        if data.len() < bitmap_offset + bitmap_size {
            debug_info!("VFNT: Not enough bitmap data. Need {} bytes from offset 0x{:x}, but file is {} bytes", 
                       bitmap_size, bitmap_offset, data.len());
            // Try to see if the bitmap fits before the character range metadata
            if CHAR_RANGE_OFFSET >= bitmap_offset + bitmap_size {
                debug_info!("VFNT: Bitmap data fits before metadata at 0x{:x}", CHAR_RANGE_OFFSET);
            } else {
                return None;
            }
        }
        
        Some(Self {
            width,
            height,
            first_char,
            num_chars,
            bitmap_data: &data[bitmap_offset..bitmap_offset + bitmap_size],
        })
    }
    
    pub fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        let char_code = ch as u8;
        if char_code < self.first_char || char_code >= self.first_char + self.num_chars {
            return None;
        }
        
        let char_index = (char_code - self.first_char) as usize;
        let bytes_per_row = (self.width as usize + 7) / 8;
        let bytes_per_char = bytes_per_row * self.height as usize;
        let offset = char_index * bytes_per_char;
        
        if offset + bytes_per_char <= self.bitmap_data.len() {
            Some(&self.bitmap_data[offset..offset + bytes_per_char])
        } else {
            None
        }
    }
}

impl BMFont {
    pub fn from_text_format(fnt_data: &'static str, texture_data: &'static [u8]) -> Option<Self> {
        let mut font = BMFont {
            line_height: 0,
            base: 0,
            scale_w: 0,
            scale_h: 0,
            chars: [None; 256],
            char_count: 0,
            texture_data,
        };

        for line in fnt_data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let mut parts_count = 0;
            let parts: [&str; 32] = {
                let mut arr = [""; 32];
                for part in line.split_whitespace() {
                    if parts_count < 32 {
                        arr[parts_count] = part;
                        parts_count += 1;
                    }
                }
                arr
            };
            if parts_count == 0 {
                continue;
            }

            match parts[0] {
                "common" => {
                    for i in 1..parts_count {
                        let part = parts[i];
                        if let Some(eq_idx) = part.find('=') {
                            let key = &part[..eq_idx];
                            let value = &part[eq_idx + 1..];
                            match key {
                                "lineHeight" => font.line_height = parse_u16(value).unwrap_or(0),
                                "base" => font.base = parse_u16(value).unwrap_or(0),
                                "scaleW" => font.scale_w = parse_u16(value).unwrap_or(0),
                                "scaleH" => font.scale_h = parse_u16(value).unwrap_or(0),
                                _ => {}
                            }
                        }
                    }
                }
                "char" => {
                    let mut char_info = CharInfo {
                        id: 0,
                        x: 0,
                        y: 0,
                        width: 0,
                        height: 0,
                        xoffset: 0,
                        yoffset: 0,
                        xadvance: 0,
                        page: 0,
                    };

                    for i in 1..parts_count {
                        let part = parts[i];
                        if let Some(eq_idx) = part.find('=') {
                            let key = &part[..eq_idx];
                            let value = &part[eq_idx + 1..];
                            match key {
                                "id" => char_info.id = parse_u16(value).unwrap_or(0),
                                "x" => char_info.x = parse_u16(value).unwrap_or(0),
                                "y" => char_info.y = parse_u16(value).unwrap_or(0),
                                "width" => char_info.width = parse_u16(value).unwrap_or(0),
                                "height" => char_info.height = parse_u16(value).unwrap_or(0),
                                "xoffset" => char_info.xoffset = parse_i16(value).unwrap_or(0),
                                "yoffset" => char_info.yoffset = parse_i16(value).unwrap_or(0),
                                "xadvance" => char_info.xadvance = parse_i16(value).unwrap_or(0),
                                "page" => char_info.page = parse_u8(value).unwrap_or(0),
                                _ => {}
                            }
                        }
                    }

                    if char_info.id < 256 {
                        font.chars[char_info.id as usize] = Some(char_info);
                        font.char_count += 1;
                    }
                }
                _ => {}
            }
        }

        if font.char_count > 0 && font.line_height > 0 {
            Some(font)
        } else {
            None
        }
    }

    pub fn get_char(&self, ch: char) -> Option<&CharInfo> {
        let char_code = ch as usize;
        if char_code < 256 {
            self.chars[char_code].as_ref()
        } else {
            None
        }
    }
}

// Simple parsing functions for no_std environment
fn parse_u16(s: &str) -> Option<u16> {
    let mut result = 0u16;
    for ch in s.chars() {
        if let Some(digit) = ch.to_digit(10) {
            result = result.wrapping_mul(10).wrapping_add(digit as u16);
        } else {
            return None;
        }
    }
    Some(result)
}

fn parse_i16(s: &str) -> Option<i16> {
    let mut chars = s.chars();
    let first_char = chars.next()?;
    
    let (negative, mut result) = if first_char == '-' {
        (true, 0i16)
    } else if let Some(digit) = first_char.to_digit(10) {
        (false, digit as i16)
    } else {
        return None;
    };
    
    for ch in chars {
        if let Some(digit) = ch.to_digit(10) {
            result = result.wrapping_mul(10).wrapping_add(digit as i16);
        } else {
            break;
        }
    }
    
    Some(if negative { -result } else { result })
}

fn parse_u8(s: &str) -> Option<u8> {
    let mut result = 0u8;
    for ch in s.chars() {
        if let Some(digit) = ch.to_digit(10) {
            result = result.wrapping_mul(10).wrapping_add(digit as u8);
        } else {
            return None;
        }
    }
    Some(result)
}

// Include IBM Plex font data
static IBM_PLEX_DATA: &[u8] = include_bytes!("../assets/ibmplex.fnt");
static IBM_PLEX_LARGE_DATA: &[u8] = include_bytes!("../assets/ibmplex-large.fnt");

// Create static instances of IBM Plex fonts
pub static IBM_PLEX_FONT: spin::Lazy<Option<VFNTFont>> = spin::Lazy::new(|| {
    VFNTFont::from_vfnt_data(IBM_PLEX_DATA)
});

pub static IBM_PLEX_LARGE_FONT: spin::Lazy<Option<VFNTFont>> = spin::Lazy::new(|| {
    VFNTFont::from_vfnt_data(IBM_PLEX_LARGE_DATA)
});