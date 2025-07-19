use alloc::boxed::Box;

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
    // Table offsets
    cmap_offset: Option<usize>,
    maxp_offset: Option<usize>,
    loca_offset: Option<usize>,
    glyf_offset: Option<usize>,
    hmtx_offset: Option<usize>,
    // Parsed table data
    num_glyphs: u16,
    index_to_loc_format: i16, // 0 for short, 1 for long
    num_h_metrics: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphBitmap {
    pub width: u16,
    pub height: u16,
    pub data: [u8; 256], // Fixed size buffer for simplicity
}

// Glyph outline data
#[derive(Debug)]
struct GlyphOutline {
    num_contours: i16,
    x_min: i16,
    y_min: i16,
    x_max: i16,
    y_max: i16,
    contours: [u16; 32], // End points of contours
    points: [GlyphPoint; 256], // Contour points
    num_points: usize,
}

#[derive(Debug, Clone, Copy)]
struct GlyphPoint {
    x: i16,
    y: i16,
    on_curve: bool,
}

// TTF table tags
const HEAD_TAG: u32 = 0x68656164; // 'head'
const HHEA_TAG: u32 = 0x68686561; // 'hhea'
const CMAP_TAG: u32 = 0x636d6170; // 'cmap'
const MAXP_TAG: u32 = 0x6d617870; // 'maxp'
const LOCA_TAG: u32 = 0x6c6f6361; // 'loca'
const GLYF_TAG: u32 = 0x676c7966; // 'glyf'
const HMTX_TAG: u32 = 0x686d7478; // 'hmtx'

impl TrueTypeFont {
    pub fn from_ttf_file(file_path: &str, render_size: u16) -> Option<Self> {
        crate::debug_info!("TTF: Entering from_ttf_file function");
        
        use crate::fs::File;
        use alloc::vec;
        
        crate::debug_info!("TTF: Loading font from file: {}", file_path);
        crate::debug_info!("TTF: About to call File::open_read");
        
        // Open file using proven File::open_read pattern
        let file = match File::open_read(file_path) {
            Ok(f) => {
                crate::debug_info!("TTF: File opened successfully, size: {} bytes", f.size());
                f
            },
            Err(e) => {
                crate::debug_info!("TTF: Failed to open font file {}: {:?}", file_path, e);
                return None;
            }
        };
        
        // Read file using proven pattern from hexdump/shell commands
        let mut file_data = vec![0u8; file.size() as usize];
        let bytes_read = match file.read(&mut file_data) {
            Ok(size) => {
                crate::debug_info!("TTF: Successfully read {} bytes from file", size);
                size
            },
            Err(e) => {
                crate::debug_info!("TTF: Failed to read font file data: {:?}", e);
                return None;
            }
        };
        
        // Truncate if we read less than expected
        file_data.truncate(bytes_read);
        
        crate::debug_info!("TTF: Successfully read {} bytes from {}", file_data.len(), file_path);
        
        // Convert Vec<u8> to &'static [u8] by leaking memory
        // This is acceptable for fonts which are loaded once and used for the entire kernel lifetime
        let static_data: &'static [u8] = Box::leak(file_data.into_boxed_slice());
        
        Self::from_ttf_data(static_data, render_size)
    }
    
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
        let mut cmap_offset = None;
        let mut maxp_offset = None;
        let mut loca_offset = None;
        let mut glyf_offset = None;
        let mut hmtx_offset = None;
        
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
                CMAP_TAG => cmap_offset = Some(offset as usize),
                MAXP_TAG => maxp_offset = Some(offset as usize),
                LOCA_TAG => loca_offset = Some(offset as usize),
                GLYF_TAG => glyf_offset = Some(offset as usize),
                HMTX_TAG => hmtx_offset = Some(offset as usize),
                _ => {}
            }
        }
        
        // Read head table for units per em and index format
        let (units_per_em, index_to_loc_format) = if let Some(offset) = head_offset {
            (
                read_u16(data, offset + 18),
                read_i16(data, offset + 50)
            )
        } else {
            crate::debug_info!("TTF: No head table found");
            return None;
        };
        
        // Read maxp table for glyph count
        let num_glyphs = if let Some(offset) = maxp_offset {
            read_u16(data, offset + 4)
        } else {
            crate::debug_info!("TTF: No maxp table found");
            return None;
        };
        
        // Read hhea table for font metrics and num_h_metrics
        let (ascender, descender, line_gap, num_h_metrics) = if let Some(offset) = hhea_offset {
            (
                read_i16(data, offset + 4),
                read_i16(data, offset + 6),
                read_i16(data, offset + 8),
                read_u16(data, offset + 34),
            )
        } else {
            crate::debug_info!("TTF: No hhea table found");
            return None;
        };
        
        crate::debug_info!("TTF: units_per_em: {}, ascender: {}, descender: {}", 
                   units_per_em, ascender, descender);
        crate::debug_info!("TTF: num_glyphs: {}, index_to_loc_format: {}, num_h_metrics: {}", 
                   num_glyphs, index_to_loc_format, num_h_metrics);
        
        crate::debug_info!("TTF: Starting to create glyph bitmaps...");
        
        // Calculate scale factor from font units to pixels
        let scale = render_size as f32 / units_per_em as f32;
        
        // Pre-render all ASCII glyphs
        let mut glyphs = [GlyphBitmap {
            width: 0,
            height: 0,
            data: [0; 256],
        }; 128];
        
        // Create a temporary instance to access methods
        let temp_font = Self {
            data,
            units_per_em,
            ascender,
            descender,
            line_gap,
            render_size,
            glyphs: [GlyphBitmap { width: 0, height: 0, data: [0; 256] }; 128],
            cmap_offset,
            maxp_offset,
            loca_offset,
            glyf_offset,
            hmtx_offset,
            num_glyphs,
            index_to_loc_format,
            num_h_metrics,
        };
        
        // Render each ASCII character
        for i in 0..128 {
            let ch = i as u8 as char;
            
            // Get glyph index for character
            if let Some(glyph_index) = temp_font.get_glyph_index(ch) {
                // Parse glyph outline
                if let Some(outline) = temp_font.parse_glyph_outline(glyph_index) {
                    // Rasterize the glyph
                    if let Some(bitmap) = temp_font.rasterize_glyph(&outline, scale) {
                        glyphs[i] = bitmap;
                        crate::debug_info!("TTF: Rendered glyph for '{}' ({}): {}x{}", 
                                   if i >= 32 && i < 127 { ch } else { '?' }, 
                                   i, bitmap.width, bitmap.height);
                    } else {
                        crate::debug_info!("TTF: Failed to rasterize glyph for '{}' ({})", 
                                   if i >= 32 && i < 127 { ch } else { '?' }, i);
                    }
                } else {
                    // No outline - might be space or other special character
                    if let Some((advance_width, _)) = temp_font.get_glyph_metrics(glyph_index) {
                        let width = (advance_width as f32 * scale) as u16;
                        glyphs[i] = GlyphBitmap {
                            width,
                            height: render_size,
                            data: [0; 256],
                        };
                    }
                }
            } else {
                crate::debug_info!("TTF: No glyph index for character {} ('{}')", 
                           i, if i >= 32 && i < 127 { ch } else { '?' });
            }
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
            cmap_offset,
            maxp_offset,
            loca_offset,
            glyf_offset,
            hmtx_offset,
            num_glyphs,
            index_to_loc_format,
            num_h_metrics,
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
    
    // Get glyph index for a character using cmap table
    fn get_glyph_index(&self, ch: char) -> Option<u16> {
        let cmap_offset = self.cmap_offset?;
        
        // Read cmap header
        let version = read_u16(self.data, cmap_offset);
        let num_tables = read_u16(self.data, cmap_offset + 2);
        
        crate::debug_info!("TTF: cmap version: {}, num_tables: {}", version, num_tables);
        
        // Look for Unicode BMP encoding (platform 3, encoding 1 or platform 0, encoding 3)
        for i in 0..num_tables {
            let entry_offset = cmap_offset + 4 + i as usize * 8;
            let platform_id = read_u16(self.data, entry_offset);
            let encoding_id = read_u16(self.data, entry_offset + 2);
            let subtable_offset = read_u32(self.data, entry_offset + 4) as usize;
            
            crate::debug_info!("TTF: cmap subtable {}: platform: {}, encoding: {}", 
                       i, platform_id, encoding_id);
            
            // Check for Unicode encodings
            if (platform_id == 0 && encoding_id == 3) || 
               (platform_id == 3 && encoding_id == 1) {
                // Parse format 4 subtable (most common for Unicode BMP)
                return self.parse_cmap_format4(cmap_offset + subtable_offset, ch);
            }
        }
        
        None
    }
    
    // Get glyph offset from loca table
    fn get_glyph_offset(&self, glyph_index: u16) -> Option<usize> {
        let loca_offset = self.loca_offset?;
        
        if glyph_index as usize > self.num_glyphs as usize {
            return None;
        }
        
        let offset = if self.index_to_loc_format == 0 {
            // Short format (multiply by 2)
            let idx_offset = loca_offset + glyph_index as usize * 2;
            read_u16(self.data, idx_offset) as usize * 2
        } else {
            // Long format
            let idx_offset = loca_offset + glyph_index as usize * 4;
            read_u32(self.data, idx_offset) as usize
        };
        
        Some(offset)
    }
    
    // Get horizontal metrics for a glyph
    fn get_glyph_metrics(&self, glyph_index: u16) -> Option<(u16, i16)> {
        let hmtx_offset = self.hmtx_offset?;
        
        let (advance_width, left_side_bearing) = if glyph_index < self.num_h_metrics {
            // Full metrics entry
            let offset = hmtx_offset + glyph_index as usize * 4;
            (
                read_u16(self.data, offset),
                read_i16(self.data, offset + 2),
            )
        } else {
            // Only left side bearing, use last advance width
            let last_aw_offset = hmtx_offset + (self.num_h_metrics - 1) as usize * 4;
            let advance_width = read_u16(self.data, last_aw_offset);
            
            let lsb_offset = hmtx_offset + self.num_h_metrics as usize * 4 + 
                           (glyph_index - self.num_h_metrics) as usize * 2;
            let left_side_bearing = read_i16(self.data, lsb_offset);
            
            (advance_width, left_side_bearing)
        };
        
        Some((advance_width, left_side_bearing))
    }
    
    // Parse cmap format 4 subtable
    fn parse_cmap_format4(&self, offset: usize, ch: char) -> Option<u16> {
        let format = read_u16(self.data, offset);
        if format != 4 {
            crate::debug_info!("TTF: Expected cmap format 4, got {}", format);
            return None;
        }
        
        let char_code = ch as u32;
        if char_code > 0xFFFF {
            return None; // Format 4 only supports BMP
        }
        
        let seg_count_x2 = read_u16(self.data, offset + 6);
        let seg_count = seg_count_x2 / 2;
        
        // Binary search for the segment containing our character
        let end_codes_offset = offset + 14;
        let start_codes_offset = end_codes_offset + seg_count_x2 as usize + 2; // +2 for reserved
        let id_delta_offset = start_codes_offset + seg_count_x2 as usize;
        let id_range_offset_offset = id_delta_offset + seg_count_x2 as usize;
        
        // Find segment
        for i in 0..seg_count {
            let end_code = read_u16(self.data, end_codes_offset + i as usize * 2);
            let start_code = read_u16(self.data, start_codes_offset + i as usize * 2);
            
            if char_code >= start_code as u32 && char_code <= end_code as u32 {
                let id_delta = read_i16(self.data, id_delta_offset + i as usize * 2);
                let id_range_offset = read_u16(self.data, id_range_offset_offset + i as usize * 2);
                
                if id_range_offset == 0 {
                    // Direct mapping
                    return Some(((char_code as i32 + id_delta as i32) & 0xFFFF) as u16);
                } else {
                    // Glyph index array
                    let glyph_index_offset = id_range_offset_offset + i as usize * 2 + id_range_offset as usize +
                                           (char_code - start_code as u32) as usize * 2;
                    let glyph_index = read_u16(self.data, glyph_index_offset);
                    
                    if glyph_index == 0 {
                        return Some(0); // Missing glyph
                    } else {
                        return Some(((glyph_index as i32 + id_delta as i32) & 0xFFFF) as u16);
                    }
                }
            }
        }
        
        Some(0) // Missing glyph
    }
    
    // Parse glyph outline from glyf table
    fn parse_glyph_outline(&self, glyph_index: u16) -> Option<GlyphOutline> {
        let glyf_offset = self.glyf_offset?;
        let glyph_offset = self.get_glyph_offset(glyph_index)?;
        
        // Check if glyph has no outline (space character, etc.)
        let next_offset = self.get_glyph_offset(glyph_index + 1)?;
        if glyph_offset == next_offset {
            return None; // Empty glyph
        }
        
        let offset = glyf_offset + glyph_offset;
        
        // Read glyph header
        let num_contours = read_i16(self.data, offset);
        let x_min = read_i16(self.data, offset + 2);
        let y_min = read_i16(self.data, offset + 4);
        let x_max = read_i16(self.data, offset + 6);
        let y_max = read_i16(self.data, offset + 8);
        
        crate::debug_info!("TTF: Glyph {}: contours={}, bbox=({},{},{},{})", 
                   glyph_index, num_contours, x_min, y_min, x_max, y_max);
        
        if num_contours < 0 {
            // Composite glyph - skip for now
            crate::debug_info!("TTF: Composite glyph not supported yet");
            return None;
        }
        
        if num_contours == 0 {
            return None; // No outline
        }
        
        let mut outline = GlyphOutline {
            num_contours,
            x_min,
            y_min,
            x_max,
            y_max,
            contours: [0; 32],
            points: [GlyphPoint { x: 0, y: 0, on_curve: true }; 256],
            num_points: 0,
        };
        
        // Read contour end points
        let mut contour_offset = offset + 10;
        for i in 0..num_contours.min(32) as usize {
            outline.contours[i] = read_u16(self.data, contour_offset);
            contour_offset += 2;
        }
        
        let num_points = if num_contours > 0 {
            outline.contours[num_contours as usize - 1] as usize + 1
        } else {
            0
        };
        
        if num_points > 256 {
            crate::debug_info!("TTF: Too many points: {}", num_points);
            return None;
        }
        
        outline.num_points = num_points;
        
        // Skip instruction length
        let instruction_length = read_u16(self.data, contour_offset);
        contour_offset += 2 + instruction_length as usize;
        
        // Read flags
        let mut flags = [0u8; 256];
        let mut flag_idx = 0;
        
        while flag_idx < num_points {
            let flag = self.data[contour_offset];
            contour_offset += 1;
            
            flags[flag_idx] = flag;
            flag_idx += 1;
            
            // Check for repeat flag
            if flag & 0x08 != 0 {
                let repeat_count = self.data[contour_offset] as usize;
                contour_offset += 1;
                
                for _ in 0..repeat_count.min(num_points - flag_idx) {
                    flags[flag_idx] = flag;
                    flag_idx += 1;
                }
            }
        }
        
        // Read X coordinates
        let mut x = 0i16;
        for i in 0..num_points {
            let flag = flags[i];
            
            if flag & 0x02 != 0 { // X_SHORT_VECTOR
                let delta = self.data[contour_offset] as i16;
                contour_offset += 1;
                
                if flag & 0x10 != 0 { // positive
                    x += delta;
                } else {
                    x -= delta;
                }
            } else if flag & 0x10 == 0 { // X coordinate data
                let delta = read_i16(self.data, contour_offset);
                contour_offset += 2;
                x += delta;
            }
            
            outline.points[i].x = x;
        }
        
        // Read Y coordinates
        let mut y = 0i16;
        for i in 0..num_points {
            let flag = flags[i];
            
            if flag & 0x04 != 0 { // Y_SHORT_VECTOR
                let delta = self.data[contour_offset] as i16;
                contour_offset += 1;
                
                if flag & 0x20 != 0 { // positive
                    y += delta;
                } else {
                    y -= delta;
                }
            } else if flag & 0x20 == 0 { // Y coordinate data
                let delta = read_i16(self.data, contour_offset);
                contour_offset += 2;
                y += delta;
            }
            
            outline.points[i].y = y;
            outline.points[i].on_curve = (flag & 0x01) != 0;
        }
        
        Some(outline)
    }
    
    // Rasterize a glyph outline into a bitmap
    fn rasterize_glyph(&self, outline: &GlyphOutline, scale: f32) -> Option<GlyphBitmap> {
        // Calculate scaled bounds
        let x_min = (outline.x_min as f32 * scale) as i32;
        let y_min = (outline.y_min as f32 * scale) as i32;
        let x_max = (outline.x_max as f32 * scale) as i32;
        let y_max = (outline.y_max as f32 * scale) as i32;
        
        let width = (x_max - x_min + 1) as u16;
        let height = (y_max - y_min + 1) as u16;
        
        if width > 64 || height > 64 {
            crate::debug_info!("TTF: Glyph too large: {}x{}", width, height);
            return None;
        }
        
        let mut bitmap = GlyphBitmap {
            width,
            height,
            data: [0; 256],
        };
        
        // Create edge table for scanline algorithm
        let mut edges: [[i16; 64]; 64] = [[0; 64]; 64]; // y -> [x_coords]
        let mut edge_counts = [0usize; 64];
        
        // Process each contour
        let mut start_idx = 0;
        for contour_idx in 0..outline.num_contours as usize {
            let end_idx = outline.contours[contour_idx] as usize;
            
            // Draw edges between points
            for i in start_idx..=end_idx {
                let p1 = &outline.points[i];
                let p2 = if i == end_idx {
                    &outline.points[start_idx]
                } else {
                    &outline.points[i + 1]
                };
                
                // Scale and translate points
                let x1 = (p1.x as f32 * scale - x_min as f32) as i32;
                let y1 = (p1.y as f32 * scale - y_min as f32) as i32;
                let x2 = (p2.x as f32 * scale - x_min as f32) as i32;
                let y2 = (p2.y as f32 * scale - y_min as f32) as i32;
                
                // Handle quadratic Bézier curves
                if !p1.on_curve && p2.on_curve {
                    // p1 is control point, p2 is on-curve
                    // For now, approximate with line
                    self.draw_line(&mut edges, &mut edge_counts, x1, y1, x2, y2);
                } else if p1.on_curve && !p2.on_curve {
                    // p1 is on-curve, p2 is control point
                    // Look ahead for next on-curve point
                    let next_idx = if i + 1 == end_idx { start_idx } else { i + 2 };
                    if next_idx <= end_idx || next_idx == start_idx {
                        let p3 = &outline.points[if next_idx > end_idx { start_idx } else { next_idx }];
                        let x3 = (p3.x as f32 * scale - x_min as f32) as i32;
                        let y3 = (p3.y as f32 * scale - y_min as f32) as i32;
                        
                        // Draw quadratic Bézier from p1 through p2 to p3
                        self.draw_quadratic_bezier(&mut edges, &mut edge_counts, x1, y1, x2, y2, x3, y3);
                    }
                } else if p1.on_curve && p2.on_curve {
                    // Both on curve - simple line
                    self.draw_line(&mut edges, &mut edge_counts, x1, y1, x2, y2);
                }
            }
            
            start_idx = end_idx + 1;
        }
        
        // Fill using scanline algorithm
        let bytes_per_row = (width as usize + 7) / 8;
        for y in 0..height.min(64) {
            let edge_count = edge_counts[y as usize];
            if edge_count == 0 {
                continue;
            }
            
            // Sort x coordinates for this scanline
            let mut x_coords = edges[y as usize];
            for i in 0..edge_count {
                for j in i + 1..edge_count {
                    if x_coords[i] > x_coords[j] {
                        let temp = x_coords[i];
                        x_coords[i] = x_coords[j];
                        x_coords[j] = temp;
                    }
                }
            }
            
            // Fill between pairs of edges
            for i in (0..edge_count).step_by(2) {
                if i + 1 >= edge_count {
                    break;
                }
                
                let x_start = x_coords[i].max(0) as usize;
                let x_end = x_coords[i + 1].min(width as i16 - 1) as usize;
                
                for x in x_start..=x_end {
                    let byte_idx = y as usize * bytes_per_row + x / 8;
                    let bit_idx = 7 - (x % 8);
                    
                    if byte_idx < 256 {
                        bitmap.data[byte_idx] |= 1 << bit_idx;
                    }
                }
            }
        }
        
        Some(bitmap)
    }
    
    // Draw a line into the edge table
    fn draw_line(&self, edges: &mut [[i16; 64]; 64], edge_counts: &mut [usize; 64], 
                 x0: i32, y0: i32, x1: i32, y1: i32) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        
        let mut x = x0;
        let mut y = y0;
        
        loop {
            // Add edge crossing
            if y >= 0 && y < 64 {
                let y_idx = y as usize;
                let count = edge_counts[y_idx];
                if count < 64 && x >= 0 && x < 64 {
                    edges[y_idx][count] = x as i16;
                    edge_counts[y_idx] += 1;
                }
            }
            
            if x == x1 && y == y1 {
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
    
    // Draw a quadratic Bézier curve (simplified - just draws as lines for now)
    fn draw_quadratic_bezier(&self, edges: &mut [[i16; 64]; 64], edge_counts: &mut [usize; 64],
                            x0: i32, y0: i32, x1: i32, y1: i32, x2: i32, y2: i32) {
        // For now, approximate with multiple line segments
        const STEPS: i32 = 8;
        
        let mut prev_x = x0;
        let mut prev_y = y0;
        
        for i in 1..=STEPS {
            let t = i as f32 / STEPS as f32;
            let t2 = t * t;
            let one_minus_t = 1.0 - t;
            let one_minus_t2 = one_minus_t * one_minus_t;
            
            // Quadratic Bézier formula
            let x = (one_minus_t2 * x0 as f32 + 2.0 * one_minus_t * t * x1 as f32 + t2 * x2 as f32) as i32;
            let y = (one_minus_t2 * y0 as f32 + 2.0 * one_minus_t * t * y1 as f32 + t2 * y2 as f32) as i32;
            
            self.draw_line(edges, edge_counts, prev_x, prev_y, x, y);
            
            prev_x = x;
            prev_y = y;
        }
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