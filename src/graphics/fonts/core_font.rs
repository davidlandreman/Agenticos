use super::embedded_font::{Embedded8x8Font, DEFAULT_8X8_FONT};
use super::vfnt::VFNTFont;
use super::truetype_font::TrueTypeFont;
use alloc::boxed::Box;

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
        // Return average character width - actual widths vary per character
        (self.render_size * 2 / 3) as usize
    }
    
    fn char_height(&self) -> usize {
        self.render_size as usize
    }
}

// Font wrapper that can hold any font type
#[derive(Copy, Clone)]
pub struct FontRef {
    font: &'static dyn Font,
}

// Make FontRef thread-safe by implementing Send and Sync
unsafe impl Send for FontRef {}
unsafe impl Sync for FontRef {}

impl FontRef {
    pub fn new(font: &'static dyn Font) -> Self {
        Self { font }
    }
    
    pub fn as_font(&self) -> &dyn Font {
        self.font
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

// Binary Font Data Assets
static ARIAL_TTF_DATA: &[u8] = include_bytes!("../../../assets/tiny.ttf");
static IBM_PLEX_DATA: &[u8] = include_bytes!("../../../assets/ibmplex.fnt");
static IBM_PLEX_LARGE_DATA: &[u8] = include_bytes!("../../../assets/ibmplex-large.fnt");

// Create static instance of Arial font
pub static ARIAL_FONT: spin::Lazy<Option<TrueTypeFont>> = spin::Lazy::new(|| {
    TrueTypeFont::from_ttf_data(ARIAL_TTF_DATA, 16)
});


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

pub fn get_arial_font() -> Option<FontRef> {
    crate::debug_info!("get_arial_font() called!!!");
    ARIAL_FONT.as_ref().map(|font| FontRef::new(font as &dyn Font))
}

pub fn get_arial_font_from_fs() -> Option<FontRef> {
    match TrueTypeFont::from_ttf_file("/arial.ttf", 16) {
        Some(font) => {
            // Create a static reference by leaking memory
            // This is acceptable for fonts which are used throughout kernel lifetime
            let static_font: &'static TrueTypeFont = Box::leak(Box::new(font));
            Some(FontRef::new(static_font as &dyn Font))
        }
        None => None,
    }
}

// Global font state for dynamic switching
use spin::Mutex;
static CURRENT_DEFAULT_FONT: Mutex<Option<FontRef>> = Mutex::new(None);

// Get default font - use embedded font initially, but allow dynamic switching
pub fn get_default_font() -> FontRef {
    // Check if we have a custom font set
    if let Some(font) = *CURRENT_DEFAULT_FONT.lock() {
        return font;
    }
    
    // Fall back to embedded font
    get_embedded_font()
}

// Set a new default font dynamically
pub fn set_default_font(font: FontRef) {
    crate::debug_info!("Setting new default font");
    *CURRENT_DEFAULT_FONT.lock() = Some(font);
}

// Reset to embedded font
pub fn reset_to_embedded_font() {
    crate::debug_info!("Resetting to embedded font");
    *CURRENT_DEFAULT_FONT.lock() = None;
}

// Try to load and set Arial font from filesystem (can be called after boot)
pub fn try_load_arial_font() -> bool {
    crate::debug_info!("Attempting to load Arial font from /arial.ttf");
    
    // Check if filesystem is available
    if !crate::fs::exists("/arial.ttf") {
        crate::debug_info!("Arial font file not found or filesystem not ready");
        return false;
    }
    
    match get_arial_font_from_fs() {
        Some(font) => {
            crate::debug_info!("Arial font loaded successfully from filesystem!");
            set_default_font(font);
            true
        }
        None => {
            crate::debug_info!("Arial font failed to load from filesystem");
            false
        }
    }
}