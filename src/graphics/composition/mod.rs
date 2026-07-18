//! Replaceable retained composition engines.

mod cpu;

pub use cpu::CpuCompositionEngine;

use alloc::collections::BTreeMap;

use crate::graphics::scene::SceneFrame;
use crate::graphics::surface::{Surface, SurfaceId};
use crate::window::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionEngineKind {
    Cpu,
    Virgl,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub windows_rasterized: u64,
    pub surface_pixels_updated: u64,
    pub layers_composed: u64,
    pub texture_bytes_uploaded: u64,
    pub output_pixels_damaged: u64,
    pub presents: u64,
}

pub trait CompositionEngine {
    fn kind(&self) -> CompositionEngineKind;
    fn compose(
        &mut self,
        scene: &SceneFrame,
        surfaces: &BTreeMap<SurfaceId, Surface>,
        damage: &[Rect],
    ) -> Result<RenderStats, CompositionError>;
    fn output(&self) -> &Surface;
    fn output_mut(&mut self) -> &mut Surface;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionError {
    InvalidOutput,
    MissingSurface(SurfaceId),
    UnsupportedTransform,
    UnsupportedEffect,
    SurfaceAllocation,
}
