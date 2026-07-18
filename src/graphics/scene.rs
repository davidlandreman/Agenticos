//! Backend-neutral retained scene description.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::graphics::composition::ClientGlId;
use crate::graphics::surface::SurfaceId;
use crate::window::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform2D {
    /// 16.16 fixed-point affine matrix.
    pub m11: i32,
    pub m12: i32,
    pub m21: i32,
    pub m22: i32,
    pub tx: i32,
    pub ty: i32,
}

impl Transform2D {
    pub const IDENTITY: Self = Self {
        m11: 1 << 16,
        m12: 0,
        m21: 0,
        m22: 1 << 16,
        tx: 0,
        ty: 0,
    };
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub const fn translation(tx: i32, ty: i32) -> Self {
        Self {
            tx,
            ty,
            ..Self::IDENTITY
        }
    }
    pub const fn is_translation(self) -> bool {
        self.m11 == 1 << 16 && self.m12 == 0 && self.m21 == 0 && self.m22 == 1 << 16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerEffect {
    None,
    /// Test/extension point for per-pixel alpha content already in the surface.
    #[expect(dead_code, reason = "intentional kernel API surface")]
    AlphaMask,
    /// Blur/sample already-composed layers behind this layer. The effective
    /// source alpha also masks the blur strength, so translucent effect edges
    /// transition continuously back to the sharp backdrop.
    BackdropSample {
        radius: u16,
    },
}

/// Conservative source-local coverage for pixels that need a backdrop sample.
/// `Regions` may include opaque/transparent neighbors, but it must contain every
/// pixel whose effective alpha is strictly between zero and 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackdropCoverage {
    Empty,
    Full,
    Regions(Vec<Rect>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerSource {
    Canonical(SurfaceId),
    VirglClient(ClientGlId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub source: LayerSource,
    pub source_rect: Rect,
    pub destination_rect: Rect,
    pub clip_rect: Rect,
    pub opacity: u8,
    pub transform: Transform2D,
    pub effect: LayerEffect,
    pub z_index: i32,
    pub visible: bool,
}

impl Layer {
    pub fn opaque(surface_id: SurfaceId, destination_rect: Rect) -> Self {
        Self {
            source: LayerSource::Canonical(surface_id),
            source_rect: Rect::new(0, 0, destination_rect.width, destination_rect.height),
            destination_rect,
            clip_rect: destination_rect,
            opacity: u8::MAX,
            transform: Transform2D::IDENTITY,
            effect: LayerEffect::None,
            z_index: 0,
            visible: true,
        }
    }

    pub fn virgl_client(
        client_id: ClientGlId,
        source_width: u32,
        source_height: u32,
        destination_rect: Rect,
    ) -> Self {
        Self {
            source: LayerSource::VirglClient(client_id),
            source_rect: Rect::new(0, 0, source_width, source_height),
            destination_rect,
            clip_rect: destination_rect,
            opacity: u8::MAX,
            transform: Transform2D::IDENTITY,
            effect: LayerEffect::None,
            z_index: 0,
            visible: true,
        }
    }

    pub const fn canonical_surface_id(self) -> Option<SurfaceId> {
        match self.source {
            LayerSource::Canonical(id) => Some(id),
            LayerSource::VirglClient(_) => None,
        }
    }

    pub fn output_bounds(self) -> Rect {
        Rect::new(
            self.destination_rect.x.saturating_add(self.transform.tx),
            self.destination_rect.y.saturating_add(self.transform.ty),
            self.destination_rect.width,
            self.destination_rect.height,
        )
    }
}

pub fn inflate_rect(rect: Rect, radius: u32) -> Rect {
    Rect::new(
        rect.x.saturating_sub(radius.min(i32::MAX as u32) as i32),
        rect.y.saturating_sub(radius.min(i32::MAX as u32) as i32),
        rect.width.saturating_add(radius.saturating_mul(2)),
        rect.height.saturating_add(radius.saturating_mul(2)),
    )
}

/// Split a total backdrop radius across the three box blurs used by both
/// composition backends. Three box blurs are a cheap Gaussian approximation;
/// keeping the partition here makes CPU and GPU effects share one contract.
pub const fn backdrop_box_radii(total: u16) -> [u16; 3] {
    [total / 3, total / 3, total / 3 + total % 3]
}

/// Sampling halo required to evaluate every visible backdrop effect in
/// z-order. Stacked glass layers accumulate their sampling reach.
pub fn backdrop_halo(layers: &[Layer]) -> u32 {
    layers.iter().fold(0u32, |halo, layer| {
        halo.saturating_add(match layer.effect {
            LayerEffect::BackdropSample { radius } if layer.visible => radius as u32,
            _ => 0,
        })
    })
}

pub struct SceneFrame {
    pub output_size: (u32, u32),
    pub layers: Vec<Layer>,
    backdrop_coverage: BTreeMap<SurfaceId, BackdropCoverage>,
}

impl SceneFrame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            output_size: (width, height),
            layers: Vec::new(),
            backdrop_coverage: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, layer: Layer) {
        self.layers.push(layer);
    }

    pub fn set_backdrop_coverage(&mut self, surface_id: SurfaceId, coverage: BackdropCoverage) {
        self.backdrop_coverage.insert(surface_id, coverage);
    }

    pub fn backdrop_coverage(&self, layer: &Layer) -> Option<&BackdropCoverage> {
        let LayerSource::Canonical(surface_id) = layer.source else {
            return None;
        };
        self.backdrop_coverage.get(&surface_id)
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn sort_by_z(&mut self) {
        // Stable sort preserves tree order among equal-z layers.
        self.layers.sort_by_key(|layer| layer.z_index);
    }
}

/// Return output-space regions in which `layer` may need its backdrop effect.
/// Missing metadata deliberately falls back to the whole layer.
pub fn backdrop_targets(
    scene: &SceneFrame,
    layer: &Layer,
    damage: Rect,
    output_bounds: Rect,
) -> Vec<Rect> {
    let layer_bounds = layer.output_bounds();
    let Some(draw) = damage
        .intersection(&layer_bounds)
        .and_then(|rect| rect.intersection(&layer.clip_rect))
        .and_then(|rect| rect.intersection(&output_bounds))
    else {
        return Vec::new();
    };

    let Some(coverage) = scene.backdrop_coverage(layer) else {
        return alloc::vec![draw];
    };
    match coverage {
        BackdropCoverage::Empty => Vec::new(),
        BackdropCoverage::Full => alloc::vec![draw],
        BackdropCoverage::Regions(regions) => regions
            .iter()
            .filter_map(|source| {
                let source = source.intersection(&layer.source_rect)?;
                let output = Rect::new(
                    layer_bounds
                        .x
                        .saturating_add(source.x.saturating_sub(layer.source_rect.x)),
                    layer_bounds
                        .y
                        .saturating_add(source.y.saturating_sub(layer.source_rect.y)),
                    source.width,
                    source.height,
                );
                output.intersection(&draw)
            })
            .collect(),
    }
}
