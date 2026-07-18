use alloc::collections::BTreeMap;

use crate::graphics::scene::{LayerEffect, SceneFrame};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::Rect;

use super::{CompositionEngine, CompositionEngineKind, CompositionError, RenderStats};

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
        if scene.output_size != (self.output.width(), self.output.height()) {
            return Err(CompositionError::InvalidOutput);
        }

        let output_bounds = Rect::new(0, 0, self.output.width(), self.output.height());
        let mut stats = RenderStats::default();
        for requested in damage {
            let Some(damage_rect) = requested.intersection(&output_bounds) else {
                continue;
            };
            self.output.clear(damage_rect, PremulArgb::TRANSPARENT);
            stats.output_pixels_damaged = stats
                .output_pixels_damaged
                .saturating_add(damage_rect.area());

            for layer in &scene.layers {
                if !layer.visible || layer.opacity == 0 {
                    continue;
                }
                if !layer.transform.is_translation() {
                    return Err(CompositionError::UnsupportedTransform);
                }
                if matches!(layer.effect, LayerEffect::BackdropSample { .. }) {
                    return Err(CompositionError::UnsupportedEffect);
                }
                let source = surfaces
                    .get(&layer.surface_id)
                    .ok_or(CompositionError::MissingSurface(layer.surface_id))?;
                let layer_bounds = layer.output_bounds();
                let Some(draw) = damage_rect
                    .intersection(&layer_bounds)
                    .and_then(|r| r.intersection(&layer.clip_rect))
                else {
                    continue;
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
                        let dst = self
                            .output
                            .pixel(x as u32, y as u32)
                            .unwrap_or(PremulArgb::TRANSPARENT);
                        self.output
                            .set_pixel(x as u32, y as u32, src.source_over(dst));
                    }
                }
            }
        }
        Ok(stats)
    }

    fn output(&self) -> &Surface {
        &self.output
    }
    fn output_mut(&mut self) -> &mut Surface {
        &mut self.output
    }
}
