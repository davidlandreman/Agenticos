//! Terminal window support for text output

use spin::Mutex;
use super::{WindowId, with_window_manager};

/// Global terminal window ID for print macros
static TERMINAL_WINDOW: Mutex<Option<WindowId>> = Mutex::new(None);

/// Set the terminal window that should receive print output
pub fn set_terminal_window(window_id: WindowId) {
    *TERMINAL_WINDOW.lock() = Some(window_id);
}

/// Get the current terminal window
pub fn get_terminal_window() -> Option<WindowId> {
    *TERMINAL_WINDOW.lock()
}

/// Write text to the terminal window
pub fn write_to_terminal(text: &str) {
    if let Some(terminal_id) = get_terminal_window() {
        with_window_manager(|wm| {
            // We need to get the window and write to it
            // For now, this is a placeholder - we need a better way to access windows
            // TODO: Implement proper terminal writing
        });
    }
}