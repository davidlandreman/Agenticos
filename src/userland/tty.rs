//! Per-process TTY state — thin shim over [`crate::terminal::pty`].
//!
//! Previously this module held a global `static TERMIOS` because there
//! was exactly one ring-3 process at a time. With multi-ring-3 + per-pty
//! state, termios + winsize now live on the [`crate::terminal::pty::PtyInner`]
//! that the calling process's terminal_id resolves to. This module keeps
//! the original API (`snapshot`, `set`, `winsize`, `is_canonical`,
//! `is_echo`, `install_default`) as a stable surface for older call
//! sites; new code should reach for the pty registry directly.
//!
//! The legacy `None`-keyed pty (installed by `install_default`) backs
//! the rare case where the kernel touches tty state before a real
//! terminal exists (early boot, tests that don't model a window).

#[allow(unused_imports)]
pub use crate::terminal::pty::{
    Termios, Winsize, ECHO, ECHOE, ECHOK, ICANON, ICRNL, IEXTEN, ISIG, IXON, NCCS, ONLCR, OPOST,
    VEOF, VERASE, VINTR, VKILL, VMIN, VQUIT, VSUSP, VTIME,
};

use crate::terminal::pty;

/// Install the legacy None-keyed pty. Called once during userland init
/// so early-boot tty queries find a backing struct.
pub fn install_default() {
    pty::install_legacy();
}

/// Read the current process's termios. Falls back to the legacy
/// None-keyed pty when no process or no pty is associated. Returns the
/// canonical defaults if nothing exists.
pub fn snapshot() -> Termios {
    if let Some(slave) = current_process_slave() {
        return slave.termios();
    }
    if let Some(slave) = pty::legacy_slave() {
        return slave.termios();
    }
    Termios::default_tty()
}

/// Replace the current process's termios. Targets the same pty
/// `snapshot` reads from.
pub fn set(t: Termios) {
    if let Some(slave) = current_process_slave() {
        slave.set_termios(t);
        return;
    }
    if let Some(slave) = pty::legacy_slave() {
        slave.set_termios(t);
    }
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn is_canonical() -> bool {
    snapshot().is_canonical()
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn is_echo() -> bool {
    snapshot().is_echo()
}

/// Winsize for the current process's pty. `TERMINAL.ELF` seeds and updates it
/// through the master-side PTY ABI; callers without a pty receive defaults.
pub fn winsize() -> Winsize {
    if let Some(slave) = current_process_slave() {
        return slave.winsize();
    }
    if let Some(slave) = pty::legacy_slave() {
        return slave.winsize();
    }
    Winsize::new(
        crate::terminal::pty::DEFAULT_ROWS,
        crate::terminal::pty::DEFAULT_COLS,
    )
}

fn current_process_slave() -> Option<pty::PtySlave> {
    let tid = crate::userland::lifecycle::with_current_group(|p| p.terminal_id)?;
    pty::slave_for_terminal(tid)
}
