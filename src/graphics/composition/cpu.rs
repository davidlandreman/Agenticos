use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::graphics::scene::{inflate_rect, LayerEffect, SceneFrame};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::Rect;

use super::{
    timestamp_cycles, CompositionEngine, CompositionEngineKind, CompositionError, RenderStats,
};

/// Pixel-correct reference compositor and fallback engine.
pub struct CpuCompositionEngine {
    output: Surface,
}

impl CpuCompositionEngine {
    pub fn new(width: u32, height: u32) -> Result<Self, CompositionError> {
        let output = Surface::new(SurfaceDesc::new(width, height))
            .map_err(|_| CompositionError::SurfaceAllocation)?;
        Ok(Self { output })
    }

    fn copy_region(&self, rect: Rect) -> Vec<PremulArgb> {
        let mut pixels = Vec::with_capacity(rect.width as usize * rect.height as usize);
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                pixels.push(
                    self.output
                        .pixel(x as u32, y as u32)
                        .unwrap_or(PremulArgb::TRANSPARENT),
                );
            }
        }
        pixels
    }

    fn blurred_backdrop(&self, rect: Rect, radius: u16) -> Vec<PremulArgb> {
        let mut pixels = self.copy_region(rect);
        if radius == 0 || rect.is_empty() {
            return pixels;
        }
        let total = radius as u32;
        let passes = [total / 3, total / 3, total / 3 + total % 3];
        let mut scratch = vec![PremulArgb::TRANSPARENT; pixels.len()];
        for radius in passes {
            if radius == 0 {
                continue;
            }
            box_blur_horizontal(
                &pixels,
                &mut scratch,
                rect.width as usize,
                rect.height as usize,
                radius as usize,
            );
            box_blur_vertical(
                &scratch,
                &mut pixels,
                rect.width as usize,
                rect.height as usize,
                radius as usize,
            );
        }
        pixels
    }
}

impl CompositionEngine for CpuCompositionEngine {
    fn kind(&self) -> CompositionEngineKind {
        CompositionEngineKind::Cpu
    }

    fn compose(
        &mut self,
        scene: &SceneFrame,
        surfaces: &BTreeMap<SurfaceId, Surface>,
        damage: &[Rect],
    ) -> Result<RenderStats, CompositionError> {
        let composition_started = timestamp_cycles();
        if scene.output_size != (self.output.width(), self.output.height()) {
            return Err(CompositionError::InvalidOutput);
        }
        for layer in &scene.layers {
            if layer.visible && !layer.transform.is_translation() {
                return Err(CompositionError::UnsupportedTransform);
            }
        }

        let output_bounds = Rect::new(0, 0, self.output.width(), self.output.height());
        let total_halo = scene.layers.iter().fold(0u32, |halo, layer| {
            halo.saturating_add(match layer.effect {
                LayerEffect::BackdropSample { radius } if layer.visible => radius as u32,
                _ => 0,
            })
        });
        let mut stats = RenderStats::default();
        for requested in damage {
            let Some(damage_rect) = requested.intersection(&output_bounds) else {
                continue;
            };
            let work_rect = inflate_rect(damage_rect, total_halo)
                .intersection(&output_bounds)
                .unwrap_or(damage_rect);
            let saved = self.copy_region(work_rect);
            self.output.clear(work_rect, PremulArgb::TRANSPARENT);
            stats.output_pixels_damaged = stats
                .output_pixels_damaged
                .saturating_add(damage_rect.area());

            for layer in &scene.layers {
                if !layer.visible || layer.opacity == 0 {
                    continue;
                }
                let source = surfaces
                    .get(&layer.surface_id)
                    .ok_or(CompositionError::MissingSurface(layer.surface_id))?;
                let layer_bounds = layer.output_bounds();
                let Some(draw) = work_rect
                    .intersection(&layer_bounds)
                    .and_then(|rect| rect.intersection(&layer.clip_rect))
                else {
                    continue;
                };
                let blurred = match layer.effect {
                    LayerEffect::BackdropSample { radius } => {
                        let blur_started = timestamp_cycles();
                        let blurred = self.blurred_backdrop(work_rect, radius);
                        stats.backdrop_blur_cycles = stats
                            .backdrop_blur_cycles
                            .saturating_add(timestamp_cycles().saturating_sub(blur_started));
                        Some(blurred)
                    }
                    LayerEffect::None | LayerEffect::AlphaMask => None,
                };
                stats.layers_composed = stats.layers_composed.saturating_add(1);

                for y in draw.y..draw.bottom() {
                    for x in draw.x..draw.right() {
                        let local_x = x - layer_bounds.x;
                        let local_y = y - layer_bounds.y;
                        if local_x < 0 || local_y < 0 {
                            continue;
                        }
                        let sx = layer.source_rect.x + local_x;
                        let sy = layer.source_rect.y + local_y;
                        if sx < 0 || sy < 0 {
                            continue;
                        }
                        let Some(src) = source.pixel(sx as u32, sy as u32) else {
                            continue;
                        };
                        let src = src.with_opacity(layer.opacity);
                        let output_dst = self
                            .output
                            .pixel(x as u32, y as u32)
                            .unwrap_or(PremulArgb::TRANSPARENT);
                        let dst = if src.a() > 0 && src.a() < u8::MAX {
                            blurred
                                .as_ref()
                                .map(|pixels| pixels[region_index(work_rect, x, y)])
                                .unwrap_or(output_dst)
                        } else {
                            output_dst
                        };
                        self.output
                            .set_pixel(x as u32, y as u32, src.source_over(dst));
                    }
                }
            }

            // The halo is scratch space, not output damage.
            for y in work_rect.y..work_rect.bottom() {
                for x in work_rect.x..work_rect.right() {
                    if !damage_rect.contains_point(crate::window::Point::new(x, y)) {
                        self.output.set_pixel(
                            x as u32,
                            y as u32,
                            saved[region_index(work_rect, x, y)],
                        );
                    }
                }
            }
        }
        let total_cycles = timestamp_cycles().saturating_sub(composition_started);
        stats.composition_cycles = total_cycles.saturating_sub(stats.backdrop_blur_cycles);
        Ok(stats)
    }

    fn output(&self) -> &Surface {
        &self.output
    }
    fn output_mut(&mut self) -> &mut Surface {
        &mut self.output
    }
}

fn region_index(rect: Rect, x: i32, y: i32) -> usize {
    (y - rect.y) as usize * rect.width as usize + (x - rect.x) as usize
}

fn box_blur_horizontal(
    input: &[PremulArgb],
    output: &mut [PremulArgb],
    width: usize,
    height: usize,
    radius: usize,
) {
    for y in 0..height {
        let row = y * width;
        let mut sums = [0u64; 4];
        let (mut left, mut right) = (0usize, radius.min(width - 1));
        for pixel in &input[row..=row + right] {
            add_pixel(&mut sums, *pixel);
        }
        for x in 0..width {
            output[row + x] = average_pixel(sums, right - left + 1);
            let next_left = (x + 1).saturating_sub(radius);
            let next_right = (x + 1).saturating_add(radius).min(width - 1);
            while left < next_left {
                sub_pixel(&mut sums, input[row + left]);
                left += 1;
            }
            while right < next_right {
                right += 1;
                add_pixel(&mut sums, input[row + right]);
            }
        }
    }
}

fn box_blur_vertical(
    input: &[PremulArgb],
    output: &mut [PremulArgb],
    width: usize,
    height: usize,
    radius: usize,
) {
    for x in 0..width {
        let mut sums = [0u64; 4];
        let (mut top, mut bottom) = (0usize, radius.min(height - 1));
        for y in top..=bottom {
            add_pixel(&mut sums, input[y * width + x]);
        }
        for y in 0..height {
            output[y * width + x] = average_pixel(sums, bottom - top + 1);
            let next_top = (y + 1).saturating_sub(radius);
            let next_bottom = (y + 1).saturating_add(radius).min(height - 1);
            while top < next_top {
                sub_pixel(&mut sums, input[top * width + x]);
                top += 1;
            }
            while bottom < next_bottom {
                bottom += 1;
                add_pixel(&mut sums, input[bottom * width + x]);
            }
        }
    }
}

fn add_pixel(s: &mut [u64; 4], p: PremulArgb) {
    s[0] += p.a() as u64;
    s[1] += p.r() as u64;
    s[2] += p.g() as u64;
    s[3] += p.b() as u64;
}
fn sub_pixel(s: &mut [u64; 4], p: PremulArgb) {
    s[0] -= p.a() as u64;
    s[1] -= p.r() as u64;
    s[2] -= p.g() as u64;
    s[3] -= p.b() as u64;
}
fn average_pixel(s: [u64; 4], count: usize) -> PremulArgb {
    let d = count.max(1) as u64;
    PremulArgb::from_premultiplied(
        ((s[0] + d / 2) / d) as u8,
        ((s[1] + d / 2) / d) as u8,
        ((s[2] + d / 2) / d) as u8,
        ((s[3] + d / 2) / d) as u8,
    )
}
