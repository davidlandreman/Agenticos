//! Narrow VirtIO-GPU 3D transport for the compositor VirGL spike.
//!
//! The transport owns capability discovery, contexts, resources, transfers,
//! bounded fenced submission, and idempotent teardown. The adjacent command
//! module contains the deliberately small VirGL encoder used by deterministic
//! render/readback qualification. Renderer selection remains closed until the
//! complete alpha-composition qualification passes.

use alloc::vec::Vec;

use super::protocol::*;
use super::{GpuError, VirtioGpu};

pub mod commands;

const GPU_CONFIG_NUM_CAPSETS_OFFSET: u32 = 12;
const MAX_CAPSETS: u32 = 64;
const MAX_CAPSET_BYTES: u32 = 64 * 1024;
const MAX_COMMAND_STREAM_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapsetInfo {
    pub id: u32,
    pub max_version: u32,
    pub max_size: u32,
}

pub struct VirglCapabilities {
    pub info: CapsetInfo,
    pub data: Vec<u8>,
    pub advertised: Vec<CapsetInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirglContext {
    pub id: u32,
    live: bool,
    live_resources: u32,
}

pub struct VirglResource {
    pub id: u32,
    pub context_id: u32,
    pub width: u32,
    pub height: u32,
    pub bind: u32,
    pub backing: Vec<u8>,
    attached_to_context: bool,
    backing_attached: bool,
    live: bool,
}

pub struct VirglScanoutFixture {
    pub context: VirglContext,
    pub resource: VirglResource,
    pub scanout_id: u32,
}

impl VirtioGpu {
    pub fn discover_virgl_capabilities(&mut self) -> Result<VirglCapabilities, GpuError> {
        if !self.virgl_advertised() {
            return Err(GpuError::VirglUnavailable);
        }
        let count = self.stable_num_capsets()?;
        if count > MAX_CAPSETS {
            return Err(GpuError::TooManyCapsets(count));
        }
        let mut advertised = Vec::with_capacity(count as usize);
        for index in 0..count {
            let request = GetCapsetInfo {
                header: CtrlHeader::command(CMD_GET_CAPSET_INFO),
                capset_index: index,
                padding: 0,
            };
            let mut response = CapsetInfoResponse::default();
            self.control
                .submit(&request, &mut response, RESP_OK_CAPSET_INFO)?;
            if response.capset_max_size > MAX_CAPSET_BYTES {
                return Err(GpuError::CapsetTooLarge(response.capset_max_size));
            }
            advertised.push(CapsetInfo {
                id: response.capset_id,
                max_version: response.capset_max_version,
                max_size: response.capset_max_size,
            });
        }

        let info = select_pinned_capset(&advertised).ok_or(GpuError::UnsupportedCapset)?;
        let request = GetCapset {
            header: CtrlHeader::command(CMD_GET_CAPSET),
            capset_id: info.id,
            capset_version: info.max_version,
        };
        let response_len = core::mem::size_of::<CtrlHeader>()
            .checked_add(info.max_size as usize)
            .ok_or(GpuError::SizeOverflow)?;
        let mut response = alloc::vec![0u8; response_len];
        self.control
            .submit_bytes(bytes_of(&request), &mut response, RESP_OK_CAPSET)?;
        let data = response.split_off(core::mem::size_of::<CtrlHeader>());
        Ok(VirglCapabilities {
            info,
            data,
            advertised,
        })
    }

    fn stable_num_capsets(&self) -> Result<u32, GpuError> {
        for _ in 0..4 {
            let before = self.device.config_generation();
            let count = self
                .device
                .read_device_config::<u32>(GPU_CONFIG_NUM_CAPSETS_OFFSET);
            let after = self.device.config_generation();
            if before == after {
                return Ok(count);
            }
        }
        Err(GpuError::Device)
    }

    pub fn create_virgl_context(
        &mut self,
        capabilities: &VirglCapabilities,
    ) -> Result<VirglContext, GpuError> {
        let id = self.next_context_id;
        self.next_context_id = id.checked_add(1).ok_or(GpuError::SizeOverflow)?;
        let mut debug_name = [0u8; 64];
        let name = b"agenticos-compositor";
        debug_name[..name.len()].copy_from_slice(name);
        let request = ContextCreate {
            header: CtrlHeader::context_command(CMD_CTX_CREATE, id),
            name_length: name.len() as u32,
            context_init: if self.features & VIRTIO_GPU_F_CONTEXT_INIT != 0 {
                capabilities.info.id & 0xff
            } else {
                0
            },
            debug_name,
        };
        self.control.submit_nodata(&request)?;
        Ok(VirglContext {
            id,
            live: true,
            live_resources: 0,
        })
    }

    pub fn destroy_virgl_context(&mut self, context: &mut VirglContext) -> Result<(), GpuError> {
        if !context.live {
            return Ok(());
        }
        if context.live_resources != 0 {
            return Err(GpuError::InvalidResource);
        }
        self.control
            .submit_nodata(&CtrlHeader::context_command(CMD_CTX_DESTROY, context.id))?;
        context.live = false;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_virgl_resource(
        &mut self,
        context: &mut VirglContext,
        target: u32,
        format: u32,
        bind: u32,
        width: u32,
        height: u32,
        byte_len: usize,
    ) -> Result<VirglResource, GpuError> {
        if !context.live || width == 0 || height == 0 || byte_len == 0 {
            return Err(GpuError::InvalidResource);
        }
        let next_resource_count = context
            .live_resources
            .checked_add(1)
            .ok_or(GpuError::SizeOverflow)?;
        let required_bytes = if target == 0 {
            width as usize
        } else {
            (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(GpuError::SizeOverflow)?
        };
        if byte_len < required_bytes {
            return Err(GpuError::InvalidResource);
        }
        let id = self.next_resource_id;
        self.next_resource_id = id.checked_add(1).ok_or(GpuError::SizeOverflow)?;
        let create = ResourceCreate3d {
            header: CtrlHeader::command(CMD_RESOURCE_CREATE_3D),
            resource_id: id,
            target,
            format,
            bind,
            width,
            height,
            depth: 1,
            array_size: 1,
            last_level: 0,
            sample_count: 0,
            flags: 0,
            padding: 0,
        };
        self.control.submit_nodata(&create)?;
        let backing = alloc::vec![0u8; byte_len];
        if let Err(error) = self.attach_backing(id, &backing) {
            let _ = self.unref(id);
            return Err(error);
        }
        let attach = ContextResource {
            header: CtrlHeader::context_command(CMD_CTX_ATTACH_RESOURCE, context.id),
            resource_id: id,
            padding: 0,
        };
        if let Err(error) = self.control.submit_nodata(&attach) {
            let _ = self.detach_backing(id);
            let _ = self.unref(id);
            return Err(error);
        }
        context.live_resources = next_resource_count;
        Ok(VirglResource {
            id,
            context_id: context.id,
            width,
            height,
            bind,
            backing,
            attached_to_context: true,
            backing_attached: true,
            live: true,
        })
    }

    pub fn transfer_virgl_resource(
        &mut self,
        resource: &mut VirglResource,
        region: GpuBox,
        to_host: bool,
    ) -> Result<(), GpuError> {
        if !resource.live {
            return Err(GpuError::InvalidResource);
        }
        let (offset, stride, layer_stride) = transfer_layout(
            resource.width,
            resource.height,
            resource.backing.len(),
            region,
        )?;
        let request = TransferHost3d {
            header: CtrlHeader::context_command(
                if to_host {
                    CMD_TRANSFER_TO_HOST_3D
                } else {
                    CMD_TRANSFER_FROM_HOST_3D
                },
                resource.context_id,
            ),
            region,
            offset,
            resource_id: resource.id,
            level: 0,
            stride,
            layer_stride,
        };
        self.control.submit_nodata(&request)
    }

    pub fn submit_virgl(
        &mut self,
        context: &VirglContext,
        command_stream: &[u32],
    ) -> Result<u64, GpuError> {
        let byte_len = command_stream
            .len()
            .checked_mul(core::mem::size_of::<u32>())
            .ok_or(GpuError::SizeOverflow)?;
        if !context.live || byte_len == 0 || byte_len > MAX_COMMAND_STREAM_BYTES {
            return Err(GpuError::InvalidCommandStream);
        }
        let fence_id = self.next_fence_id;
        self.next_fence_id = fence_id.checked_add(1).ok_or(GpuError::SizeOverflow)?;
        let submit = Submit3d {
            header: CtrlHeader::fenced(CMD_SUBMIT_3D, context.id, fence_id),
            size: byte_len as u32,
            padding: 0,
        };
        let mut request = Vec::with_capacity(core::mem::size_of::<Submit3d>() + byte_len);
        request.extend_from_slice(bytes_of(&submit));
        for word in command_stream {
            request.extend_from_slice(&word.to_le_bytes());
        }
        let mut response = CtrlHeader::default();
        self.control.submit_fenced_bytes(
            &request,
            bytes_of_mut(&mut response),
            RESP_OK_NODATA,
            fence_id,
        )?;
        Ok(fence_id)
    }

    pub fn enabled_scanout(&mut self) -> Result<u32, GpuError> {
        self.display_info()?
            .scanouts
            .iter()
            .position(|scanout| scanout.enabled != 0)
            .map(|index| index as u32)
            .ok_or(GpuError::NoScanout)
    }

    pub fn set_virgl_scanout(
        &mut self,
        scanout_id: u32,
        resource: &VirglResource,
    ) -> Result<(), GpuError> {
        if !resource.live || resource.bind & VIRGL_BIND_SCANOUT == 0 {
            return Err(GpuError::InvalidResource);
        }
        self.control.submit_nodata(&SetScanout {
            header: CtrlHeader::command(CMD_SET_SCANOUT),
            rect: GpuRect {
                x: 0,
                y: 0,
                width: resource.width,
                height: resource.height,
            },
            scanout_id,
            resource_id: resource.id,
        })
    }

    pub fn flush_virgl_scanout(
        &mut self,
        resource: &VirglResource,
        rect: GpuRect,
    ) -> Result<(), GpuError> {
        let right = rect
            .x
            .checked_add(rect.width)
            .ok_or(GpuError::SizeOverflow)?;
        let bottom = rect
            .y
            .checked_add(rect.height)
            .ok_or(GpuError::SizeOverflow)?;
        if !resource.live
            || resource.bind & VIRGL_BIND_SCANOUT == 0
            || rect.width == 0
            || rect.height == 0
            || right > resource.width
            || bottom > resource.height
        {
            return Err(GpuError::InvalidRect);
        }
        self.control.submit_nodata(&ResourceFlush {
            header: CtrlHeader::command(CMD_RESOURCE_FLUSH),
            rect,
            resource_id: resource.id,
            padding: 0,
        })
    }

    pub fn disable_scanout(&mut self, scanout_id: u32) -> Result<(), GpuError> {
        self.control.submit_nodata(&SetScanout {
            header: CtrlHeader::command(CMD_SET_SCANOUT),
            rect: GpuRect::default(),
            scanout_id,
            resource_id: 0,
        })
    }

    /// Render a host-visible magenta frame into a scanout-bound 3D resource.
    /// No transfer-from-host occurs: the dedicated host runner requires the
    /// Cocoa presenter to borrow and blit this texture before guest teardown.
    pub fn virgl_scanout_smoke(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<VirglScanoutFixture, GpuError> {
        use commands::{ClearColor, VirglCommandEncoder};

        if width == 0 || height == 0 {
            return Err(GpuError::InvalidRect);
        }
        let capabilities = self.discover_virgl_capabilities()?;
        let mut context = self.create_virgl_context(&capabilities)?;
        let mut resource = match self.create_virgl_resource(
            &mut context,
            2,
            FORMAT_B8G8R8A8_UNORM,
            VIRGL_BIND_RENDER_TARGET | VIRGL_BIND_SCANOUT,
            width,
            height,
            (width as usize)
                .checked_mul(height as usize)
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(GpuError::SizeOverflow)?,
        ) {
            Ok(resource) => resource,
            Err(error) => {
                let _ = self.destroy_virgl_context(&mut context);
                return Err(error);
            }
        };
        let result = (|| {
            let mut encoder = VirglCommandEncoder::new();
            encoder.create_surface(1, resource.id, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
            encoder.set_framebuffer(1)?;
            encoder.clear_color(ClearColor::MAGENTA)?;
            encoder.destroy_surface(1)?;
            self.submit_virgl(&context, encoder.words())?;
            let scanout_id = self.enabled_scanout()?;
            self.set_virgl_scanout(scanout_id, &resource)?;
            self.flush_virgl_scanout(
                &resource,
                GpuRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
            )?;
            Ok(scanout_id)
        })();
        match result {
            Ok(scanout_id) => Ok(VirglScanoutFixture {
                context,
                resource,
                scanout_id,
            }),
            Err(error) => {
                let _ = self.destroy_virgl_resource(&mut context, &mut resource);
                let _ = self.destroy_virgl_context(&mut context);
                Err(error)
            }
        }
    }

    pub fn destroy_virgl_scanout_fixture(
        &mut self,
        fixture: &mut VirglScanoutFixture,
    ) -> Result<(), GpuError> {
        self.disable_scanout(fixture.scanout_id)?;
        self.destroy_virgl_resource(&mut fixture.context, &mut fixture.resource)?;
        self.destroy_virgl_context(&mut fixture.context)
    }

    /// Render an exact red clear through VirGL and verify the guest backing
    /// after a transfer-from-host. This is the first runtime qualification
    /// gate; it proves command decoding and host rendering, not just feature
    /// advertisement.
    pub fn virgl_clear_readback_smoke(&mut self) -> Result<u64, GpuError> {
        use commands::{ClearColor, VirglCommandEncoder};

        const WIDTH: u32 = 4;
        const HEIGHT: u32 = 4;
        const PIPE_TEXTURE_2D: u32 = 2;
        const PIPE_BIND_RENDER_TARGET: u32 = 1 << 1;

        let capabilities = self.discover_virgl_capabilities()?;
        let mut context = self.create_virgl_context(&capabilities)?;
        let mut resource = match self.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            PIPE_BIND_RENDER_TARGET,
            WIDTH,
            HEIGHT,
            (WIDTH * HEIGHT * 4) as usize,
        ) {
            Ok(resource) => resource,
            Err(error) => {
                let _ = self.destroy_virgl_context(&mut context);
                return Err(error);
            }
        };

        let result = (|| {
            let surface_id = 1;
            let mut encoder = VirglCommandEncoder::new();
            encoder.create_surface(surface_id, resource.id, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
            encoder.set_framebuffer(surface_id)?;
            encoder.clear_color(ClearColor::RED)?;
            encoder.destroy_surface(surface_id)?;
            let fence = self.submit_virgl(&context, encoder.words())?;
            self.transfer_virgl_resource(
                &mut resource,
                GpuBox {
                    x: 0,
                    y: 0,
                    z: 0,
                    width: WIDTH,
                    height: HEIGHT,
                    depth: 1,
                },
                false,
            )?;
            if !resource
                .backing
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 255, 255])
            {
                return Err(GpuError::ReadbackMismatch);
            }
            Ok(fence)
        })();

        let resource_cleanup = self.destroy_virgl_resource(&mut context, &mut resource);
        let context_cleanup = self.destroy_virgl_context(&mut context);
        match result {
            Err(error) => Err(error),
            Ok(_) if resource_cleanup.is_err() => resource_cleanup.map(|_| 0),
            Ok(_) if context_cleanup.is_err() => context_cleanup.map(|_| 0),
            Ok(fence) => Ok(fence),
        }
    }

    /// Upload a 2x2 premultiplied texture, draw it through a scissored quad
    /// over opaque blue, and require the exact source-over result on readback.
    pub fn virgl_alpha_readback_smoke(&mut self) -> Result<u64, GpuError> {
        use commands::{ClearColor, VirglCommandEncoder};

        const WIDTH: u32 = 8;
        const HEIGHT: u32 = 8;
        const PIPE_BUFFER: u32 = 0;
        const PIPE_TEXTURE_2D: u32 = 2;
        const PIPE_BIND_RENDER_TARGET: u32 = 1 << 1;
        const PIPE_BIND_SAMPLER_VIEW: u32 = 1 << 3;
        const PIPE_BIND_VERTEX_BUFFER: u32 = 1 << 4;
        const FORMAT_R32G32B32A32_FLOAT: u32 = 31;
        const FORMAT_R8_UNORM: u32 = 64;
        const VERTEX_SHADER: u32 = 0;
        const FRAGMENT_SHADER: u32 = 1;
        const OBJECT_BLEND: u32 = 1;
        const OBJECT_RASTERIZER: u32 = 2;
        const OBJECT_DSA: u32 = 3;
        const OBJECT_SHADER: u32 = 4;
        const OBJECT_VERTEX_ELEMENTS: u32 = 5;
        const OBJECT_SAMPLER_VIEW: u32 = 6;
        const OBJECT_SAMPLER_STATE: u32 = 7;

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

        let capabilities = self.discover_virgl_capabilities()?;
        let mut context = self.create_virgl_context(&capabilities)?;
        let mut output = match self.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            PIPE_BIND_RENDER_TARGET,
            WIDTH,
            HEIGHT,
            (WIDTH * HEIGHT * 4) as usize,
        ) {
            Ok(resource) => resource,
            Err(error) => {
                let _ = self.destroy_virgl_context(&mut context);
                return Err(error);
            }
        };

        let positions = [
            [-1.0, -1.0, 0.0, 1.0],
            [1.0, -1.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [-1.0, -1.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [-1.0, 1.0, 0.0, 1.0],
        ];
        let texture_coordinates = [
            [0.0f32, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
        ];
        let mut vertex_bytes = Vec::with_capacity(6 * 8 * 4);
        for (position, texcoord) in positions.into_iter().zip(texture_coordinates) {
            for component in position.into_iter().chain(texcoord) {
                vertex_bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        let mut vertices = match self.create_virgl_resource(
            &mut context,
            PIPE_BUFFER,
            FORMAT_R8_UNORM,
            PIPE_BIND_VERTEX_BUFFER,
            vertex_bytes.len() as u32,
            1,
            vertex_bytes.len(),
        ) {
            Ok(resource) => resource,
            Err(error) => {
                let _ = self.destroy_virgl_resource(&mut context, &mut output);
                let _ = self.destroy_virgl_context(&mut context);
                return Err(error);
            }
        };

        let mut texture = match self.create_virgl_resource(
            &mut context,
            PIPE_TEXTURE_2D,
            FORMAT_B8G8R8A8_UNORM,
            PIPE_BIND_SAMPLER_VIEW,
            2,
            2,
            16,
        ) {
            Ok(mut resource) => {
                for pixel in resource.backing.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[0, 0, 128, 128]);
                }
                resource
            }
            Err(error) => {
                let _ = self.destroy_virgl_resource(&mut context, &mut vertices);
                let _ = self.destroy_virgl_resource(&mut context, &mut output);
                let _ = self.destroy_virgl_context(&mut context);
                return Err(error);
            }
        };

        let result = (|| {
            let surface = 1;
            let vertex_elements = 2;
            let vertex_shader = 3;
            let fragment_shader = 4;
            let blend = 5;
            let dsa = 6;
            let rasterizer = 7;
            let sampler = 8;
            let sampler_view = 9;
            self.transfer_virgl_resource(
                &mut texture,
                GpuBox {
                    x: 0,
                    y: 0,
                    z: 0,
                    width: 2,
                    height: 2,
                    depth: 1,
                },
                true,
            )?;
            let mut encoder = VirglCommandEncoder::new();
            encoder.create_surface(surface, output.id, FORMAT_B8G8R8A8_UNORM, 0, 0)?;
            encoder.set_framebuffer(surface)?;
            encoder.clear_color(ClearColor::BLUE)?;
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
            encoder.create_sampler_view(sampler_view, texture.id, FORMAT_B8G8R8A8_UNORM)?;
            encoder.bind_fragment_sampler(sampler, sampler_view)?;
            encoder.create_source_over_blend(blend)?;
            encoder.bind_object(OBJECT_BLEND, blend)?;
            encoder.create_disabled_dsa(dsa)?;
            encoder.bind_object(OBJECT_DSA, dsa)?;
            encoder.create_rasterizer(rasterizer, true)?;
            encoder.bind_object(OBJECT_RASTERIZER, rasterizer)?;
            encoder.set_viewport(WIDTH, HEIGHT)?;
            encoder.set_scissor(2, 2, 6, 6)?;
            encoder.draw_triangles(6)?;
            encoder.destroy_object(OBJECT_SAMPLER_VIEW, sampler_view)?;
            encoder.destroy_object(OBJECT_SAMPLER_STATE, sampler)?;
            encoder.destroy_object(OBJECT_RASTERIZER, rasterizer)?;
            encoder.destroy_object(OBJECT_DSA, dsa)?;
            encoder.destroy_object(OBJECT_BLEND, blend)?;
            encoder.destroy_object(OBJECT_SHADER, fragment_shader)?;
            encoder.destroy_object(OBJECT_SHADER, vertex_shader)?;
            encoder.destroy_object(OBJECT_VERTEX_ELEMENTS, vertex_elements)?;
            encoder.destroy_surface(surface)?;
            let fence = self.submit_virgl(&context, encoder.words())?;
            self.transfer_virgl_resource(
                &mut output,
                GpuBox {
                    x: 0,
                    y: 0,
                    z: 0,
                    width: WIDTH,
                    height: HEIGHT,
                    depth: 1,
                },
                false,
            )?;
            for y in 0..HEIGHT as usize {
                for x in 0..WIDTH as usize {
                    let offset = (y * WIDTH as usize + x) * 4;
                    let expected: &[u8] = if (2..6).contains(&x) && (2..6).contains(&y) {
                        &[127, 0, 128, 255]
                    } else {
                        &[255, 0, 0, 255]
                    };
                    if &output.backing[offset..offset + 4] != expected {
                        return Err(GpuError::ReadbackMismatch);
                    }
                }
            }
            Ok(fence)
        })();

        let texture_cleanup = self.destroy_virgl_resource(&mut context, &mut texture);
        let vertex_cleanup = self.destroy_virgl_resource(&mut context, &mut vertices);
        let output_cleanup = self.destroy_virgl_resource(&mut context, &mut output);
        let context_cleanup = self.destroy_virgl_context(&mut context);
        match result {
            Err(error) => Err(error),
            Ok(_) if texture_cleanup.is_err() => texture_cleanup.map(|_| 0),
            Ok(_) if vertex_cleanup.is_err() => vertex_cleanup.map(|_| 0),
            Ok(_) if output_cleanup.is_err() => output_cleanup.map(|_| 0),
            Ok(_) if context_cleanup.is_err() => context_cleanup.map(|_| 0),
            Ok(fence) => Ok(fence),
        }
    }

    /// Repeatedly create and tear down a context plus attached render target.
    /// This catches leaked context attachments and non-idempotent cleanup
    /// before the long-lived compositor takes ownership of the device.
    pub fn virgl_lifecycle_smoke(&mut self, repetitions: u32) -> Result<(), GpuError> {
        const PIPE_TEXTURE_2D: u32 = 2;
        const PIPE_BIND_RENDER_TARGET: u32 = 1 << 1;
        if repetitions == 0 || repetitions > 1_000 {
            return Err(GpuError::InvalidResource);
        }
        let capabilities = self.discover_virgl_capabilities()?;
        for _ in 0..repetitions {
            let mut context = self.create_virgl_context(&capabilities)?;
            let mut resource = match self.create_virgl_resource(
                &mut context,
                PIPE_TEXTURE_2D,
                FORMAT_B8G8R8A8_UNORM,
                PIPE_BIND_RENDER_TARGET,
                1,
                1,
                4,
            ) {
                Ok(resource) => resource,
                Err(error) => {
                    let _ = self.destroy_virgl_context(&mut context);
                    return Err(error);
                }
            };
            self.destroy_virgl_resource(&mut context, &mut resource)?;
            self.destroy_virgl_context(&mut context)?;
            // Teardown is explicitly idempotent.
            self.destroy_virgl_resource(&mut context, &mut resource)?;
            self.destroy_virgl_context(&mut context)?;
        }
        Ok(())
    }

    pub fn destroy_virgl_resource(
        &mut self,
        context: &mut VirglContext,
        resource: &mut VirglResource,
    ) -> Result<(), GpuError> {
        if !resource.live {
            return Ok(());
        }
        if !context.live || resource.context_id != context.id {
            return Err(GpuError::InvalidResource);
        }
        if resource.attached_to_context {
            self.control.submit_nodata(&ContextResource {
                header: CtrlHeader::context_command(CMD_CTX_DETACH_RESOURCE, resource.context_id),
                resource_id: resource.id,
                padding: 0,
            })?;
            resource.attached_to_context = false;
        }
        if resource.backing_attached {
            self.detach_backing(resource.id)?;
            resource.backing_attached = false;
        }
        self.unref(resource.id)?;
        resource.live = false;
        context.live_resources = context.live_resources.saturating_sub(1);
        Ok(())
    }

    pub(super) fn detach_backing(&mut self, resource_id: u32) -> Result<(), GpuError> {
        self.control.submit_nodata(&ResourceRef {
            header: CtrlHeader::command(CMD_RESOURCE_DETACH_BACKING),
            resource_id,
            padding: 0,
        })
    }
}

/// Validate a compositor BGRA transfer and return `(offset, stride,
/// layer_stride)` for the VirtIO request.
pub fn transfer_layout(
    resource_width: u32,
    resource_height: u32,
    backing_len: usize,
    region: GpuBox,
) -> Result<(u64, u32, u32), GpuError> {
    if region.width == 0
        || region.height == 0
        || region.depth != 1
        || region.z != 0
        || resource_width == 0
        || resource_height == 0
    {
        return Err(GpuError::InvalidResource);
    }
    let right = region
        .x
        .checked_add(region.width)
        .ok_or(GpuError::SizeOverflow)?;
    let bottom = region
        .y
        .checked_add(region.height)
        .ok_or(GpuError::SizeOverflow)?;
    if right > resource_width || bottom > resource_height {
        return Err(GpuError::InvalidResource);
    }
    let stride = resource_width
        .checked_mul(4)
        .ok_or(GpuError::SizeOverflow)?;
    let layer_stride = stride
        .checked_mul(resource_height)
        .ok_or(GpuError::SizeOverflow)?;
    let offset = (region.y as usize)
        .checked_mul(stride as usize)
        .and_then(|row| row.checked_add(region.x as usize * 4))
        .ok_or(GpuError::SizeOverflow)?;
    let transfer_end = (bottom.saturating_sub(1) as usize)
        .checked_mul(stride as usize)
        .and_then(|row| row.checked_add(right as usize * 4))
        .ok_or(GpuError::SizeOverflow)?;
    if transfer_end > backing_len {
        return Err(GpuError::InvalidResource);
    }
    Ok((offset as u64, stride, layer_stride))
}

/// Pin the transport to the two protocol revisions implemented by the classic
/// VirGL command stream. Prefer the fixed VirGL2 capset when both are present.
pub fn select_pinned_capset(capsets: &[CapsetInfo]) -> Option<CapsetInfo> {
    capsets
        .iter()
        .copied()
        .find(|info| info.id == CAPSET_VIRGL2 && info.max_version == 2)
        .or_else(|| {
            capsets
                .iter()
                .copied()
                .find(|info| info.id == CAPSET_VIRGL && info.max_version == 1)
        })
}
