//! AgenticOS ring-3 GUI syscall handlers (5001-5005 and selectable events at
//! 5011).

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::abi::{EFAULT, EINVAL, EIO, EMFILE, ENOENT};
use crate::userland::fdtable::{FdSlot, GuiEventHandle};
use crate::userland::gui::{self, GuiEvent, GuiWindowRecord, GUI_NONBLOCK};
use crate::window::windows::{FrameWindow, RemoteSurface};
use crate::window::{Rect, Window};

const MAX_TITLE_BYTES: usize = 256;
const MAX_SURFACE_DIMENSION: u32 = 4096;
const MAX_PRESENT_BYTES: usize = 64 * 1024 * 1024;

fn copy_title(pointer: u64, length: usize) -> Result<String, i64> {
    if length > MAX_TITLE_BYTES {
        return Err(EINVAL);
    }
    let mut title_bytes = vec![0u8; length];
    if crate::userland::usercopy::copy_from_user(&mut title_bytes, pointer).is_err() {
        return Err(EFAULT);
    }
    if title_bytes.is_empty() {
        return Ok(String::from("AgenticOS Application"));
    }
    core::str::from_utf8(&title_bytes)
        .map(ToString::to_string)
        .map_err(|_| EINVAL)
}

#[cfg(feature = "test")]
pub const TEST_GUI_CALLER_PID: u32 = u32::MAX - 1;

pub(crate) fn caller_pid() -> Result<u32, i64> {
    match crate::userland::lifecycle::current_user_pid() {
        Some(_) => Ok(crate::userland::lifecycle::current_tgid()),
        None => {
            #[cfg(feature = "test")]
            {
                return Ok(TEST_GUI_CALLER_PID);
            }
            #[cfg(not(feature = "test"))]
            {
                Err(crate::userland::abi::EPERM)
            }
        }
    }
}

/// `(flags) -> selectable_fd | -errno`.
pub fn gui_event_open_handler(args: &mut SyscallArgs) -> i64 {
    const O_NONBLOCK: u64 = 0x800;
    const O_CLOEXEC: u64 = 0x80000;
    let flags = args.rdi;
    if flags & !(O_NONBLOCK | O_CLOEXEC) != 0 {
        return EINVAL;
    }
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let slot = FdSlot::GuiEvents {
        handle: GuiEventHandle::new(pid, flags & O_NONBLOCK != 0),
        cloexec: flags & O_CLOEXEC != 0,
    };
    crate::userland::lifecycle::with_active_user(|process| process.fd_table.alloc(slot))
        .map_or(EMFILE, i64::from)
}

/// `(width, height, title_ptr, title_len, flags) -> handle | -errno`.
pub fn gui_win_create_handler(args: &mut SyscallArgs) -> i64 {
    let _abi_contract = (gui::GUI_ABI_VERSION, gui::GUI_PIXEL_FORMAT_XRGB8888);
    let width = args.rdi as u32;
    let height = args.rsi as u32;
    let title_len = args.r10 as usize;
    let flags = args.r8;
    if width == 0
        || height == 0
        || width > MAX_SURFACE_DIMENSION
        || height > MAX_SURFACE_DIMENSION
        || title_len > MAX_TITLE_BYTES
        || flags != 0
    {
        return EINVAL;
    }
    let title = match copy_title(args.rdx, title_len) {
        Ok(title) => title,
        Err(error) => return error,
    };
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let handle = match gui::allocate_handle(pid) {
        Ok(handle) => handle,
        Err(error) => return error,
    };

    let created = crate::window::with_window_manager(|wm| {
        let desktop_id = wm
            .get_active_screen()
            .and_then(|screen| screen.root_window)
            .ok_or(EIO)?;
        let (screen_width, screen_height) = wm.screen_dimensions();
        let metrics = crate::window::theme::metrics();
        let frame_width = width
            .checked_add(metrics.border_width.saturating_mul(2))
            .ok_or(EINVAL)?;
        let frame_height = height
            .checked_add(metrics.title_bar_height)
            .and_then(|value| value.checked_add(metrics.border_width.saturating_mul(2)))
            .ok_or(EINVAL)?;
        let cascade = ((handle.saturating_sub(1) % 8) * 24) as i32;
        let x = ((screen_width.saturating_sub(frame_width)) / 2) as i32 + cascade;
        let y = ((screen_height.saturating_sub(frame_height)) / 2) as i32 + cascade;

        let frame_id = wm.create_window(Some(desktop_id));
        let mut frame = Box::new(FrameWindow::new(frame_id, &title));
        frame.set_parent(Some(desktop_id));
        frame.set_bounds(Rect::new(x, y, frame_width, frame_height));

        let surface_id = wm.create_window(Some(frame_id));
        let content = frame.content_area();
        let mut surface = Box::new(RemoteSurface::new(surface_id, content, pid, handle));
        surface.set_parent(Some(frame_id));
        frame.set_content_window(surface_id);

        wm.set_window_impl(frame_id, frame);
        wm.set_window_impl(surface_id, surface);
        wm.bring_to_front(frame_id);
        Ok(GuiWindowRecord {
            frame_id,
            surface_id,
        })
    });
    let record = match created {
        Some(Ok(record)) => record,
        Some(Err(error)) => return error,
        None => return EIO,
    };
    if let Err(error) = gui::register_window(pid, handle, record) {
        let _ = crate::window::with_window_manager(|wm| wm.destroy_window(record.frame_id));
        return error;
    }
    let _ = crate::window::with_window_manager(|wm| wm.focus_window(record.surface_id));
    handle as i64
}

/// `(handle, pixels_ptr, width, height, stride_bytes) -> 0 | -errno`.
pub fn gui_win_present_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let handle = args.rdi as u32;
    let width = args.rdx as u32;
    let height = args.r10 as u32;
    let stride = args.r8 as usize;
    if width == 0 || height == 0 || width > MAX_SURFACE_DIMENSION || height > MAX_SURFACE_DIMENSION
    {
        return EINVAL;
    }
    let row_bytes = match (width as usize).checked_mul(4) {
        Some(value) => value,
        None => return EINVAL,
    };
    if stride < row_bytes {
        return EINVAL;
    }
    let byte_len = match stride.checked_mul(height as usize) {
        Some(value) if value <= MAX_PRESENT_BYTES => value,
        _ => return EINVAL,
    };
    let record = match gui::window_record(pid, handle) {
        Some(record) => record,
        None => return ENOENT,
    };
    let mut pixels = vec![0u8; byte_len];
    if crate::userland::usercopy::copy_from_user(&mut pixels, args.rsi).is_err() {
        return EFAULT;
    }
    let mut presented = false;
    let found = crate::window::with_window_manager(|wm| {
        wm.with_window_mut(record.surface_id, |window| {
            if let Some(surface) = window.as_remote_surface_mut() {
                presented = surface.present(&pixels, width, height, stride);
            }
        })
    });
    match found {
        Some(true) if presented => 0,
        Some(true) => EINVAL,
        _ => EIO,
    }
}

/// `(event_buf_ptr, buf_len, flags) -> 0 | -errno`.
pub fn gui_next_event_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    if args.rsi < core::mem::size_of::<GuiEvent>() as u64 || args.rdx & !GUI_NONBLOCK != 0 {
        return EINVAL;
    }
    if crate::userland::usercopy::ensure_user_range(
        args.rdi,
        core::mem::size_of::<GuiEvent>() as u64,
        true,
    )
    .is_err()
    {
        return EFAULT;
    }
    if let Some(event) = gui::pop_event(pid) {
        return match crate::userland::usercopy::write_unaligned(args.rdi, &event) {
            Ok(()) => 0,
            Err(error) => error,
        };
    }
    if args.rdx & GUI_NONBLOCK != 0 {
        return crate::userland::abi::EAGAIN;
    }
    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForGuiEvent,
        )
    }
}

/// `(handle) -> 0 | -errno`.
pub fn gui_win_destroy_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    crate::userland::gui_gl::destroy_for_window(pid, args.rdi as u32);
    let record = match gui::take_window(pid, args.rdi as u32) {
        Some(record) => record,
        None => return ENOENT,
    };
    match crate::window::with_window_manager(|wm| wm.destroy_window(record.frame_id)) {
        Some(()) => 0,
        None => EIO,
    }
}

/// `(handle, title_ptr, title_len) -> 0 | -errno`.
pub fn gui_win_set_title_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let record = match gui::window_record(pid, args.rdi as u32) {
        Some(record) => record,
        None => return ENOENT,
    };
    let title = match copy_title(args.rsi, args.rdx as usize) {
        Ok(title) => title,
        Err(error) => return error,
    };
    let mut updated = false;
    let found = crate::window::with_window_manager(|wm| {
        wm.with_window_mut(record.frame_id, |window| {
            if let Some(frame) = window.as_frame_window_mut() {
                frame.set_title(&title);
                updated = true;
            }
        })
    });
    match found {
        Some(true) if updated => 0,
        _ => EIO,
    }
}
