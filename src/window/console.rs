//! Console output support for the window system

use spin::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Global console output buffer
static CONSOLE_BUFFER: Mutex<ConsoleBuffer> = Mutex::new(ConsoleBuffer::new());

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
pub fn write_str(s: &str) {
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
}

/// Get and clear all pending console output
pub fn take_output() -> (Vec<String>, String) {
    let mut buffer = CONSOLE_BUFFER.lock();
    
    let lines = core::mem::replace(&mut buffer.lines, Vec::new());
    let pending = buffer.pending_line.clone();
    
    (lines, pending)
}

/// Writer implementation for core::fmt
pub struct ConsoleWriter;

impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write_str(s);
        Ok(())
    }
}