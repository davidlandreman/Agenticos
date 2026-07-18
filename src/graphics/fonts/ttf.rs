//! TrueType / OpenType font backend.
//!
//! Parses outlines via `ttf-parser` and rasterizes them with
//! `ab_glyph_rasterizer` into per-glyph 8bpp coverage bitmaps. ASCII printable
//! glyphs are pre-rendered at construction; anything else is lazily rasterized
//! on first lookup and cached in a small `BTreeMap`.
//!
//! The kernel ships exactly one face, used as a *monospaced* system font.
//! `cell_width` is taken from the advance of `'M'` after scaling — for a true
//! monospaced face every glyph has the same advance, so this matches what
//! consumers see when laying out text in a fixed grid.

use super::core_font::{Font, Glyph};
use ab_glyph_rasterizer::{Point, Rasterizer};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use libm::{ceilf, floorf, roundf};
use spin::Mutex;
use ttf_parser::{Face, OutlineBuilder};

const FIRST_ASCII: u32 = 32;
const NUM_ASCII: usize = 95; // 32..=126

#[derive(Default)]
struct GlyphSlot {
    width: u16,
    height: u16,
    /// Bitmap left edge relative to the pen position, in pixels.
    x_offset: i16,
    /// Bitmap top edge relative to the baseline. Negative means above.
    y_offset: i16,
    /// Advance width in pixels.
    advance: u16,
    /// Per-glyph 8bpp coverage. `width * height` bytes; empty for advance-only
    /// glyphs (e.g. space) or missing glyphs.
    coverage: Box<[u8]>,
}

pub struct TtfFont {
    /// Backing TTF bytes. Owned so the `Face<'static>` borrow is sound.
    _data: Box<[u8]>,
    face: Face<'static>,
    px_size: u16,
    scale: f32,
    line_height: u32,
    ascent: u32,
    cell_width: u32,
    ascii_slots: [GlyphSlot; NUM_ASCII],
    /// Lazy cache for non-ASCII glyphs.
    extras: Mutex<BTreeMap<char, Box<GlyphSlot>>>,
}

unsafe impl Send for TtfFont {}
unsafe impl Sync for TtfFont {}

impl TtfFont {
    /// Parse a TTF/OTF face and pre-rasterize printable ASCII at `px_size`.
    /// Returns `None` if the data fails to parse.
    pub fn from_data(data: &[u8], px_size: u16) -> Option<Self> {
        let owned: Box<[u8]> = data.to_vec().into_boxed_slice();
        // SAFETY: `owned` lives in this struct alongside `face`. Struct fields
        // drop in declaration order, so `face` (declared before `_data`) drops
        // first; the borrow remains valid for `face`'s entire lifetime.
        let static_data: &'static [u8] =
            unsafe { core::slice::from_raw_parts(owned.as_ptr(), owned.len()) };

        let face = match Face::parse(static_data, 0) {
            Ok(f) => f,
            Err(e) => {
                crate::debug_warn!("TTF: parse failed: {:?}", e);
                return None;
            }
        };

        let units_per_em = face.units_per_em() as f32;
        if units_per_em <= 0.0 {
            crate::debug_warn!("TTF: invalid units_per_em");
            return None;
        }
        let scale = px_size as f32 / units_per_em;

        let ascent = roundf(face.ascender() as f32 * scale).max(0.0) as u32;
        let descent = roundf(-face.descender() as f32 * scale).max(0.0) as u32;
        let line_gap = roundf(face.line_gap() as f32 * scale).max(0.0) as u32;
        let line_height = ascent + descent + line_gap;

        let cell_width = advance_for_char(&face, scale, 'M')
            .or_else(|| advance_for_char(&face, scale, 'x'))
            .unwrap_or((px_size as u32).max(1) / 2)
            .max(1);

        let mut ascii_slots: [GlyphSlot; NUM_ASCII] =
            core::array::from_fn(|_| GlyphSlot::default());
        for i in 0..NUM_ASCII {
            let ch = char::from_u32(FIRST_ASCII + i as u32).unwrap();
            ascii_slots[i] = rasterize_glyph(&face, scale, ch);
        }

        Some(TtfFont {
            _data: owned,
            face,
            px_size,
            scale,
            line_height,
            ascent,
            cell_width,
            ascii_slots,
            extras: Mutex::new(BTreeMap::new()),
        })
    }

    fn slot_glyph<'a>(slot: &'a GlyphSlot) -> Glyph<'a> {
        Glyph {
            width: slot.width as u32,
            height: slot.height as u32,
            x_offset: slot.x_offset as i32,
            y_offset: slot.y_offset as i32,
            advance: slot.advance as u32,
            coverage: &slot.coverage,
        }
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub fn px_size(&self) -> u16 {
        self.px_size
    }
}

impl Font for TtfFont {
    fn glyph(&self, ch: char) -> Option<Glyph<'_>> {
        let code = ch as u32;
        if (FIRST_ASCII..FIRST_ASCII + NUM_ASCII as u32).contains(&code) {
            return Some(Self::slot_glyph(
                &self.ascii_slots[(code - FIRST_ASCII) as usize],
            ));
        }

        // Lazy path: rasterize, store in extras as a Box so the slot's address
        // is stable across BTreeMap rebalances. We then return a slice with a
        // lifetime tied to `self` — sound because we never remove entries.
        {
            let extras = self.extras.lock();
            if let Some(slot_box) = extras.get(&ch) {
                let slot_ptr: *const GlyphSlot = &**slot_box;
                drop(extras);
                // SAFETY: extras entries are append-only for this font's
                // lifetime; the Box keeps the GlyphSlot at a stable address.
                let slot: &GlyphSlot = unsafe { &*slot_ptr };
                return Some(Self::slot_glyph(slot));
            }
        }

        let new_slot = Box::new(rasterize_glyph(&self.face, self.scale, ch));
        let slot_ptr: *const GlyphSlot = &*new_slot;
        self.extras.lock().insert(ch, new_slot);
        let slot: &GlyphSlot = unsafe { &*slot_ptr };
        Some(Self::slot_glyph(slot))
    }

    fn line_height(&self) -> u32 {
        self.line_height
    }

    fn ascent(&self) -> u32 {
        self.ascent
    }

    fn cell_width(&self) -> u32 {
        self.cell_width
    }
}

fn rasterize_glyph(face: &Face, scale: f32, ch: char) -> GlyphSlot {
    let Some(gid) = face.glyph_index(ch) else {
        return GlyphSlot::default();
    };

    let advance = face
        .glyph_hor_advance(gid)
        .map(|a| roundf(a as f32 * scale) as u16)
        .unwrap_or(0);

    let Some(bbox) = face.glyph_bounding_box(gid) else {
        // Advance-only glyph (e.g. space).
        return GlyphSlot {
            advance,
            ..GlyphSlot::default()
        };
    };

    let px_min_x = floorf(bbox.x_min as f32 * scale) as i32;
    let px_max_x = ceilf(bbox.x_max as f32 * scale) as i32;
    let px_min_y = floorf(bbox.y_min as f32 * scale) as i32;
    let px_max_y = ceilf(bbox.y_max as f32 * scale) as i32;

    let width = (px_max_x - px_min_x).max(0) as u32;
    let height = (px_max_y - px_min_y).max(0) as u32;
    if width == 0 || height == 0 {
        return GlyphSlot {
            advance,
            ..GlyphSlot::default()
        };
    }

    let mut rasterizer = Rasterizer::new(width as usize, height as usize);
    let mut builder = OutlineToRasterizer {
        rasterizer: &mut rasterizer,
        scale,
        offset_x: px_min_x as f32,
        // Flip y-axis: TTF is y-up, rasterizer is y-down with origin at top.
        offset_y: px_max_y as f32,
        last: Point { x: 0.0, y: 0.0 },
        start: Point { x: 0.0, y: 0.0 },
    };
    if face.outline_glyph(gid, &mut builder).is_none() {
        return GlyphSlot {
            advance,
            ..GlyphSlot::default()
        };
    }

    let mut coverage = vec![0u8; (width * height) as usize].into_boxed_slice();
    rasterizer.for_each_pixel(|idx, alpha| {
        coverage[idx] = roundf((alpha * 255.0).clamp(0.0, 255.0)) as u8;
    });

    GlyphSlot {
        width: width as u16,
        height: height as u16,
        x_offset: px_min_x as i16,
        y_offset: -(px_max_y as i16),
        advance,
        coverage,
    }
}

fn advance_for_char(face: &Face, scale: f32, ch: char) -> Option<u32> {
    let gid = face.glyph_index(ch)?;
    let aw = face.glyph_hor_advance(gid)? as f32 * scale;
    Some(roundf(aw).max(0.0) as u32)
}

/// Bridges `ttf_parser::OutlineBuilder` to `ab_glyph_rasterizer::Rasterizer`,
/// converting font-unit y-up coordinates into pixel y-down coordinates.
struct OutlineToRasterizer<'r> {
    rasterizer: &'r mut Rasterizer,
    scale: f32,
    offset_x: f32,
    offset_y: f32,
    last: Point,
    start: Point,
}

impl<'r> OutlineToRasterizer<'r> {
    fn map(&self, x: f32, y: f32) -> Point {
        Point {
            x: x * self.scale - self.offset_x,
            y: self.offset_y - y * self.scale,
        }
    }
}

impl<'r> OutlineBuilder for OutlineToRasterizer<'r> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.last = p;
        self.start = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.rasterizer.draw_line(self.last, p);
        self.last = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let c = self.map(x1, y1);
        let p = self.map(x, y);
        self.rasterizer.draw_quad(self.last, c, p);
        self.last = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.map(x1, y1);
        let c2 = self.map(x2, y2);
        let p = self.map(x, y);
        self.rasterizer.draw_cubic(self.last, c1, c2, p);
        self.last = p;
    }

    fn close(&mut self) {
        if self.last.x != self.start.x || self.last.y != self.start.y {
            self.rasterizer.draw_line(self.last, self.start);
        }
        self.last = self.start;
    }
}
