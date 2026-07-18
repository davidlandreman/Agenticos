//! Minimal classic VirGL command-stream encoder.
//!
//! Wire values and layouts are derived from virglrenderer's MIT-licensed
//! `src/virgl_protocol.h` and `tests/testvirgl_encode.c` at commit
//! `960bd6674a25a438da2aac8a0af8c6d6e2b3a77e`. Keeping this encoder explicit
//! and bounded makes the qualification stream deterministic and auditable.

use alloc::vec::Vec;

use super::super::GpuError;

const VIRGL_CCMD_CREATE_OBJECT: u32 = 1;
const VIRGL_CCMD_BIND_OBJECT: u32 = 2;
const VIRGL_CCMD_DESTROY_OBJECT: u32 = 3;
const VIRGL_CCMD_SET_VIEWPORT_STATE: u32 = 4;
const VIRGL_CCMD_SET_FRAMEBUFFER_STATE: u32 = 5;
const VIRGL_CCMD_SET_VERTEX_BUFFERS: u32 = 6;
const VIRGL_CCMD_CLEAR: u32 = 7;
const VIRGL_CCMD_DRAW_VBO: u32 = 8;
const VIRGL_CCMD_RESOURCE_INLINE_WRITE: u32 = 9;
const VIRGL_CCMD_SET_SAMPLER_VIEWS: u32 = 10;
const VIRGL_CCMD_SET_SCISSOR_STATE: u32 = 15;
const VIRGL_CCMD_BIND_SAMPLER_STATES: u32 = 18;
const VIRGL_CCMD_BIND_SHADER: u32 = 31;
const VIRGL_CCMD_LINK_SHADER: u32 = 52;
const VIRGL_OBJECT_BLEND: u32 = 1;
const VIRGL_OBJECT_RASTERIZER: u32 = 2;
const VIRGL_OBJECT_DSA: u32 = 3;
const VIRGL_OBJECT_SHADER: u32 = 4;
const VIRGL_OBJECT_VERTEX_ELEMENTS: u32 = 5;
const VIRGL_OBJECT_SAMPLER_VIEW: u32 = 6;
const VIRGL_OBJECT_SAMPLER_STATE: u32 = 7;
const VIRGL_OBJECT_SURFACE: u32 = 8;
const PIPE_CLEAR_COLOR0: u32 = 1 << 2;
const PIPE_CLEAR_DEPTH: u32 = 1 << 0;
const MAX_ENCODER_DWORDS: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearColor([f32; 4]);

impl ClearColor {
    pub const TRANSPARENT: Self = Self([0.0, 0.0, 0.0, 0.0]);
    pub const RED: Self = Self([1.0, 0.0, 0.0, 1.0]);
    pub const BLUE: Self = Self([0.0, 0.0, 1.0, 1.0]);
    pub const MAGENTA: Self = Self([1.0, 0.0, 1.0, 1.0]);

    pub const fn from_array(color: [f32; 4]) -> Self {
        Self(color)
    }
}

pub struct VirglCommandEncoder {
    words: Vec<u32>,
}

impl VirglCommandEncoder {
    pub fn new() -> Self {
        Self { words: Vec::new() }
    }

    pub fn words(&self) -> &[u32] {
        &self.words
    }

    pub fn create_surface(
        &mut self,
        surface_id: u32,
        resource_id: u32,
        format: u32,
        level: u32,
        first_layer: u16,
    ) -> Result<(), GpuError> {
        if surface_id == 0 || resource_id == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SURFACE, 5)?;
        self.emit_words(&[
            surface_id,
            resource_id,
            format,
            level,
            first_layer as u32 | ((first_layer as u32) << 16),
        ])
    }

    pub fn set_framebuffer(&mut self, color_surface_id: u32) -> Result<(), GpuError> {
        if color_surface_id == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3)?;
        self.emit_words(&[1, 0, color_surface_id])
    }

    pub fn set_framebuffer_with_depth(
        &mut self,
        color_surface_id: u32,
        depth_surface_id: u32,
    ) -> Result<(), GpuError> {
        if color_surface_id == 0 || depth_surface_id == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_SET_FRAMEBUFFER_STATE, 0, 3)?;
        self.emit_words(&[1, depth_surface_id, color_surface_id])
    }

    pub fn clear_color(&mut self, color: ClearColor) -> Result<(), GpuError> {
        self.clear(color, None)
    }

    pub fn clear_color_depth(&mut self, color: [f32; 4], depth: f64) -> Result<(), GpuError> {
        self.clear(ClearColor(color), Some(depth))
    }

    fn clear(&mut self, color: ClearColor, depth: Option<f64>) -> Result<(), GpuError> {
        self.emit_command(VIRGL_CCMD_CLEAR, 0, 8)?;
        self.emit_word(PIPE_CLEAR_COLOR0 | if depth.is_some() { PIPE_CLEAR_DEPTH } else { 0 })?;
        for component in color.0 {
            self.emit_word(component.to_bits())?;
        }
        let depth = depth.unwrap_or(1.0).to_bits();
        self.emit_word(depth as u32)?;
        self.emit_word((depth >> 32) as u32)?;
        self.emit_word(0)
    }

    pub fn destroy_surface(&mut self, surface_id: u32) -> Result<(), GpuError> {
        if surface_id == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_DESTROY_OBJECT, VIRGL_OBJECT_SURFACE, 1)?;
        self.emit_word(surface_id)
    }

    pub fn create_vertex_elements(
        &mut self,
        handle: u32,
        elements: &[(u32, u32, u32)],
    ) -> Result<(), GpuError> {
        if handle == 0 || elements.is_empty() || elements.len() > 16 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(
            VIRGL_CCMD_CREATE_OBJECT,
            VIRGL_OBJECT_VERTEX_ELEMENTS,
            1 + elements.len() as u32 * 4,
        )?;
        self.emit_word(handle)?;
        for &(source_offset, buffer_index, format) in elements {
            self.emit_words(&[source_offset, 0, buffer_index, format])?;
        }
        Ok(())
    }

    pub fn bind_object(&mut self, object: u32, handle: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_BIND_OBJECT, object, 1)?;
        self.emit_word(handle)
    }

    pub fn set_vertex_buffer(&mut self, resource_id: u32, stride: u32) -> Result<(), GpuError> {
        if resource_id == 0 || stride == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_SET_VERTEX_BUFFERS, 0, 3)?;
        self.emit_words(&[stride, 0, resource_id])
    }

    pub fn inline_write_buffer(&mut self, resource_id: u32, bytes: &[u8]) -> Result<(), GpuError> {
        if resource_id == 0 || bytes.is_empty() || bytes.len() > u32::MAX as usize {
            return Err(GpuError::InvalidCommandStream);
        }
        let data_words = bytes.len().div_ceil(4);
        self.emit_command(VIRGL_CCMD_RESOURCE_INLINE_WRITE, 0, 11 + data_words as u32)?;
        self.emit_words(&[resource_id, 0, 0, 0, 0, 0, 0, 0, bytes.len() as u32, 1, 1])?;
        self.emit_padded_bytes(bytes)
    }

    pub fn create_shader(
        &mut self,
        handle: u32,
        shader_type: u32,
        text: &str,
    ) -> Result<(), GpuError> {
        if handle == 0 || text.is_empty() || text.as_bytes().contains(&0) {
            return Err(GpuError::InvalidCommandStream);
        }
        let byte_len = text.len().checked_add(1).ok_or(GpuError::SizeOverflow)?;
        let data_words = byte_len.div_ceil(4);
        self.emit_command(
            VIRGL_CCMD_CREATE_OBJECT,
            VIRGL_OBJECT_SHADER,
            5 + data_words as u32,
        )?;
        self.emit_words(&[handle, shader_type, byte_len as u32, 300, 0])?;
        self.emit_padded_bytes_with_nul(text.as_bytes())
    }

    pub fn bind_shader(&mut self, handle: u32, shader_type: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_BIND_SHADER, 0, 2)?;
        self.emit_words(&[handle, shader_type])
    }

    pub fn link_shaders(&mut self, vertex: u32, fragment: u32) -> Result<(), GpuError> {
        if vertex == 0 || fragment == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_LINK_SHADER, 0, 6)?;
        self.emit_words(&[vertex, fragment, 0, 0, 0, 0])
    }

    pub fn create_source_over_blend(&mut self, handle: u32) -> Result<(), GpuError> {
        const ONE: u32 = 1;
        const INV_SRC_ALPHA: u32 = 0x13;
        const RGBA_MASK: u32 = 0xf;
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_BLEND, 11)?;
        self.emit_words(&[handle, 0, 0])?;
        let target = 1
            | (ONE << 4)
            | (INV_SRC_ALPHA << 9)
            | (ONE << 17)
            | (INV_SRC_ALPHA << 22)
            | (RGBA_MASK << 27);
        self.emit_word(target)?;
        self.emit_words(&[0; 7])
    }

    pub fn create_replace_blend(&mut self, handle: u32) -> Result<(), GpuError> {
        const RGBA_MASK: u32 = 0xf;
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_BLEND, 11)?;
        self.emit_words(&[handle, 0, 0, RGBA_MASK << 27])?;
        self.emit_words(&[0; 7])
    }

    pub fn create_disabled_dsa(&mut self, handle: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_DSA, 5)?;
        self.emit_words(&[handle, 0, 0, 0, 0])
    }

    pub fn create_depth_less_dsa(&mut self, handle: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        // depth_enable=1, depth_writemask=1, PIPE_FUNC_LESS=1.
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_DSA, 5)?;
        self.emit_words(&[handle, 1 | (1 << 1) | (1 << 2), 0, 0, 0])
    }

    pub fn create_rasterizer(&mut self, handle: u32, scissor: bool) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_RASTERIZER, 9)?;
        let state = (1 << 1) | ((scissor as u32) << 14) | (1 << 29) | (1 << 30);
        self.emit_words(&[handle, state, 0, 0, 0, 0, 0, 0, 0])
    }

    pub fn create_rasterizer_cull_back(
        &mut self,
        handle: u32,
        scissor: bool,
    ) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        // depth_clip=1, PIPE_FACE_BACK=2, scissor optional, front_ccw=1,
        // and the same half-pixel/bottom-edge rules as the compositor.
        let state =
            (1 << 1) | (2 << 8) | ((scissor as u32) << 14) | (1 << 15) | (1 << 29) | (1 << 30);
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_RASTERIZER, 9)?;
        self.emit_words(&[handle, state, 0, 0, 0, 0, 0, 0, 0])
    }

    pub fn set_viewport_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        let half_width = width as f32 / 2.0;
        let half_height = height as f32 / 2.0;
        self.emit_command(VIRGL_CCMD_SET_VIEWPORT_STATE, 0, 7)?;
        self.emit_words(&[
            0,
            half_width.to_bits(),
            half_height.to_bits(),
            0.5f32.to_bits(),
            (x as f32 + half_width).to_bits(),
            (y as f32 + half_height).to_bits(),
            0.5f32.to_bits(),
        ])
    }

    /// Set an OpenGL-style viewport on a top-left-origin render target.
    /// Clip-space +Y maps toward the top of the client surface, matching the
    /// fixed-function API while retained compositor viewports remain unflipped.
    pub fn set_gl_viewport_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), GpuError> {
        if width == 0 || height == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        let half_width = width as f32 / 2.0;
        let half_height = height as f32 / 2.0;
        self.emit_command(VIRGL_CCMD_SET_VIEWPORT_STATE, 0, 7)?;
        self.emit_words(&[
            0,
            half_width.to_bits(),
            (-half_height).to_bits(),
            0.5f32.to_bits(),
            (x as f32 + half_width).to_bits(),
            (y as f32 + half_height).to_bits(),
            0.5f32.to_bits(),
        ])
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        self.set_viewport_rect(0, 0, width, height)
    }

    pub fn draw_triangles(&mut self, vertex_count: u32) -> Result<(), GpuError> {
        self.draw_triangles_from(0, vertex_count)
    }

    pub fn draw_triangles_from(
        &mut self,
        first_vertex: u32,
        vertex_count: u32,
    ) -> Result<(), GpuError> {
        if vertex_count == 0 || vertex_count % 3 != 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_DRAW_VBO, 0, 12)?;
        self.emit_words(&[
            first_vertex,
            vertex_count,
            4,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            first_vertex.saturating_add(vertex_count - 1),
            0,
        ])
    }

    pub fn set_scissor(
        &mut self,
        min_x: u16,
        min_y: u16,
        max_x: u16,
        max_y: u16,
    ) -> Result<(), GpuError> {
        if min_x >= max_x || min_y >= max_y {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_SET_SCISSOR_STATE, 0, 3)?;
        self.emit_words(&[
            0,
            min_x as u32 | ((min_y as u32) << 16),
            max_x as u32 | ((max_y as u32) << 16),
        ])
    }

    pub fn create_nearest_sampler(&mut self, handle: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SAMPLER_STATE, 9)?;
        // Clamp-to-edge (2) for S/T/R; nearest image/mipmap filters are zero.
        let state = 2 | (2 << 3) | (2 << 6);
        self.emit_words(&[
            handle,
            state,
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0,
            0,
            0,
            0,
        ])
    }

    pub fn create_sampler_view(
        &mut self,
        handle: u32,
        resource_id: u32,
        format: u32,
    ) -> Result<(), GpuError> {
        if handle == 0 || resource_id == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_CREATE_OBJECT, VIRGL_OBJECT_SAMPLER_VIEW, 6)?;
        let rgba_swizzle = 1 << 3 | 2 << 6 | 3 << 9;
        self.emit_words(&[handle, resource_id, format, 0, 0, rgba_swizzle])
    }

    pub fn bind_fragment_sampler(
        &mut self,
        sampler_handle: u32,
        view_handle: u32,
    ) -> Result<(), GpuError> {
        if sampler_handle == 0 || view_handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.set_fragment_sampler_view(view_handle)?;
        self.bind_fragment_sampler_state(sampler_handle)
    }

    pub fn set_fragment_sampler_view(&mut self, view_handle: u32) -> Result<(), GpuError> {
        if view_handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        const FRAGMENT_SHADER: u32 = 1;
        self.emit_command(VIRGL_CCMD_SET_SAMPLER_VIEWS, 0, 3)?;
        self.emit_words(&[FRAGMENT_SHADER, 0, view_handle])
    }

    pub fn clear_fragment_sampler_view(&mut self) -> Result<(), GpuError> {
        const FRAGMENT_SHADER: u32 = 1;
        self.emit_command(VIRGL_CCMD_SET_SAMPLER_VIEWS, 0, 3)?;
        self.emit_words(&[FRAGMENT_SHADER, 0, 0])
    }

    pub fn bind_fragment_sampler_state(&mut self, sampler_handle: u32) -> Result<(), GpuError> {
        if sampler_handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        const FRAGMENT_SHADER: u32 = 1;
        self.emit_command(VIRGL_CCMD_BIND_SAMPLER_STATES, 0, 3)?;
        self.emit_words(&[FRAGMENT_SHADER, 0, sampler_handle])
    }

    pub fn destroy_object(&mut self, object: u32, handle: u32) -> Result<(), GpuError> {
        if handle == 0 {
            return Err(GpuError::InvalidCommandStream);
        }
        self.emit_command(VIRGL_CCMD_DESTROY_OBJECT, object, 1)?;
        self.emit_word(handle)
    }

    fn emit_command(&mut self, command: u32, object: u32, length: u32) -> Result<(), GpuError> {
        self.emit_word(command | (object << 8) | (length << 16))
    }

    fn emit_words(&mut self, words: &[u32]) -> Result<(), GpuError> {
        if self.words.len().saturating_add(words.len()) > MAX_ENCODER_DWORDS {
            return Err(GpuError::InvalidCommandStream);
        }
        self.words.extend_from_slice(words);
        Ok(())
    }

    fn emit_word(&mut self, word: u32) -> Result<(), GpuError> {
        self.emit_words(&[word])
    }

    fn emit_padded_bytes(&mut self, bytes: &[u8]) -> Result<(), GpuError> {
        for chunk in bytes.chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            self.emit_word(u32::from_le_bytes(word))?;
        }
        Ok(())
    }

    fn emit_padded_bytes_with_nul(&mut self, bytes: &[u8]) -> Result<(), GpuError> {
        let mut offset = 0;
        while offset <= bytes.len() {
            let mut word = [0u8; 4];
            let remaining = bytes.len().saturating_sub(offset);
            let count = remaining.min(4);
            if count != 0 {
                word[..count].copy_from_slice(&bytes[offset..offset + count]);
            }
            self.emit_word(u32::from_le_bytes(word))?;
            offset += 4;
        }
        Ok(())
    }
}
