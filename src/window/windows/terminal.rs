//! Terminal window with input handling capabilities

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;
use crate::window::{
    Window, Rect, Event, EventResult, GraphicsDevice,
    keyboard::keycode_to_char,
};
use crate::window::event::KeyCode;
use super::base::WindowBase;
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
    /// Create a new terminal window with a specific ID
    pub fn new_with_id(id: crate::window::WindowId, bounds: Rect) -> Self {
        let mut text_window = TextWindow::new_with_id(id, bounds);

        // Write initial text to show the terminal is ready
        text_window.write_str("AgenticOS Terminal Ready\n");
        text_window.write_str("Initializing...\n\n");

        // Log cursor position and buffer state
        let (col, row) = text_window.cursor_position();
        crate::debug_info!("Initial text written, cursor at ({}, {})", col, row);

        // Force invalidation to ensure initial paint
        text_window.invalidate();

        crate::debug_info!("TerminalWindow created with id={:?}, bounds: {:?}", id, bounds);

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

    /// Create a new terminal window (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(crate::window::WindowId::new(), bounds)
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
    
    /// Process any pending console output
    pub fn process_output(&mut self) {
        self.text_window.process_console_output();
    }

    /// Process per-terminal output buffer
    fn process_terminal_output(&mut self) {
        let terminal_id = self.text_window.id();
        let outputs = crate::window::terminal::take_terminal_output(terminal_id);

        for output in outputs {
            for ch in output.chars() {
                if ch == '\n' {
                    self.text_window.newline();
                } else {
                    self.text_window.write_char(ch);
                }
            }
        }

        // Update input start position after output
        let (col, row) = self.text_window.cursor_position();
        self.input_start_col = col;
        self.input_start_row = row;
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

        // Phase-1 stdin routing: if a ring-3 user process is currently
        // running, deliver the line + '\n' into its stdin queue rather
        // than back to the in-kernel shell. The shell is parked while a
        // user app runs (`command_running` is true on its `ShellInstance`),
        // so a line routed there would just queue up and be re-executed
        // as a shell command after the app exits — clearly wrong.
        if crate::userland::stdin::is_active() {
            crate::userland::stdin::push_bytes(input.as_bytes());
            crate::userland::stdin::push_bytes(b"\n");
        } else {
            // Route input to this terminal's shell
            let terminal_id = self.text_window.id();
            crate::window::terminal::route_terminal_input(terminal_id, input);
        }

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
    fn base(&self) -> &WindowBase {
        self.text_window.base()
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        self.text_window.base_mut()
    }

    // TextWindow's custom set_bounds reallocates its grid buffer; route
    // through it instead of touching WindowBase directly.
    fn set_bounds(&mut self, bounds: Rect) {
        self.text_window.set_bounds(bounds);
    }

    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.text_window.set_bounds_no_invalidate(bounds);
    }

    /// Forward TextWindow's narrow per-cell hint so the compositor's
    /// dirty-rect tracker only marks the changed cells (plus the cursor
    /// cell), not the entire terminal-content bounds. Without this, the
    /// desktop's per-region wallpaper blit covers the whole TextWindow
    /// area and TextWindow's incremental paint can't restore it — typed
    /// characters would appear alone on wallpaper.
    fn dirty_rect_hint(&self) -> Option<Rect> {
        self.text_window.dirty_rect_hint()
    }

    /// Drain pending output buffers BEFORE the compositor consults dirty
    /// state. If we deferred this work to `paint()` (as the pre-Phase-B
    /// code did), `mark_dirty_for_invalidated_windows` would call
    /// `dirty_rect_hint` against an empty `dirty_cells`, mark the full
    /// terminal bounds dirty, and the desktop's per-region wallpaper
    /// blit would overwrite the rest of the terminal — leaving older
    /// output as wallpaper after the incremental paint redrew only the
    /// freshly-arrived cells.
    fn prepare_for_render(&mut self) {
        self.process_terminal_output();
        if self.text_window.needs_repaint() {
            self.text_window.process_console_output();
        }
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        crate::debug_trace!("TerminalWindow::paint called");

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
                // Phase 3: when a user process is active and has put the
                // tty into raw mode (ICANON off), bypass the terminal's
                // line editor and forward each keystroke as raw bytes to
                // user stdin. zsh's zle does this so it can paint its
                // own prompt and handle history/completion.
                if crate::userland::stdin::is_active()
                    && !crate::userland::tty::is_canonical()
                {
                    let bytes = encode_keystroke_for_raw_mode(
                        kbd_event.key_code,
                        kbd_event.modifiers,
                    );
                    if !bytes.is_empty() {
                        crate::userland::stdin::push_bytes(&bytes);
                        if crate::userland::tty::is_echo() {
                            // ECHO in raw mode: echo only printable bytes
                            // (skip control characters / escape sequences,
                            // which the user app will redraw itself).
                            for &b in &bytes {
                                if (0x20..0x7F).contains(&b) {
                                    self.text_window.write_char(b as char);
                                }
                            }
                        }
                    }
                    return EventResult::Handled;
                }

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

                            // Phase 3: when a user is active and ECHO is
                            // off in canonical mode (e.g. password
                            // prompt), accept the byte but don't paint it.
                            self.input_buffer.push(ch);
                            let echo_to_screen = !crate::userland::stdin::is_active()
                                || crate::userland::tty::is_echo();
                            if echo_to_screen {
                                self.text_window.write_char(ch);
                            }

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

    fn can_focus(&self) -> bool {
        self.text_window.can_focus()
    }
}

/// Translate a keyboard event into the raw bytes a Linux TTY would
/// deliver in raw mode. Used only when a user process is active and
/// has cleared `ICANON` via `tcsetattr`.
///
/// - **Ctrl+letter** → control byte (Ctrl-A=0x01 ... Ctrl-Z=0x1A).
/// - **Backspace** → 0x7F (DEL — Linux convention; `stty erase` lets
///   apps remap to 0x08 (BS) but the wire byte is DEL).
/// - **Enter** → 0x0D (CR). zsh will translate via ICRNL if set.
/// - **Arrow keys** → ESC [ A/B/C/D (the ANSI VT100 sequences zle
///   reads).
/// - **Escape** → 0x1B.
/// - **Tab** → 0x09.
/// - Anything else falls back to the printable-character mapping that
///   the canonical-mode path uses.
fn encode_keystroke_for_raw_mode(
    key: crate::window::event::KeyCode,
    modifiers: crate::window::event::KeyModifiers,
) -> Vec<u8> {
    use crate::window::event::KeyCode;
    // Ctrl + ASCII letter → control byte. Hits before the named-key
    // matches so Ctrl-M etc. produce the right byte instead of '\r'.
    if modifiers.ctrl {
        if let Some(ch) = keycode_to_char(key, crate::window::event::KeyModifiers::default()) {
            if ch.is_ascii_alphabetic() {
                let lower = ch.to_ascii_lowercase() as u8;
                return alloc::vec![lower - b'a' + 1];
            }
        }
    }
    match key {
        KeyCode::Enter => alloc::vec![b'\r'],
        KeyCode::Backspace => alloc::vec![0x7F],
        KeyCode::Tab => alloc::vec![b'\t'],
        KeyCode::Escape => alloc::vec![0x1B],
        KeyCode::Up => alloc::vec![0x1B, b'[', b'A'],
        KeyCode::Down => alloc::vec![0x1B, b'[', b'B'],
        KeyCode::Right => alloc::vec![0x1B, b'[', b'C'],
        KeyCode::Left => alloc::vec![0x1B, b'[', b'D'],
        KeyCode::Home => alloc::vec![0x1B, b'[', b'H'],
        KeyCode::End => alloc::vec![0x1B, b'[', b'F'],
        KeyCode::Delete => alloc::vec![0x1B, b'[', b'3', b'~'],
        _ => {
            if let Some(ch) = keycode_to_char(key, modifiers) {
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            } else {
                Vec::new()
            }
        }
    }
}