//! Font subsystem tests. Run after `init_fonts()` so the bundled system TTF
//! has already been parsed into the default font.

use crate::graphics::fonts::core_font::{get_default_font, get_embedded_font};
use crate::lib::test_utils::Testable;

fn test_default_font_parsed() {
    let font = get_default_font();
    let glyph = font
        .glyph('A')
        .expect("default font has no 'A' glyph — TTF parse failed?");
    assert!(font.line_height() > 0, "line_height must be positive");
    assert!(font.cell_width() > 0, "cell_width must be positive");
    assert!(font.ascent() > 0, "ascent must be positive");
    assert!(glyph.advance > 0, "glyph 'A' has zero advance");
}

fn test_monospace_invariant() {
    let font = get_default_font();
    let a = font.glyph('A').expect("missing 'A'").advance;
    let m = font.glyph('M').expect("missing 'M'").advance;
    let i = font.glyph('i').expect("missing 'i'").advance;
    assert_eq!(a, m, "expected monospace face: 'A' advance != 'M' advance");
    assert_eq!(a, i, "expected monospace face: 'A' advance != 'i' advance");
    assert_eq!(
        a,
        font.cell_width(),
        "cell_width must equal glyph advance for a monospace face"
    );
}

fn test_measure_text_sums_advances() {
    let font = get_default_font();
    let cell_w = font.cell_width();
    // For a monospace face, "hello" is exactly 5 advances.
    let mut sum: u32 = 0;
    for ch in "hello".chars() {
        sum += font.glyph(ch).map(|g| g.advance).unwrap_or(cell_w);
    }
    assert_eq!(
        sum,
        5 * cell_w,
        "5 monospace chars must sum to 5 * cell_width"
    );
}

fn test_glyph_coverage_has_antialiasing() {
    // Stronger than a CRC golden: proves the rasterizer ran *and* produced
    // partial coverage, which catches both "glyph empty" and "glyph 1bpp"
    // regressions without being brittle to library upgrades.
    let font = get_default_font();
    let glyph = font.glyph('A').expect("missing 'A'");
    assert!(glyph.width > 0, "'A' has zero width");
    assert!(glyph.height > 0, "'A' has zero height");
    let mut any_opaque = false;
    let mut any_partial = false;
    for &alpha in glyph.coverage.iter() {
        if alpha == 0xFF {
            any_opaque = true;
        }
        if alpha > 0 && alpha < 0xFF {
            any_partial = true;
        }
    }
    assert!(
        any_opaque,
        "'A' has no fully-opaque pixels — rasterizer broken?"
    );
    assert!(
        any_partial,
        "'A' has no partial-coverage pixels — AA disabled?"
    );
}

fn test_embedded_fallback_has_full_ascii() {
    // The embedded 8x8 fallback must cover printable ASCII end-to-end so a
    // TTF parse failure still leaves a usable kernel.
    let font = get_embedded_font();
    for code in 32u32..=126 {
        let ch = char::from_u32(code).unwrap();
        assert!(
            font.glyph(ch).is_some(),
            "embedded font missing glyph for {:?}",
            ch
        );
    }
    assert_eq!(font.cell_width(), 8);
    assert_eq!(font.line_height(), 8);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_default_font_parsed,
        &test_monospace_invariant,
        &test_measure_text_sums_advances,
        &test_glyph_coverage_has_antialiasing,
        &test_embedded_fallback_has_full_ascii,
    ]
}
