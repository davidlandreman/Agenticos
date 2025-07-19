
// TrueType font structure for parsing TTF files
#[derive(Debug)]
pub struct TrueTypeFont {
    data: &'static [u8],
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub render_size: u16,
    // Pre-rendered glyphs for ASCII characters
    pub glyphs: [GlyphBitmap; 128],
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphBitmap {
    pub width: u16,
    pub height: u16,
    pub data: [u8; 256], // Fixed size buffer for simplicity
}

// TTF table tags
const HEAD_TAG: u32 = 0x68656164; // 'head'
const HHEA_TAG: u32 = 0x68686561; // 'hhea'

impl TrueTypeFont {
    pub fn from_ttf_data(data: &'static [u8], render_size: u16) -> Option<Self> {
        crate::debug_info!("TTF: Starting to parse TrueType font, size: {} bytes, render_size: {}", data.len(), render_size);
        
        if data.len() < 12 {
            crate::debug_info!("TTF: File too small");
            return None;
        }
        
        // Read the offset subtable
        let version = read_u32(data, 0);
        let num_tables = read_u16(data, 4);
        
        crate::debug_info!("TTF: Version: 0x{:08x}, Tables: {}", version, num_tables);
        
        // Verify it's a TrueType font (version 1.0)
        if version != 0x00010000 && version != 0x74727565 { // 'true'
            crate::debug_info!("TTF: Invalid version");
            return None;
        }
        
        // Read table directory
        let mut head_offset = None;
        let mut hhea_offset = None;
        
        for i in 0..num_tables {
            let entry_offset = 12 + i as usize * 16;
            if entry_offset + 16 > data.len() {
                break;
            }
            
            let tag = read_u32(data, entry_offset);
            let offset = read_u32(data, entry_offset + 8);
            
            match tag {
                HEAD_TAG => head_offset = Some(offset as usize),
                HHEA_TAG => hhea_offset = Some(offset as usize),
                _ => {}
            }
        }
        
        // Read head table for units per em
        let units_per_em = if let Some(offset) = head_offset {
            read_u16(data, offset + 18)
        } else {
            crate::debug_info!("TTF: No head table found");
            return None;
        };
        
        // Read hhea table for font metrics
        let (ascender, descender, line_gap) = if let Some(offset) = hhea_offset {
            (
                read_i16(data, offset + 4),
                read_i16(data, offset + 6),
                read_i16(data, offset + 8),
            )
        } else {
            crate::debug_info!("TTF: No hhea table found");
            (units_per_em as i16 * 3 / 4, -(units_per_em as i16 / 4), 0)
        };
        
        crate::debug_info!("TTF: units_per_em: {}, ascender: {}, descender: {}", 
                   units_per_em, ascender, descender);
        
        crate::debug_info!("TTF: Starting to create glyph bitmaps...");
        
        // Pre-render all ASCII glyphs
        let mut glyphs = [GlyphBitmap {
            width: 0,
            height: 0,
            data: [0; 256],
        }; 128];
        
        // For now, create simple placeholder glyphs
        // In a real implementation, this would parse glyf table and render actual outlines
        for i in 0..128 {
            let width = render_size * 3 / 4;
            let height = render_size;
            
            let mut bitmap = GlyphBitmap {
                width,
                height,
                data: [0; 256],
            };
            
            // Create a simple glyph representation
            let bytes_per_row = (width as usize + 7) / 8;
            
            // For printable ASCII, create a simple pattern
            if i >= 32 && i < 127 {
                // Draw a box
                for x in 0..width {
                    let byte_idx = (x / 8) as usize;
                    let bit_idx = (x % 8) as usize;
                    
                    // Top row
                    bitmap.data[byte_idx] |= 1 << (7 - bit_idx);
                    
                    // Bottom row  
                    let bottom_offset = (height as usize - 1) * bytes_per_row;
                    if bottom_offset + byte_idx < 256 {
                        bitmap.data[bottom_offset + byte_idx] |= 1 << (7 - bit_idx);
                    }
                }
                
                // Left and right borders
                for y in 1..height - 1 {
                    let row_offset = y as usize * bytes_per_row;
                    
                    if row_offset < 256 {
                        bitmap.data[row_offset] |= 0x80; // Left
                    }
                    
                    let right_x = width - 1;
                    let byte_idx = (right_x / 8) as usize;
                    let bit_idx = (right_x % 8) as usize;
                    
                    if row_offset + byte_idx < 256 {
                        bitmap.data[row_offset + byte_idx] |= 1 << (7 - bit_idx); // Right
                    }
                }
                
                // Add character-specific pattern
                let pattern = (i as u8).wrapping_sub(32);
                let center_y = height / 2;
                
                // Draw a simple pattern based on character code
                for dy in 0..3 {
                    let y = center_y.saturating_sub(1) + dy;
                    if y < height {
                        let row_offset = y as usize * bytes_per_row;
                        if row_offset < 256 {
                            bitmap.data[row_offset] |= (pattern >> dy) & 0x7F;
                        }
                    }
                }
            }
            
            glyphs[i] = bitmap;
        }
        
        crate::debug_info!("TTF: Font creation complete, returning TrueTypeFont instance");
        
        Some(Self {
            data,
            units_per_em,
            ascender,
            descender,
            line_gap,
            render_size,
            glyphs,
        })
    }
    
    pub fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        let char_code = ch as u8;
        
        if char_code >= 128 {
            return None;
        }
        
        let glyph = &self.glyphs[char_code as usize];
        let bytes_per_row = (glyph.width as usize + 7) / 8;
        let total_bytes = bytes_per_row * glyph.height as usize;
        
        Some(&glyph.data[..total_bytes])
    }
}

// Helper functions to read big-endian data
fn read_u32(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        crate::debug_info!("TTF: read_u32 out of bounds: offset {} + 4 > len {}", offset, data.len());
        return 0;
    }
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    if offset + 2 > data.len() {
        return 0;
    }
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

fn read_i16(data: &[u8], offset: usize) -> i16 {
    if offset + 2 > data.len() {
        return 0;
    }
    i16::from_be_bytes([data[offset], data[offset + 1]])
}