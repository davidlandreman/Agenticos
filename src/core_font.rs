use crate::embedded_font::{Embedded8x8Font, DEFAULT_8X8_FONT};
use crate::vfnt::VFNTFont;
use crate::truetype_font::TrueTypeFont;

// Unified font trait that all font types must implement
pub trait Font {
    /// Get the bitmap data for a character
    fn get_char_bitmap(&self, ch: char) -> Option<&[u8]>;
    
    /// Get the width of characters in this font
    fn char_width(&self) -> usize;
    
    /// Get the height of characters in this font
    fn char_height(&self) -> usize;
    
    /// Get the number of bytes per row for the bitmap data
    fn bytes_per_row(&self) -> usize {
        (self.char_width() + 7) / 8
    }
}

// Implement Font trait for Embedded8x8Font
impl Font for Embedded8x8Font {
    fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        self.get_char_bitmap(ch).map(|bitmap| &bitmap[..])
    }
    
    fn char_width(&self) -> usize {
        8
    }
    
    fn char_height(&self) -> usize {
        8
    }
}

// Implement Font trait for VFNTFont
impl Font for VFNTFont {
    fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        self.get_char_bitmap(ch)
    }
    
    fn char_width(&self) -> usize {
        self.width as usize
    }
    
    fn char_height(&self) -> usize {
        self.height as usize
    }
}

// Implement Font trait for TrueTypeFont
impl Font for TrueTypeFont {
    fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        self.get_char_bitmap(ch)
    }
    
    fn char_width(&self) -> usize {
        (self.render_size * 3 / 4) as usize
    }
    
    fn char_height(&self) -> usize {
        self.render_size as usize
    }
}

// Font wrapper that can hold any font type
pub struct FontRef {
    font: &'static dyn Font,
}

impl FontRef {
    pub fn new(font: &'static dyn Font) -> Self {
        Self { font }
    }
    
    pub fn get_char_bitmap(&self, ch: char) -> Option<&[u8]> {
        self.font.get_char_bitmap(ch)
    }
    
    pub fn char_width(&self) -> usize {
        self.font.char_width()
    }
    
    pub fn char_height(&self) -> usize {
        self.font.char_height()
    }
    
    pub fn bytes_per_row(&self) -> usize {
        self.font.bytes_per_row()
    }
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

// Helper functions to get font references
pub fn get_embedded_font() -> FontRef {
    FontRef::new(&DEFAULT_8X8_FONT as &dyn Font)
}


pub fn get_ibm_plex_font() -> Option<FontRef> {
    IBM_PLEX_FONT.as_ref().map(|font| FontRef::new(font as &dyn Font))
}

pub fn get_ibm_plex_large_font() -> Option<FontRef> {
    IBM_PLEX_LARGE_FONT.as_ref().map(|font| FontRef::new(font as &dyn Font))
}

// Include Arial TTF data
static ARIAL_TTF_DATA: &[u8] = include_bytes!("../assets/arial.ttf");

// Create static instance of Arial font
pub static ARIAL_FONT: spin::Lazy<Option<TrueTypeFont>> = spin::Lazy::new(|| {
    crate::debug_info!("ARIAL_FONT: Starting lazy initialization...");
    crate::debug_info!("ARIAL_FONT: Arial TTF data size: {} bytes", ARIAL_TTF_DATA.len());
    
    let result = TrueTypeFont::from_ttf_data(ARIAL_TTF_DATA, 16); // 16 pixel size
    
    match &result {
        Some(_) => crate::debug_info!("ARIAL_FONT: Successfully created TrueTypeFont"),
        None => crate::debug_info!("ARIAL_FONT: Failed to create TrueTypeFont"),
    }
    
    result
});

pub fn get_arial_font() -> Option<FontRef> {
    ARIAL_FONT.as_ref().map(|font| FontRef::new(font as &dyn Font))
}

// Get default font - try Arial with proper debugging
pub fn get_default_font() -> FontRef {
    return get_embedded_font();

    //return get_ibm_plex_font().unwrap();

    /* Try to load Arial font
    crate::debug_info!("About to access ARIAL_FONT lazy static...");
    match ARIAL_FONT.as_ref() {
        Some(font) => {
            crate::debug_info!("Arial font loaded successfully!");
            FontRef::new(font as &dyn Font)
        }
        None => {
            crate::debug_info!("Arial font failed to load, using embedded font");
            get_embedded_font()
        }
    } */
}