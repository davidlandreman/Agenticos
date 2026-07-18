//! Replaceable retained composition engines.

mod cpu;
mod virgl;

pub use cpu::CpuCompositionEngine;
#[cfg(feature = "test")]
pub(crate) use virgl::stage_surface_rect_for_test;
pub use virgl::VirglCompositionEngine;

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
    pub frames: u64,
    pub windows_rasterized: u64,
    pub surface_pixels_updated: u64,
    pub layers_composed: u64,
    pub texture_bytes_uploaded: u64,
    pub texture_upload_regions: u64,
    pub texture_cache_hits: u64,
    pub texture_cache_misses: u64,
    pub texture_cache_replacements: u64,
    pub texture_cache_evictions: u64,
    pub texture_resources_created: u64,
    pub texture_resources_destroyed: u64,
    /// Fixed framebuffer/shader/state objects created for this frame.
    pub pipeline_objects_created: u64,
    pub sampler_views_created: u64,
    pub sampler_views_destroyed: u64,
    pub vertex_resources_created: u64,
    pub vertex_resources_destroyed: u64,
    pub vertex_buffer_capacity: u64,
    pub texture_cache_bytes: u64,
    pub texture_cache_peak_bytes: u64,
    pub command_stream_dwords: u64,
    pub gpu_submissions: u64,
    pub output_damage_regions: u64,
    pub output_pixels_damaged: u64,
    pub presents: u64,
    /// Bytes copied from a host GPU resource back into guest memory.
    pub gpu_readback_bytes: u64,
    /// Direct-scanout flush commands issued for this frame.
    pub scanout_flushes: u64,
    /// Output-to-scratch copies issued for backdrop effects.
    pub backdrop_copies: u64,
    pub backdrop_copy_pixels: u64,
    /// Separable render passes issued into blur ping-pong targets.
    pub backdrop_blur_passes: u64,
    pub backdrop_blur_pixels: u64,
    /// Fixed guest backing held by the two blur scratch targets.
    pub backdrop_scratch_bytes: u64,
    /// Hardware cursor define/move commands issued for this frame.
    pub cursor_updates: u64,
    /// Guest CPU cycles spent rasterizing widgets/chrome into retained surfaces.
    pub surface_raster_cycles: u64,
    /// Guest CPU cycles spent staging surface damage for a host texture.
    pub texture_upload_cycles: u64,
    /// Guest CPU cycles spent composing layers, excluding backdrop blur.
    pub composition_cycles: u64,
    /// Guest CPU cycles spent producing backdrop samples/blur passes.
    pub backdrop_blur_cycles: u64,
    /// Guest CPU cycles blocked on a host GPU fence.
    pub fence_wait_cycles: u64,
    /// Guest CPU cycles blocked on explicit GPU-to-guest readback.
    pub gpu_readback_cycles: u64,
    /// Guest CPU cycles spent in the selected scanout presenter.
    pub presentation_cycles: u64,
}

/// Low-overhead stage clock used by compositor telemetry. AgenticOS only
/// targets x86-64, so the architectural timestamp counter is always present.
#[inline]
pub fn timestamp_cycles() -> u64 {
    // SAFETY: `_rdtsc` has no memory operands and is available on x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub trait CompositionEngine: Send {
    fn kind(&self) -> CompositionEngineKind;
    fn compose(
        &mut self,
        scene: &SceneFrame,
        surfaces: &BTreeMap<SurfaceId, Surface>,
        damage: &[Rect],
    ) -> Result<RenderStats, CompositionError>;
    fn output(&self) -> &Surface;
    fn output_mut(&mut self) -> &mut Surface;

    /// Whether presentation bypasses the guest CPU output surface.
    fn uses_direct_scanout(&self) -> bool {
        false
    }

    /// Install/flush a host GPU scanout. CPU engines leave presentation to a
    /// separate 2D or boot-framebuffer presenter.
    fn present_direct(&mut self, _damage: &[Rect]) -> Result<u64, CompositionError> {
        Ok(0)
    }

    fn hardware_cursor_needs_image(&self) -> bool {
        false
    }

    /// Define or move the direct presenter's hardware cursor. `pixels` is
    /// required only for the first definition and contains 64x64 ARGB words.
    fn update_hardware_cursor(
        &mut self,
        _x: u32,
        _y: u32,
        _pixels: Option<&[u32]>,
    ) -> Result<bool, CompositionError> {
        Ok(false)
    }

    /// Explicit diagnostic/test oracle. Production direct scanout never calls
    /// this method during normal composition or presentation.
    #[cfg(feature = "test")]
    fn readback_output(&mut self) -> Result<u64, CompositionError> {
        Ok(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionError {
    InvalidOutput,
    MissingSurface(SurfaceId),
    UnsupportedTransform,
    UnsupportedEffect,
    SurfaceAllocation,
    GpuFailure,
}
