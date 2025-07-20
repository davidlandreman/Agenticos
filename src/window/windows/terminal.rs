//! Terminal window with input handling capabilities

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;
use crate::window::{
    Window, WindowId, Rect, Event, EventResult, GraphicsDevice,
    keyboard::keycode_to_char, KeyboardEvent,
};
use crate::window::event::KeyCode;
use super::text::TextWindow;

/// Callback type for when input is received
pub type InputCallback = Box<dyn FnMut(String) + Send>;

/// A terminal window that can handle keyboard input
pub struct TerminalWindow {
    /// The underlying text window
    text_window: TextWindow,
    /// Current input buffer
    input_buffer: String,
    /// Callback for when Enter is pressed
    input_callback: Option<InputCallback>,
    /// Command history
    history: Vec<String>,
    /// Current position in history
    history_index: usize,
    /// Starting column of current input line
    input_start_col: usize,
    /// Starting row of current input line
    input_start_row: usize,
}

impl TerminalWindow {
    /// Create a new terminal window
    pub fn new(bounds: Rect) -> Self {
        let mut text_window = TextWindow::new(bounds);
        
        // Write initial text to show the terminal is ready
        text_window.write_str("AgenticOS Terminal Ready\n");
        text_window.write_str("Initializing...\n\n");
        
        // Log cursor position and buffer state
        let (col, row) = text_window.cursor_position();
        crate::debug_info!("Initial text written, cursor at ({}, {})", col, row);
        
        // Force invalidation to ensure initial paint
        text_window.invalidate();
        
        crate::debug_info!("TerminalWindow created with bounds: {:?}", bounds);
        
        TerminalWindow {
            text_window,
            input_buffer: String::new(),
            input_callback: None,
            history: Vec::new(),
            history_index: 0,
            input_start_col: 0,
            input_start_row: 0,
        }
    }
    
    /// Set callback for when Enter is pressed
    pub fn on_input<F>(&mut self, callback: F) 
    where 
        F: FnMut(String) + Send + 'static 
    {
        self.input_callback = Some(Box::new(callback));
    }
    
    /// Write output to the terminal
    pub fn write(&mut self, text: &str) {
        // Save current cursor position as input start
        let (col, row) = self.text_window.cursor_position();
        self.input_start_col = col;
        self.input_start_row = row;
        
        self.text_window.write_str(text);
    }
    
    /// Write a line to the terminal
    pub fn write_line(&mut self, text: &str) {
        self.write(text);
        self.text_window.newline();
    }
    
    /// Clear the current input line
    fn clear_input_line(&mut self) {
        // Move cursor to start of input
        self.text_window.set_cursor_position(self.input_start_col, self.input_start_row);
        
        // Clear from cursor to end of input
        let input_len = self.input_buffer.len();
        for _ in 0..input_len {
            self.text_window.write_char(' ');
        }
        
        // Move cursor back to start
        self.text_window.set_cursor_position(self.input_start_col, self.input_start_row);
    }
    
    /// Replace the current input with new text
    fn replace_input(&mut self, new_input: &str) {
        self.clear_input_line();
        self.input_buffer.clear();
        self.input_buffer.push_str(new_input);
        self.text_window.write_str(&self.input_buffer);
    }
    
    /// Handle a backspace key press
    fn handle_backspace(&mut self) {
        if !self.input_buffer.is_empty() {
            self.input_buffer.pop();
            self.text_window.backspace();
        }
    }
    
    /// Handle enter key press
    fn handle_enter(&mut self) {
        let input = mem::take(&mut self.input_buffer);
        crate::debug_info!("Terminal: Enter pressed, input='{}' (len={})", input, input.len());
        self.text_window.newline();
        
        // Add to history if not empty
        if !input.is_empty() {
            self.history.push(input.clone());
            self.history_index = self.history.len();
        }
        
        // Call callback if set
        if let Some(ref mut callback) = self.input_callback {
            callback(input.clone());
        }
        
        // Also call the global terminal input handler
        crate::window::terminal::handle_terminal_input(input);
        
        // Update input start position for next input
        let (col, row) = self.text_window.cursor_position();
        self.input_start_col = col;
        self.input_start_row = row;
    }
    
    /// Handle up arrow (previous history)
    fn handle_up_arrow(&mut self) {
        if self.history_index > 0 {
            self.history_index -= 1;
            let history_entry = self.history[self.history_index].clone();
            self.replace_input(&history_entry);
        }
    }
    
    /// Handle down arrow (next history)
    fn handle_down_arrow(&mut self) {
        if self.history_index < self.history.len() {
            self.history_index += 1;
            if self.history_index == self.history.len() {
                self.replace_input("");
            } else {
                let history_entry = self.history[self.history_index].clone();
                self.replace_input(&history_entry);
            }
        }
    }
}

// Implement Window trait by delegating to text_window
impl Window for TerminalWindow {
    fn id(&self) -> WindowId {
        self.text_window.id()
    }
    
    fn bounds(&self) -> Rect {
        self.text_window.bounds()
    }
    
    fn visible(&self) -> bool {
        self.text_window.visible()
    }
    
    fn parent(&self) -> Option<WindowId> {
        self.text_window.parent()
    }
    
    fn children(&self) -> &[WindowId] {
        self.text_window.children()
    }
    
    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        crate::debug_trace!("TerminalWindow::paint called");
        
        // Process any pending console output BEFORE painting
        // This ensures output is added to the buffer before we render
        self.text_window.process_console_output();
        
        // Paint the text window
        self.text_window.paint(device);
        
        // After painting, sync our input position with the text window's cursor
        // This is important because the TextWindow may have processed console output
        let (col, row) = self.text_window.cursor_position();
        
        // Only update input position if we're not in the middle of typing
        // (cursor should be after the prompt and any input)
        if self.input_buffer.is_empty() {
            self.input_start_col = col;
            self.input_start_row = row;
            crate::debug_trace!("Updated input position to ({}, {})", col, row);
        }
    }
    
    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                match kbd_event.key_code {
                    KeyCode::Enter => {
                        self.handle_enter();
                        EventResult::Handled
                    }
                    KeyCode::Backspace => {
                        self.handle_backspace();
                        EventResult::Handled
                    }
                    KeyCode::Up => {
                        self.handle_up_arrow();
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        self.handle_down_arrow();
                        EventResult::Handled
                    }
                    _ => {
                        crate::debug_trace!("Terminal handling key: {:?}", kbd_event.key_code);
                        if let Some(ch) = keycode_to_char(kbd_event.key_code, kbd_event.modifiers) {
                            crate::debug_trace!("Converted to character: '{}'", ch);
                            
                            // Log current cursor position
                            let (cur_col, cur_row) = self.text_window.cursor_position();
                            crate::debug_trace!("Current cursor at ({}, {}), input starts at ({}, {})", 
                                cur_col, cur_row, self.input_start_col, self.input_start_row);
                            
                            self.input_buffer.push(ch);
                            
                            // Write the character
                            self.text_window.write_char(ch);
                            
                            EventResult::Handled
                        } else {
                            crate::debug_trace!("No character mapping for key");
                            EventResult::Ignored
                        }
                    }
                }
            }
            _ => self.text_window.handle_event(event),
        }
    }
    
    fn set_focus(&mut self, focused: bool) {
        self.text_window.set_focus(focused);
    }
    
    fn has_focus(&self) -> bool {
        self.text_window.has_focus()
    }
    
    fn can_focus(&self) -> bool {
        self.text_window.can_focus()
    }
    
    fn needs_repaint(&self) -> bool {
        self.text_window.needs_repaint()
    }
    
    fn invalidate(&mut self) {
        self.text_window.invalidate();
    }
}