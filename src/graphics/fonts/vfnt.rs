use crate::debug_info;

#[derive(Debug)]
pub struct VFNTFont {
    pub width: u8,
    pub height: u8,
    pub first_char: u8,
    pub num_chars: u8,
    pub bitmap_data: &'static [u8],
}

impl VFNTFont {
    pub fn from_vfnt_data(data: &'static [u8]) -> Option<Self> {
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