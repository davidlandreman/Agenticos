//! Backend-neutral retained scene description.

use alloc::vec::Vec;

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
    /// Blur/sample already-composed layers behind this layer.
    BackdropSample {
        radius: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub surface_id: SurfaceId,
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
            surface_id,
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

pub struct SceneFrame {
    pub output_size: (u32, u32),
    pub layers: Vec<Layer>,
}

impl SceneFrame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            output_size: (width, height),
            layers: Vec::new(),
        }
    }

    pub fn push(&mut self, layer: Layer) {
        self.layers.push(layer);
    }

    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn sort_by_z(&mut self) {
        // Stable sort preserves tree order among equal-z layers.
        self.layers.sort_by_key(|layer| layer.z_index);
    }
}
