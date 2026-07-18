//! VirGL retained composition engine.
//!
//! Guest surfaces remain canonical premultiplied ARGB. Stable surface IDs map
//! to persistent BGRA host textures, and only acknowledged local damage is
//! staged between frames. Ordered quads render with source-over on the host
//! GPU. Production frames remain in that GPU resource and are presented
//! through VirtIO-GPU direct scanout; readback is explicit and is reserved for
//! tests and diagnostics.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::drivers::virtio::gpu::protocol::{
    GpuBox, GpuRect, FORMAT_B8G8R8A8_UNORM, VIRGL_BIND_RENDER_TARGET, VIRGL_BIND_SCANOUT,
};
use crate::drivers::virtio::gpu::virgl::commands::VirglCommandEncoder;
use crate::drivers::virtio::gpu::virgl::{VirglContext, VirglResource};
use crate::drivers::virtio::gpu::{CursorResource, VirtioGpu};
use crate::graphics::scene::SceneFrame;
#[cfg(feature = "test")]
use crate::graphics::surface::PremulArgb;
use crate::graphics::surface::{Surface, SurfaceDesc, SurfaceId};
use crate::window::Rect;

use super::{
    timestamp_cycles, CompositionEngine, CompositionEngineKind, CompositionError, RenderStats,
};

const PIPE_BUFFER: u32 = 0;
const PIPE_TEXTURE_2D: u32 = 2;
const PIPE_BIND_SAMPLER_VIEW: u32 = 1 << 3;
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
const FIRST_SAMPLER_VIEW: u32 = 100;
const PIPELINE_OBJECT_COUNT: u64 = 10;

const VERTEX_SHADER: u32 = 0;
const FRAGMENT_SHADER: u32 = 1;
const VS: &str = "VERT\n\
DCL IN[0]\n\
DCL IN[1]\n\
DCL OUT[0], POSITION\n\
DCL OUT[1], GENERIC[0]\n\
  0: MOV OUT[1], IN[1]\n\
  1: MOV OUT[0], IN[0]\n\
  2: END\n";
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

struct PreparedLayer {
    sampler_view: u32,
    scissor: Rect,
    first_vertex: u32,
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
        let scanout_id = match gpu.enabled_scanout() {
            Ok(scanout_id) => scanout_id,
            Err(_) => {
                let mut output_resource = output_resource;
                let _ = gpu.destroy_virgl_resource(&mut context, &mut output_resource);
                let _ = gpu.destroy_virgl_context(&mut context);
                return Err(CompositionError::GpuFailure);
            }
        };
        Ok(Self {
            gpu: Some(gpu),
            context: Some(context),
            output_resource: Some(output_resource),
            output,
            scanout_id,
            scanout_active: false,
            cursor: None,
            texture_cache: BTreeMap::new(),
            texture_cache_bytes: 0,
            texture_cache_peak_bytes: 0,
            retired_textures: Vec::new(),
            // VirGL handles share one context-wide namespace even when object
            // kinds differ. Keep dynamic views above the fixed 1..=8 range.
            next_sampler_view: FIRST_SAMPLER_VIEW,
            pipeline_initialized: false,
            vertex_resource: None,
            retired_vertex_resources: Vec::new(),
            vertex_capacity: 0,
            vertex_bytes: Vec::new(),
        })
    }

    fn compose_frame(
        &mut self,
        gpu: &mut VirtioGpu,
        context: &mut VirglContext,
        output_resource: &mut VirglResource,
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
                    PIPE_BIND_SAMPLER_VIEW,
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

        let composition_started = timestamp_cycles();
        let mut draw_calls = 0u64;
        let encode_result = (|| {
            let mut encoder = VirglCommandEncoder::new();
            if initialize_pipeline {
                encode_pipeline_create(&mut encoder, output_resource.id, width, height)?;
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
                    encoder.set_fragment_sampler_view(layer.sampler_view)?;
                    encoder.set_scissor(
                        draw_scissor.x as u16,
                        draw_scissor.y as u16,
                        draw_scissor.right() as u16,
                        draw_scissor.bottom() as u16,
                    )?;
                    encoder.draw_triangles_from(layer.first_vertex, 6)?;
                    draw_calls = draw_calls.saturating_add(1);
                }
            }
            Ok::<VirglCommandEncoder, crate::drivers::virtio::gpu::GpuError>(encoder)
        })();
        stats.composition_cycles = timestamp_cycles().saturating_sub(composition_started);
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
        let result = self.compose_frame(
            &mut gpu,
            &mut context,
            &mut output_resource,
            scene,
            surfaces,
            damage,
        );
        self.gpu = Some(gpu);
        self.context = Some(context);
        self.output_resource = Some(output_resource);
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
    width: u32,
    height: u32,
) -> Result<(), crate::drivers::virtio::gpu::GpuError> {
    encoder.create_surface(OUTPUT_SURFACE, output_resource, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
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
    encoder.create_nearest_sampler(SAMPLER_STATE)?;
    encoder.bind_fragment_sampler_state(SAMPLER_STATE)?;
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
    encoder.destroy_object(OBJECT_SAMPLER_STATE, SAMPLER_STATE)?;
    encoder.destroy_object(OBJECT_RASTERIZER, RASTERIZER_STATE)?;
    encoder.destroy_object(OBJECT_DSA, DSA_STATE)?;
    encoder.destroy_object(OBJECT_BLEND, REPLACE_BLEND_STATE)?;
    encoder.destroy_object(OBJECT_BLEND, BLEND_STATE)?;
    encoder.destroy_object(OBJECT_SHADER, CLEAR_FRAGMENT_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_SHADER, FRAGMENT_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_SHADER, VERTEX_SHADER_HANDLE)?;
    encoder.destroy_object(OBJECT_VERTEX_ELEMENTS, VERTEX_ELEMENTS)?;
    encoder.destroy_surface(OUTPUT_SURFACE)
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
