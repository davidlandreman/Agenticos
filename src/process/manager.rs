//! Process-manager singleton.
//!
//! Currently the only state it carries is the *active stdin buffer* — a
//! per-(focused-terminal) producer/consumer hook the keyboard driver
//! uses to deliver characters into the in-kernel `read` path used by
//! kernel-side processes (the userland ring-3 stdin queue lives in
//! `src/userland/stdin.rs` separately).
//!
//! The kernel-side shell command registry (`register_command`,
//! `execute_command`, …) was removed when zsh became the default
//! terminal shell; see
//! `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.
//! GUI app launches now go through `sys_gui_launch` →
//! `src/commands/gui_launch_table.rs`.

use spin::Mutex;
use lazy_static::lazy_static;
use crate::lib::arc::Arc;
use crate::stdlib::io::StdinBuffer;

lazy_static! {
    static ref PROCESS_MANAGER: Mutex<ProcessManager> = Mutex::new(ProcessManager::new());
}

pub struct ProcessManager {
    active_stdin_buffer: Option<Arc<Mutex<StdinBuffer>>>,
}

impl ProcessManager {
    const fn new() -> Self {
        Self {
            active_stdin_buffer: None,
        }
    }

    pub fn set_active_stdin(&mut self, buffer: Arc<Mutex<StdinBuffer>>) {
        self.active_stdin_buffer = Some(buffer);
    }

    pub fn clear_active_stdin(&mut self) {
        self.active_stdin_buffer = None;
    }

    pub fn push_keyboard_input(&self, ch: char) {
        crate::debug_trace!("ProcessManager::push_keyboard_input called with '{}'", ch);
        if let Some(ref buffer) = self.active_stdin_buffer {
            crate::debug_trace!("Found active stdin buffer, calling push_char_no_echo");
            buffer.lock().push_char_no_echo(ch);
        } else {
            crate::debug_debug!("No active stdin buffer registered");
        }
    }
}

pub fn set_active_stdin(buffer: Arc<Mutex<StdinBuffer>>) {
    PROCESS_MANAGER.lock().set_active_stdin(buffer);
}

pub fn clear_active_stdin() {
    PROCESS_MANAGER.lock().clear_active_stdin();
}

pub fn push_keyboard_input(ch: char) {
    PROCESS_MANAGER.lock().push_keyboard_input(ch);
}
