//! Font subsystem tests. Run after `init_fonts()` so the bundled system TTF
//! has already been parsed into the default font.

use crate::graphics::fonts::core_font::{get_default_font, get_embedded_font, get_terminal_font};
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

fn test_powerline_glyphs_and_metrics() {
    let font = get_default_font();
    for ch in ['\u{E0A0}', '\u{E0B0}', '\u{E0B1}', '\u{E0B2}', '\u{E0B3}'] {
        let glyph = font.glyph(ch).expect("missing Powerline glyph");
        assert!(glyph.width > 0, "Powerline glyph {:?} has zero width", ch);
        assert!(glyph.height > 0, "Powerline glyph {:?} has zero height", ch);
        assert!(
            glyph.coverage.iter().any(|&alpha| alpha != 0),
            "Powerline glyph {:?} has empty coverage",
            ch
        );
    }

    // JetBrains Mono 2.304 at SYSTEM_FONT_PX=14. Pinning these metrics keeps
    // the desktop and terminal grids stable across font refreshes.
    assert_eq!(font.cell_width(), 8);
    assert_eq!(font.line_height(), 18);
}

fn test_terminal_font_is_larger_than_ui_font() {
    let ui = get_default_font();
    let terminal = get_terminal_font();
    assert!(
        terminal.cell_width() > ui.cell_width(),
        "terminal characters should be wider than default UI characters"
    );
    assert!(
        terminal.line_height() > ui.line_height(),
        "terminal lines should be taller than default UI lines"
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
        &test_powerline_glyphs_and_metrics,
        &test_terminal_font_is_larger_than_ui_font,
        &test_embedded_fallback_has_full_ascii,
    ]
}
