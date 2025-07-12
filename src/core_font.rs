// Re-export types and constants from modules
pub use crate::embedded_font::{Embedded8x8Font, DEFAULT_8X8_FONT};
pub use crate::vfnt::VFNTFont;

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