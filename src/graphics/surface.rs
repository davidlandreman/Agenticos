//! Canonical premultiplied-alpha retained surfaces.

use alloc::vec;
use alloc::vec::Vec;

use crate::window::Rect;

/// Bound the number of transfer rectangles a surface can accumulate between
/// successful composition frames. Beyond this point one full upload is less
/// risky than an unbounded stream of tiny VirtIO commands.
const MAX_DAMAGE_REGIONS: usize = 16;

/// Stable identity shared by scene layers and composition engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(pub u64);

/// A premultiplied ARGB8888 pixel (`0xAARRGGBB`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PremulArgb(pub u32);

impl PremulArgb {
    pub const TRANSPARENT: Self = Self(0);

    pub const fn from_premultiplied(a: u8, r: u8, g: u8, b: u8) -> Self {
        Self(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self::from_premultiplied(a, premultiply(r, a), premultiply(g, a), premultiply(b, a))
    }

    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// Return straight-alpha channels. RGB is zero when alpha is zero.
    pub fn to_rgba(self) -> (u8, u8, u8, u8) {
        let a = self.a();
        if a == 0 {
            return (0, 0, 0, 0);
        }
        (
            unpremultiply(self.r(), a),
            unpremultiply(self.g(), a),
            unpremultiply(self.b(), a),
            a,
        )
    }

    /// Apply layer opacity while preserving the premultiplied invariant.
    pub fn with_opacity(self, opacity: u8) -> Self {
        if opacity == u8::MAX {
            return self;
        }
        Self::from_premultiplied(
            scale(self.a(), opacity),
            scale(self.r(), opacity),
            scale(self.g(), opacity),
            scale(self.b(), opacity),
        )
    }

    /// Porter-Duff source-over in premultiplied integer space.
    pub fn source_over(self, dst: Self) -> Self {
        let inv = u8::MAX - self.a();
        Self::from_premultiplied(
            add_sat(self.a(), scale(dst.a(), inv)),
            add_sat(self.r(), scale(dst.r(), inv)),
            add_sat(self.g(), scale(dst.g(), inv)),
            add_sat(self.b(), scale(dst.b(), inv)),
        )
    }
}

#[inline]
fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
}

#[inline]
fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    ((channel as u32 * 255 + alpha as u32 / 2) / alpha as u32).min(255) as u8
}

#[inline]
fn scale(value: u8, factor: u8) -> u8 {
    ((value as u16 * factor as u16 + 127) / 255) as u8
}

#[inline]
fn add_sat(left: u8, right: u8) -> u8 {
    (left as u16 + right as u16).min(255) as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDesc {
    pub width: u32,
    pub height: u32,
}

impl SurfaceDesc {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn pixel_len(self) -> Result<usize, SurfaceError> {
        if self.width == 0 || self.height == 0 {
            return Err(SurfaceError::Empty);
        }
        (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or(SurfaceError::SizeOverflow)
    }

    pub fn byte_len(self) -> Result<usize, SurfaceError> {
        self.pixel_len()?
            .checked_mul(4)
            .ok_or(SurfaceError::SizeOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceError {
    Empty,
    SizeOverflow,
    BudgetExceeded,
}

/// Guest-owned canonical surface plus local damage.
pub struct Surface {
    desc: SurfaceDesc,
    pixels: Vec<PremulArgb>,
    damage: Vec<Rect>,
}

impl Surface {
    pub fn new(desc: SurfaceDesc) -> Result<Self, SurfaceError> {
        let len = desc.pixel_len()?;
        Ok(Self {
            desc,
            pixels: vec![PremulArgb::TRANSPARENT; len],
            damage: vec![Rect::new(0, 0, desc.width, desc.height)],
        })
    }

    pub const fn desc(&self) -> SurfaceDesc {
        self.desc
    }
    pub const fn width(&self) -> u32 {
        self.desc.width
    }
    pub const fn height(&self) -> u32 {
        self.desc.height
    }
    pub fn byte_len(&self) -> usize {
        self.pixels.len() * 4
    }
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn pixels(&self) -> &[PremulArgb] {
        &self.pixels
    }
    #[cfg(feature = "test")]
    pub(crate) fn pixels_mut(&mut self) -> &mut [PremulArgb] {
        &mut self.pixels
    }
    pub fn damage(&self) -> &[Rect] {
        &self.damage
    }
    pub fn damage_snapshot(&self) -> Vec<Rect> {
        self.damage.clone()
    }

    /// Clear a previously observed damage set only if no drawing changed it
    /// while the consumer was working. Returning false is conservative: the
    /// next frame may upload pixels twice, but it cannot lose an update.
    pub fn acknowledge_damage(&mut self, snapshot: &[Rect]) -> bool {
        if self.damage != snapshot {
            return false;
        }
        self.damage.clear();
        true
    }
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn clear_damage(&mut self) {
        self.damage.clear();
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn resize(&mut self, desc: SurfaceDesc) -> Result<bool, SurfaceError> {
        if self.desc == desc {
            return Ok(false);
        }
        let replacement = Self::new(desc)?;
        *self = replacement;
        Ok(true)
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<PremulArgb> {
        self.index(x, y).map(|idx| self.pixels[idx])
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: PremulArgb) {
        if let Some(idx) = self.index(x, y) {
            self.pixels[idx] = pixel;
        }
    }

    pub fn row(&self, y: u32) -> Option<&[PremulArgb]> {
        if y >= self.desc.height {
            return None;
        }
        let start = y as usize * self.desc.width as usize;
        Some(&self.pixels[start..start + self.desc.width as usize])
    }

    pub fn clear(&mut self, rect: Rect, pixel: PremulArgb) {
        let Some(rect) = self.clip(rect) else {
            return;
        };
        for y in rect.y as u32..rect.bottom() as u32 {
            let start = y as usize * self.desc.width as usize + rect.x as usize;
            let end = start + rect.width as usize;
            self.pixels[start..end].fill(pixel);
        }
        self.mark_damage(rect);
    }

    pub fn mark_damage(&mut self, rect: Rect) {
        let Some(mut merged) = self.clip(rect) else {
            return;
        };
        let mut index = 0;
        while index < self.damage.len() {
            if touches_or_overlaps(self.damage[index], merged) {
                merged = merged.union(&self.damage.remove(index));
                index = 0;
            } else {
                index += 1;
            }
        }
        self.damage.push(merged);
        if self.damage.len() > MAX_DAMAGE_REGIONS {
            self.damage.clear();
            self.damage
                .push(Rect::new(0, 0, self.desc.width, self.desc.height));
        }
    }

    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.desc.width || y >= self.desc.height {
            return None;
        }
        Some(y as usize * self.desc.width as usize + x as usize)
    }

    fn clip(&self, rect: Rect) -> Option<Rect> {
        rect.intersection(&Rect::new(0, 0, self.desc.width, self.desc.height))
    }
}

fn touches_or_overlaps(a: Rect, b: Rect) -> bool {
    a.x <= b.right() && a.right() >= b.x && a.y <= b.bottom() && a.bottom() >= b.y
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceClass {
    Visible,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    Hidden,
    Output,
}

/// Explicit accounting for retained allocations.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceBudget {
    limit: usize,
    visible: usize,
    hidden: usize,
    output: usize,
    peak: usize,
}

impl SurfaceBudget {
    pub const fn new(limit: usize) -> Self {
        Self {
            limit,
            visible: 0,
            hidden: 0,
            output: 0,
            peak: 0,
        }
    }

    pub fn reserve(&mut self, class: SurfaceClass, bytes: usize) -> Result<(), SurfaceError> {
        let total = self
            .total()
            .checked_add(bytes)
            .ok_or(SurfaceError::SizeOverflow)?;
        if total > self.limit {
            return Err(SurfaceError::BudgetExceeded);
        }
        *self.bucket_mut(class) = self
            .bucket(class)
            .checked_add(bytes)
            .ok_or(SurfaceError::SizeOverflow)?;
        self.peak = self.peak.max(total);
        Ok(())
    }

    pub fn release(&mut self, class: SurfaceClass, bytes: usize) {
        *self.bucket_mut(class) = self.bucket(class).saturating_sub(bytes);
    }

    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const fn limit(&self) -> usize {
        self.limit
    }
    pub const fn total(&self) -> usize {
        self.visible + self.hidden + self.output
    }
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub const fn visible_bytes(&self) -> usize {
        self.visible
    }
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const fn hidden_bytes(&self) -> usize {
        self.hidden
    }
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const fn output_bytes(&self) -> usize {
        self.output
    }
    pub const fn peak_bytes(&self) -> usize {
        self.peak
    }

    fn bucket(&self, class: SurfaceClass) -> usize {
        match class {
            SurfaceClass::Visible => self.visible,
            SurfaceClass::Hidden => self.hidden,
            SurfaceClass::Output => self.output,
        }
    }

    fn bucket_mut(&mut self, class: SurfaceClass) -> &mut usize {
        match class {
            SurfaceClass::Visible => &mut self.visible,
            SurfaceClass::Hidden => &mut self.hidden,
            SurfaceClass::Output => &mut self.output,
        }
    }
}
