//! VirGL retained composition engine.
//!
//! Guest surfaces remain canonical premultiplied ARGB. Stable surface IDs map
//! to persistent BGRA host textures, and only acknowledged local damage is
//! staged between frames. Ordered quads render with source-over on the host
//! GPU. Production frames remain in that GPU resource and are presented
//! through VirtIO-GPU direct scanout; readback is explicit and is reserved for
//! tests and diagnostics.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use crate::drivers::virtio::gpu::protocol::{
    GpuBox, GpuRect, FORMAT_B8G8R8A8_UNORM, VIRGL_BIND_RENDER_TARGET, VIRGL_BIND_SAMPLER_VIEW,
    VIRGL_BIND_SCANOUT,
};
use crate::drivers::virtio::gpu::virgl::commands::VirglCommandEncoder;
use crate::drivers::virtio::gpu::virgl::{VirglContext, VirglResource};
use crate::drivers::virtio::gpu::{CursorResource, VirtioGpu};
use crate::graphics::scene::{
    backdrop_box_radii, backdrop_halo, inflate_rect, LayerEffect, SceneFrame,
};
use crate::graphics::surface::{PremulArgb, Surface, SurfaceDesc, SurfaceId};
use crate::window::Rect;

use super::{
    timestamp_cycles, CompositionEngine, CompositionEngineKind, CompositionError,
    CpuCompositionEngine, RenderStats,
};

const PIPE_BUFFER: u32 = 0;
const PIPE_TEXTURE_2D: u32 = 2;
const PIPE_BIND_VERTEX_BUFFER: u32 = 1 << 4;
const FORMAT_R32G32B32A32_FLOAT: u32 = 31;
const FORMAT_R8_UNORM: u32 = 64;
const MAX_TEXTURE_CACHE_BYTES: usize = 48 * 1024 * 1024;
const MIN_VERTEX_BUFFER_BYTES: usize = 4 * 1024;

const OBJECT_BLEND: u32 = 1;
const OBJECT_RASTERIZER: u32 = 2;
const OBJECT_DSA: u32 = 3;
const OBJECT_SHADER: u32 = 4;
const OBJECT_VERTEX_ELEMENTS: u32 = 5;
const OBJECT_SAMPLER_VIEW: u32 = 6;
const OBJECT_SAMPLER_STATE: u32 = 7;

const OUTPUT_SURFACE: u32 = 1;
const VERTEX_ELEMENTS: u32 = 2;
const VERTEX_SHADER_HANDLE: u32 = 3;
const FRAGMENT_SHADER_HANDLE: u32 = 4;
const BLEND_STATE: u32 = 5;
const DSA_STATE: u32 = 6;
const RASTERIZER_STATE: u32 = 7;
const SAMPLER_STATE: u32 = 8;
const CLEAR_FRAGMENT_SHADER_HANDLE: u32 = 9;
const REPLACE_BLEND_STATE: u32 = 10;
const EFFECT_FRAGMENT_SHADER_HANDLE: u32 = 11;
const BLUR_HORIZONTAL_RADIUS_1_SHADER: u32 = 12;
const BLUR_VERTICAL_RADIUS_1_SHADER: u32 = 13;
const BLUR_HORIZONTAL_RADIUS_2_SHADER: u32 = 14;
const BLUR_VERTICAL_RADIUS_2_SHADER: u32 = 15;
const BLUR_SURFACE_A: u32 = 16;
const BLUR_SURFACE_B: u32 = 17;
const BLUR_SAMPLER_VIEW_A: u32 = 18;
const BLUR_SAMPLER_VIEW_B: u32 = 19;
const FIRST_SAMPLER_VIEW: u32 = 100;
const PIPELINE_OBJECT_COUNT: u64 = 19;
const MAX_GPU_BACKDROP_RADIUS: u16 = 4;

const VERTEX_SHADER: u32 = 0;
const FRAGMENT_SHADER: u32 = 1;
const VS: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], GENERIC[0]\n\
DCL OUT[2], GENERIC[1]\n\
IMM FLT32 { 0.5, 0.5, 0.5, 0.5 }\n\
  0: MOV OUT[1], IN[1]\n\
  1: MAD OUT[2], IN[0], IMM[0], IMM[0]\n\
  2: MOV OUT[0], IN[0]\n\
  3: END\n";
const FS: &str = "FRAG\n\
DCL IN[0], GENERIC[0], LINEAR\n\
DCL OUT[0], COLOR\n\
DCL SAMP[0]\n\
DCL SVIEW[0], 2D, FLOAT\n\
 DCL TEMP[0]\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL OUT[0], TEMP[0], IN[0].zzzz\n\
  2: END\n";
const CLEAR_FS: &str = "FRAG\n\
DCL OUT[0], COLOR\n\
IMM FLT32 { 0.0, 0.0, 0.0, 0.0 }\n\
  0: MOV OUT[0], IMM[0]\n\
  1: END\n";
const EFFECT_FS: &str = "FRAG\n\
DCL IN[0], GENERIC[0], LINEAR\n\
DCL IN[1], GENERIC[1], LINEAR\n\
DCL OUT[0], COLOR\n\
DCL SAMP[0]\n\
DCL SAMP[1]\n\
DCL SVIEW[0], 2D, FLOAT\n\
DCL SVIEW[1], 2D, FLOAT\n\
DCL TEMP[0..3]\n\
IMM FLT32 { -0.001, 1.0, 0.0, 0.0 }\n\
  0: TEX TEMP[0], IN[0], SAMP[0], 2D\n\
  1: MUL TEMP[0], TEMP[0], IN[0].zzzz\n\
  2: ADD TEMP[1], TEMP[0].wwww, IMM[0].xxxx\n\
  3: KILL_IF TEMP[1]\n\
  4: TEX TEMP[2], IN[1], SAMP[1], 2D\n\
  5: ADD TEMP[3], -TEMP[0].wwww, IMM[0].yyyy\n\
  6: MAD OUT[0], TEMP[2], TEMP[3], TEMP[0]\n\
  7: END\n";

struct PreparedLayer {
    sampler_view: u32,
    scissor: Rect,
    first_vertex: u32,
    effect: LayerEffect,
}

struct CachedTexture {
    desc: SurfaceDesc,
    resource: VirglResource,
    sampler_view: u32,
    sampler_view_live: bool,
}

pub struct VirglCompositionEngine {
    gpu: Option<VirtioGpu>,
    context: Option<VirglContext>,
    output_resource: Option<VirglResource>,
    blur_resource_a: Option<VirglResource>,
    blur_resource_b: Option<VirglResource>,
    output: Surface,
    scanout_id: u32,
    scanout_active: bool,
    cursor: Option<CursorResource>,
    texture_cache: BTreeMap<SurfaceId, CachedTexture>,
    texture_cache_bytes: usize,
    texture_cache_peak_bytes: usize,
    retired_textures: Vec<CachedTexture>,
    next_sampler_view: u32,
    pipeline_initialized: bool,
    vertex_resource: Option<VirglResource>,
    retired_vertex_resources: Vec<VirglResource>,
    vertex_capacity: usize,
    vertex_bytes: Vec<u8>,
}

impl VirglCompositionEngine {
    pub fn new(width: u32, height: u32) -> Result<Self, CompositionError> {
        if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
            return Err(CompositionError::InvalidOutput);
        }
        let output = Surface::new(SurfaceDesc::new(width, height))
            .map_err(|_| CompositionError::SurfaceAllocation)?;
        let mut gpu = VirtioGpu::discover().map_err(|_| CompositionError::GpuFailure)?;
        gpu.virgl_clear_readback_smoke()
            .map_err(|_| CompositionError::GpuFailure)?;
        gpu.virgl_alpha_readback_smoke()
            .map_err(|_| CompositionError::GpuFailure)?;
        gpu.virgl_lifecycle_smoke(1)
            .map_err(|_| CompositionError::GpuFailure)?;
        let capabilities = gpu
            .discover_virgl_capabilities()
            .map_err(|_| CompositionError::GpuFailure)?;
        let mut context = gpu
            .create_virgl_context(&capabilities)
            .map_err(|_| CompositionError::GpuFailure)?;
        let output_resource = match gpu.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            VIRGL_BIND_RENDER_TARGET | VIRGL_BIND_SCANOUT,
            width,
            height,
            output.byte_len(),
        ) {
            Ok(resource) => resource,
            Err(_) => {
                let _ = gpu.destroy_virgl_context(&mut context);
                return Err(CompositionError::GpuFailure);
            }
        };
        let blur_bind = VIRGL_BIND_RENDER_TARGET | VIRGL_BIND_SAMPLER_VIEW;
        let blur_resource_a = match gpu.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            blur_bind,
            width,
            height,
            output.byte_len(),
        ) {
            Ok(resource) => resource,
            Err(_) => {
                let mut output_resource = output_resource;
                let _ = gpu.destroy_virgl_resource(&mut context, &mut output_resource);
                let _ = gpu.destroy_virgl_context(&mut context);
                return Err(CompositionError::GpuFailure);
            }
        };
        let blur_resource_b = match gpu.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            blur_bind,
            width,
            height,
            output.byte_len(),
        ) {
            Ok(resource) => resource,
            Err(_) => {
                let mut blur_resource_a = blur_resource_a;
                let mut output_resource = output_resource;
                let _ = gpu.destroy_virgl_resource(&mut context, &mut blur_resource_a);
                let _ = gpu.destroy_virgl_resource(&mut context, &mut output_resource);
                let _ = gpu.destroy_virgl_context(&mut context);
                return Err(CompositionError::GpuFailure);
            }
        };
        let scanout_id = match gpu.enabled_scanout() {
            Ok(scanout_id) => scanout_id,
            Err(_) => {
                let mut blur_resource_b = blur_resource_b;
                let mut blur_resource_a = blur_resource_a;
                let mut output_resource = output_resource;
                let _ = gpu.destroy_virgl_resource(&mut context, &mut blur_resource_b);
                let _ = gpu.destroy_virgl_resource(&mut context, &mut blur_resource_a);
                let _ = gpu.destroy_virgl_resource(&mut context, &mut output_resource);
                let _ = gpu.destroy_virgl_context(&mut context);
                return Err(CompositionError::GpuFailure);
            }
        };
        let mut engine = Self {
            gpu: Some(gpu),
            context: Some(context),
            output_resource: Some(output_resource),
            blur_resource_a: Some(blur_resource_a),
            blur_resource_b: Some(blur_resource_b),
            output,
            scanout_id,
            scanout_active: false,
            cursor: None,
            texture_cache: BTreeMap::new(),
            texture_cache_bytes: 0,
            texture_cache_peak_bytes: 0,
            retired_textures: Vec::new(),
            // VirGL handles share one context-wide namespace even when object
            // kinds differ. Keep dynamic views above the fixed pipeline range.
            next_sampler_view: FIRST_SAMPLER_VIEW,
            pipeline_initialized: false,
            vertex_resource: None,
            retired_vertex_resources: Vec::new(),
            vertex_capacity: 0,
            vertex_bytes: Vec::new(),
        };
        engine.qualify_backdrop_pipeline()?;
        Ok(engine)
    }

    /// Prove the production copy/ping-pong/two-sampler/discard path before
    /// reporting the engine as available. The output is not scanned out yet,
    /// and the first real frame overwrites this bounded fixture.
    fn qualify_backdrop_pipeline(&mut self) -> Result<(), CompositionError> {
        let fixture_width = self.output.width().min(8);
        let fixture_height = self.output.height().min(8);
        let backdrop_id = SurfaceId(u64::MAX - 1);
        let glass_id = SurfaceId(u64::MAX);
        let mut backdrop = Surface::new(SurfaceDesc::new(fixture_width, fixture_height))
            .map_err(|_| CompositionError::SurfaceAllocation)?;
        for y in 0..fixture_height {
            for x in 0..fixture_width {
                backdrop.set_pixel(
                    x,
                    y,
                    PremulArgb::from_rgba((x * 31) as u8, (y * 27) as u8, 40, u8::MAX),
                );
            }
        }
        let mut glass = Surface::new(SurfaceDesc::new(fixture_width, fixture_height))
            .map_err(|_| CompositionError::SurfaceAllocation)?;
        glass.clear(
            Rect::new(0, 0, fixture_width, fixture_height),
            PremulArgb::TRANSPARENT,
        );
        let translucent_x = fixture_width / 2;
        let translucent_y = fixture_height / 2;
        glass.set_pixel(
            translucent_x,
            translucent_y,
            PremulArgb::from_rgba(240, 240, 255, 128),
        );

        let mut surfaces = BTreeMap::new();
        surfaces.insert(backdrop_id, backdrop);
        surfaces.insert(glass_id, glass);
        let mut scene = SceneFrame::new(self.output.width(), self.output.height());
        scene.push(crate::graphics::scene::Layer::opaque(
            backdrop_id,
            Rect::new(0, 0, fixture_width, fixture_height),
        ));
        let full = [Rect::new(0, 0, self.output.width(), self.output.height())];
        let mut cpu = CpuCompositionEngine::new(self.output.width(), self.output.height())?;
        cpu.compose(&scene, &surfaces, &full)?;
        self.compose(&scene, &surfaces, &full)?;

        let mut glass_layer = crate::graphics::scene::Layer::opaque(
            glass_id,
            Rect::new(0, 0, fixture_width, fixture_height),
        );
        glass_layer.effect = LayerEffect::BackdropSample { radius: 4 };
        scene.push(glass_layer);
        let effect_damage = [Rect::new(0, 0, fixture_width, fixture_height)];
        cpu.compose(&scene, &surfaces, &effect_damage)?;
        self.compose(&scene, &surfaces, &effect_damage)?;

        let output_width = self.output.width();
        let sample_points = [(0, 0), (translucent_x, translucent_y)];
        let actual_samples = {
            let (gpu, _, output) = self.gpu_parts()?;
            gpu.transfer_virgl_resource(
                output,
                GpuBox {
                    x: 0,
                    y: 0,
                    z: 0,
                    width: fixture_width,
                    height: fixture_height,
                    depth: 1,
                },
                false,
            )
            .map_err(|_| CompositionError::GpuFailure)?;
            let mut samples = Vec::with_capacity(sample_points.len());
            for &(x, y) in &sample_points {
                let offset = ((y * output_width + x) * 4) as usize;
                samples.push(PremulArgb(u32::from_le_bytes([
                    output.backing[offset],
                    output.backing[offset + 1],
                    output.backing[offset + 2],
                    output.backing[offset + 3],
                ])));
            }
            samples
        };
        for (index, (&(x, y), actual)) in sample_points.iter().zip(actual_samples).enumerate() {
            let expected = cpu
                .output()
                .pixel(x, y)
                .ok_or(CompositionError::GpuFailure)?;
            let tolerance = if index == 0 { 1 } else { 4 };
            if [
                expected.a().abs_diff(actual.a()),
                expected.r().abs_diff(actual.r()),
                expected.g().abs_diff(actual.g()),
                expected.b().abs_diff(actual.b()),
            ]
            .iter()
            .any(|difference| *difference > tolerance)
            {
                crate::debug_info!(
                    "VirGL backdrop qualification mismatch x={} expected={:#010x} actual={:#010x}",
                    x,
                    expected.0,
                    actual.0
                );
                return Err(CompositionError::GpuFailure);
            }
        }

        // Evict the fixture's cached textures while the context is healthy;
        // the fixed pipeline and blur resources intentionally remain live.
        self.compose(
            &SceneFrame::new(self.output.width(), self.output.height()),
            &BTreeMap::new(),
            &[],
        )?;
        crate::debug_info!("VirGL backdrop blur qualification passed");
        Ok(())
    }

    fn compose_frame(
        &mut self,
        gpu: &mut VirtioGpu,
        context: &mut VirglContext,
        output_resource: &mut VirglResource,
        blur_resource_a: &mut VirglResource,
        blur_resource_b: &mut VirglResource,
        scene: &SceneFrame,
        surfaces: &BTreeMap<SurfaceId, Surface>,
        damage: &[Rect],
    ) -> Result<RenderStats, CompositionError> {
        let width = self.output.width();
        let height = self.output.height();
        let bounds = Rect::new(0, 0, width, height);
        let damage_rects: Vec<Rect> = damage
            .iter()
            .filter_map(|requested| requested.intersection(&bounds))
            .collect();
        let mut stats = RenderStats::default();
        let upload_started = timestamp_cycles();

        let stale_ids: Vec<SurfaceId> = self
            .texture_cache
            .keys()
            .copied()
            .filter(|surface_id| !surfaces.contains_key(surface_id))
            .collect();
        for surface_id in stale_ids {
            let cached = self
                .texture_cache
                .remove(&surface_id)
                .ok_or(CompositionError::GpuFailure)?;
            let bytes = cached.resource.backing.len();
            self.retired_textures.push(cached);
            self.texture_cache_bytes = self.texture_cache_bytes.saturating_sub(bytes);
            stats.texture_cache_evictions = stats.texture_cache_evictions.saturating_add(1);
        }

        let mut prepared_surfaces = BTreeSet::new();
        for layer in &scene.layers {
            if !layer.visible || layer.opacity == 0 || !prepared_surfaces.insert(layer.surface_id) {
                continue;
            }
            let source = surfaces
                .get(&layer.surface_id)
                .ok_or(CompositionError::MissingSurface(layer.surface_id))?;
            let desc = source.desc();
            let cached_desc = self
                .texture_cache
                .get(&layer.surface_id)
                .map(|cached| cached.desc);

            if cached_desc == Some(desc) {
                stats.texture_cache_hits = stats.texture_cache_hits.saturating_add(1);
                let cached = self
                    .texture_cache
                    .get_mut(&layer.surface_id)
                    .ok_or(CompositionError::GpuFailure)?;
                for &rect in source.damage() {
                    let Some(rect) = rect.intersection(&Rect::new(0, 0, desc.width, desc.height))
                    else {
                        continue;
                    };
                    let bytes = stage_surface_rect(source, &mut cached.resource.backing, rect)?;
                    gpu.transfer_virgl_resource(
                        &mut cached.resource,
                        GpuBox {
                            x: rect.x as u32,
                            y: rect.y as u32,
                            z: 0,
                            width: rect.width,
                            height: rect.height,
                            depth: 1,
                        },
                        true,
                    )
                    .map_err(|_| CompositionError::GpuFailure)?;
                    stats.texture_bytes_uploaded =
                        stats.texture_bytes_uploaded.saturating_add(bytes);
                    stats.texture_upload_regions = stats.texture_upload_regions.saturating_add(1);
                }
                continue;
            }

            let old_bytes = self
                .texture_cache
                .get(&layer.surface_id)
                .map(|cached| cached.resource.backing.len())
                .unwrap_or(0);
            let future_bytes = self
                .texture_cache_bytes
                .saturating_sub(old_bytes)
                .checked_add(source.byte_len())
                .ok_or(CompositionError::SurfaceAllocation)?;
            if future_bytes > MAX_TEXTURE_CACHE_BYTES {
                return Err(CompositionError::SurfaceAllocation);
            }

            let sampler_view = self.allocate_sampler_view()?;
            let mut resource = gpu
                .create_virgl_resource(
                    context,
                    PIPE_TEXTURE_2D,
                    FORMAT_B8G8R8A8_UNORM,
                    VIRGL_BIND_SAMPLER_VIEW,
                    source.width(),
                    source.height(),
                    source.byte_len(),
                )
                .map_err(|_| CompositionError::GpuFailure)?;
            stats.texture_resources_created = stats.texture_resources_created.saturating_add(1);
            let full = Rect::new(0, 0, source.width(), source.height());
            let bytes = match stage_surface_rect(source, &mut resource.backing, full) {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = gpu.destroy_virgl_resource(context, &mut resource);
                    return Err(error);
                }
            };
            if gpu
                .transfer_virgl_resource(
                    &mut resource,
                    GpuBox {
                        x: 0,
                        y: 0,
                        z: 0,
                        width: source.width(),
                        height: source.height(),
                        depth: 1,
                    },
                    true,
                )
                .is_err()
            {
                let _ = gpu.destroy_virgl_resource(context, &mut resource);
                return Err(CompositionError::GpuFailure);
            }
            stats.texture_bytes_uploaded = stats.texture_bytes_uploaded.saturating_add(bytes);
            stats.texture_upload_regions = stats.texture_upload_regions.saturating_add(1);

            let replacement = CachedTexture {
                desc,
                resource,
                sampler_view,
                sampler_view_live: false,
            };
            if let Some(old) = self.texture_cache.insert(layer.surface_id, replacement) {
                stats.texture_cache_replacements =
                    stats.texture_cache_replacements.saturating_add(1);
                self.retired_textures.push(old);
            } else {
                stats.texture_cache_misses = stats.texture_cache_misses.saturating_add(1);
            }
            self.texture_cache_bytes = future_bytes;
            self.texture_cache_peak_bytes = self.texture_cache_peak_bytes.max(future_bytes);
        }
        stats.texture_upload_cycles = timestamp_cycles().saturating_sub(upload_started);
        stats.texture_cache_bytes = self.texture_cache_bytes as u64;
        stats.texture_cache_peak_bytes = self.texture_cache_peak_bytes as u64;

        let mut prepared_layers = Vec::<PreparedLayer>::new();
        self.vertex_bytes.clear();
        let clear_first_vertex = if damage_rects.is_empty() {
            None
        } else {
            append_clear_quad_vertices(&mut self.vertex_bytes);
            Some(0)
        };
        for layer in &scene.layers {
            if !layer.visible || layer.opacity == 0 {
                continue;
            }
            let source = surfaces
                .get(&layer.surface_id)
                .ok_or(CompositionError::MissingSurface(layer.surface_id))?;
            let layer_bounds = layer.output_bounds();
            let Some(scissor) = layer_bounds
                .intersection(&layer.clip_rect)
                .and_then(|rect| rect.intersection(&bounds))
            else {
                continue;
            };
            if !damage_rects
                .iter()
                .any(|damage_rect| scissor.intersection(damage_rect).is_some())
            {
                continue;
            }
            let sampler_view = self
                .texture_cache
                .get(&layer.surface_id)
                .map(|cached| cached.sampler_view)
                .ok_or(CompositionError::GpuFailure)?;
            let first_vertex = u32::try_from(self.vertex_bytes.len() / 32)
                .map_err(|_| CompositionError::SurfaceAllocation)?;
            append_layer_vertices(&mut self.vertex_bytes, layer, width, height, source);
            prepared_layers.push(PreparedLayer {
                sampler_view,
                scissor,
                first_vertex,
                effect: layer.effect,
            });
        }

        if !self.vertex_bytes.is_empty() && self.vertex_bytes.len() > self.vertex_capacity {
            let capacity = self
                .vertex_bytes
                .len()
                .max(MIN_VERTEX_BUFFER_BYTES)
                .checked_next_power_of_two()
                .ok_or(CompositionError::SurfaceAllocation)?;
            let width = u32::try_from(capacity).map_err(|_| CompositionError::SurfaceAllocation)?;
            let resource = gpu
                .create_virgl_resource(
                    context,
                    PIPE_BUFFER,
                    FORMAT_R8_UNORM,
                    PIPE_BIND_VERTEX_BUFFER,
                    width,
                    1,
                    capacity,
                )
                .map_err(|_| CompositionError::GpuFailure)?;
            if let Some(old) = self.vertex_resource.replace(resource) {
                self.retired_vertex_resources.push(old);
            }
            self.vertex_capacity = capacity;
            stats.vertex_resources_created = stats.vertex_resources_created.saturating_add(1);
        }

        let sampler_views_to_create: Vec<(SurfaceId, u32, u32)> = self
            .texture_cache
            .iter()
            .filter(|(_, cached)| !cached.sampler_view_live)
            .map(|(&surface_id, cached)| (surface_id, cached.sampler_view, cached.resource.id))
            .collect();
        let sampler_views_to_destroy: Vec<u32> = self
            .retired_textures
            .iter()
            .filter(|cached| cached.sampler_view_live)
            .map(|cached| cached.sampler_view)
            .collect();
        let initialize_pipeline = !self.pipeline_initialized;
        let total_backdrop_halo = backdrop_halo(&scene.layers);

        let composition_started = timestamp_cycles();
        let mut draw_calls = 0u64;
        let mut backdrop_copies = 0u64;
        let mut backdrop_copy_pixels = 0u64;
        let mut backdrop_blur_passes = 0u64;
        let mut backdrop_blur_pixels = 0u64;
        let mut backdrop_blur_cycles = 0u64;
        let encode_result = (|| {
            let mut encoder = VirglCommandEncoder::new();
            if initialize_pipeline {
                encode_pipeline_create(
                    &mut encoder,
                    output_resource.id,
                    blur_resource_a.id,
                    blur_resource_b.id,
                    width,
                    height,
                )?;
            }
            encoder.set_framebuffer(OUTPUT_SURFACE)?;

            if !sampler_views_to_destroy.is_empty() {
                encoder.clear_fragment_sampler_view()?;
                for &view in &sampler_views_to_destroy {
                    encoder.destroy_object(OBJECT_SAMPLER_VIEW, view)?;
                }
            }
            for &(_, view, texture_id) in &sampler_views_to_create {
                encoder.create_sampler_view(view, texture_id, FORMAT_B8G8R8A8_UNORM)?;
            }

            if let Some(vertices) = self.vertex_resource.as_ref() {
                if !self.vertex_bytes.is_empty() {
                    encoder.inline_write_buffer(vertices.id, &self.vertex_bytes)?;
                    encoder.set_vertex_buffer(vertices.id, 32)?;
                }
            }

            for damage_rect in &damage_rects {
                encoder.bind_shader(CLEAR_FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER)?;
                encoder.bind_object(OBJECT_BLEND, REPLACE_BLEND_STATE)?;
                encoder.set_scissor(
                    damage_rect.x as u16,
                    damage_rect.y as u16,
                    damage_rect.right() as u16,
                    damage_rect.bottom() as u16,
                )?;
                encoder.draw_triangles_from(
                    clear_first_vertex
                        .ok_or(crate::drivers::virtio::gpu::GpuError::InvalidCommandStream)?,
                    6,
                )?;
                encoder.bind_shader(FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER)?;
                encoder.bind_object(OBJECT_BLEND, BLEND_STATE)?;
                for layer in &prepared_layers {
                    let Some(draw_scissor) = layer.scissor.intersection(damage_rect) else {
                        continue;
                    };
                    match layer.effect {
                        LayerEffect::BackdropSample { radius } => {
                            let blur_started = timestamp_cycles();
                            let work_rect = inflate_rect(*damage_rect, total_backdrop_halo)
                                .intersection(&bounds)
                                .ok_or(
                                    crate::drivers::virtio::gpu::GpuError::InvalidCommandStream,
                                )?;
                            encoder.resource_copy_region(
                                blur_resource_a.id,
                                work_rect.x as u32,
                                work_rect.y as u32,
                                0,
                                output_resource.id,
                                GpuBox {
                                    x: work_rect.x as u32,
                                    y: work_rect.y as u32,
                                    z: 0,
                                    width: work_rect.width,
                                    height: work_rect.height,
                                    depth: 1,
                                },
                            )?;
                            backdrop_copies = backdrop_copies.saturating_add(1);
                            backdrop_copy_pixels =
                                backdrop_copy_pixels.saturating_add(work_rect.area());

                            for pass_radius in backdrop_box_radii(radius) {
                                if pass_radius == 0 {
                                    continue;
                                }
                                let (horizontal, vertical) = blur_shader_handles(pass_radius)
                                    .ok_or(
                                        crate::drivers::virtio::gpu::GpuError::InvalidCommandStream,
                                    )?;
                                encoder.set_framebuffer(BLUR_SURFACE_B)?;
                                encoder.bind_shader(horizontal, FRAGMENT_SHADER)?;
                                encoder.bind_object(OBJECT_BLEND, REPLACE_BLEND_STATE)?;
                                encoder.set_fragment_sampler_view(BLUR_SAMPLER_VIEW_A)?;
                                set_encoder_scissor(&mut encoder, work_rect)?;
                                encoder.draw_triangles_from(
                                    clear_first_vertex.ok_or(
                                        crate::drivers::virtio::gpu::GpuError::InvalidCommandStream,
                                    )?,
                                    6,
                                )?;

                                encoder.set_framebuffer(BLUR_SURFACE_A)?;
                                encoder.bind_shader(vertical, FRAGMENT_SHADER)?;
                                encoder.set_fragment_sampler_view(BLUR_SAMPLER_VIEW_B)?;
                                set_encoder_scissor(&mut encoder, work_rect)?;
                                encoder.draw_triangles_from(
                                    clear_first_vertex.ok_or(
                                        crate::drivers::virtio::gpu::GpuError::InvalidCommandStream,
                                    )?,
                                    6,
                                )?;
                                backdrop_blur_passes = backdrop_blur_passes.saturating_add(2);
                                backdrop_blur_pixels = backdrop_blur_pixels
                                    .saturating_add(work_rect.area().saturating_mul(2));
                            }

                            encoder.set_framebuffer(OUTPUT_SURFACE)?;
                            encoder.bind_shader(EFFECT_FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER)?;
                            encoder.bind_object(OBJECT_BLEND, REPLACE_BLEND_STATE)?;
                            encoder.set_fragment_sampler_views(
                                0,
                                &[layer.sampler_view, BLUR_SAMPLER_VIEW_A],
                            )?;
                            set_encoder_scissor(&mut encoder, draw_scissor)?;
                            encoder.draw_triangles_from(layer.first_vertex, 6)?;
                            backdrop_blur_cycles = backdrop_blur_cycles
                                .saturating_add(timestamp_cycles().saturating_sub(blur_started));
                        }
                        LayerEffect::None | LayerEffect::AlphaMask => {
                            encoder.set_framebuffer(OUTPUT_SURFACE)?;
                            encoder.bind_shader(FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER)?;
                            encoder.bind_object(OBJECT_BLEND, BLEND_STATE)?;
                            encoder.set_fragment_sampler_view(layer.sampler_view)?;
                            set_encoder_scissor(&mut encoder, draw_scissor)?;
                            encoder.draw_triangles_from(layer.first_vertex, 6)?;
                        }
                    }
                    draw_calls = draw_calls.saturating_add(1);
                }
            }
            Ok::<VirglCommandEncoder, crate::drivers::virtio::gpu::GpuError>(encoder)
        })();
        let total_composition_cycles = timestamp_cycles().saturating_sub(composition_started);
        stats.backdrop_blur_cycles = backdrop_blur_cycles;
        stats.composition_cycles = total_composition_cycles.saturating_sub(backdrop_blur_cycles);
        let encoder = match encode_result {
            Ok(encoder) => encoder,
            Err(_) => return Err(CompositionError::GpuFailure),
        };
        stats.command_stream_dwords = encoder.words().len() as u64;

        let fence_started = timestamp_cycles();
        let render_result = gpu.submit_virgl(context, encoder.words());
        stats.fence_wait_cycles = timestamp_cycles().saturating_sub(fence_started);
        if render_result.is_err() {
            return Err(CompositionError::GpuFailure);
        }
        stats.gpu_submissions = 1;

        if initialize_pipeline {
            self.pipeline_initialized = true;
            stats.pipeline_objects_created = PIPELINE_OBJECT_COUNT;
        }
        for &(surface_id, sampler_view, _) in &sampler_views_to_create {
            if let Some(cached) = self.texture_cache.get_mut(&surface_id) {
                if cached.sampler_view == sampler_view {
                    cached.sampler_view_live = true;
                }
            }
        }
        stats.sampler_views_created = sampler_views_to_create.len() as u64;
        stats.sampler_views_destroyed = sampler_views_to_destroy.len() as u64;
        for retired in &mut self.retired_textures {
            retired.sampler_view_live = false;
        }
        while let Some(mut retired) = self.retired_textures.pop() {
            if gpu
                .destroy_virgl_resource(context, &mut retired.resource)
                .is_err()
            {
                self.retired_textures.push(retired);
                return Err(CompositionError::GpuFailure);
            }
            stats.texture_resources_destroyed = stats.texture_resources_destroyed.saturating_add(1);
        }
        while let Some(mut retired) = self.retired_vertex_resources.pop() {
            if gpu.destroy_virgl_resource(context, &mut retired).is_err() {
                self.retired_vertex_resources.push(retired);
                return Err(CompositionError::GpuFailure);
            }
            stats.vertex_resources_destroyed = stats.vertex_resources_destroyed.saturating_add(1);
        }
        stats.vertex_buffer_capacity = self.vertex_capacity as u64;
        stats.layers_composed = draw_calls;
        stats.backdrop_copies = backdrop_copies;
        stats.backdrop_copy_pixels = backdrop_copy_pixels;
        stats.backdrop_blur_passes = backdrop_blur_passes;
        stats.backdrop_blur_pixels = backdrop_blur_pixels;
        stats.backdrop_scratch_bytes = self.output.byte_len().saturating_mul(2) as u64;
        stats.output_damage_regions = damage_rects.len() as u64;
        stats.output_pixels_damaged = damage_rects.iter().map(Rect::area).sum();
        Ok(stats)
    }

    fn allocate_sampler_view(&mut self) -> Result<u32, CompositionError> {
        let sampler_view = self.next_sampler_view;
        self.next_sampler_view = sampler_view
            .checked_add(1)
            .ok_or(CompositionError::SurfaceAllocation)?;
        Ok(sampler_view)
    }

    fn gpu_parts(
        &mut self,
    ) -> Result<(&mut VirtioGpu, &mut VirglContext, &mut VirglResource), CompositionError> {
        match (
            self.gpu.as_mut(),
            self.context.as_mut(),
            self.output_resource.as_mut(),
        ) {
            (Some(gpu), Some(context), Some(output)) => Ok((gpu, context, output)),
            _ => Err(CompositionError::GpuFailure),
        }
    }
}

impl CompositionEngine for VirglCompositionEngine {
    fn kind(&self) -> CompositionEngineKind {
        CompositionEngineKind::Virgl
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
        for layer in &scene.layers {
            if layer.visible && !layer.transform.is_translation() {
                return Err(CompositionError::UnsupportedTransform);
            }
            if layer.visible && layer.opacity != 0 && !surfaces.contains_key(&layer.surface_id) {
                return Err(CompositionError::MissingSurface(layer.surface_id));
            }
            if layer.visible
                && matches!(
                    layer.effect,
                    LayerEffect::BackdropSample { radius }
                        if radius > MAX_GPU_BACKDROP_RADIUS
                )
            {
                return Err(CompositionError::UnsupportedEffect);
            }
        }

        let Some(mut gpu) = self.gpu.take() else {
            return Err(CompositionError::GpuFailure);
        };
        let Some(mut context) = self.context.take() else {
            self.gpu = Some(gpu);
            return Err(CompositionError::GpuFailure);
        };
        let Some(mut output_resource) = self.output_resource.take() else {
            self.gpu = Some(gpu);
            self.context = Some(context);
            return Err(CompositionError::GpuFailure);
        };
        let Some(mut blur_resource_a) = self.blur_resource_a.take() else {
            self.gpu = Some(gpu);
            self.context = Some(context);
            self.output_resource = Some(output_resource);
            return Err(CompositionError::GpuFailure);
        };
        let Some(mut blur_resource_b) = self.blur_resource_b.take() else {
            self.gpu = Some(gpu);
            self.context = Some(context);
            self.output_resource = Some(output_resource);
            self.blur_resource_a = Some(blur_resource_a);
            return Err(CompositionError::GpuFailure);
        };
        let result = self.compose_frame(
            &mut gpu,
            &mut context,
            &mut output_resource,
            &mut blur_resource_a,
            &mut blur_resource_b,
            scene,
            surfaces,
            damage,
        );
        self.gpu = Some(gpu);
        self.context = Some(context);
        self.output_resource = Some(output_resource);
        self.blur_resource_a = Some(blur_resource_a);
        self.blur_resource_b = Some(blur_resource_b);
        result
    }

    fn output(&self) -> &Surface {
        &self.output
    }

    fn output_mut(&mut self) -> &mut Surface {
        &mut self.output
    }

    fn uses_direct_scanout(&self) -> bool {
        true
    }

    fn present_direct(&mut self, damage: &[Rect]) -> Result<u64, CompositionError> {
        let scanout_id = self.scanout_id;
        let was_active = self.scanout_active;
        let width = self.output.width();
        let height = self.output.height();
        let bounds = Rect::new(0, 0, width, height);
        let flushes = {
            let (gpu, _, output) = self.gpu_parts()?;
            if !was_active {
                gpu.set_virgl_scanout(scanout_id, output)
                    .map_err(|_| CompositionError::GpuFailure)?;
                gpu.flush_virgl_scanout(
                    output,
                    GpuRect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                )
                .map_err(|_| CompositionError::GpuFailure)?;
                1
            } else {
                let mut flushes = 0u64;
                for requested in damage {
                    let Some(rect) = requested.intersection(&bounds) else {
                        continue;
                    };
                    gpu.flush_virgl_scanout(
                        output,
                        GpuRect {
                            x: rect.x as u32,
                            y: rect.y as u32,
                            width: rect.width,
                            height: rect.height,
                        },
                    )
                    .map_err(|_| CompositionError::GpuFailure)?;
                    flushes = flushes.saturating_add(1);
                }
                flushes
            }
        };
        self.scanout_active = true;
        Ok(flushes)
    }

    fn hardware_cursor_needs_image(&self) -> bool {
        self.cursor.is_none()
    }

    fn update_hardware_cursor(
        &mut self,
        x: u32,
        y: u32,
        pixels: Option<&[u32]>,
    ) -> Result<bool, CompositionError> {
        let scanout_id = self.scanout_id;
        let Some(mut gpu) = self.gpu.take() else {
            return Err(CompositionError::GpuFailure);
        };
        let result = if let Some(cursor) = self.cursor.as_ref() {
            gpu.move_cursor(cursor, x, y)
        } else if let Some(pixels) = pixels {
            match gpu.create_cursor(scanout_id, x, y, pixels) {
                Ok(cursor) => {
                    self.cursor = Some(cursor);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        } else {
            Err(crate::drivers::virtio::gpu::GpuError::InvalidResource)
        };
        self.gpu = Some(gpu);
        result
            .map(|_| true)
            .map_err(|_| CompositionError::GpuFailure)
    }

    #[cfg(feature = "test")]
    fn readback_output(&mut self) -> Result<u64, CompositionError> {
        let width = self.output.width();
        let height = self.output.height();
        let output_bytes = {
            let (gpu, _, output) = self.gpu_parts()?;
            gpu.transfer_virgl_resource(
                output,
                GpuBox {
                    x: 0,
                    y: 0,
                    z: 0,
                    width,
                    height,
                    depth: 1,
                },
                false,
            )
            .map_err(|_| CompositionError::GpuFailure)?;
            output.backing.clone()
        };
        for (pixel, bytes) in self
            .output
            .pixels_mut()
            .iter_mut()
            .zip(output_bytes.chunks_exact(4))
        {
            *pixel = PremulArgb(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
        }
        Ok(output_bytes.len() as u64)
    }
}

fn encode_pipeline_create(
    encoder: &mut VirglCommandEncoder,
    output_resource: u32,
    blur_resource_a: u32,
    blur_resource_b: u32,
    width: u32,
    height: u32,
) -> Result<(), crate::drivers::virtio::gpu::GpuError> {
    encoder.create_surface(OUTPUT_SURFACE, output_resource, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
    encoder.create_surface(BLUR_SURFACE_A, blur_resource_a, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
    encoder.create_surface(BLUR_SURFACE_B, blur_resource_b, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
    encoder.create_vertex_elements(
        VERTEX_ELEMENTS,
        &[
            (0, 0, FORMAT_R32G32B32A32_FLOAT),
            (16, 0, FORMAT_R32G32B32A32_FLOAT),
        ],
    )?;
    encoder.bind_object(OBJECT_VERTEX_ELEMENTS, VERTEX_ELEMENTS)?;
    encoder.create_shader(VERTEX_SHADER_HANDLE, VERTEX_SHADER, VS)?;
    encoder.bind_shader(VERTEX_SHADER_HANDLE, VERTEX_SHADER)?;
    encoder.create_shader(FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER, FS)?;
    encoder.bind_shader(FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER)?;
    encoder.link_shaders(VERTEX_SHADER_HANDLE, FRAGMENT_SHADER_HANDLE)?;
    encoder.create_shader(CLEAR_FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER, CLEAR_FS)?;
    encoder.link_shaders(VERTEX_SHADER_HANDLE, CLEAR_FRAGMENT_SHADER_HANDLE)?;
    encoder.create_shader(EFFECT_FRAGMENT_SHADER_HANDLE, FRAGMENT_SHADER, EFFECT_FS)?;
    encoder.link_shaders(VERTEX_SHADER_HANDLE, EFFECT_FRAGMENT_SHADER_HANDLE)?;
    for (handle, horizontal, radius) in [
        (BLUR_HORIZONTAL_RADIUS_1_SHADER, true, 1),
        (BLUR_VERTICAL_RADIUS_1_SHADER, false, 1),
        (BLUR_HORIZONTAL_RADIUS_2_SHADER, true, 2),
        (BLUR_VERTICAL_RADIUS_2_SHADER, false, 2),
    ] {
        let shader = build_blur_shader(horizontal, radius, width, height)?;
        encoder.create_shader(handle, FRAGMENT_SHADER, &shader)?;
        encoder.link_shaders(VERTEX_SHADER_HANDLE, handle)?;
    }
    encoder.create_nearest_sampler(SAMPLER_STATE)?;
    encoder.bind_fragment_sampler_states(0, &[SAMPLER_STATE, SAMPLER_STATE])?;
    encoder.create_sampler_view(BLUR_SAMPLER_VIEW_A, blur_resource_a, FORMAT_B8G8R8A8_UNORM)?;
    encoder.create_sampler_view(BLUR_SAMPLER_VIEW_B, blur_resource_b, FORMAT_B8G8R8A8_UNORM)?;
    encoder.create_source_over_blend(BLEND_STATE)?;
    encoder.bind_object(OBJECT_BLEND, BLEND_STATE)?;
    encoder.create_replace_blend(REPLACE_BLEND_STATE)?;
    encoder.create_disabled_dsa(DSA_STATE)?;
    encoder.bind_object(OBJECT_DSA, DSA_STATE)?;
    encoder.create_rasterizer(RASTERIZER_STATE, true)?;
    encoder.bind_object(OBJECT_RASTERIZER, RASTERIZER_STATE)?;
    encoder.set_viewport(width, height)
}

fn encode_pipeline_destroy(
    encoder: &mut VirglCommandEncoder,
) -> Result<(), crate::drivers::virtio::gpu::GpuError> {
    encoder.clear_fragment_sampler_views(0, 2)?;
    encoder.destroy_object(OBJECT_SAMPLER_VIEW, BLUR_SAMPLER_VIEW_B)?;
    encoder.destroy_object(OBJECT_SAMPLER_VIEW, BLUR_SAMPLER_VIEW_A)?;
    encoder.destroy_object(OBJECT_SAMPLER_STATE, SAMPLER_STATE)?;
    encoder.destroy_object(OBJECT_RASTERIZER, RASTERIZER_STATE)?;
    encoder.destroy_object(OBJECT_DSA, DSA_STATE)?;
    encoder.destroy_object(OBJECT_BLEND, REPLACE_BLEND_STATE)?;
    encoder.destroy_object(OBJECT_BLEND, BLEND_STATE)?;
    encoder.destroy_object(OBJECT_SHADER, BLUR_VERTICAL_RADIUS_2_SHADER)?;
    encoder.destroy_object(OBJECT_SHADER, BLUR_HORIZONTAL_RADIUS_2_SHADER)?;
    encoder.destroy_object(OBJECT_SHADER, BLUR_VERTICAL_RADIUS_1_SHADER)?;
    encoder.destroy_object(OBJECT_SHADER, BLUR_HORIZONTAL_RADIUS_1_SHADER)?;
    encoder.destroy_object(OBJECT_SHADER, EFFECT_FRAGMENT_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_SHADER, CLEAR_FRAGMENT_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_SHADER, FRAGMENT_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_SHADER, VERTEX_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_VERTEX_ELEMENTS, VERTEX_ELEMENTS)?;
    encoder.destroy_surface(BLUR_SURFACE_B)?;
    encoder.destroy_surface(BLUR_SURFACE_A)?;
    encoder.destroy_surface(OUTPUT_SURFACE)
}

fn blur_shader_handles(radius: u16) -> Option<(u32, u32)> {
    match radius {
        1 => Some((
            BLUR_HORIZONTAL_RADIUS_1_SHADER,
            BLUR_VERTICAL_RADIUS_1_SHADER,
        )),
        2 => Some((
            BLUR_HORIZONTAL_RADIUS_2_SHADER,
            BLUR_VERTICAL_RADIUS_2_SHADER,
        )),
        _ => None,
    }
}

fn set_encoder_scissor(
    encoder: &mut VirglCommandEncoder,
    rect: Rect,
) -> Result<(), crate::drivers::virtio::gpu::GpuError> {
    encoder.set_scissor(
        rect.x as u16,
        rect.y as u16,
        rect.right() as u16,
        rect.bottom() as u16,
    )
}

fn build_blur_shader(
    horizontal: bool,
    radius: u16,
    width: u32,
    height: u32,
) -> Result<String, crate::drivers::virtio::gpu::GpuError> {
    if radius == 0 || width == 0 || height == 0 {
        return Err(crate::drivers::virtio::gpu::GpuError::InvalidCommandStream);
    }
    let mut shader = String::new();
    writeln!(shader, "FRAG").map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    writeln!(shader, "DCL IN[0], GENERIC[1], LINEAR")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    writeln!(shader, "DCL OUT[0], COLOR")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    writeln!(shader, "DCL SAMP[0]")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    writeln!(shader, "DCL SVIEW[0], 2D, FLOAT")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    writeln!(shader, "DCL TEMP[0..2]")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;

    let taps = radius as i32 * 2 + 1;
    for offset in -(radius as i32)..=radius as i32 {
        let (x, y) = if horizontal {
            (offset as f32 / width as f32, 0.0)
        } else {
            (0.0, offset as f32 / height as f32)
        };
        writeln!(shader, "IMM FLT32 {{ {x:.9}, {y:.9}, 0.0, 0.0 }}")
            .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    }
    writeln!(
        shader,
        "IMM FLT32 {{ {:.9}, 0.0, 0.0, 0.0 }}",
        1.0f32 / taps as f32
    )
    .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;

    let mut instruction = 0u32;
    for tap in 0..taps as u32 {
        writeln!(shader, "  {instruction}: ADD TEMP[0], IN[0], IMM[{tap}]")
            .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
        instruction += 1;
        let destination = if tap == 0 { 1 } else { 2 };
        writeln!(
            shader,
            "  {instruction}: TEX TEMP[{destination}], TEMP[0], SAMP[0], 2D"
        )
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
        instruction += 1;
        if tap != 0 {
            writeln!(shader, "  {instruction}: ADD TEMP[1], TEMP[1], TEMP[2]")
                .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
            instruction += 1;
        }
    }
    writeln!(
        shader,
        "  {instruction}: MUL OUT[0], TEMP[1], IMM[{}].xxxx",
        taps
    )
    .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    instruction += 1;
    writeln!(shader, "  {instruction}: END")
        .map_err(|_| crate::drivers::virtio::gpu::GpuError::SizeOverflow)?;
    Ok(shader)
}

impl Drop for VirglCompositionEngine {
    fn drop(&mut self) {
        let (Some(mut gpu), Some(mut context)) = (self.gpu.take(), self.context.take()) else {
            return;
        };
        if let Some(mut cursor) = self.cursor.take() {
            let _ = gpu.destroy_cursor(&mut cursor);
        }
        if self.scanout_active {
            let _ = gpu.disable_scanout(self.scanout_id);
            self.scanout_active = false;
        }

        let live_sampler_views: Vec<u32> = self
            .texture_cache
            .values()
            .chain(self.retired_textures.iter())
            .filter(|cached| cached.sampler_view_live)
            .map(|cached| cached.sampler_view)
            .collect();
        if self.pipeline_initialized || !live_sampler_views.is_empty() {
            let mut encoder = VirglCommandEncoder::new();
            let encoded = (|| {
                if !live_sampler_views.is_empty() {
                    encoder.clear_fragment_sampler_view()?;
                    for view in live_sampler_views {
                        encoder.destroy_object(OBJECT_SAMPLER_VIEW, view)?;
                    }
                }
                if self.pipeline_initialized {
                    encode_pipeline_destroy(&mut encoder)?;
                }
                Ok::<(), crate::drivers::virtio::gpu::GpuError>(())
            })();
            if encoded.is_ok() {
                let _ = gpu.submit_virgl(&context, encoder.words());
            }
            self.pipeline_initialized = false;
        }
        while let Some(surface_id) = self.texture_cache.keys().next().copied() {
            if let Some(mut cached) = self.texture_cache.remove(&surface_id) {
                let _ = gpu.destroy_virgl_resource(&mut context, &mut cached.resource);
            }
        }
        while let Some(mut cached) = self.retired_textures.pop() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut cached.resource);
        }
        self.texture_cache_bytes = 0;
        if let Some(mut vertices) = self.vertex_resource.take() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut vertices);
        }
        while let Some(mut vertices) = self.retired_vertex_resources.pop() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut vertices);
        }
        self.vertex_capacity = 0;
        if let Some(mut blur) = self.blur_resource_b.take() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut blur);
        }
        if let Some(mut blur) = self.blur_resource_a.take() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut blur);
        }
        if let Some(mut output) = self.output_resource.take() {
            let _ = gpu.destroy_virgl_resource(&mut context, &mut output);
        }
        let _ = gpu.destroy_virgl_context(&mut context);
        gpu.reset();
    }
}

fn append_layer_vertices(
    bytes: &mut Vec<u8>,
    layer: &crate::graphics::scene::Layer,
    output_width: u32,
    output_height: u32,
    source: &Surface,
) {
    let bounds = layer.output_bounds();
    let left = screen_to_ndc_x(bounds.x, output_width);
    let right = screen_to_ndc_x(bounds.right(), output_width);
    let top = screen_to_ndc_y(bounds.y, output_height);
    let bottom = screen_to_ndc_y(bounds.bottom(), output_height);
    let u0 = layer.source_rect.x.max(0) as f32 / source.width() as f32;
    let v0 = layer.source_rect.y.max(0) as f32 / source.height() as f32;
    let u1 = layer.source_rect.right().max(0) as f32 / source.width() as f32;
    let v1 = layer.source_rect.bottom().max(0) as f32 / source.height() as f32;
    let opacity = layer.opacity as f32 / u8::MAX as f32;
    let vertices = [
        ([left, top, 0.0, 1.0], [u0, v0, opacity, 1.0]),
        ([right, top, 0.0, 1.0], [u1, v0, opacity, 1.0]),
        ([right, bottom, 0.0, 1.0], [u1, v1, opacity, 1.0]),
        ([left, top, 0.0, 1.0], [u0, v0, opacity, 1.0]),
        ([right, bottom, 0.0, 1.0], [u1, v1, opacity, 1.0]),
        ([left, bottom, 0.0, 1.0], [u0, v1, opacity, 1.0]),
    ];
    for (position, texcoord) in vertices {
        for value in position.into_iter().chain(texcoord) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn append_clear_quad_vertices(bytes: &mut Vec<u8>) {
    let vertices: [([f32; 4], [f32; 4]); 6] = [
        ([-1.0, 1.0, 0.0, 1.0], [0.0; 4]),
        ([1.0, 1.0, 0.0, 1.0], [0.0; 4]),
        ([1.0, -1.0, 0.0, 1.0], [0.0; 4]),
        ([-1.0, 1.0, 0.0, 1.0], [0.0; 4]),
        ([1.0, -1.0, 0.0, 1.0], [0.0; 4]),
        ([-1.0, -1.0, 0.0, 1.0], [0.0; 4]),
    ];
    for (position, texcoord) in vertices {
        for value in position.into_iter().chain(texcoord) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn stage_surface_rect(
    surface: &Surface,
    backing: &mut [u8],
    requested: Rect,
) -> Result<u64, CompositionError> {
    if backing.len() != surface.byte_len() {
        return Err(CompositionError::GpuFailure);
    }
    let bounds = Rect::new(0, 0, surface.width(), surface.height());
    let rect = requested
        .intersection(&bounds)
        .ok_or(CompositionError::GpuFailure)?;
    let x = rect.x as usize;
    let row_bytes = (rect.width as usize)
        .checked_mul(4)
        .ok_or(CompositionError::GpuFailure)?;
    for y in rect.y as u32..rect.bottom() as u32 {
        let source_row = surface.row(y).ok_or(CompositionError::GpuFailure)?;
        let source = source_row
            .get(x..x + rect.width as usize)
            .ok_or(CompositionError::GpuFailure)?;
        let destination_start = (y as usize)
            .checked_mul(surface.width() as usize)
            .and_then(|offset| offset.checked_add(x))
            .and_then(|offset| offset.checked_mul(4))
            .ok_or(CompositionError::GpuFailure)?;
        let destination = backing
            .get_mut(destination_start..destination_start + row_bytes)
            .ok_or(CompositionError::GpuFailure)?;
        // SAFETY: `PremulArgb` is `repr(transparent)` over `u32`; AgenticOS is
        // x86-64 little-endian, so its in-memory AARRGGBB word is the BGRA byte
        // order required by B8G8R8A8_UNORM. Both slices were bounds-checked and
        // cannot overlap because the source is the canonical surface while the
        // destination is the VirtIO resource backing.
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.as_ptr().cast::<u8>(),
                destination.as_mut_ptr(),
                row_bytes,
            );
        }
    }
    Ok(rect.area().saturating_mul(4))
}

#[cfg(feature = "test")]
pub(crate) fn stage_surface_rect_for_test(
    surface: &Surface,
    backing: &mut [u8],
    rect: Rect,
) -> Result<u64, CompositionError> {
    stage_surface_rect(surface, backing, rect)
}

fn screen_to_ndc_x(x: i32, width: u32) -> f32 {
    x as f32 * 2.0 / width as f32 - 1.0
}

fn screen_to_ndc_y(y: i32, height: u32) -> f32 {
    y as f32 * 2.0 / height as f32 - 1.0
}
