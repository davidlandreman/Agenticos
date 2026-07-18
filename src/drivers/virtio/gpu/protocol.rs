//! VirtIO 1.3 section 5.7 wire layouts (little-endian x86 guest).
//!
//! Provenance: OASIS VirtIO 1.3, GPU Device command and response structures,
//! published under the OASIS specification licenses. Layout names and values
//! are cross-checked against QEMU's BSD-licensed `virtio_gpu.h` at upstream
//! commit `cf3e71d8fc8ba681266759bb6cb2e45a45983e3e` (the qualified host pin).
//! Local deviation: native integers are used because AgenticOS is x86-64
//! little-endian only; every wire structure has a compile-time size check.

pub const VIRTIO_GPU_F_VIRGL: u32 = 1 << 0;
pub const VIRTIO_GPU_F_EDID: u32 = 1 << 1;
pub const VIRTIO_GPU_F_CONTEXT_INIT: u32 = 1 << 4;
pub const CTRL_FLAG_FENCE: u32 = 1 << 0;

// Classic VirGL bind flags from virglrenderer src/virgl_hw.h at pinned
// commit 960bd6674a25a438da2aac8a0af8c6d6e2b3a77e.
pub const VIRGL_BIND_RENDER_TARGET: u32 = 1 << 1;
pub const VIRGL_BIND_DEPTH_STENCIL: u32 = 1 << 0;
pub const VIRGL_BIND_SAMPLER_VIEW: u32 = 1 << 3;
pub const VIRGL_BIND_VERTEX_BUFFER: u32 = 1 << 4;
pub const VIRGL_BIND_SCANOUT: u32 = 1 << 18;

pub const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const CMD_SET_SCANOUT: u32 = 0x0103;
pub const CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
pub const CMD_GET_CAPSET_INFO: u32 = 0x0108;
pub const CMD_GET_CAPSET: u32 = 0x0109;

pub const CMD_CTX_CREATE: u32 = 0x0200;
pub const CMD_CTX_DESTROY: u32 = 0x0201;
pub const CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
pub const CMD_CTX_DETACH_RESOURCE: u32 = 0x0203;
pub const CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
pub const CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
pub const CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
pub const CMD_SUBMIT_3D: u32 = 0x0207;
pub const CMD_UPDATE_CURSOR: u32 = 0x0300;
pub const CMD_MOVE_CURSOR: u32 = 0x0301;

pub const RESP_OK_NODATA: u32 = 0x1100;
pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
pub const RESP_OK_CAPSET_INFO: u32 = 0x1102;
pub const RESP_OK_CAPSET: u32 = 0x1103;

pub const CAPSET_VIRGL: u32 = 1;
pub const CAPSET_VIRGL2: u32 = 2;

pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const FORMAT_Z16_UNORM: u32 = 16;
pub const FORMAT_Z32_FLOAT: u32 = 18;
pub const FORMAT_Z24_UNORM_S8_UINT: u32 = 19;
pub const FORMAT_Z24X8_UNORM: u32 = 21;
pub const MAX_SCANOUTS: usize = 16;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CtrlHeader {
    pub command_type: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub context_id: u32,
    pub padding: u32,
}

impl CtrlHeader {
    pub const fn command(command_type: u32) -> Self {
        Self {
            command_type,
            flags: 0,
            fence_id: 0,
            context_id: 0,
            padding: 0,
        }
    }

    pub const fn context_command(command_type: u32, context_id: u32) -> Self {
        Self {
            context_id,
            ..Self::command(command_type)
        }
    }

    pub const fn fenced(command_type: u32, context_id: u32, fence_id: u64) -> Self {
        Self {
            command_type,
            flags: CTRL_FLAG_FENCE,
            fence_id,
            context_id,
            padding: 0,
        }
    }

    pub const fn matches_fence(self, fence_id: u64) -> bool {
        self.flags & CTRL_FLAG_FENCE != 0 && self.fence_id == fence_id
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayOne {
    pub rect: GpuRect,
    pub enabled: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DisplayInfoResponse {
    pub header: CtrlHeader,
    pub scanouts: [DisplayOne; MAX_SCANOUTS],
}

impl Default for DisplayInfoResponse {
    fn default() -> Self {
        Self {
            header: CtrlHeader::default(),
            scanouts: [DisplayOne::default(); MAX_SCANOUTS],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceCreate2d {
    pub header: CtrlHeader,
    pub resource_id: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceRef {
    pub header: CtrlHeader,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceAttachBacking {
    pub header: CtrlHeader,
    pub resource_id: u32,
    pub entry_count: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GetCapsetInfo {
    pub header: CtrlHeader,
    pub capset_index: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapsetInfoResponse {
    pub header: CtrlHeader,
    pub capset_id: u32,
    pub capset_max_version: u32,
    pub capset_max_size: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GetCapset {
    pub header: CtrlHeader,
    pub capset_id: u32,
    pub capset_version: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemEntry {
    pub address: u64,
    pub length: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuBox {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceCreate3d {
    pub header: CtrlHeader,
    pub resource_id: u32,
    pub target: u32,
    pub format: u32,
    pub bind: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub array_size: u32,
    pub last_level: u32,
    pub sample_count: u32,
    pub flags: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferHost3d {
    pub header: CtrlHeader,
    pub region: GpuBox,
    pub offset: u64,
    pub resource_id: u32,
    pub level: u32,
    pub stride: u32,
    pub layer_stride: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextCreate {
    pub header: CtrlHeader,
    pub name_length: u32,
    pub context_init: u32,
    pub debug_name: [u8; 64],
}

impl Default for ContextCreate {
    fn default() -> Self {
        Self {
            header: CtrlHeader::default(),
            name_length: 0,
            context_init: 0,
            debug_name: [0; 64],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextResource {
    pub header: CtrlHeader,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Submit3d {
    pub header: CtrlHeader,
    pub size: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SetScanout {
    pub header: CtrlHeader,
    pub rect: GpuRect,
    pub scanout_id: u32,
    pub resource_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct TransferToHost2d {
    pub header: CtrlHeader,
    pub rect: GpuRect,
    pub offset: u64,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceFlush {
    pub header: CtrlHeader,
    pub rect: GpuRect,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorPosition {
    pub scanout_id: u32,
    pub x: u32,
    pub y: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateCursor {
    pub header: CtrlHeader,
    pub position: CursorPosition,
    pub resource_id: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub padding: u32,
}

pub type MoveCursor = UpdateCursor;

const _: [(); 24] = [(); core::mem::size_of::<CtrlHeader>()];
const _: [(); 16] = [(); core::mem::size_of::<GpuRect>()];
const _: [(); 40] = [(); core::mem::size_of::<ResourceCreate2d>()];
const _: [(); 32] = [(); core::mem::size_of::<ResourceAttachBacking>()];
const _: [(); 32] = [(); core::mem::size_of::<GetCapsetInfo>()];
const _: [(); 40] = [(); core::mem::size_of::<CapsetInfoResponse>()];
const _: [(); 32] = [(); core::mem::size_of::<GetCapset>()];
const _: [(); 16] = [(); core::mem::size_of::<MemEntry>()];
const _: [(); 24] = [(); core::mem::size_of::<GpuBox>()];
const _: [(); 72] = [(); core::mem::size_of::<ResourceCreate3d>()];
const _: [(); 72] = [(); core::mem::size_of::<TransferHost3d>()];
const _: [(); 96] = [(); core::mem::size_of::<ContextCreate>()];
const _: [(); 32] = [(); core::mem::size_of::<ContextResource>()];
const _: [(); 32] = [(); core::mem::size_of::<Submit3d>()];
const _: [(); 48] = [(); core::mem::size_of::<SetScanout>()];
const _: [(); 56] = [(); core::mem::size_of::<TransferToHost2d>()];
const _: [(); 48] = [(); core::mem::size_of::<ResourceFlush>()];
const _: [(); 56] = [(); core::mem::size_of::<UpdateCursor>()];

pub fn bytes_of<T>(value: &T) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    }
}

pub fn bytes_of_mut<T>(value: &mut T) -> &mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(value as *mut T as *mut u8, core::mem::size_of::<T>())
    }
}
