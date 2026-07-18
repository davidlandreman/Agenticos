//! VirGL retained composition engine.
//!
//! Guest surfaces remain canonical premultiplied ARGB. Each frame stages
//! those surfaces as BGRA textures and renders ordered quads with source-over
//! on the host GPU. Production frames remain in that GPU resource and are
//! presented through VirtIO-GPU direct scanout; readback is explicit and is
//! reserved for tests and diagnostics.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::drivers::virtio::gpu::protocol::{
    GpuBox, GpuRect, FORMAT_B8G8R8A8_UNORM, VIRGL_BIND_RENDER_TARGET, VIRGL_BIND_SCANOUT,
};
use crate::drivers::virtio::gpu::virgl::commands::{ClearColor, VirglCommandEncoder};
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

const OBJECT_BLEND: u32 = 1;
const OBJECT_RASTERIZER: u32 = 2;
const OBJECT_DSA: u32 = 3;
const OBJECT_SHADER: u32 = 4;
const OBJECT_VERTEX_ELEMENTS: u32 = 5;
const OBJECT_SAMPLER_VIEW: u32 = 6;
const OBJECT_SAMPLER_STATE: u32 = 7;

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
  0: TEX OUT[0], IN[0], SAMP[0], 2D\n\
  1: END\n";

struct UploadedLayer {
    texture: VirglResource,
    scissor: Rect,
    first_vertex: u32,
}

pub struct VirglCompositionEngine {
    gpu: Option<VirtioGpu>,
    context: Option<VirglContext>,
    output_resource: Option<VirglResource>,
    output: Surface,
    scanout_id: u32,
    scanout_active: bool,
    cursor: Option<CursorResource>,
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
        })
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
        _damage: &[Rect],
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

        let width = self.output.width();
        let height = self.output.height();
        let bounds = Rect::new(0, 0, width, height);
        let upload_started = timestamp_cycles();
        let (gpu, context, output_resource) = self.gpu_parts()?;
        let mut uploaded = Vec::<UploadedLayer>::new();
        let mut vertex_bytes = Vec::new();
        let mut texture_bytes_uploaded = 0u64;

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

            let mut texture = match gpu.create_virgl_resource(
                context,
                PIPE_TEXTURE_2D,
                FORMAT_B8G8R8A8_UNORM,
                PIPE_BIND_SAMPLER_VIEW,
                source.width(),
                source.height(),
                source.byte_len(),
            ) {
                Ok(texture) => texture,
                Err(_) => {
                    for layer in &mut uploaded {
                        let _ = gpu.destroy_virgl_resource(context, &mut layer.texture);
                    }
                    return Err(CompositionError::GpuFailure);
                }
            };
            for (bytes, pixel) in texture
                .backing
                .chunks_exact_mut(4)
                .zip(source.pixels().iter().copied())
            {
                bytes.copy_from_slice(&pixel.with_opacity(layer.opacity).0.to_le_bytes());
            }
            if gpu
                .transfer_virgl_resource(
                    &mut texture,
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
                let _ = gpu.destroy_virgl_resource(context, &mut texture);
                for layer in &mut uploaded {
                    let _ = gpu.destroy_virgl_resource(context, &mut layer.texture);
                }
                return Err(CompositionError::GpuFailure);
            }
            texture_bytes_uploaded =
                texture_bytes_uploaded.saturating_add(texture.backing.len() as u64);

            let first_vertex = (vertex_bytes.len() / 32) as u32;
            append_layer_vertices(&mut vertex_bytes, layer, width, height, source);
            uploaded.push(UploadedLayer {
                texture,
                scissor,
                first_vertex,
            });
        }
        let texture_upload_cycles = timestamp_cycles().saturating_sub(upload_started);

        let mut vertex_resource = if vertex_bytes.is_empty() {
            None
        } else {
            match gpu.create_virgl_resource(
                context,
                PIPE_BUFFER,
                FORMAT_R8_UNORM,
                PIPE_BIND_VERTEX_BUFFER,
                vertex_bytes.len() as u32,
                1,
                vertex_bytes.len(),
            ) {
                Ok(resource) => Some(resource),
                Err(_) => {
                    for layer in &mut uploaded {
                        let _ = gpu.destroy_virgl_resource(context, &mut layer.texture);
                    }
                    return Err(CompositionError::GpuFailure);
                }
            }
        };

        let composition_started = timestamp_cycles();
        let render_result = (|| {
            let surface = 1;
            let vertex_elements = 2;
            let vertex_shader = 3;
            let fragment_shader = 4;
            let blend = 5;
            let dsa = 6;
            let rasterizer = 7;
            let sampler = 8;
            let mut encoder = VirglCommandEncoder::new();
            encoder.create_surface(surface, output_resource.id, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
            encoder.set_framebuffer(surface)?;
            encoder.clear_color(ClearColor::TRANSPARENT)?;

            if let Some(vertices) = vertex_resource.as_ref() {
                encoder.create_vertex_elements(
                    vertex_elements,
                    &[
                        (0, 0, FORMAT_R32G32B32A32_FLOAT),
                        (16, 0, FORMAT_R32G32B32A32_FLOAT),
                    ],
                )?;
                encoder.bind_object(OBJECT_VERTEX_ELEMENTS, vertex_elements)?;
                encoder.inline_write_buffer(vertices.id, &vertex_bytes)?;
                encoder.set_vertex_buffer(vertices.id, 32)?;
                encoder.create_shader(vertex_shader, VERTEX_SHADER, VS)?;
                encoder.bind_shader(vertex_shader, VERTEX_SHADER)?;
                encoder.create_shader(fragment_shader, FRAGMENT_SHADER, FS)?;
                encoder.bind_shader(fragment_shader, FRAGMENT_SHADER)?;
                encoder.link_shaders(vertex_shader, fragment_shader)?;
                encoder.create_nearest_sampler(sampler)?;
                encoder.create_source_over_blend(blend)?;
                encoder.bind_object(OBJECT_BLEND, blend)?;
                encoder.create_disabled_dsa(dsa)?;
                encoder.bind_object(OBJECT_DSA, dsa)?;
                encoder.create_rasterizer(rasterizer, true)?;
                encoder.bind_object(OBJECT_RASTERIZER, rasterizer)?;
                encoder.set_viewport(width, height)?;

                for (index, layer) in uploaded.iter().enumerate() {
                    let view = 100u32.saturating_add(index as u32);
                    encoder.create_sampler_view(view, layer.texture.id, FORMAT_B8G8R8A8_UNORM)?;
                    encoder.bind_fragment_sampler(sampler, view)?;
                    encoder.set_scissor(
                        layer.scissor.x as u16,
                        layer.scissor.y as u16,
                        layer.scissor.right() as u16,
                        layer.scissor.bottom() as u16,
                    )?;
                    encoder.draw_triangles_from(layer.first_vertex, 6)?;
                    encoder.destroy_object(OBJECT_SAMPLER_VIEW, view)?;
                }

                encoder.destroy_object(OBJECT_SAMPLER_STATE, sampler)?;
                encoder.destroy_object(OBJECT_RASTERIZER, rasterizer)?;
                encoder.destroy_object(OBJECT_DSA, dsa)?;
                encoder.destroy_object(OBJECT_BLEND, blend)?;
                encoder.destroy_object(OBJECT_SHADER, fragment_shader)?;
                encoder.destroy_object(OBJECT_SHADER, vertex_shader)?;
                encoder.destroy_object(OBJECT_VERTEX_ELEMENTS, vertex_elements)?;
            }
            encoder.destroy_surface(surface)?;
            gpu.submit_virgl(context, encoder.words())
        })();

        let _fence = match render_result {
            Ok(fence) => fence,
            Err(_) => {
                if let Some(vertices) = vertex_resource.as_mut() {
                    let _ = gpu.destroy_virgl_resource(context, vertices);
                }
                for layer in &mut uploaded {
                    let _ = gpu.destroy_virgl_resource(context, &mut layer.texture);
                }
                return Err(CompositionError::GpuFailure);
            }
        };
        let fence_wait_cycles = timestamp_cycles().saturating_sub(composition_started);

        if let Some(vertices) = vertex_resource.as_mut() {
            let _ = gpu.destroy_virgl_resource(context, vertices);
        }
        for layer in &mut uploaded {
            let _ = gpu.destroy_virgl_resource(context, &mut layer.texture);
        }
        Ok(RenderStats {
            layers_composed: uploaded.len() as u64,
            texture_bytes_uploaded,
            output_pixels_damaged: width as u64 * height as u64,
            texture_upload_cycles,
            composition_cycles: timestamp_cycles().saturating_sub(composition_started),
            fence_wait_cycles,
            ..RenderStats::default()
        })
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
    let vertices = [
        ([left, top, 0.0, 1.0], [u0, v0, 0.0, 1.0]),
        ([right, top, 0.0, 1.0], [u1, v0, 0.0, 1.0]),
        ([right, bottom, 0.0, 1.0], [u1, v1, 0.0, 1.0]),
        ([left, top, 0.0, 1.0], [u0, v0, 0.0, 1.0]),
        ([right, bottom, 0.0, 1.0], [u1, v1, 0.0, 1.0]),
        ([left, bottom, 0.0, 1.0], [u0, v1, 0.0, 1.0]),
    ];
    for (position, texcoord) in vertices {
        for value in position.into_iter().chain(texcoord) {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
}

fn screen_to_ndc_x(x: i32, width: u32) -> f32 {
    x as f32 * 2.0 / width as f32 - 1.0
}

fn screen_to_ndc_y(y: i32, height: u32) -> f32 {
    y as f32 * 2.0 / height as f32 - 1.0
}
