use spin::Mutex;
use lazy_static::lazy_static;
use crate::lib::arc::Arc;
use crate::stdlib::io::StdinBuffer;
use crate::process::process::RunnableProcess;
use crate::window::WindowId;
use alloc::{vec::Vec, string::String, boxed::Box, collections::BTreeMap, format};

lazy_static! {
    static ref PROCESS_MANAGER: Mutex<ProcessManager> = Mutex::new(ProcessManager::new());
}

/// Command factory function type - creates a new instance of a command process
pub type CommandFactory = fn(args: Vec<String>) -> Box<dyn RunnableProcess>;

/// Result type for process execution
pub type ProcessResult = Result<(), String>;

pub struct ProcessManager {
    active_stdin_buffer: Option<Arc<Mutex<StdinBuffer>>>,
    command_registry: BTreeMap<String, CommandFactory>,
}

impl ProcessManager {
    const fn new() -> Self {
        Self {
            active_stdin_buffer: None,
            command_registry: BTreeMap::new(),
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
    
    /// Register a command with the process manager
    pub fn register_command(&mut self, name: &str, factory: CommandFactory) {
        self.command_registry.insert(String::from(name), factory);
        crate::debug_info!("Registered command: {}", name);
    }
    
    /// Execute a command by name with arguments
    ///
    /// Currently runs commands synchronously. The terminal_id is used to route
    /// output to the correct terminal.
    ///
    /// NOTE: Process spawning infrastructure exists (spawn_process, scheduler)
    /// but context switching back to kernel loop isn't fully integrated yet.
    /// Commands run synchronously for now until that's resolved.
    pub fn execute_command(&self, command_line: &str, terminal_id: Option<WindowId>) -> ProcessResult {
        let parts: Vec<&str> = command_line.trim().split_whitespace().collect();
        if parts.is_empty() {
            return Ok(()); // Empty command, do nothing
        }

        let command_name = parts[0];
        let args: Vec<String> = parts[1..].iter().map(|s| String::from(*s)).collect();

        if let Some(factory) = self.command_registry.get(command_name) {
            crate::debug_info!("Executing command: {} with {} args", command_name, args.len());

            // Set up output routing to the correct terminal
            if let Some(tid) = terminal_id {
                crate::window::terminal::set_current_output_terminal(tid);
            }

            // Run the command synchronously
            let mut process = factory(args);
            process.run();

            // Clear output routing
            crate::window::terminal::clear_current_output_terminal();

            Ok(())
        } else {
            Err(format!("Unknown command: {}", command_name))
        }
    }

    /// Execute a command synchronously (blocking) - same as execute_command for now
    pub fn execute_command_sync(&self, command_line: &str) -> ProcessResult {
        self.execute_command(command_line, None)
    }
    
    /// Get list of registered commands
    pub fn list_commands(&self) -> Vec<String> {
        self.command_registry.keys().cloned().collect()
    }
    
    /// Check if a command is registered
    pub fn has_command(&self, name: &str) -> bool {
        self.command_registry.contains_key(name)
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

/// Register a command globally
pub fn register_command(name: &str, factory: CommandFactory) {
    PROCESS_MANAGER.lock().register_command(name, factory);
}

/// Execute a command globally (spawns as a process)
///
/// Commands are spawned as separate processes and run by the scheduler.
pub fn execute_command(command_line: &str, terminal_id: Option<WindowId>) -> ProcessResult {
    PROCESS_MANAGER.lock().execute_command(command_line, terminal_id)
}

/// Execute a command synchronously (blocking)
pub fn execute_command_sync(command_line: &str) -> ProcessResult {
    PROCESS_MANAGER.lock().execute_command_sync(command_line)
}

/// Get list of all registered commands
pub fn list_commands() -> Vec<String> {
    PROCESS_MANAGER.lock().list_commands()
}

/// Check if a command is registered
pub fn has_command(name: &str) -> bool {
    PROCESS_MANAGER.lock().has_command(name)
}