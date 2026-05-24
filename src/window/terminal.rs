//! Terminal window support for text output
//!
//! Supports multiple terminal windows with per-terminal output routing.

use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use super::{WindowId, with_window_manager};

/// Global terminal window ID for print macros (the default/focused terminal)
static TERMINAL_WINDOW: Mutex<Option<WindowId>> = Mutex::new(None);

/// Current output terminal - used for routing println! output from commands
/// This is set by the shell before executing a command
static CURRENT_OUTPUT_TERMINAL: Mutex<Option<WindowId>> = Mutex::new(None);

/// Per-terminal output buffers
static TERMINAL_BUFFERS: Mutex<BTreeMap<WindowId, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Set the terminal window that should receive print output (default terminal)
pub fn set_terminal_window(window_id: WindowId) {
    *TERMINAL_WINDOW.lock() = Some(window_id);
    // Initialize buffer for this terminal
    TERMINAL_BUFFERS.lock().entry(window_id).or_insert_with(Vec::new);
}

/// Get the current default terminal window
pub fn get_terminal_window() -> Option<WindowId> {
    *TERMINAL_WINDOW.lock()
}

/// Set the current output terminal for command execution
/// This should be called by the shell before running a command
pub fn set_current_output_terminal(window_id: WindowId) {
    *CURRENT_OUTPUT_TERMINAL.lock() = Some(window_id);
}

/// Clear the current output terminal
pub fn clear_current_output_terminal() {
    *CURRENT_OUTPUT_TERMINAL.lock() = None;
}

/// Get the current output terminal (for routing println! output)
/// Falls back to the default terminal if not set
pub fn get_current_output_terminal() -> Option<WindowId> {
    let current = *CURRENT_OUTPUT_TERMINAL.lock();
    if current.is_some() {
        current
    } else {
        *TERMINAL_WINDOW.lock()
    }
}

/// Register a new terminal for output AND install its stdin queue so
/// keystrokes typed in this terminal land in their own per-terminal
/// buffer (not the global one). Pre-fix, two terminals shared a
/// single stdin queue and `ls` typed in terminal 2 could be drained
/// by zsh1 — see the multi-terminal stdin learning.
pub fn register_terminal(window_id: WindowId) {
    TERMINAL_BUFFERS.lock().entry(window_id).or_insert_with(Vec::new);
    crate::userland::stdin::install_for_terminal(window_id);
}

/// Unregister a terminal and drop its stdin queue.
pub fn unregister_terminal(window_id: WindowId) {
    TERMINAL_BUFFERS.lock().remove(&window_id);
    crate::userland::stdin::clear_for_terminal(window_id);
}

/// Write text to a specific terminal window.
///
/// **Deadlock-safe**: appends to the per-terminal buffer only; does
/// NOT touch the WINDOW_MANAGER lock. The compositor's
/// [`invalidate_dirty_terminals`] pass picks up the new content and
/// invalidates the window on its own scheduling slice. Pre-fix, this
/// function locked WINDOW_MANAGER to invalidate inline — but ring-3
/// write syscalls run at CPL=0 with `is_in_spawned_process=false`, so
/// the timer ISR doesn't preempt them. If the compositor was
/// preempted mid-`render_frame` (holding WINDOW_MANAGER), a ring-3
/// write would spin on the lock forever.
pub fn write_to_terminal_id(terminal_id: WindowId, text: &str) {
    let mut buffers = TERMINAL_BUFFERS.lock();
    if let Some(buffer) = buffers.get_mut(&terminal_id) {
        buffer.push(String::from(text));
    }
}

/// Write text to the current output terminal (used by print! macro)
/// Routes to the current output terminal if set, otherwise the default terminal
pub fn write_to_terminal(text: &str) {
    // Use current output terminal (set by shell) or fall back to default
    if let Some(terminal_id) = get_current_output_terminal() {
        // Write to this terminal's buffer
        write_to_terminal_id(terminal_id, text);
    }
}

/// Take pending output for a terminal
pub fn take_terminal_output(terminal_id: WindowId) -> Vec<String> {
    let mut buffers = TERMINAL_BUFFERS.lock();
    if let Some(buffer) = buffers.get_mut(&terminal_id) {
        core::mem::take(buffer)
    } else {
        Vec::new()
    }
}

/// Check if a terminal has pending output
pub fn has_terminal_output(terminal_id: WindowId) -> bool {
    let buffers = TERMINAL_BUFFERS.lock();
    buffers.get(&terminal_id).map_or(false, |b| !b.is_empty())
}

/// Compositor-side: invalidate every terminal window with pending
/// buffered output. Called from
/// `crate::window::compositor::run` each iteration; pairs with
/// [`write_to_terminal_id`]'s deadlock-safe "buffer only, defer
/// invalidation" contract. Snapshots dirty IDs under the
/// `TERMINAL_BUFFERS` lock, then drops it before taking
/// WINDOW_MANAGER — no nested locks.
pub fn invalidate_dirty_terminals() {
    let dirty: Vec<WindowId> = {
        let buffers = TERMINAL_BUFFERS.lock();
        buffers
            .iter()
            .filter(|(_, b)| !b.is_empty())
            .map(|(id, _)| *id)
            .collect()
    };
    if dirty.is_empty() {
        return;
    }
    with_window_manager(|wm| {
        for id in dirty {
            if let Some(window) = wm.window_registry.get_mut(&id) {
                window.invalidate();
            }
        }
    });
}

