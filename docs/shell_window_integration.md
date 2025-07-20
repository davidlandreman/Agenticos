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
- No mouse cursor movement
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
    
    // Route to window manager if available
    if window::is_initialized() {
        window::with_window_manager(|wm| {
            wm.handle_keyboard_scancode(scancode);
        });
    } else {
        // Fall back to current queue-based system
        add_scancode(scancode);
    }
    
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

### Phase 4: Process Output Redirection

#### 4.1 Create Process Output Interface
```rust
// In process/io.rs
pub trait ProcessIO {
    fn write(&mut self, data: &str);
    fn writeln(&mut self, data: &str) {
        self.write(data);
        self.write("\n");
    }
}

pub struct TerminalIO {
    terminal: Arc<Mutex<TerminalWindow>>,
}

impl ProcessIO for TerminalIO {
    fn write(&mut self, data: &str) {
        self.terminal.lock().write(data);
    }
}
```

#### 4.2 Update Commands to Use ProcessIO
```rust
// Example: updating ls command
impl RunnableProcess for LsProcess {
    fn run(&mut self, io: &mut dyn ProcessIO) {
        match list_directory(&self.path) {
            Ok(entries) => {
                for entry in entries {
                    io.writeln(&format!("{}", entry.name));
                }
            }
            Err(e) => {
                io.writeln(&format!("Error: {}", e));
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

## Conclusion

Integrating the shell with the window system requires:
1. Routing keyboard input through the window manager
2. Implementing a proper terminal window with input buffering
3. Making the shell non-blocking and event-driven
4. Updating commands to output through the terminal

This will create a responsive system where the mouse continues to work while typing and commands execute within the window system framework.