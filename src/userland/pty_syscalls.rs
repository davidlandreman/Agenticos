//! Ring-3 pty master ABI for the userland terminal emulator (`TERMINAL.ELF`).
//!
//! The kernel keeps the pty (fd pair, termios, winsize, line discipline). This
//! module hands a ring-3 emulator the *master* end so it can drive a child
//! shell running on the slave — the split Linux makes between the kernel N_TTY
//! and a userland xterm.
//!
//! `pty_open` keys a pty on the caller's own GUI window (its `surface_id`
//! `WindowId`), sets the caller's `terminal_id` so a subsequently-`fork`ed
//! child inherits the slave through the existing sentinel-stdio model, and
//! returns a `FdSlot::PtyMaster` descriptor. `pty_set_winsize` updates the
//! grid geometry and raises SIGWINCH on the child.
//!
//! Non-blocking by construction: `read(master_fd)` returns `-EAGAIN` on an
//! empty queue, so the emulator polls the master fd alongside its GUI event fd.

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::terminal::pty;
use crate::userland::abi::{EBADF, EINVAL, EMFILE};
use crate::userland::fdtable::FdSlot;
use crate::userland::gui_syscalls::caller_pid;

const O_CLOEXEC: u64 = 0x80000;

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// `pty_open(window_handle, rows, cols, flags) -> master_fd | -errno`.
///
/// `window_handle` is a GUI window handle the caller already owns
/// (`gui_win_create`). `rows`/`cols` seed the initial winsize (0 → defaults).
/// `flags` accepts `O_CLOEXEC`.
pub fn pty_open_handler(args: &mut SyscallArgs) -> i64 {
    let handle = args.rdi as u32;
    let rows = if args.rsi == 0 { DEFAULT_ROWS } else { args.rsi as u16 };
    let cols = if args.rdx == 0 { DEFAULT_COLS } else { args.rdx as u16 };
    let flags = args.r10;
    if flags & !O_CLOEXEC != 0 {
        return EINVAL;
    }

    let pid = match caller_pid() {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    // Resolve the caller's window to the content-well WindowId we key the pty
    // on. Ownership is enforced by `window_record` (per-PID map).
    let Some(record) = crate::userland::gui::window_record(pid, handle) else {
        return EBADF;
    };
    let terminal_id = record.surface_id;

    let master = pty::install_for_terminal(terminal_id, rows, cols);
    let slot = FdSlot::PtyMaster {
        master,
        cloexec: flags & O_CLOEXEC != 0,
    };

    // Bind the caller (and, by fork inheritance, its child shell) to this pty,
    // then install the master descriptor.
    crate::userland::lifecycle::with_active_user(|process| {
        process.terminal_id = Some(terminal_id);
        process.fd_table.alloc(slot)
    })
    .map_or(EMFILE, i64::from)
}

/// `pty_set_winsize(master_fd, rows, cols) -> 0 | -errno`.
///
/// Updates the pty winsize and, on an actual change, raises SIGWINCH on the
/// child bound to this terminal.
pub fn pty_set_winsize_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let rows = args.rsi as u16;
    let cols = args.rdx as u16;
    if rows == 0 || cols == 0 {
        return EINVAL;
    }

    let master = crate::userland::lifecycle::with_active_user(|process| {
        match process.fd_table.get(fd) {
            Some(FdSlot::PtyMaster { master, .. }) => Some(master.clone()),
            _ => None,
        }
    });
    let Some(master) = master else {
        return EBADF;
    };

    let changed = master.set_winsize(pty::Winsize::new(rows, cols));
    if changed {
        crate::userland::lifecycle::raise_signal_on_terminal(
            master.terminal_id(),
            crate::userland::signal::SIGWINCH,
        );
    }
    0
}
