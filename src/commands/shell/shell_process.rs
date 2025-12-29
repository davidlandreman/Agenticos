//! Per-terminal shell process for multitasking
//!
//! This module provides a shell implementation that runs as a cooperative process
//! associated with a specific terminal window. It processes input when available
//! and yields control when waiting for more input.

use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::window::WindowId;
use crate::process::ProcessId;
use crate::{print, println};

/// Shell instance state
pub struct ShellInstance {
    /// The terminal window this shell is attached to
    pub terminal_id: WindowId,
    /// Process ID for this shell
    pub pid: ProcessId,
    /// Pending input lines to process
    input_queue: Vec<String>,
    /// Whether the shell is initialized (welcome message shown)
    initialized: bool,
    /// Whether the shell should exit
    should_exit: bool,
}

impl ShellInstance {
    /// Create a new shell instance
    pub fn new(terminal_id: WindowId, pid: ProcessId) -> Self {
        ShellInstance {
            terminal_id,
            pid,
            input_queue: Vec::new(),
            initialized: false,
            should_exit: false,
        }
    }

    /// Queue input for processing
    pub fn push_input(&mut self, line: String) {
        self.input_queue.push(line);
    }

    /// Check if there's pending work
    pub fn has_pending_work(&self) -> bool {
        !self.initialized || !self.input_queue.is_empty()
    }

    /// Process one unit of work (poll-based)
    /// Returns true if still running, false if shell should exit
    pub fn poll(&mut self) -> bool {
        if self.should_exit {
            return false;
        }

        // Initialize on first poll
        if !self.initialized {
            self.write_output("\nAgenticOS Terminal\n");
            self.write_output("Type 'help' for commands.\n\n");
            self.write_prompt();
            self.initialized = true;
            return true;
        }

        // Process one input line if available
        if let Some(line) = self.input_queue.pop() {
            self.process_command(&line);
        }

        !self.should_exit
    }

    /// Write output to this shell's terminal
    fn write_output(&self, text: &str) {
        // Use the terminal-specific output
        crate::window::terminal::write_to_terminal_id(self.terminal_id, text);
    }

    /// Write the shell prompt
    fn write_prompt(&self) {
        self.write_output("AgenticOS> ");
    }

    /// Process a command
    fn process_command(&mut self, line: &str) {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            self.write_prompt();
            return;
        }

        match trimmed {
            "help" => {
                self.write_output("Available commands:\n");
                self.write_output("  help     - Show this help message\n");
                self.write_output("  clear    - Clear the screen\n");
                self.write_output("  cmd      - Spawn a new terminal window\n");
                self.write_output("  exit     - Close this terminal\n");
                self.write_output("\nRegistered programs:\n");
                for cmd in crate::process::list_commands() {
                    self.write_output(&format!("  {}\n", cmd));
                }
                self.write_prompt();
            }
            "exit" | "quit" => {
                self.write_output("Goodbye!\n");
                self.should_exit = true;
                // Terminal will be closed by the process manager
            }
            "clear" | "cls" => {
                self.write_output("\n\n\n\n\n");
                self.write_prompt();
            }
            "cmd" => {
                match crate::window::terminal_factory::spawn_terminal_with_shell() {
                    Ok(instance) => {
                        self.write_output(&format!("Spawned terminal {}\n", instance.number));
                    }
                    Err(e) => {
                        self.write_output(&format!("Failed to spawn terminal: {}\n", e));
                    }
                }
                self.write_prompt();
            }
            _ => {
                // Try to execute as a registered command
                // The command will be spawned as a process and run by the scheduler
                // Output routing is handled by the spawned process itself
                match crate::process::execute_command(trimmed, Some(self.terminal_id)) {
                    Ok(()) => {
                        // Command was spawned successfully
                        // Note: Output may appear after the prompt since it runs asynchronously
                        self.write_prompt();
                    }
                    Err(e) => {
                        self.write_output(&format!("Error: {}\n", e));
                        self.write_prompt();
                    }
                }
            }
        }
    }
}

/// Global registry of shell instances
static SHELL_REGISTRY: Mutex<BTreeMap<WindowId, ShellInstance>> = Mutex::new(BTreeMap::new());

/// Register a new shell for a terminal
pub fn register_shell(terminal_id: WindowId, pid: ProcessId) {
    let shell = ShellInstance::new(terminal_id, pid);
    SHELL_REGISTRY.lock().insert(terminal_id, shell);
    crate::debug_info!("Registered shell for terminal {:?} with PID {:?}", terminal_id, pid);
}

/// Unregister a shell
pub fn unregister_shell(terminal_id: WindowId) {
    SHELL_REGISTRY.lock().remove(&terminal_id);
    crate::debug_info!("Unregistered shell for terminal {:?}", terminal_id);
}

/// Send input to a shell
pub fn send_input(terminal_id: WindowId, line: String) {
    let pid_opt = {
        let mut registry = SHELL_REGISTRY.lock();
        if let Some(shell) = registry.get_mut(&terminal_id) {
            shell.push_input(line);
            Some(shell.pid)
        } else {
            None
        }
    };

    // Signal the shell's process to wake up if it's sleeping
    if let Some(pid) = pid_opt {
        crate::process::signal_process(pid, crate::process::WakeEvents::INPUT);
    }
}

/// Poll all shells - returns list of terminals whose shells have exited
pub fn poll_all_shells() -> Vec<WindowId> {
    // Collect terminals that need polling WITHOUT holding the lock during poll
    // This prevents deadlock when a shell command (like "cmd") tries to register a new shell
    let terminals_to_poll: Vec<WindowId> = {
        let registry = SHELL_REGISTRY.lock();
        registry.iter()
            .filter(|(_, shell)| shell.has_pending_work())
            .map(|(id, _)| *id)
            .collect()
    };

    let mut exited = Vec::new();

    // Poll each shell by temporarily removing it from the registry
    // This allows the shell to spawn new terminals without deadlock
    for terminal_id in terminals_to_poll {
        // Take the shell out of the registry
        let shell_opt = {
            let mut registry = SHELL_REGISTRY.lock();
            registry.remove(&terminal_id)
        };

        if let Some(mut shell) = shell_opt {
            // Poll WITHOUT holding the lock - this is the key!
            let still_running = shell.poll();

            if still_running {
                // Put the shell back in the registry
                let mut registry = SHELL_REGISTRY.lock();
                registry.insert(terminal_id, shell);
            } else {
                exited.push(terminal_id);
            }
        }
    }

    exited
}

/// Check if a terminal has a shell
pub fn has_shell(terminal_id: WindowId) -> bool {
    SHELL_REGISTRY.lock().contains_key(&terminal_id)
}

/// Legacy function for compatibility - runs a shell (now uses poll-based approach)
pub fn run_shell(_terminal_id: WindowId) {
    // This is no longer used - shells are registered and polled
    // Kept for API compatibility with terminal_factory
}
