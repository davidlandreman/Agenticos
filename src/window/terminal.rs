//! Terminal window support for text output
//!
//! Supports multiple terminal windows with per-terminal output routing.

use spin::Mutex;
use alloc::boxed::Box;
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

/// Register a new terminal for output
pub fn register_terminal(window_id: WindowId) {
    TERMINAL_BUFFERS.lock().entry(window_id).or_insert_with(Vec::new);
}

/// Unregister a terminal
pub fn unregister_terminal(window_id: WindowId) {
    TERMINAL_BUFFERS.lock().remove(&window_id);
}

/// Write text to a specific terminal window
pub fn write_to_terminal_id(terminal_id: WindowId, text: &str) {
    // Add to this terminal's output buffer
    {
        let mut buffers = TERMINAL_BUFFERS.lock();
        if let Some(buffer) = buffers.get_mut(&terminal_id) {
            buffer.push(String::from(text));
        }
    }

    // Force window invalidation to trigger repaint
    with_window_manager(|wm| {
        if let Some(window) = wm.window_registry.get_mut(&terminal_id) {
            window.invalidate();
        }
    });
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

/// Route input from a terminal to its shell
pub fn route_terminal_input(terminal_id: WindowId, line: String) {
    // Route to the shell_process module
    crate::commands::shell::shell_process::send_input(terminal_id, line);
}