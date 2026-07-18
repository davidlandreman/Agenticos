//! Bounded fixed-function OpenGL-style frontend for AgenticOS VirGL windows.

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

pub const COLOR_BUFFER_BIT: u32 = 1 << 0;
pub const DEPTH_BUFFER_BIT: u32 = 1 << 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Primitive {
    Triangles,
    Quads,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixMode {
    Projection,
    ModelView,
}

#[derive(Clone, Copy)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Self = Self([
        1.0, 0.0, 0.0, 0.0, // column 0
        0.0, 1.0, 0.0, 0.0, // column 1
        0.0, 0.0, 1.0, 0.0, // column 2
        0.0, 0.0, 0.0, 1.0, // column 3
    ]);

    pub fn multiply(self, rhs: Self) -> Self {
        let mut out = [0.0; 16];
        for column in 0..4 {
            for row in 0..4 {
                let mut value = 0.0;
                for k in 0..4 {
                    value += self.0[k * 4 + row] * rhs.0[column * 4 + k];
                }
                out[column * 4 + row] = value;
            }
        }
        Self(out)
    }

    pub fn transform(self, value: [f32; 4]) -> [f32; 4] {
        let mut out = [0.0; 4];
        for row in 0..4 {
            out[row] = self.0[row] * value[0]
                + self.0[4 + row] * value[1]
                + self.0[8 + row] * value[2]
                + self.0[12 + row] * value[3];
        }
        out
    }

    pub fn translation(x: f32, y: f32, z: f32) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.0[12] = x;
        matrix.0[13] = y;
        matrix.0[14] = z;
        matrix
    }

    pub fn scale(x: f32, y: f32, z: f32) -> Self {
        Self([
            x, 0.0, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 0.0, z, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn rotation_x(degrees: f32) -> Self {
        let radians = degrees * core::f32::consts::PI / 180.0;
        let (sin, cos) = (libm::sinf(radians), libm::cosf(radians));
        Self([
            1.0, 0.0, 0.0, 0.0, 0.0, cos, sin, 0.0, 0.0, -sin, cos, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn rotation_y(degrees: f32) -> Self {
        let radians = degrees * core::f32::consts::PI / 180.0;
        let (sin, cos) = (libm::sinf(radians), libm::cosf(radians));
        Self([
            cos, 0.0, -sin, 0.0, 0.0, 1.0, 0.0, 0.0, sin, 0.0, cos, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn rotation_z(degrees: f32) -> Self {
        let radians = degrees * core::f32::consts::PI / 180.0;
        let (sin, cos) = (libm::sinf(radians), libm::cosf(radians));
        Self([
            cos, sin, 0.0, 0.0, -sin, cos, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn perspective(fov_y_degrees: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / libm::tanf(fov_y_degrees * core::f32::consts::PI / 360.0);
        let nf = 1.0 / (near - far);
        Self([
            f / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            (far + near) * nf,
            -1.0,
            0.0,
            0.0,
            2.0 * far * near * nf,
            0.0,
        ])
    }

    pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Self {
        Self([
            2.0 / (right - left),
            0.0,
            0.0,
            0.0,
            0.0,
            2.0 / (top - bottom),
            0.0,
            0.0,
            0.0,
            0.0,
            -2.0 / (far - near),
            0.0,
            -(right + left) / (right - left),
            -(top + bottom) / (top - bottom),
            -(far + near) / (far - near),
            1.0,
        ])
    }
}

struct BeginState {
    primitive: Primitive,
    vertices: Vec<runtime::GlVertex>,
}

pub struct Context {
    window: u32,
    handle: u32,
    info: runtime::GlInfo,
    clear_color: [f32; 4],
    viewport: (u32, u32, u32, u32),
    projection: Mat4,
    model_view: Mat4,
    projection_stack: Vec<Mat4>,
    model_view_stack: Vec<Mat4>,
    matrix_mode: MatrixMode,
    current_color: [f32; 4],
    depth_test: bool,
    cull_back: bool,
    begin: Option<BeginState>,
    draws: Vec<runtime::GlDraw>,
    vertices: Vec<runtime::GlVertex>,
    packet: Vec<u8>,
    error: u32,
}

impl Context {
    pub fn new(window: u32) -> Result<Self, i64> {
        let handle = runtime::gui_gl_context_create(window, 0);
        if handle < 0 {
            return Err(handle);
        }
        let mut info = runtime::GlInfo::default();
        let result = runtime::gui_gl_get_info(handle as u32, &mut info);
        if result < 0 {
            let _ = runtime::gui_gl_context_destroy(handle as u32);
            return Err(result);
        }
        Ok(Self {
            window,
            handle: handle as u32,
            info,
            clear_color: [0.02, 0.03, 0.08, 1.0],
            viewport: (0, 0, info.width, info.height),
            projection: Mat4::IDENTITY,
            model_view: Mat4::IDENTITY,
            projection_stack: Vec::new(),
            model_view_stack: Vec::new(),
            matrix_mode: MatrixMode::ModelView,
            current_color: [1.0; 4],
            depth_test: true,
            cull_back: false,
            begin: None,
            draws: Vec::new(),
            vertices: Vec::new(),
            packet: Vec::new(),
            error: 0,
        })
    }

    pub const fn dimensions(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }

    pub fn resize(&mut self) -> Result<(), i64> {
        let _ = runtime::gui_gl_context_destroy(self.handle);
        self.handle = 0;
        let replacement = Self::new(self.window)?;
        *self = replacement;
        Ok(())
    }

    pub fn begin_frame(&mut self) {
        self.draws.clear();
        self.vertices.clear();
        self.begin = None;
    }

    pub fn clear_color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        self.clear_color = [red, green, blue, alpha].map(|value| value.clamp(0.0, 1.0));
    }

    pub fn viewport(&mut self, x: u32, y: u32, width: u32, height: u32) {
        self.viewport = (x, y, width, height);
    }

    pub fn depth_test(&mut self, enabled: bool) {
        self.depth_test = enabled;
    }

    pub fn cull_back_faces(&mut self, enabled: bool) {
        self.cull_back = enabled;
    }

    pub fn matrix_mode(&mut self, mode: MatrixMode) {
        self.matrix_mode = mode;
    }

    pub fn load_identity(&mut self) {
        *self.current_matrix_mut() = Mat4::IDENTITY;
    }

    pub fn load_matrix(&mut self, matrix: Mat4) {
        *self.current_matrix_mut() = matrix;
    }

    pub fn multiply_matrix(&mut self, matrix: Mat4) {
        let current = self.current_matrix_mut();
        *current = current.multiply(matrix);
    }

    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        self.multiply_matrix(Mat4::translation(x, y, z));
    }

    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        self.multiply_matrix(Mat4::scale(x, y, z));
    }

    pub fn rotate_x(&mut self, degrees: f32) {
        self.multiply_matrix(Mat4::rotation_x(degrees));
    }

    pub fn rotate_y(&mut self, degrees: f32) {
        self.multiply_matrix(Mat4::rotation_y(degrees));
    }

    pub fn rotate_z(&mut self, degrees: f32) {
        self.multiply_matrix(Mat4::rotation_z(degrees));
    }

    pub fn perspective(&mut self, fov: f32, aspect: f32, near: f32, far: f32) {
        self.multiply_matrix(Mat4::perspective(fov, aspect, near, far));
    }

    pub fn orthographic(
        &mut self,
        left: f32,
        right: f32,
        bottom: f32,
        top: f32,
        near: f32,
        far: f32,
    ) {
        self.multiply_matrix(Mat4::orthographic(left, right, bottom, top, near, far));
    }

    pub fn push_matrix(&mut self) {
        match self.matrix_mode {
            MatrixMode::Projection => self.projection_stack.push(self.projection),
            MatrixMode::ModelView => self.model_view_stack.push(self.model_view),
        }
    }

    pub fn pop_matrix(&mut self) {
        let value = match self.matrix_mode {
            MatrixMode::Projection => self.projection_stack.pop(),
            MatrixMode::ModelView => self.model_view_stack.pop(),
        };
        if let Some(value) = value {
            *self.current_matrix_mut() = value;
        } else {
            self.error = 0x0502; // GL_INVALID_OPERATION
        }
    }

    pub fn begin(&mut self, primitive: Primitive) {
        if self.begin.is_some() {
            self.error = 0x0502;
            return;
        }
        self.begin = Some(BeginState {
            primitive,
            vertices: Vec::new(),
        });
    }

    pub fn color(&mut self, red: f32, green: f32, blue: f32, alpha: f32) {
        self.current_color = [red, green, blue, alpha].map(|value| value.clamp(0.0, 1.0));
    }

    pub fn vertex(&mut self, x: f32, y: f32, z: f32) {
        let transform = self.projection.multiply(self.model_view);
        let vertex = runtime::GlVertex {
            position: transform.transform([x, y, z, 1.0]),
            color: self.current_color,
        };
        if let Some(begin) = self.begin.as_mut() {
            begin.vertices.push(vertex);
        } else {
            self.error = 0x0502;
        }
    }

    pub fn end(&mut self) {
        let Some(begin) = self.begin.take() else {
            self.error = 0x0502;
            return;
        };
        let first = self.vertices.len();
        match begin.primitive {
            Primitive::Triangles if begin.vertices.len() % 3 == 0 => {
                self.vertices.extend_from_slice(&begin.vertices);
            }
            Primitive::Quads if begin.vertices.len() % 4 == 0 => {
                for quad in begin.vertices.chunks_exact(4) {
                    self.vertices
                        .extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
                }
            }
            _ => {
                self.error = 0x0502;
                return;
            }
        }
        let count = self.vertices.len() - first;
        if count != 0 {
            self.draws.push(runtime::GlDraw {
                first_vertex: first as u32,
                vertex_count: count as u32,
                flags: (if self.depth_test {
                    runtime::GL_DRAW_DEPTH_TEST
                } else {
                    0
                }) | if self.cull_back {
                    runtime::GL_DRAW_CULL_BACK
                } else {
                    0
                },
                reserved: 0,
            });
        }
    }

    pub fn swap_buffers(&mut self) -> Result<u64, i64> {
        if self.begin.is_some() || self.error != 0 {
            return Err(-22);
        }
        if self.info.supported_draw_flags & runtime::GL_DRAW_DEPTH_TEST == 0 {
            self.sort_depth_fallback();
            if self.error != 0 {
                return Err(-22);
            }
        }
        if self.vertices.len() > self.info.max_vertices as usize
            || self.draws.len() > self.info.max_draws as usize
        {
            return Err(-12);
        }
        self.build_packet();
        if self.packet.len() > self.info.max_packet_bytes as usize {
            return Err(-12);
        }
        let result = runtime::gui_gl_submit_frame(self.handle, &self.packet, 0);
        if result < 0 {
            Err(result)
        } else {
            self.draws.clear();
            self.vertices.clear();
            Ok(result as u64)
        }
    }

    pub fn get_error(&mut self) -> u32 {
        let error = self.error;
        self.error = 0;
        error
    }

    pub fn destroy(&mut self) {
        if self.handle != 0 {
            let _ = runtime::gui_gl_context_destroy(self.handle);
            self.handle = 0;
        }
    }

    fn current_matrix_mut(&mut self) -> &mut Mat4 {
        match self.matrix_mode {
            MatrixMode::Projection => &mut self.projection,
            MatrixMode::ModelView => &mut self.model_view,
        }
    }

    fn sort_depth_fallback(&mut self) {
        let source_draws = core::mem::take(&mut self.draws);
        let source_vertices = core::mem::take(&mut self.vertices);
        let mut pending = Vec::<(f32, u32, [runtime::GlVertex; 3])>::new();

        for draw in source_draws {
            let start = draw.first_vertex as usize;
            let end = start.saturating_add(draw.vertex_count as usize);
            let Some(vertices) = source_vertices.get(start..end) else {
                self.error = 0x0502;
                return;
            };
            if draw.flags & runtime::GL_DRAW_DEPTH_TEST != 0 {
                for triangle in vertices.chunks_exact(3) {
                    let values = [triangle[0], triangle[1], triangle[2]];
                    let depth = values
                        .iter()
                        .map(|vertex| {
                            let w = vertex.position[3];
                            if w.abs() > f32::EPSILON {
                                vertex.position[2] / w
                            } else {
                                vertex.position[2]
                            }
                        })
                        .sum::<f32>()
                        / 3.0;
                    pending.push((depth, draw.flags & !runtime::GL_DRAW_DEPTH_TEST, values));
                }
            } else {
                flush_sorted_triangles(&mut pending, &mut self.draws, &mut self.vertices);
                append_vertices(&mut self.draws, &mut self.vertices, draw.flags, vertices);
            }
        }
        flush_sorted_triangles(&mut pending, &mut self.draws, &mut self.vertices);
    }

    fn build_packet(&mut self) {
        const HEADER_BYTES: usize = core::mem::size_of::<runtime::GlFrameHeader>();
        self.packet.clear();
        let draw_bytes = self.draws.len() * core::mem::size_of::<runtime::GlDraw>();
        let vertex_bytes = self.vertices.len() * core::mem::size_of::<runtime::GlVertex>();
        let total = HEADER_BYTES + draw_bytes + vertex_bytes;
        let vertex_offset = HEADER_BYTES + draw_bytes;
        let (vx, vy, vw, vh) = self.viewport;
        for value in [
            runtime::GL_ABI_MAGIC,
            runtime::GL_ABI_VERSION,
            total as u32,
            0,
            self.info.width,
            self.info.height,
            vx,
            vy,
            vw,
            vh,
        ] {
            push_u32(&mut self.packet, value);
        }
        for value in self.clear_color {
            push_f32(&mut self.packet, value);
        }
        push_f32(&mut self.packet, 1.0);
        push_u32(&mut self.packet, self.draws.len() as u32);
        push_u32(&mut self.packet, self.vertices.len() as u32);
        push_u32(&mut self.packet, HEADER_BYTES as u32);
        push_u32(&mut self.packet, vertex_offset as u32);
        for _ in 0..3 {
            push_u32(&mut self.packet, 0);
        }
        for draw in &self.draws {
            push_u32(&mut self.packet, draw.first_vertex);
            push_u32(&mut self.packet, draw.vertex_count);
            push_u32(&mut self.packet, draw.flags);
            push_u32(&mut self.packet, 0);
        }
        for vertex in &self.vertices {
            for value in vertex.position.into_iter().chain(vertex.color) {
                push_f32(&mut self.packet, value);
            }
        }
    }
}

fn flush_sorted_triangles(
    pending: &mut Vec<(f32, u32, [runtime::GlVertex; 3])>,
    draws: &mut Vec<runtime::GlDraw>,
    vertices: &mut Vec<runtime::GlVertex>,
) {
    pending.sort_by(|left, right| right.0.total_cmp(&left.0));
    for (_, flags, triangle) in pending.drain(..) {
        append_vertices(draws, vertices, flags, &triangle);
    }
}

fn append_vertices(
    draws: &mut Vec<runtime::GlDraw>,
    vertices: &mut Vec<runtime::GlVertex>,
    flags: u32,
    incoming: &[runtime::GlVertex],
) {
    if incoming.is_empty() {
        return;
    }
    let first = vertices.len() as u32;
    vertices.extend_from_slice(incoming);
    if let Some(last) = draws.last_mut() {
        if last.flags == flags && last.first_vertex + last.vertex_count == first {
            last.vertex_count = last.vertex_count.saturating_add(incoming.len() as u32);
            return;
        }
    }
    draws.push(runtime::GlDraw {
        first_vertex: first,
        vertex_count: incoming.len() as u32,
        flags,
        reserved: 0,
    });
}

impl Drop for Context {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    push_u32(bytes, value.to_bits());
}
