//! Rasterized copy of the system caption face used by kernel title bars.
//!
//! Keeping the rasterizer here lets ring-3 surfaces use the exact same
//! bundled TTF and sizing as their server-rendered window decorations. ASCII
//! glyphs are cached on first use of the canvas font, so repainting widgets
//! does not repeatedly parse or rasterize outlines.

use ab_glyph_rasterizer::{Point, Rasterizer};
use alloc::boxed::Box;
use alloc::vec;
use libm::{ceilf, floorf, roundf};
use spin::Once;
use ttf_parser::{Face, OutlineBuilder};

const FIRST_ASCII: u32 = 32;
const NUM_ASCII: usize = 95;
const CAPTION_FONT_PX: u16 = 11;

/// Monospaced advance of JetBrains Mono at the title-bar size.
pub const CELL_WIDTH: i32 = 7;
/// Line advance of JetBrains Mono at the title-bar size.
pub const LINE_HEIGHT: i32 = 14;

static SYSTEM_TTF_DATA: &[u8] = include_bytes!("../../../../assets/system.ttf");
static FONT: Once<CanvasFont> = Once::new();

pub(crate) struct Glyph {
    pub width: u16,
    pub height: u16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub coverage: Box<[u8]>,
}

impl Default for Glyph {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            x_offset: 0,
            y_offset: 0,
            coverage: Box::new([]),
        }
    }
}

pub(crate) struct CanvasFont {
    ascent: i32,
    glyphs: [Glyph; NUM_ASCII],
}

impl CanvasFont {
    fn new() -> Self {
        let face = Face::parse(SYSTEM_TTF_DATA, 0)
            .unwrap_or_else(|_| panic!("bundled system.ttf must be valid"));
        let scale = CAPTION_FONT_PX as f32 / face.units_per_em() as f32;
        let ascent = roundf(face.ascender() as f32 * scale).max(0.0) as i32;
        let glyphs = core::array::from_fn(|index| {
            let character = char::from_u32(FIRST_ASCII + index as u32).unwrap();
            rasterize_glyph(&face, scale, character)
        });

        debug_assert_eq!(advance_for_char(&face, scale, 'M'), CELL_WIDTH as u16);
        let descent = roundf(-face.descender() as f32 * scale).max(0.0) as i32;
        let line_gap = roundf(face.line_gap() as f32 * scale).max(0.0) as i32;
        debug_assert_eq!(ascent + descent + line_gap, LINE_HEIGHT);

        Self { ascent, glyphs }
    }

    pub(crate) fn glyph(&self, character: char) -> Option<&Glyph> {
        let code = character as u32;
        if !(FIRST_ASCII..FIRST_ASCII + NUM_ASCII as u32).contains(&code) {
            return None;
        }
        Some(&self.glyphs[(code - FIRST_ASCII) as usize])
    }

    pub(crate) fn ascent(&self) -> i32 {
        self.ascent
    }
}

pub(crate) fn canvas_font() -> &'static CanvasFont {
    FONT.call_once(CanvasFont::new)
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

    let mut coverage = vec![0; width * height].into_boxed_slice();
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
