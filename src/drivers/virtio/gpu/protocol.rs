//! VirtIO 1.3 section 5.7 wire layouts (little-endian x86 guest).
//!
//! Provenance: OASIS VirtIO 1.3, GPU Device command and response structures.

pub const VIRTIO_GPU_F_VIRGL: u32 = 1 << 0;
pub const VIRTIO_GPU_F_EDID: u32 = 1 << 1;

pub const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const CMD_SET_SCANOUT: u32 = 0x0103;
pub const CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

pub const RESP_OK_NODATA: u32 = 0x1100;
pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;

pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;
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
pub struct MemEntry {
    pub address: u64,
    pub length: u32,
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
#[expect(dead_code, reason = "intentional kernel API surface")]
pub struct UpdateCursor {
    pub header: CtrlHeader,
    pub position: CursorPosition,
    pub resource_id: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub padding: u32,
}

const _: [(); 24] = [(); core::mem::size_of::<CtrlHeader>()];
const _: [(); 16] = [(); core::mem::size_of::<GpuRect>()];
const _: [(); 40] = [(); core::mem::size_of::<ResourceCreate2d>()];
const _: [(); 32] = [(); core::mem::size_of::<ResourceAttachBacking>()];
const _: [(); 16] = [(); core::mem::size_of::<MemEntry>()];
const _: [(); 48] = [(); core::mem::size_of::<SetScanout>()];
const _: [(); 56] = [(); core::mem::size_of::<TransferToHost2d>()];
const _: [(); 48] = [(); core::mem::size_of::<ResourceFlush>()];

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
