//! Per-terminal stdin byte queue — thin shim over
//! [`crate::terminal::pty`].
//!
//! The data formerly stored here (per-WindowId `VecDeque<u8>`) now lives
//! inside [`crate::terminal::pty::PtyInner::slave_queue`]. Production reads
//! resolve the current process's slave; the remaining unkeyed API is a test
//! fixture.
//!
//! Lookup model matches the previous design: the pty registry is keyed
//! by `Option<WindowId>` (the `None` slot reserved for tests / boot-time
//! paths without a terminal). Per-process lookup uses
//! `Process.terminal_id`.

use crate::terminal::pty;

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

/// Consume a pending canonical-mode EOF (VEOF typed on an empty line).
/// True exactly once per EOF; the caller's `read(0)` returns 0.
pub fn take_eof_for_current_process() -> bool {
    current_slave()
        .map(|slave| slave.take_eof())
        .unwrap_or(false)
}

fn current_slave() -> Option<pty::PtySlave> {
    let tid = crate::userland::lifecycle::with_current_group(|p| p.terminal_id)?;
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
