//! Console output support for the window system

use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;
use crate::window::WindowId;

/// Global console output buffer
static CONSOLE_BUFFER: Mutex<ConsoleBuffer> = Mutex::new(ConsoleBuffer::new());

/// Pending window invalidations.
///
/// This allows code to queue invalidations without holding the WindowManager lock.
/// The WindowManager processes these at the start of each render cycle.
static PENDING_INVALIDATIONS: Mutex<Vec<WindowId>> = Mutex::new(Vec::new());

/// Queue a window invalidation without acquiring the WindowManager lock.
///
/// This is safe to call from anywhere, including during event handling.
/// The invalidation will be processed at the start of the next render cycle.
pub fn queue_invalidation(window_id: WindowId) {
    if let Some(mut pending) = PENDING_INVALIDATIONS.try_lock() {
        if !pending.contains(&window_id) {
            pending.push(window_id);
        }
    }
    // If we can't get the lock, the invalidation will be picked up later
}

/// Take all pending invalidations.
///
/// Returns the list of windows that need invalidation.
pub fn take_pending_invalidations() -> Vec<WindowId> {
    let mut pending = PENDING_INVALIDATIONS.lock();
    core::mem::take(&mut *pending)
}

struct ConsoleBuffer {
    lines: Vec<String>,
    pending_line: String,
}

impl ConsoleBuffer {
    const fn new() -> Self {
        ConsoleBuffer {
            lines: Vec::new(),
            pending_line: String::new(),
        }
    }
}

/// Write a string to the console buffer
/// If a current output terminal is set, routes to that terminal's buffer instead
pub fn write_str(s: &str) {
    // Check if we have a current output terminal set (by the shell)
    if let Some(terminal_id) = crate::window::terminal::get_current_output_terminal() {
        // Route to the specific terminal's buffer
        crate::window::terminal::write_to_terminal_id(terminal_id, s);
        return;
    }

    // Fall back to global console buffer (for early boot output, etc.)
    {
        let mut buffer = CONSOLE_BUFFER.lock();

        for ch in s.chars() {
            if ch == '\n' {
                // Complete the current line
                let line = core::mem::replace(&mut buffer.pending_line, String::new());
                buffer.lines.push(line);
            } else {
                buffer.pending_line.push(ch);
            }
        }
    } // Release the lock before calling window manager

    // Don't try to invalidate the window here - it causes a deadlock when called
    // from within window event handling. The window will be invalidated by the
    // terminal window when it processes the console output during paint.
}

/// Check if there is any pending console output without taking it
pub fn has_output() -> bool {
    let buffer = CONSOLE_BUFFER.lock();
    !buffer.lines.is_empty() || !buffer.pending_line.is_empty()
}

/// Get and clear all pending console output
pub fn take_output() -> (Vec<String>, String) {
    let mut buffer = CONSOLE_BUFFER.lock();

    let lines = core::mem::replace(&mut buffer.lines, Vec::new());
    let pending = core::mem::replace(&mut buffer.pending_line, String::new());

    (lines, pending)
}

/// Restore output that was taken but not processed.
/// This is used when we need to delay processing.
pub fn restore_output(lines: Vec<String>, pending: String) {
    if lines.is_empty() && pending.is_empty() {
        return;
    }

    let mut buffer = CONSOLE_BUFFER.lock();
    // Prepend the lines (they should come before any new output)
    let mut new_lines = lines;
    new_lines.append(&mut buffer.lines);
    buffer.lines = new_lines;

    // Prepend the pending text
    if !pending.is_empty() {
        let new_pending = pending + &buffer.pending_line;
        buffer.pending_line = new_pending;
    }
}

/// Writer implementation for core::fmt
pub struct ConsoleWriter;

impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write_str(s);
        Ok(())
    }
}