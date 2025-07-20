# Shell Window Integration Plan

## Overview

The AgenticOS shell currently blocks the window system's render loop, preventing proper window updates and mouse interaction. This document outlines the steps needed to integrate the shell with the window system to create a fully interactive terminal experience.

## Current Problem

The shell (`ShellProcess`) runs a blocking loop that:
1. Prints a prompt
2. Waits for keyboard input via `get_line()`
3. Processes the command
4. Repeats

This blocks the kernel's idle loop, preventing `window::render_frame()` from being called, which means:
- No window updates
- No visual feedback

## Goal Architecture

```
┌─────────────────────────────────────────┐
│          Window Manager                 │
│  ┌─────────────────────────────────┐   │
│  │      Terminal Window             │   │
│  │  ┌─────────────────────────┐    │   │
│  │  │   Shell Process         │    │   │
│  │  │  - Non-blocking         │    │   │
│  │  │  - Event-driven         │    │   │
│  │  └─────────────────────────┘    │   │
│  │                                  │   │
│  │  Input Buffer ← Keyboard Events │   │
│  │  Output → Window Text Buffer    │   │
│  └─────────────────────────────────┘   │
└─────────────────────────────────────────┘
         ↑                    ↑
    Keyboard IRQ         Render Loop
```

### Standard I/O Flow

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│   Application   │     │     Process     │     │    Terminal     │
│                 │     │   I/O Context   │     │     Window      │
├─────────────────┤     ├─────────────────┤     ├─────────────────┤
│                 │     │                 │     │                 │
│ println!("Hi")  │────>│ stdout.write()  │────>│ text_buffer.add │
│                 │     │                 │     │                 │
│ stdin.read_line│<────│ stdin.read()    │<────│ input_buffer    │
│                 │     │                 │     │                 │
└─────────────────┘     └─────────────────┘     └─────────────────┘
```

### I/O Context Lifecycle

1. **Window System Active** (loads early in boot):
   - stdin: Reads from terminal window's input buffer
   - stdout/stderr: Write to terminal window's text buffer
   - print! macros automatically route through terminal
   - No need for direct framebuffer fallback since window system is always available

2. **Future: Multiple Terminals**:
   - Each terminal has its own I/O context
   - Processes inherit I/O context from parent
   - Can redirect to files or pipes

## Implementation Steps

### Phase 1: Keyboard Event Integration

#### 1.1 Update Window Manager to Handle Keyboard Input
```rust
// In window/manager.rs
impl WindowManager {
    /// Process keyboard interrupt data
    pub fn handle_keyboard_scancode(&mut self, scancode: u8) {
        // Convert scancode to KeyCode
        if let Some(key_code) = scancode_to_keycode(scancode) {
            let event = KeyboardEvent {
                key_code,
                pressed: !is_break_code(scancode),
                modifiers: self.current_modifiers,
            };
            
            self.route_keyboard_event(event);
        }
    }
}
```

#### 1.2 Create Scancode to KeyCode Converter
```rust
// In window/keyboard.rs
pub fn scancode_to_keycode(scancode: u8) -> Option<KeyCode> {
    match scancode & 0x7F { // Remove break bit
        0x01 => Some(KeyCode::Escape),
        0x02 => Some(KeyCode::Key1),
        // ... map all scancodes
        0x1C => Some(KeyCode::Enter),
        0x39 => Some(KeyCode::Space),
        _ => None,
    }
}
```

#### 1.3 Update Keyboard ISR to Route Through Window Manager
```rust
// In arch/x86_64/interrupts.rs
extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let scancode = unsafe { Port::new(0x60).read() };
    
    // Always route to window manager (loads early in boot)
    window::with_window_manager(|wm| {
        wm.handle_keyboard_scancode(scancode);
    });
    
    unsafe {
        PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}
```

### Phase 2: Terminal Window Implementation

#### 2.1 Create Proper Terminal Window
```rust
// In window/windows/terminal.rs
pub struct TerminalWindow {
    base: WindowBase,
    text_window: TextWindow,
    input_buffer: String,
    input_callback: Option<Box<dyn FnMut(String)>>,
    history: Vec<String>,
    history_index: usize,
}

impl TerminalWindow {
    pub fn new(bounds: Rect) -> Self {
        let mut base = WindowBase::new(bounds);
        base.set_can_focus(true);
        
        TerminalWindow {
            base,
            text_window: TextWindow::new(bounds),
            input_buffer: String::new(),
            input_callback: None,
            history: Vec::new(),
            history_index: 0,
        }
    }
    
    /// Set callback for when Enter is pressed
    pub fn on_input<F>(&mut self, callback: F) 
    where 
        F: FnMut(String) + 'static 
    {
        self.input_callback = Some(Box::new(callback));
    }
    
    /// Write output to the terminal
    pub fn write(&mut self, text: &str) {
        self.text_window.write_str(text);
    }
}
```

#### 2.2 Handle Keyboard Events in Terminal
```rust
impl Window for TerminalWindow {
    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                match kbd_event.key_code {
                    KeyCode::Enter => {
                        let input = mem::take(&mut self.input_buffer);
                        self.text_window.newline();
                        
                        // Add to history
                        if !input.is_empty() {
                            self.history.push(input.clone());
                            self.history_index = self.history.len();
                        }
                        
                        // Call callback if set
                        if let Some(ref mut callback) = self.input_callback {
                            callback(input);
                        }
                        
                        EventResult::Handled
                    }
                    KeyCode::Backspace => {
                        if !self.input_buffer.is_empty() {
                            self.input_buffer.pop();
                            self.text_window.backspace();
                        }
                        EventResult::Handled
                    }
                    KeyCode::Up => {
                        // Handle history navigation
                        if self.history_index > 0 {
                            self.history_index -= 1;
                            self.replace_input(&self.history[self.history_index].clone());
                        }
                        EventResult::Handled
                    }
                    // Handle other special keys...
                    _ => {
                        if let Some(ch) = keycode_to_char(kbd_event.key_code, kbd_event.modifiers) {
                            self.input_buffer.push(ch);
                            self.text_window.write_char(ch);
                            EventResult::Handled
                        } else {
                            EventResult::Ignored
                        }
                    }
                }
            }
            _ => EventResult::Propagate,
        }
    }
}
```

### Phase 3: Non-Blocking Shell

#### 3.1 Create Async Shell Interface
```rust
// In commands/shell/async_shell.rs
pub struct AsyncShell {
    terminal: Arc<Mutex<TerminalWindow>>,
    current_line: Option<String>,
    running_command: Option<Box<dyn RunnableProcess>>,
}

impl AsyncShell {
    pub fn new(terminal: Arc<Mutex<TerminalWindow>>) -> Self {
        let shell = AsyncShell {
            terminal: terminal.clone(),
            current_line: None,
            running_command: None,
        };
        
        // Set up input callback
        terminal.lock().on_input(|line| {
            // Store line for processing in update()
            self.current_line = Some(line);
        });
        
        // Print initial prompt
        terminal.lock().write("AgenticOS> ");
        
        shell
    }
    
    /// Called from main loop to process shell state
    pub fn update(&mut self) {
        // Process any pending input
        if let Some(line) = self.current_line.take() {
            self.process_command(line);
        }
        
        // Check if running command is complete
        if let Some(ref mut cmd) = self.running_command {
            if cmd.is_complete() {
                self.running_command = None;
                self.terminal.lock().write("\nAgenticOS> ");
            }
        }
    }
}
```

#### 3.2 Update Kernel Main Loop
```rust
// In kernel.rs
pub fn run() -> ! {
    // ... initialization ...
    
    // Create terminal window
    let terminal = window::create_terminal_window();
    
    // Create async shell
    let mut shell = AsyncShell::new(terminal);
    
    // Main kernel loop
    loop {
        // Update shell state
        shell.update();
        
        // Render windows
        window::render_frame();
        
        // Wait for interrupt
        x86_64::instructions::hlt();
    }
}
```

### Phase 4: Standard I/O Integration

#### 4.1 Design Philosophy
The window system needs to provide stdin/stdout/stderr abstractions that:
- Work with existing print macros (`println!`, `print!`)
- Support process I/O redirection
- Allow multiple terminals to have independent I/O streams
- Maintain compatibility with early boot (before window system)

#### 4.2 Standard Output Architecture
```rust
// In process/io.rs
pub trait Write {
    fn write_str(&mut self, s: &str) -> Result<(), Error>;
}

pub struct Stdout {
    target: StdoutTarget,
}

enum StdoutTarget {
    /// Window system active - terminal window
    Terminal(WindowId),
    /// Redirected to file (future)
    File(FileHandle),
    /// Piped to another process (future)
    Pipe(PipeHandle),
}

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> Result<(), Error> {
        match &self.target {
            StdoutTarget::Terminal(window_id) => {
                // Route through window system
                window::write_to_terminal(*window_id, s);
            }
            // Future: File and Pipe implementations
        }
        Ok(())
    }
}
```

#### 4.3 Standard Input Architecture
```rust
pub struct Stdin {
    source: StdinSource,
}

enum StdinSource {
    /// Window system active - terminal window input
    Terminal(WindowId),
    /// Redirected from file (future)
    File(FileHandle),
    /// Piped from another process (future)
    Pipe(PipeHandle),
}

impl Stdin {
    /// Read a line (blocking in current implementation)
    pub fn read_line(&mut self) -> Result<String, Error> {
        match &self.source {
            StdinSource::Terminal(window_id) => {
                // Get input from terminal window
                window::read_line_from_terminal(*window_id)
            }
            // Future: File and Pipe implementations
        }
    }
    
    /// Future: Non-blocking read
    pub fn try_read_line(&mut self) -> Result<Option<String>, Error> {
        // Non-blocking implementation for event-driven shell
    }
}
```

#### 4.4 Process I/O Context
```rust
// In process/mod.rs
pub struct IoContext {
    pub stdin: Stdin,
    pub stdout: Stdout,
    pub stderr: Stderr,
}

impl IoContext {
    /// Create I/O context for a terminal window
    pub fn terminal(window_id: WindowId) -> Self {
        IoContext {
            stdin: Stdin { source: StdinSource::Terminal(window_id) },
            stdout: Stdout { target: StdoutTarget::Terminal(window_id) },
            stderr: Stderr { target: StderrTarget::Terminal(window_id) },
        }
    }
}
```

#### 4.5 Update Print Macros
```rust
// In lib/io.rs or similar
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        // Always use window system since it loads early
        if let Some(io) = $crate::process::current_io_context() {
            use core::fmt::Write;
            let _ = write!(io.stdout, $($arg)*);
        }
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
```

#### 4.6 Terminal Window I/O Integration
```rust
// In window/windows/terminal.rs
impl TerminalWindow {
    /// Read a line from the terminal (blocks until Enter)
    pub fn read_line(&mut self) -> String {
        // Set up a future/promise for the result
        let (sender, receiver) = channel();
        
        self.input_callback = Some(Box::new(move |line| {
            sender.send(line);
        }));
        
        // This is still blocking, but happens in terminal context
        receiver.recv()
    }
    
    /// Try to read a line (non-blocking)
    pub fn try_read_line(&mut self) -> Option<String> {
        self.pending_input.take()
    }
    
    /// Write to terminal output
    pub fn write(&mut self, text: &str) {
        self.text_window.write_str(text);
        // Trigger repaint
        self.base.invalidate();
    }
}
```

#### 4.7 Shell Integration with I/O Context
```rust
// In commands/shell/mod.rs
impl ShellProcess {
    pub fn new_with_io(io: IoContext) -> Self {
        ShellProcess {
            base: BaseProcess::new("shell"),
            io_context: io,
            // ... other fields
        }
    }
    
    pub fn run(&mut self) {
        loop {
            // Use stdout for prompt
            write!(self.io_context.stdout, "AgenticOS> ").unwrap();
            
            // Use stdin for input
            match self.io_context.stdin.read_line() {
                Ok(line) => {
                    self.execute_command(line);
                }
                Err(e) => {
                    writeln!(self.io_context.stderr, "Error reading input: {}", e).unwrap();
                }
            }
        }
    }
}
```

#### 4.8 Command I/O Redirection
```rust
// Update RunnableProcess trait
pub trait RunnableProcess: Process + HasBaseProcess {
    fn run(&mut self, io: &mut IoContext);
    
    // Backwards compatibility wrapper
    fn run_legacy(&mut self) {
        // Since window system is always available, use default terminal
        let mut io = IoContext::terminal(WindowId::default());
        self.run(&mut io);
    }
}

// Example: ls command with I/O context
impl RunnableProcess for LsProcess {
    fn run(&mut self, io: &mut IoContext) {
        match list_directory(&self.path) {
            Ok(entries) => {
                for entry in entries {
                    writeln!(io.stdout, "{}", entry.name).unwrap();
                }
            }
            Err(e) => {
                writeln!(io.stderr, "ls: {}: {}", self.path, e).unwrap();
            }
        }
    }
}
```

## Technical Challenges

### 1. Scancode to Character Mapping
- Need full scancode set 2 mapping
- Handle shift, caps lock, num lock states
- Support for international keyboards

### 2. Command Execution
- Commands currently run synchronously
- May need to implement command timeouts
- Consider future async command support

### 3. Control Sequences
- Ctrl+C for interrupt
- Ctrl+D for EOF
- Ctrl+L for clear screen
- Arrow keys for history/editing

### 4. Terminal Emulation
- ANSI escape sequence support
- Cursor positioning
- Colors and attributes
- Screen clearing/scrolling

## Testing Plan

1. **Keyboard Input Test**
   - Verify all keys produce correct characters
   - Test modifier combinations
   - Verify special keys (arrows, function keys)

2. **Shell Functionality Test**
   - Run all existing commands
   - Test command history
   - Verify output appears correctly
   - Test long-running commands

3. **Integration Test**
   - Mouse continues to work during typing
   - Multiple terminals (future)
   - Copy/paste support (future)

## Future Enhancements

1. **Multiple Terminals**
   - Alt+F1, Alt+F2 switching
   - Split screen terminals
   - Tabbed interface

2. **Advanced Terminal Features**
   - Scrollback buffer
   - Search in output
   - Copy/paste with mouse
   - Configurable colors/fonts

3. **Process Management**
   - Background processes
   - Job control
   - Process pipes

## Implementation Priority

1. **High Priority** (Required for basic functionality)
   - Keyboard event routing to window manager
   - Basic terminal window with input handling
   - Non-blocking shell main loop
   - Character input and display

2. **Medium Priority** (Improves usability)
   - Command history
   - Backspace/delete handling
   - Arrow key navigation
   - Ctrl+C interrupt

3. **Low Priority** (Nice to have)
   - ANSI escape sequences
   - Multiple terminals
   - Advanced editing (Ctrl+A, Ctrl+E, etc.)
   - Scrollback buffer

## Benefits of the I/O Context Model

### 1. Unified Interface
- All processes use the same stdin/stdout/stderr abstractions
- Print macros work identically in early boot and window system
- Easy transition from direct framebuffer to window-based output

### 2. Flexibility
- Can redirect output to files: `ls > files.txt`
- Can pipe between processes: `ls | grep foo`
- Can have multiple terminals with independent I/O

### 3. Backwards Compatibility
- Existing commands continue to work with legacy `run()` method
- All I/O is routed through the window system
- No need for direct framebuffer fallback

### 4. Future Extensibility
- Easy to add network streams (telnet/ssh)
- Can implement pseudo-terminals (pty)
- Ready for user-space processes with proper I/O isolation

## Example: Complete I/O Flow

```rust
// 1. User types "ls" and presses Enter in terminal window
TerminalWindow::handle_event(KeyCode::Enter)
  → input_callback("ls")
  → AsyncShell receives "ls"

// 2. Shell creates ls process with terminal I/O
let io = IoContext::terminal(terminal_window_id);
let mut ls = LsProcess::new_with_io(io);

// 3. ls writes output
writeln!(io.stdout, "file1.txt")
  → Stdout::write_str("file1.txt\n")
  → window::write_to_terminal(window_id, "file1.txt\n")
  → TerminalWindow::write("file1.txt\n")
  → TextWindow renders text

// 4. Window manager renders frame
window::render_frame()
  → WindowManager::render()
  → TerminalWindow::paint()
  → User sees "file1.txt" on screen
```

## Conclusion

Integrating the shell with the window system requires:
1. Routing keyboard input through the window manager
2. Implementing a proper terminal window with input buffering
3. Making the shell non-blocking and event-driven
4. Creating a proper I/O context system for stdin/stdout/stderr
5. Updating commands to use the I/O context

This will create a responsive system where:
- The mouse continues to work while typing
- Commands execute within the window system framework
- Output is properly routed to the correct terminal
- Future features like pipes and redirection are possible