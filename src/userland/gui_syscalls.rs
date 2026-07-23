//! AgenticOS ring-3 GUI syscall handlers (5001-5005, selectable events at
//! 5011, and pointer selection at 5019).

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

/// `(width, height, title_ptr, title_len, flags, position) -> handle | -errno`.
///
/// `flags` may combine [`GUI_WINDOW_FIXED_SIZE`](gui::GUI_WINDOW_FIXED_SIZE)
/// with the shell-only chrome flags
/// [`GUI_WINDOW_UNDECORATED`](gui::GUI_WINDOW_UNDECORATED) /
/// [`GUI_WINDOW_PANEL`](gui::GUI_WINDOW_PANEL). For a plain undecorated
/// surface, `position` (arg 6, `r9`) packs a top-left `(x, y)` as two `i32`s
/// (x in the low 32 bits, y in the high 32). Panels ignore `position` and dock
/// to the bottom of the screen spanning the full width.
pub fn gui_win_create_handler(args: &mut SyscallArgs) -> i64 {
    let _abi_contract = (gui::GUI_ABI_VERSION, gui::GUI_PIXEL_FORMAT_XRGB8888);
    let width = args.rdi as u32;
    let height = args.rsi as u32;
    let title_len = args.r10 as usize;
    let flags = args.r8;
    let position = args.r9;
    const ALLOWED_FLAGS: u64 = gui::GUI_WINDOW_FIXED_SIZE
        | gui::GUI_WINDOW_UNDECORATED
        | gui::GUI_WINDOW_PANEL
        | gui::GUI_WINDOW_NO_FOCUS;
    let panel = flags & gui::GUI_WINDOW_PANEL != 0;
    // A panel is a specialized undecorated surface.
    let undecorated = panel || flags & gui::GUI_WINDOW_UNDECORATED != 0;
    let no_focus = flags & gui::GUI_WINDOW_NO_FOCUS != 0;
    if width == 0
        || height == 0
        || width > MAX_SURFACE_DIMENSION
        || height > MAX_SURFACE_DIMENSION
        || (!undecorated
            && flags & gui::GUI_WINDOW_FIXED_SIZE == 0
            && width < crate::window::theme::minimum_resizable_client_width())
        || title_len > MAX_TITLE_BYTES
        || flags & !ALLOWED_FLAGS != 0
        // NO_FOCUS is only meaningful for undecorated chrome.
        || (no_focus && !undecorated)
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
    // Chrome surfaces (undecorated/panel) are privileged: only the registered
    // desktop shell may create them.
    if undecorated && !gui::is_desktop_shell(pid) {
        return crate::userland::abi::EPERM;
    }
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

        if undecorated {
            // Bare RemoteSurface parented directly to the desktop root — no
            // frame chrome. Panels dock full-width to the bottom; other
            // undecorated surfaces honor the caller-supplied position.
            let (x, y, surface_width) = if panel {
                (
                    0,
                    (screen_height.saturating_sub(height)) as i32,
                    screen_width,
                )
            } else {
                (
                    (position & 0xFFFF_FFFF) as u32 as i32,
                    (position >> 32) as u32 as i32,
                    width,
                )
            };
            let surface_id = wm.create_window(Some(desktop_id));
            let bounds = Rect::new(x, y, surface_width, height);
            let mut surface = Box::new(RemoteSurface::new(surface_id, bounds, pid, handle));
            surface.set_parent(Some(desktop_id));
            wm.set_window_impl(surface_id, surface);
            if let Some(desktop) = wm.window_registry.get_mut(&desktop_id) {
                desktop.add_child(surface_id);
            }
            wm.bring_to_front(surface_id);
            if panel {
                // Reserve the work-area strut and keep app frames above it.
                wm.set_taskbar_id(Some(surface_id));
            }
            // Undecorated chrome has no distinct frame; reuse the surface id.
            return Ok(GuiWindowRecord {
                frame_id: surface_id,
                surface_id,
            });
        }

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
        frame.set_resizable(flags & gui::GUI_WINDOW_FIXED_SIZE == 0);
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
    // Panels never take focus; NO_FOCUS chrome (e.g. a Start-menu fly-out)
    // must not steal focus from its parent popup.
    if !panel && !no_focus {
        let _ = crate::window::with_window_manager(|wm| wm.focus_window(record.surface_id));
    }
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
    let result = match crate::window::with_window_manager(|wm| wm.destroy_window(record.frame_id)) {
        Some(()) => 0,
        None => EIO,
    };
    gui::release_window_pty(record.surface_id);
    result
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

/// `(handle, cursor_kind) -> 0 | -errno`.
///
/// Cursor kinds are `0=Arrow`, `1=Wait`, and `2=Text`. The handle lookup is
/// scoped to the calling process, so a client cannot affect another process's
/// window.
pub fn gui_win_set_cursor_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    let record = match gui::window_record(pid, args.rdi as u32) {
        Some(record) => record,
        None => return ENOENT,
    };
    let Some(icon) = crate::window::CursorIcon::from_abi(args.rsi as u32) else {
        return EINVAL;
    };
    match crate::window::with_window_manager(|wm| {
        wm.set_remote_cursor_icon(record.surface_id, icon)
    }) {
        Some(Some(_)) => 0,
        _ => EIO,
    }
}

/// `(flags) -> 0 | -errno`. Claim the singleton desktop-shell role for the
/// calling process. `flags` is reserved and must be zero. Idempotent for the
/// current holder; `-EEXIST` if a different live shell already holds it. The
/// role is released automatically when the process exits.
pub fn gui_shell_register_handler(args: &mut SyscallArgs) -> i64 {
    if args.rdi != 0 {
        return EINVAL;
    }
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    match gui::register_desktop_shell(pid) {
        Ok(()) => 0,
        Err(error) => error,
    }
}

/// `(buf_ptr, buf_len_bytes) -> record_count | -errno`. Snapshot the current
/// top-level frames as [`gui::ShellWindowRecord`]s into the caller's buffer,
/// returning the number written (capped at buffer capacity). Desktop-shell
/// only.
pub fn gui_shell_list_windows_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    if !gui::is_desktop_shell(pid) {
        return crate::userland::abi::EPERM;
    }
    let record_size = core::mem::size_of::<gui::ShellWindowRecord>();
    let capacity = (args.rsi as usize) / record_size;
    if capacity == 0 {
        return 0;
    }

    let list = crate::window::with_window_manager(|wm| wm.shell_window_list()).unwrap_or_default();
    let count = list.len().min(capacity);

    let mut bytes = alloc::vec::Vec::with_capacity(count * record_size);
    for (id, title, state) in list.iter().take(count) {
        let record = gui::ShellWindowRecord::new(id.0 as u64, u32::from(*state), title);
        let raw = unsafe {
            core::slice::from_raw_parts(
                (&record as *const gui::ShellWindowRecord) as *const u8,
                record_size,
            )
        };
        bytes.extend_from_slice(raw);
    }
    if crate::userland::usercopy::copy_to_user(args.rdi, &bytes).is_err() {
        return EFAULT;
    }
    count as i64
}

/// `(frame_id, action) -> 0 | -errno`. Apply a taskbar action to a frame the
/// shell does not own. `action`: `0` activate, `1` minimize, `2`
/// maximize/restore toggle, `3` restore, `4` close. Desktop-shell only.
pub fn gui_shell_window_action_handler(args: &mut SyscallArgs) -> i64 {
    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    if !gui::is_desktop_shell(pid) {
        return crate::userland::abi::EPERM;
    }
    let frame_id = crate::window::WindowId(args.rdi as usize);
    let action = args.rsi as u32;
    match crate::window::with_window_manager(|wm| wm.shell_window_action(frame_id, action)) {
        Some(true) => 0,
        Some(false) => ENOENT,
        None => EIO,
    }
}
