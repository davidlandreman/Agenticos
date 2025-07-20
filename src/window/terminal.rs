//! Terminal window support for text output

use spin::Mutex;
use alloc::boxed::Box;
use alloc::string::String;
use super::{WindowId, with_window_manager};

/// Global terminal window ID for print macros
static TERMINAL_WINDOW: Mutex<Option<WindowId>> = Mutex::new(None);

/// Input callback type
type InputCallback = Box<dyn FnMut(String) + Send>;

/// Global input callback for the terminal
static INPUT_CALLBACK: Mutex<Option<InputCallback>> = Mutex::new(None);

/// Set the terminal window that should receive print output
pub fn set_terminal_window(window_id: WindowId) {
    *TERMINAL_WINDOW.lock() = Some(window_id);
}

/// Get the current terminal window
pub fn get_terminal_window() -> Option<WindowId> {
    *TERMINAL_WINDOW.lock()
}

/// Set the input callback for the terminal
pub fn set_terminal_input_callback<F>(callback: F) 
where 
    F: FnMut(String) + Send + 'static 
{
    *INPUT_CALLBACK.lock() = Some(Box::new(callback));
}

/// Handle terminal input
pub fn handle_terminal_input(line: String) {
    if let Some(ref mut callback) = *INPUT_CALLBACK.lock() {
        callback(line);
    }
}

/// Write text to the terminal window
pub fn write_to_terminal(text: &str) {
    if let Some(terminal_id) = get_terminal_window() {
        // Write to console buffer
        crate::print!("{}", text);
        
        // Force window invalidation to trigger repaint
        with_window_manager(|wm| {
            if let Some(window) = wm.window_registry.get_mut(&terminal_id) {
                window.invalidate();
            }
        });
        
        // If this text ends without a newline, it's likely a prompt
        // Update the terminal's input position
        if !text.ends_with('\n') {
            with_window_manager(|wm| {
                // This is a bit hacky - we need to update the TerminalWindow's input position
                // after the console buffer is processed. For now, we'll handle this in the 
                // terminal window's paint method
            });
        }
    }
}