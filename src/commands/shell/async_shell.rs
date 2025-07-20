//! Asynchronous shell that integrates with the window system

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use spin::Mutex;
use crate::window::WindowId;
use crate::process::{execute_command, RunnableProcess};
use crate::{debug_info, debug_trace};

/// Shell state for non-blocking operation
pub struct AsyncShell {
    /// The terminal window ID
    terminal_id: WindowId,
    /// Current line being processed
    current_line: Option<String>,
    /// Currently running command
    running_command: Option<Box<dyn RunnableProcess>>,
    /// Working directory
    working_directory: String,
}

impl AsyncShell {
    /// Create a new async shell connected to a terminal window
    pub fn new(terminal_id: WindowId) -> Self {
        let mut shell = AsyncShell {
            terminal_id,
            current_line: None,
            running_command: None,
            working_directory: String::from("/"),
        };
        
        // Input callback is set up globally via terminal module
        
        // Write initial message and prompt
        shell.write_to_terminal("AgenticOS Terminal\n");
        shell.write_to_terminal("Type 'help' for commands.\n\n");
        shell.write_prompt();
        
        shell
    }
    
    /// Write the shell prompt
    fn write_prompt(&self) {
        debug_trace!("Writing shell prompt");
        self.write_to_terminal("AgenticOS> ");
    }
    
    /// Write text to the terminal
    fn write_to_terminal(&self, text: &str) {
        crate::window::terminal::write_to_terminal(text);
    }
    
    /// Process input from the terminal
    pub fn on_input(&mut self, line: String) {
        debug_trace!("Shell received input: '{}' (len={})", line, line.len());
        self.current_line = Some(line);
    }
    
    /// Update the shell state - call this from the main loop
    pub fn update(&mut self) {
        // Process any pending input
        if let Some(line) = self.current_line.take() {
            debug_trace!("Shell update: processing line '{}'", line);
            self.process_command(line);
        }
        
        // Note: running_command is always None because commands run synchronously
        // Prompts are written by process_command() and execute_external_command()
        // so we don't need to write them here
    }
    
    /// Process a command line
    fn process_command(&mut self, line: String) {
        let trimmed = line.trim();
        
        if trimmed.is_empty() {
            self.write_prompt();
            return;
        }
        
        // Handle built-in commands
        match trimmed {
            "exit" | "quit" => {
                self.write_to_terminal("Goodbye!\n");
                // In a real system, we'd exit the shell here
                // For now, just print the prompt again
                self.write_prompt();
            }
            "clear" | "cls" => {
                // Clear screen command - would clear the terminal
                // For now just add some newlines
                self.write_to_terminal("\n\n\n\n\n");
                self.write_prompt();
            }
            _ => {
                // Try to execute as a command
                self.execute_external_command(trimmed);
            }
        }
    }
    
    /// Execute an external command
    fn execute_external_command(&mut self, command_line: &str) {
        // Try to execute the command
        match execute_command(command_line) {
            Ok(()) => {
                // Command completed successfully
                self.write_prompt();
            }
            Err(e) => {
                self.write_to_terminal(&format!("Error: {}\n", e));
                self.write_prompt();
            }
        }
    }
}

/// Global async shell instance
static ASYNC_SHELL: Mutex<Option<AsyncShell>> = Mutex::new(None);

/// Initialize the async shell with a terminal window
pub fn init_async_shell(terminal_id: WindowId) {
    let shell = AsyncShell::new(terminal_id);
    *ASYNC_SHELL.lock() = Some(shell);
    
    // Input is routed via the global terminal callback
}

/// Update the async shell - call from main loop
pub fn update_async_shell() {
    if let Some(ref mut shell) = *ASYNC_SHELL.lock() {
        shell.update();
    }
}

/// Handle input from the terminal
pub fn handle_shell_input(line: String) {
    if let Some(ref mut shell) = *ASYNC_SHELL.lock() {
        shell.on_input(line);
    }
}