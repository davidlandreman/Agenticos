//! Per-active-process stdin byte queue for ring-3 user processes.
//!
//! Phase 1 of the path toward a real shell. Behavior is deliberately minimal:
//!
//! - **Producer:** the focused `TerminalWindow` pushes a completed line + `\n`
//!   on Enter, instead of routing the line to its in-kernel shell.
//! - **Consumer:** the `read(0, …)` syscall handler drains bytes into the
//!   user's buffer, blocking via `sti; hlt` while the queue is empty.
//!
//! Lifetime: installed by `enter_user_mode`, cleared after the long-jump
//! returns. There is at most one user process at a time (D5), so a single
//! global queue is sufficient.

use alloc::collections::VecDeque;
use spin::Mutex;

/// Soft cap on queued unread bytes. zsh's longest line of input is well under
/// this; bounding the queue keeps a runaway producer (e.g. paste-storm into
/// the terminal) from holding the kernel heap hostage.
const MAX_QUEUED_BYTES: usize = 64 * 1024;

static USER_STDIN: Mutex<Option<VecDeque<u8>>> = Mutex::new(None);

/// Install an empty queue. Called by `enter_user_mode` before the iretq.
pub fn install() {
    *USER_STDIN.lock() = Some(VecDeque::new());
}

/// Drop the queue. Called after the long-jump returns from ring 3.
pub fn clear() {
    *USER_STDIN.lock() = None;
}

/// True while a user process owns the stdin queue.
pub fn is_active() -> bool {
    USER_STDIN.lock().is_some()
}

/// Append bytes from the producer side. Silently drops when no user process
/// is active, or when the queue is at capacity (the alternative — blocking
/// the producer — would deadlock the keyboard event handler).
pub fn push_bytes(bytes: &[u8]) {
    let mut g = USER_STDIN.lock();
    let Some(buf) = g.as_mut() else { return; };
    let room = MAX_QUEUED_BYTES.saturating_sub(buf.len());
    let take = core::cmp::min(room, bytes.len());
    buf.extend(bytes[..take].iter().copied());
}

/// Drain up to `dst.len()` bytes into `dst`. Returns the number copied; 0
/// when the queue is empty or absent.
pub fn pop_into(dst: &mut [u8]) -> usize {
    let mut g = USER_STDIN.lock();
    let Some(buf) = g.as_mut() else { return 0; };
    let n = core::cmp::min(dst.len(), buf.len());
    for slot in dst.iter_mut().take(n) {
        *slot = buf.pop_front().unwrap();
    }
    n
}

/// Number of bytes currently queued. Test-facing.
#[cfg(feature = "test")]
pub fn queued_len() -> usize {
    USER_STDIN.lock().as_ref().map(|b| b.len()).unwrap_or(0)
}
