//! Monospaced JetBrains Mono rasterizer for the terminal grid.
//!
//! Parses the bundled `assets/system.ttf` at a chosen pixel size, eagerly
//! rasterizes printable ASCII into 8-bit coverage bitmaps, and lazily caches
//! non-ASCII glyphs. This is the terminal-scale, color-agnostic sibling of
//! `userland/libs/gui/src/font.rs` (which is fixed at the 11px caption size and
//! ASCII-only); the coverage math is the same, taken from the kernel
//! `src/graphics/fonts/ttf.rs` reference.

use ab_glyph_rasterizer::{Point, Rasterizer};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use libm::{ceilf, floorf, roundf};
use ttf_parser::{Face, OutlineBuilder};

const FIRST_ASCII: u32 = 32;
const NUM_ASCII: usize = 95; // 32..=126

static SYSTEM_TTF_DATA: &[u8] = include_bytes!("../../../../assets/system.ttf");

/// An 8-bit coverage bitmap for one glyph, positioned relative to the pen.
#[derive(Default)]
pub struct Glyph {
    pub width: u16,
    pub height: u16,
    /// Left edge relative to the pen origin.
    pub x_offset: i16,
    /// Top edge relative to the baseline (negative = above baseline).
    pub y_offset: i16,
    pub coverage: Box<[u8]>,
}

/// The rasterized terminal face plus its cell metrics.
pub struct TermFont {
    face: Face<'static>,
    scale: f32,
    ascent: i32,
    cell_width: u32,
    line_height: u32,
    ascii: [Glyph; NUM_ASCII],
    cache: BTreeMap<char, Glyph>,
}

impl TermFont {
    /// Parse the bundled TTF at `px_size` and pre-render printable ASCII.
    pub fn new(px_size: u16) -> Self {
        let face = Face::parse(SYSTEM_TTF_DATA, 0)
            .unwrap_or_else(|_| panic!("bundled system.ttf must be valid"));
        let scale = px_size as f32 / face.units_per_em() as f32;
        let ascent = roundf(face.ascender() as f32 * scale).max(0.0) as i32;
        let descent = roundf(-face.descender() as f32 * scale).max(0.0) as i32;
        let line_gap = roundf(face.line_gap() as f32 * scale).max(0.0) as i32;
        let line_height = (ascent + descent + line_gap).max(1) as u32;
        // Monospaced: every advance matches 'M'.
        let cell_width = advance_for_char(&face, scale, 'M').max(1) as u32;

        let ascii = core::array::from_fn(|index| {
            let character = char::from_u32(FIRST_ASCII + index as u32).unwrap();
            rasterize_glyph(&face, scale, character)
        });

        Self {
            face,
            scale,
            ascent,
            cell_width,
            line_height,
            ascii,
            cache: BTreeMap::new(),
        }
    }

    pub fn cell_width(&self) -> u32 {
        self.cell_width
    }

    pub fn line_height(&self) -> u32 {
        self.line_height
    }

    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    /// Fetch a glyph, rasterizing and caching non-ASCII on first use.
    pub fn glyph(&mut self, character: char) -> &Glyph {
        let code = character as u32;
        if (FIRST_ASCII..FIRST_ASCII + NUM_ASCII as u32).contains(&code) {
            return &self.ascii[(code - FIRST_ASCII) as usize];
        }
        if !self.cache.contains_key(&character) {
            let glyph = rasterize_glyph(&self.face, self.scale, character);
            self.cache.insert(character, glyph);
        }
        self.cache.get(&character).unwrap()
    }
}

fn advance_for_char(face: &Face<'_>, scale: f32, character: char) -> u16 {
    face.glyph_index(character)
        .and_then(|glyph| face.glyph_hor_advance(glyph))
        .map(|advance| roundf(advance as f32 * scale).max(0.0) as u16)
        .unwrap_or(0)
}

fn rasterize_glyph(face: &Face<'_>, scale: f32, character: char) -> Glyph {
    let Some(glyph_id) = face.glyph_index(character) else {
        return Glyph::default();
    };
    let Some(bounds) = face.glyph_bounding_box(glyph_id) else {
        return Glyph::default();
    };

    let min_x = floorf(bounds.x_min as f32 * scale) as i32;
    let max_x = ceilf(bounds.x_max as f32 * scale) as i32;
    let min_y = floorf(bounds.y_min as f32 * scale) as i32;
    let max_y = ceilf(bounds.y_max as f32 * scale) as i32;
    let width = (max_x - min_x).max(0) as usize;
    let height = (max_y - min_y).max(0) as usize;
    if width == 0 || height == 0 {
        return Glyph::default();
    }

    let mut rasterizer = Rasterizer::new(width, height);
    let mut builder = OutlineRasterizer {
        rasterizer: &mut rasterizer,
        scale,
        offset_x: min_x as f32,
        offset_y: max_y as f32,
        last: Point { x: 0.0, y: 0.0 },
        start: Point { x: 0.0, y: 0.0 },
    };
    if face.outline_glyph(glyph_id, &mut builder).is_none() {
        return Glyph::default();
    }

    let mut coverage = vec![0u8; width * height].into_boxed_slice();
    rasterizer.for_each_pixel(|index, alpha| {
        coverage[index] = roundf((alpha * 255.0).clamp(0.0, 255.0)) as u8;
    });
    Glyph {
        width: width as u16,
        height: height as u16,
        x_offset: min_x as i16,
        y_offset: -(max_y as i16),
        coverage,
    }
}

struct OutlineRasterizer<'a> {
    rasterizer: &'a mut Rasterizer,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    last: Point,
    start: Point,
}

impl OutlineRasterizer<'_> {
    fn map(&self, x: f32, y: f32) -> Point {
        Point {
            x: x * self.scale - self.offset_x,
            y: self.offset_y - y * self.scale,
        }
    }
}

impl OutlineBuilder for OutlineRasterizer<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.last = point;
        self.start = point;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let point = self.map(x, y);
        self.rasterizer.draw_line(self.last, point);
        self.last = point;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let control = self.map(x1, y1);
        let point = self.map(x, y);
        self.rasterizer.draw_quad(self.last, control, point);
        self.last = point;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let control1 = self.map(x1, y1);
        let control2 = self.map(x2, y2);
        let point = self.map(x, y);
        self.rasterizer
            .draw_cubic(self.last, control1, control2, point);
        self.last = point;
    }

    fn close(&mut self) {
        self.rasterizer.draw_line(self.last, self.start);
        self.last = self.start;
    }
}
