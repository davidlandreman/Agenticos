//! Embedded 8x8 bitmap font, used as the parse-failure fallback for the
//! system TTF.
//!
//! The source data is 1-bit packed; the [`Font`] trait expects 8bpp coverage.
//! We expand the bit-rows into 8bpp at compile time via const evaluation so
//! glyph lookup is a simple slice borrow with no synchronization.

use super::core_font::{Font, Glyph};
use fontdata::DEFAULT_8X8_FONT_DATA;

const FIRST_CHAR: u8 = 32;
const NUM_CHARS: usize = 95;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
const GLYPH_BYTES: usize = GLYPH_W * GLYPH_H; // 64

const fn expand_coverage() -> [[u8; GLYPH_BYTES]; NUM_CHARS] {
    let mut out = [[0u8; GLYPH_BYTES]; NUM_CHARS];
    let mut idx = 0;
    while idx < NUM_CHARS {
        let mut row = 0;
        while row < GLYPH_H {
            let bits = DEFAULT_8X8_FONT_DATA[idx][row];
            let mut col = 0;
            while col < GLYPH_W {
                let on = (bits >> (7 - col)) & 1 != 0;
                out[idx][row * GLYPH_W + col] = if on { 0xFF } else { 0x00 };
                col += 1;
            }
            row += 1;
        }
        idx += 1;
    }
    out
}

static COVERAGE: [[u8; GLYPH_BYTES]; NUM_CHARS] = expand_coverage();

pub struct Embedded8x8Font;

pub static DEFAULT_8X8_FONT: Embedded8x8Font = Embedded8x8Font;

impl Font for Embedded8x8Font {
    fn glyph(&self, ch: char) -> Option<Glyph<'_>> {
        let code = ch as u32;
        if code < FIRST_CHAR as u32 || code >= FIRST_CHAR as u32 + NUM_CHARS as u32 {
            return None;
        }
        let index = (code - FIRST_CHAR as u32) as usize;
        Some(Glyph {
            width: GLYPH_W as u32,
            height: GLYPH_H as u32,
            x_offset: 0,
            // Bitmap top sits `ascent` pixels above the baseline.
            y_offset: -(GLYPH_H as i32),
            advance: GLYPH_W as u32,
            coverage: &COVERAGE[index],
        })
    }

    fn line_height(&self) -> u32 {
        GLYPH_H as u32
    }

    fn ascent(&self) -> u32 {
        GLYPH_H as u32
    }

    fn cell_width(&self) -> u32 {
        GLYPH_W as u32
    }
}
