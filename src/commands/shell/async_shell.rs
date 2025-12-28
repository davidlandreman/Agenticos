//! Shell instance management
//!
//! This module has been replaced by the process-based shell system.
//! See terminal_factory and shell_process for the new implementation.
//!
//! Legacy functions are kept for backwards compatibility during transition.

use alloc::string::String;
use crate::window::WindowId;

/// Legacy: Initialize shell for a terminal (now done by terminal_factory)
pub fn init_async_shell(_terminal_id: WindowId) {
    // Shell initialization is now handled by spawn_terminal_with_shell
    // The initial terminal's shell is created in kernel.rs
}

/// Legacy: Update shells (now done by process scheduler polling)
pub fn update_async_shell() {
    // Shell updates are now driven by the process scheduler
    // See crate::process::poll_ready_processes()
}

/// Legacy: Handle input (now routed through window focus system)
pub fn handle_shell_input(_line: String) {
    // Input is now routed through the focused terminal's process
    // See terminal.rs handle_enter() which calls send_input_to_process()
}