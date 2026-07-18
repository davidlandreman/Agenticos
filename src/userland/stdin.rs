//! Per-terminal stdin byte queue — thin shim over
//! [`crate::terminal::pty`].
//!
//! The data formerly stored here (per-WindowId `VecDeque<u8>`) now lives
//! inside [`crate::terminal::pty::PtyInner::slave_queue`]. This module
//! keeps the original API as a stable surface so existing call sites in
//! `syscalls.rs`, `window::windows::terminal`, and `window::terminal`
//! don't need to change in lockstep.
//!
//! Lookup model matches the previous design: the pty registry is keyed
//! by `Option<WindowId>` (the `None` slot reserved for tests / boot-time
//! paths without a terminal). Per-process lookup uses
//! `Process.terminal_id`.

use crate::terminal::pty;
use crate::window::WindowId;

// ---------- per-terminal API (production) ----------

pub fn install_for_terminal(terminal_id: WindowId) {
    pty::install_for_terminal(
        terminal_id,
        crate::terminal::config::DEFAULT_ROWS,
        crate::terminal::config::DEFAULT_COLS,
    );
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn clear_for_terminal(terminal_id: WindowId) {
    pty::clear_for_terminal(terminal_id);
}

pub fn is_active_for_terminal(terminal_id: WindowId) -> bool {
    pty::is_active_for_terminal(terminal_id)
}

/// Append bytes to `terminal_id`'s slave queue and wake any ring-3
/// process blocked in `read(0)` bound to that terminal. Silently drops
/// bytes when no pty exists or the queue is full.
pub fn push_bytes_for_terminal(terminal_id: WindowId, bytes: &[u8]) {
    let Some(master) = pty::master_for_terminal(terminal_id) else {
        return;
    };
    let pushed = master.push_input(bytes);
    if pushed {
        crate::userland::lifecycle::wake_ring3_blocked_on_input(Some(terminal_id));
    }
}

pub fn is_active_for_current_process() -> bool {
    current_slave().is_some()
}

pub fn pop_into_for_current_process(dst: &mut [u8]) -> usize {
    let Some(slave) = current_slave() else {
        return 0;
    };
    slave.read(dst)
}

pub fn queued_len_for_current_process() -> usize {
    current_slave().map(|slave| slave.readable()).unwrap_or(0)
}

fn current_slave() -> Option<pty::PtySlave> {
    let tid = crate::userland::lifecycle::with_current_process(|p| p.terminal_id)?;
    pty::slave_for_terminal(tid)
}

// ---------- legacy / test API (None-keyed) ----------

pub fn install() {
    pty::install_legacy();
}

pub fn clear() {
    pty::clear_legacy();
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn is_active() -> bool {
    pty::is_active_legacy()
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn push_bytes(bytes: &[u8]) {
    let Some(master) = pty::legacy_master() else {
        return;
    };
    let pushed = master.push_input(bytes);
    if pushed {
        crate::userland::lifecycle::wake_ring3_blocked_on_input(None);
    }
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn pop_into(dst: &mut [u8]) -> usize {
    let Some(slave) = pty::legacy_slave() else {
        return 0;
    };
    slave.read(dst)
}

#[cfg(feature = "test")]
pub fn queued_len() -> usize {
    pty::legacy_slave().map(|s| s.readable()).unwrap_or(0)
}
