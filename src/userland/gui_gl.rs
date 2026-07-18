//! Validated ring-3 fixed-function GL frame ABI (syscalls 5006-5009).

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::arch::x86_64::syscall::SyscallArgs;
use crate::graphics::composition::{
    ClientGlDraw, ClientGlFrame, ClientGlId, ClientGlVertex, CLIENT_GL_DRAW_CULL_BACK,
    CLIENT_GL_DRAW_DEPTH_TEST, CLIENT_GL_MAX_DRAWS, CLIENT_GL_MAX_PACKET_BYTES,
    CLIENT_GL_MAX_VERTICES,
};
use crate::userland::abi::{EBUSY, EFAULT, EINVAL, EIO, ENOENT};
use crate::userland::gui::GuiWindowRecord;
use crate::window::Rect;

pub const GL_ABI_MAGIC: u32 = 0x314C_4741; // "AGL1" in little endian.
pub const GL_ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlFrameHeader {
    pub magic: u32,
    pub version: u32,
    pub byte_len: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub viewport_x: u32,
    pub viewport_y: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub clear_color: [f32; 4],
    pub clear_depth: f32,
    pub draw_count: u32,
    pub vertex_count: u32,
    pub draw_offset: u32,
    pub vertex_offset: u32,
    pub reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GlInfo {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub max_packet_bytes: u32,
    pub max_draws: u32,
    pub max_vertices: u32,
    pub supported_draw_flags: u32,
    pub last_error: i32,
    pub last_submitted_serial: u64,
    pub last_completed_serial: u64,
}

const _: [(); 88] = [(); core::mem::size_of::<GlFrameHeader>()];
const _: [(); 48] = [(); core::mem::size_of::<GlInfo>()];
const _: [(); 16] = [(); core::mem::size_of::<ClientGlDraw>()];
const _: [(); 32] = [(); core::mem::size_of::<ClientGlVertex>()];

#[derive(Debug, Clone, Copy)]
struct GlRecord {
    window_handle: u32,
    window: GuiWindowRecord,
    client_id: ClientGlId,
    next_serial: u64,
    last_submitted_serial: u64,
}

struct GlProcessState {
    next_handle: u32,
    contexts: BTreeMap<u32, GlRecord>,
}

impl GlProcessState {
    const fn new() -> Self {
        Self {
            next_handle: 1,
            contexts: BTreeMap::new(),
        }
    }
}

static GL_STATES: InterruptMutex<BTreeMap<u32, GlProcessState>> =
    InterruptMutex::new(BTreeMap::new());

fn allocate_context_handle(pid: u32) -> Result<u32, i64> {
    let mut states = GL_STATES.lock();
    let state = states.entry(pid).or_insert_with(GlProcessState::new);
    let start = state.next_handle.max(1);
    let mut handle = start;
    loop {
        if !state.contexts.contains_key(&handle) {
            state.next_handle = handle.wrapping_add(1).max(1);
            return Ok(handle);
        }
        handle = handle.wrapping_add(1).max(1);
        if handle == start {
            return Err(crate::userland::abi::EMFILE);
        }
    }
}

fn record(pid: u32, handle: u32) -> Option<GlRecord> {
    GL_STATES
        .lock()
        .get(&pid)
        .and_then(|state| state.contexts.get(&handle).copied())
}

fn take_record(pid: u32, handle: u32) -> Option<GlRecord> {
    GL_STATES
        .lock()
        .get_mut(&pid)
        .and_then(|state| state.contexts.remove(&handle))
}

fn next_serial(pid: u32, handle: u32) -> Result<u64, i64> {
    let mut states = GL_STATES.lock();
    let entry = states
        .get_mut(&pid)
        .and_then(|state| state.contexts.get_mut(&handle))
        .ok_or(ENOENT)?;
    let serial = entry.next_serial;
    entry.next_serial = serial.checked_add(1).ok_or(EIO)?;
    entry.last_submitted_serial = serial;
    Ok(serial)
}

fn window_has_context(pid: u32, window_handle: u32) -> bool {
    GL_STATES
        .lock()
        .get(&pid)
        .map(|state| {
            state
                .contexts
                .values()
                .any(|record| record.window_handle == window_handle)
        })
        .unwrap_or(false)
}

pub fn context_create_handler(args: &mut SyscallArgs) -> i64 {
    if args.rsi != 0 {
        return EINVAL;
    }
    let pid = match super::gui_syscalls::caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let window_handle = args.rdi as u32;
    if window_has_context(pid, window_handle) {
        return EBUSY;
    }
    let window = match super::gui::window_record(pid, window_handle) {
        Some(window) => window,
        None => return ENOENT,
    };
    let created = crate::window::with_window_manager(|wm| {
        let bounds = wm
            .window_registry
            .get(&window.surface_id)
            .map(|surface| surface.bounds())
            .ok_or(EIO)?;
        let client_id = wm.create_gl_client(bounds.width, bounds.height)?;
        let mut attached = false;
        wm.with_window_mut(window.surface_id, |surface| {
            if let Some(remote) = surface.as_remote_surface_mut() {
                attached = remote.attach_gl_client(client_id);
            }
        });
        if !attached {
            let _ = wm.destroy_gl_client(client_id);
            return Err(EBUSY);
        }
        Ok(client_id)
    });
    let client_id = match created {
        Some(Ok(id)) => id,
        Some(Err(error)) => return error,
        None => return EIO,
    };
    let handle = match allocate_context_handle(pid) {
        Ok(handle) => handle,
        Err(error) => {
            destroy_client_record(GlRecord {
                window_handle,
                window,
                client_id,
                next_serial: 1,
                last_submitted_serial: 0,
            });
            return error;
        }
    };
    let mut states = GL_STATES.lock();
    let state = states.entry(pid).or_insert_with(GlProcessState::new);
    state.contexts.insert(
        handle,
        GlRecord {
            window_handle,
            window,
            client_id,
            next_serial: 1,
            last_submitted_serial: 0,
        },
    );
    handle as i64
}

pub fn submit_frame_handler(args: &mut SyscallArgs) -> i64 {
    if args.r10 != 0 {
        return EINVAL;
    }
    let pid = match super::gui_syscalls::caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let handle = args.rdi as u32;
    let record = match record(pid, handle) {
        Some(record) => record,
        None => return ENOENT,
    };
    let byte_len = args.rdx as usize;
    if byte_len < core::mem::size_of::<GlFrameHeader>() || byte_len > CLIENT_GL_MAX_PACKET_BYTES {
        return EINVAL;
    }
    let mut packet = vec![0u8; byte_len];
    if super::usercopy::copy_from_user(&mut packet, args.rsi).is_err() {
        return EFAULT;
    }
    let mut frame = match validate_packet(&packet) {
        Ok(frame) => frame,
        Err(error) => return error,
    };
    let info =
        crate::window::with_window_manager(|wm| wm.gl_client_info(record.client_id)).flatten();
    let Some(info) = info else {
        return EIO;
    };
    if frame.width != info.width || frame.height != info.height {
        return EINVAL;
    }
    let serial = match next_serial(pid, handle) {
        Ok(serial) => serial,
        Err(error) => return error,
    };
    frame.serial = serial;
    match crate::window::with_window_manager(|wm| {
        wm.submit_gl_client_frame(record.window.surface_id, record.client_id, frame)
    }) {
        Some(Ok(())) => serial as i64,
        Some(Err(error)) => error,
        None => EIO,
    }
}

pub fn get_info_handler(args: &mut SyscallArgs) -> i64 {
    if args.rdx < core::mem::size_of::<GlInfo>() as u64 {
        return EINVAL;
    }
    let pid = match super::gui_syscalls::caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let record = match record(pid, args.rdi as u32) {
        Some(record) => record,
        None => return ENOENT,
    };
    let engine =
        crate::window::with_window_manager(|wm| wm.gl_client_info(record.client_id)).flatten();
    let Some(engine) = engine else {
        return EIO;
    };
    let info = GlInfo {
        version: GL_ABI_VERSION,
        width: engine.width,
        height: engine.height,
        max_packet_bytes: CLIENT_GL_MAX_PACKET_BYTES as u32,
        max_draws: CLIENT_GL_MAX_DRAWS as u32,
        max_vertices: CLIENT_GL_MAX_VERTICES as u32,
        supported_draw_flags: engine.supported_draw_flags,
        last_error: engine.last_error,
        last_submitted_serial: record.last_submitted_serial,
        last_completed_serial: engine.last_completed_serial,
    };
    match super::usercopy::write_unaligned(args.rsi, &info) {
        Ok(()) => 0,
        Err(_) => EFAULT,
    }
}

pub fn context_destroy_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match super::gui_syscalls::caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let Some(record) = take_record(pid, args.rdi as u32) else {
        return ENOENT;
    };
    destroy_client_record(record)
}

pub fn destroy_for_window(pid: u32, window_handle: u32) {
    let records: Vec<GlRecord> = {
        let mut states = GL_STATES.lock();
        let Some(state) = states.get_mut(&pid) else {
            return;
        };
        let handles: Vec<u32> = state
            .contexts
            .iter()
            .filter_map(|(&handle, record)| {
                (record.window_handle == window_handle).then_some(handle)
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| state.contexts.remove(&handle))
            .collect()
    };
    for record in records {
        let _ = destroy_client_record(record);
    }
}

pub fn cleanup_process(pid: u32) {
    let records: Vec<GlRecord> = GL_STATES
        .lock()
        .remove(&pid)
        .map(|state| state.contexts.into_values().collect())
        .unwrap_or_default();
    for record in records {
        let _ = destroy_client_record(record);
    }
}

fn destroy_client_record(record: GlRecord) -> i64 {
    match crate::window::with_window_manager(|wm| {
        wm.with_window_mut(record.window.surface_id, |surface| {
            if let Some(remote) = surface.as_remote_surface_mut() {
                let _ = remote.detach_gl_client(record.client_id);
            }
        });
        wm.destroy_gl_client(record.client_id)
    }) {
        Some(Ok(())) => 0,
        Some(Err(error)) => error,
        None => EIO,
    }
}

fn validate_packet(packet: &[u8]) -> Result<ClientGlFrame, i64> {
    let header = read_wire::<GlFrameHeader>(packet, 0).ok_or(EINVAL)?;
    if header.magic != GL_ABI_MAGIC
        || header.version != GL_ABI_VERSION
        || header.byte_len as usize != packet.len()
        || header.flags != 0
        || header.reserved != [0; 3]
        || header.width == 0
        || header.height == 0
        || header.width > 4096
        || header.height > 4096
        || header.draw_count as usize > CLIENT_GL_MAX_DRAWS
        || header.vertex_count as usize > CLIENT_GL_MAX_VERTICES
    {
        return Err(EINVAL);
    }
    let viewport_right = header
        .viewport_x
        .checked_add(header.viewport_width)
        .ok_or(EINVAL)?;
    let viewport_bottom = header
        .viewport_y
        .checked_add(header.viewport_height)
        .ok_or(EINVAL)?;
    if header.viewport_width == 0
        || header.viewport_height == 0
        || viewport_right > header.width
        || viewport_bottom > header.height
        || !header.clear_depth.is_finite()
        || header.clear_depth != 1.0
        || header
            .clear_color
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(EINVAL);
    }

    let draw_offset = header.draw_offset as usize;
    let vertex_offset = header.vertex_offset as usize;
    let draw_bytes = (header.draw_count as usize)
        .checked_mul(core::mem::size_of::<ClientGlDraw>())
        .ok_or(EINVAL)?;
    let vertex_bytes = (header.vertex_count as usize)
        .checked_mul(core::mem::size_of::<ClientGlVertex>())
        .ok_or(EINVAL)?;
    let draw_end = draw_offset.checked_add(draw_bytes).ok_or(EINVAL)?;
    let vertex_end = vertex_offset.checked_add(vertex_bytes).ok_or(EINVAL)?;
    if draw_offset != core::mem::size_of::<GlFrameHeader>()
        || draw_end != vertex_offset
        || vertex_end != packet.len()
    {
        return Err(EINVAL);
    }

    let mut draws = Vec::with_capacity(header.draw_count as usize);
    for index in 0..header.draw_count as usize {
        let offset = draw_offset + index * core::mem::size_of::<ClientGlDraw>();
        let draw = read_wire::<ClientGlDraw>(packet, offset).ok_or(EINVAL)?;
        let end = draw
            .first_vertex
            .checked_add(draw.vertex_count)
            .ok_or(EINVAL)?;
        if draw.vertex_count == 0
            || draw.vertex_count % 3 != 0
            || end > header.vertex_count
            || draw.flags & !(CLIENT_GL_DRAW_DEPTH_TEST | CLIENT_GL_DRAW_CULL_BACK) != 0
            || draw.reserved != 0
        {
            return Err(EINVAL);
        }
        draws.push(draw);
    }

    let mut vertices = Vec::with_capacity(header.vertex_count as usize);
    for index in 0..header.vertex_count as usize {
        let offset = vertex_offset + index * core::mem::size_of::<ClientGlVertex>();
        let vertex = read_wire::<ClientGlVertex>(packet, offset).ok_or(EINVAL)?;
        if vertex.position.iter().any(|value| !value.is_finite())
            || vertex
                .color
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(EINVAL);
        }
        vertices.push(vertex);
    }
    Ok(ClientGlFrame {
        serial: 0,
        width: header.width,
        height: header.height,
        viewport: Rect::new(
            header.viewport_x as i32,
            header.viewport_y as i32,
            header.viewport_width,
            header.viewport_height,
        ),
        clear_color: header.clear_color,
        clear_depth: header.clear_depth as f64,
        draws,
        vertices,
    })
}

fn read_wire<T: Copy>(bytes: &[u8], offset: usize) -> Option<T> {
    let end = offset.checked_add(core::mem::size_of::<T>())?;
    let source = bytes.get(offset..end)?;
    // SAFETY: the range has exactly `size_of::<T>()` readable bytes and
    // `read_unaligned` does not require the byte slice to share T's alignment.
    Some(unsafe { source.as_ptr().cast::<T>().read_unaligned() })
}

#[cfg(feature = "test")]
pub fn reset_for_test() {
    GL_STATES.lock().clear();
}

#[cfg(feature = "test")]
pub(crate) fn validate_packet_for_test(packet: &[u8]) -> Result<ClientGlFrame, i64> {
    validate_packet(packet)
}
