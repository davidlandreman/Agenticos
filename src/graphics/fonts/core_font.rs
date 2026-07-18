//! Font abstraction. Glyphs carry their own metrics; coverage is 8bpp alpha.
//!
//! Boot-time selection only — `init_fonts()` is called once after heap init,
//! before any window drawing. There is no runtime swap path.

use super::embedded_font::DEFAULT_8X8_FONT;
use super::ttf::TtfFont;
use alloc::boxed::Box;
use spin::Once;

/// A rasterized glyph with metrics relative to the pen position and baseline.
///
/// `coverage` is `width * height` bytes of 8bpp alpha (0 = transparent, 255 = opaque),
/// row-major, top-to-bottom.
pub struct Glyph<'a> {
    pub width: u32,
    pub height: u32,
    /// Pixels from the pen's x to the bitmap's left edge. May be negative
    /// (e.g. italic glyphs that extend left of the advance origin).
    pub x_offset: i32,
    /// Pixels from the baseline y to the bitmap's top edge. Negative for the
    /// common case where the glyph is above the baseline.
    pub y_offset: i32,
    /// Pixels to advance the pen after drawing this glyph.
    pub advance: u32,
    pub coverage: &'a [u8],
}

/// Glyph-centric font interface. All measurements are in pixels.
pub trait Font: Send + Sync {
    /// Look up a glyph for `ch`. Returns `None` if the character has no glyph.
    fn glyph(&self, ch: char) -> Option<Glyph<'_>>;

    /// Total vertical advance for one line of text.
    fn line_height(&self) -> u32;

    /// Pixels from the cell's top to the baseline.
    fn ascent(&self) -> u32;

    /// Per-character advance for a monospaced font. Used by the cell-grid TTY
    /// and by widgets that lay out text in fixed-width columns.
    fn cell_width(&self) -> u32;
}

/// Cheap-to-copy borrowed handle to a `'static` font.
#[derive(Copy, Clone)]
pub struct FontRef {
    font: &'static dyn Font,
}

impl FontRef {
    pub fn new(font: &'static dyn Font) -> Self {
        Self { font }
    }

    pub fn as_font(&self) -> &'static dyn Font {
        self.font
    }

    pub fn glyph(&self, ch: char) -> Option<Glyph<'_>> {
        self.font.glyph(ch)
    }

    pub fn line_height(&self) -> u32 {
        self.font.line_height()
    }

    pub fn ascent(&self) -> u32 {
        self.font.ascent()
    }

    pub fn cell_width(&self) -> u32 {
        self.font.cell_width()
    }
}

// === Default font selection ===

/// Default size for the system TTF, in pixels.
const SYSTEM_FONT_PX: u16 = 14;

/// Bundled monospaced TTF used as the default system font.
static SYSTEM_TTF_DATA: &[u8] = include_bytes!("../../../assets/system.ttf");

/// Boot-set default font. Set exactly once by [`init_fonts`]. Reads before
/// init fall through to the embedded 8x8 fallback.
static DEFAULT_FONT: Once<FontRef> = Once::new();

/// Parse the bundled system TTF and install it as the default font. Called
/// once during kernel boot, after heap init and before any window drawing.
///
/// On parse failure, leaves `DEFAULT_FONT` unset so `get_default_font` returns
/// the embedded fallback. The kernel boots and is usable, just with the
/// 8x8 bitmap font.
pub fn init_fonts() {
    DEFAULT_FONT.call_once(
        || match TtfFont::from_data(SYSTEM_TTF_DATA, SYSTEM_FONT_PX) {
            Some(font) => {
                crate::debug_info!(
                    "init_fonts: parsed system.ttf at {}px (cell {}x{}, ascent {})",
                    SYSTEM_FONT_PX,
                    font.cell_width(),
                    font.line_height(),
                    font.ascent(),
                );
                // Box::leak: fonts live for the entire kernel lifetime.
                let leaked: &'static TtfFont = Box::leak(Box::new(font));
                FontRef::new(leaked)
            }
            None => {
                crate::debug_warn!(
                    "init_fonts: system.ttf failed to parse; using embedded 8x8 fallback"
                );
                FontRef::new(&DEFAULT_8X8_FONT as &dyn Font)
            }
        },
    );
}

/// Return the system default font. Before [`init_fonts`] runs (or if it fails),
/// this returns the embedded 8x8 fallback.
pub fn get_default_font() -> FontRef {
    if let Some(font) = DEFAULT_FONT.get() {
        return *font;
    }
    get_embedded_font()
}

/// Return the embedded 8x8 fallback font. Used during early boot before
/// `init_fonts` runs, and as the parse-failure fallback.
pub fn get_embedded_font() -> FontRef {
    FontRef::new(&DEFAULT_8X8_FONT as &dyn Font)
}
