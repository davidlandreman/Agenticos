//! Per-terminal stdin byte queue for ring-3 user processes.
//!
//! Each terminal window owns its own queue, keyed by `WindowId`. The
//! `TerminalWindow` pushes input bytes to its own queue; the
//! `read(0, …)` syscall handler looks up the calling process's
//! `terminal_id` and reads from that queue.
//!
//! Pre-multi-terminal, this was a single global queue — fine when
//! exactly one ring-3 process existed at a time, broken once two zshs
//! coexisted (the first reader to wake drained the shared queue, so
//! `ls` typed in terminal 2 would run inside zsh1 and print to
//! terminal 1).
//!
//! A `None` key reserves a "no terminal" queue used by test paths
//! that don't model a terminal window. Production paths key by
//! `Some(terminal_id)`.

use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

use crate::window::WindowId;

/// Soft cap on queued unread bytes per terminal. Bounds a runaway
/// producer (e.g. paste-storm into the terminal) from holding the
/// kernel heap hostage.
const MAX_QUEUED_BYTES: usize = 64 * 1024;

type Key = Option<WindowId>;

static USER_STDIN: Mutex<BTreeMap<Key, VecDeque<u8>>> = Mutex::new(BTreeMap::new());

// ---------- per-terminal API (production) ----------

/// Install an empty queue for `terminal_id`. Called from
/// `register_terminal` when a terminal window is created.
pub fn install_for_terminal(terminal_id: WindowId) {
    USER_STDIN
        .lock()
        .entry(Some(terminal_id))
        .or_insert_with(VecDeque::new);
}

/// Drop a terminal's queue. Called from `unregister_terminal` when the
/// terminal window is torn down.
pub fn clear_for_terminal(terminal_id: WindowId) {
    USER_STDIN.lock().remove(&Some(terminal_id));
}

/// True iff a queue exists for `terminal_id`.
pub fn is_active_for_terminal(terminal_id: WindowId) -> bool {
    USER_STDIN.lock().contains_key(&Some(terminal_id))
}

/// Append bytes to `terminal_id`'s queue and wake any ring-3 process
/// blocked in `read(0, ...)` that's bound to that terminal. Silently
/// drops bytes when no queue exists for the terminal or the queue is
/// at capacity (the alternative — blocking the producer — would
/// deadlock the keyboard input path).
pub fn push_bytes_for_terminal(terminal_id: WindowId, bytes: &[u8]) {
    let pushed = {
        let mut g = USER_STDIN.lock();
        let Some(buf) = g.get_mut(&Some(terminal_id)) else { return; };
        let room = MAX_QUEUED_BYTES.saturating_sub(buf.len());
        let take = core::cmp::min(room, bytes.len());
        buf.extend(bytes[..take].iter().copied());
        take
    };
    if pushed > 0 {
        crate::userland::lifecycle::wake_ring3_blocked_on_input(Some(terminal_id));
    }
}

/// True iff a queue exists for the currently-running ring-3 process's
/// terminal. The syscall path uses this to gate `read(0, ...)`.
pub fn is_active_for_current_process() -> bool {
    let key = current_process_key();
    USER_STDIN.lock().contains_key(&key)
}

/// Drain up to `dst.len()` bytes from the currently-running ring-3
/// process's terminal queue. Returns the number copied; 0 when there's
/// no queue or the queue is empty.
pub fn pop_into_for_current_process(dst: &mut [u8]) -> usize {
    let key = current_process_key();
    let mut g = USER_STDIN.lock();
    let Some(buf) = g.get_mut(&key) else { return 0; };
    let n = core::cmp::min(dst.len(), buf.len());
    for slot in dst.iter_mut().take(n) {
        *slot = buf.pop_front().unwrap();
    }
    n
}

fn current_process_key() -> Key {
    crate::userland::lifecycle::with_current_process(|p| p.terminal_id)
}

// ---------- legacy / test API (None-keyed fallback queue) ----------
//
// Tests and the legacy `enter_user_mode_with_aspace` path don't carry a
// terminal_id, so they push and pop against the `None` key. Production
// ring-3 processes have `Process.terminal_id == Some(tid)` and never
// touch this queue.

/// Install the None-keyed legacy queue. Idempotent.
pub fn install() {
    USER_STDIN.lock().entry(None).or_insert_with(VecDeque::new);
}

/// Drop the None-keyed legacy queue. Idempotent.
pub fn clear() {
    USER_STDIN.lock().remove(&None);
}

/// True iff the None-keyed legacy queue exists. Used by tests; the
/// production read path uses [`is_active_for_current_process`] instead.
pub fn is_active() -> bool {
    USER_STDIN.lock().contains_key(&None)
}

/// Append bytes to the None-keyed legacy queue. Used by tests; the
/// production input-routing path uses [`push_bytes_for_terminal`].
pub fn push_bytes(bytes: &[u8]) {
    let pushed = {
        let mut g = USER_STDIN.lock();
        let Some(buf) = g.get_mut(&None) else { return; };
        let room = MAX_QUEUED_BYTES.saturating_sub(buf.len());
        let take = core::cmp::min(room, bytes.len());
        buf.extend(bytes[..take].iter().copied());
        take
    };
    if pushed > 0 {
        crate::userland::lifecycle::wake_ring3_blocked_on_input(None);
    }
}

/// Drain up to `dst.len()` bytes from the None-keyed legacy queue.
/// Used by tests; production read path uses
/// [`pop_into_for_current_process`].
pub fn pop_into(dst: &mut [u8]) -> usize {
    let mut g = USER_STDIN.lock();
    let Some(buf) = g.get_mut(&None) else { return 0; };
    let n = core::cmp::min(dst.len(), buf.len());
    for slot in dst.iter_mut().take(n) {
        *slot = buf.pop_front().unwrap();
    }
    n
}

/// Number of bytes queued in the None-keyed legacy queue. Test-facing.
#[cfg(feature = "test")]
pub fn queued_len() -> usize {
    USER_STDIN
        .lock()
        .get(&None)
        .map(|b| b.len())
        .unwrap_or(0)
}
