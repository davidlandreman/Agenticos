//! Text window for grid-based text rendering

use alloc::{vec, vec::Vec};
use alloc::string::{String, ToString};
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::{Font, get_default_font};
use super::base::WindowBase;

/// A character cell in the text grid
#[derive(Clone, Copy)]
struct CharCell {
    ch: char,
    fg_color: Color,
    bg_color: Color,
}

impl Default for CharCell {
    fn default() -> Self {
        CharCell {
            ch: ' ',
            fg_color: Color::WHITE,
            bg_color: Color::BLACK,
        }
    }
}

/// A window that renders text in a grid
pub struct TextWindow {
    /// Base window functionality
    base: WindowBase,
    /// Text buffer organized as rows and columns
    buffer: Vec<Vec<CharCell>>,
    /// Number of columns
    cols: usize,
    /// Number of rows
    rows: usize,
    /// Cursor position
    cursor_x: usize,
    cursor_y: usize,
    /// Current foreground color
    current_fg: Color,
    /// Current background color
    current_bg: Color,
    /// Character dimensions
    char_width: usize,
    char_height: usize,
}

impl TextWindow {
    /// Create a new text window
    pub fn new(bounds: Rect) -> Self {
        let font = get_default_font();
        let char_width = font.char_width();
        let char_height = font.char_height();
        
        // Calculate grid dimensions
        let cols = (bounds.width as usize) / char_width;
        let rows = (bounds.height as usize) / char_height;
        
        // Initialize buffer
        let buffer = vec![vec![CharCell::default(); cols]; rows];
        
        let mut base = WindowBase::new(bounds);
        base.set_can_focus(true); // Text windows can receive focus
        
        TextWindow {
            base,
            buffer,
            cols,
            rows,
            cursor_x: 0,
            cursor_y: 0,
            current_fg: Color::WHITE,
            current_bg: Color::BLACK,
            char_width,
            char_height,
        }
    }
    
    /// Write a character at the cursor position
    pub fn write_char(&mut self, ch: char) {
        if ch == '\n' {
            self.newline();
            return;
        }
        
        if ch == '\r' {
            self.cursor_x = 0;
            return;
        }
        
        if self.cursor_x < self.cols && self.cursor_y < self.rows {
            self.buffer[self.cursor_y][self.cursor_x] = CharCell {
                ch,
                fg_color: self.current_fg,
                bg_color: self.current_bg,
            };
            self.cursor_x += 1;
            
            if self.cursor_x >= self.cols {
                self.newline();
            }
            
            self.base.invalidate();
        }
    }
    
    /// Write a string at the cursor position
    pub fn write_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.write_char(ch);
        }
    }
    
    /// Move to a new line
    fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        
        if self.cursor_y >= self.rows {
            // Scroll up
            self.scroll_up();
            self.cursor_y = self.rows - 1;
        }
    }
    
    /// Scroll the buffer up by one line
    fn scroll_up(&mut self) {
        // Remove first line and add empty line at bottom
        self.buffer.remove(0);
        self.buffer.push(vec![CharCell::default(); self.cols]);
        self.base.invalidate();
    }
    
    /// Clear the text window
    pub fn clear(&mut self) {
        self.buffer = vec![vec![CharCell::default(); self.cols]; self.rows];
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.base.invalidate();
    }
    
    /// Set cursor position
    pub fn set_cursor(&mut self, x: usize, y: usize) {
        if x < self.cols && y < self.rows {
            self.cursor_x = x;
            self.cursor_y = y;
        }
    }
    
    /// Set text colors
    pub fn set_colors(&mut self, fg: Color, bg: Color) {
        self.current_fg = fg;
        self.current_bg = bg;
    }
}

impl Window for TextWindow {
    fn id(&self) -> WindowId {
        self.base.id()
    }
    
    fn bounds(&self) -> Rect {
        self.base.bounds()
    }
    
    fn visible(&self) -> bool {
        self.base.visible()
    }
    
    fn parent(&self) -> Option<WindowId> {
        self.base.parent()
    }
    
    fn children(&self) -> &[WindowId] {
        self.base.children()
    }
    
    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }
        
        // Check for console output and add it to the window
        let (lines, pending) = crate::window::console::take_output();
        if !lines.is_empty() || !pending.is_empty() {
            for line in lines {
                self.write_str(&line);
                self.newline();
            }
            if !pending.is_empty() {
                self.write_str(&pending);
            }
            // Mark that we need repaint since we added text
            self.base.invalidate();
        }
        
        let bounds = self.bounds();
        let font = get_default_font();
        
        // Clear background
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            bounds.height as usize,
            Color::BLACK,
        );
        
        // Render each character
        for (row, line) in self.buffer.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                let x = bounds.x as usize + col * self.char_width;
                let y = bounds.y as usize + row * self.char_height;
                
                // Draw background if not black
                if cell.bg_color != Color::BLACK {
                    device.fill_rect(
                        x,
                        y,
                        self.char_width,
                        self.char_height,
                        cell.bg_color,
                    );
                }
                
                // Draw character
                if cell.ch != ' ' {
                    device.draw_text(x, y, &cell.ch.to_string(), font.as_font(), cell.fg_color);
                }
            }
        }
        
        // Draw cursor if focused
        if self.has_focus() && self.cursor_x < self.cols && self.cursor_y < self.rows {
            let cursor_x = bounds.x as usize + self.cursor_x * self.char_width;
            let cursor_y = bounds.y as usize + self.cursor_y * self.char_height;
            
            // Draw cursor as a filled rectangle
            device.fill_rect(
                cursor_x,
                cursor_y + self.char_height - 2,
                self.char_width,
                2,
                Color::WHITE,
            );
        }
        
        self.base.clear_needs_repaint();
    }
    
    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }
    
    fn invalidate(&mut self) {
        self.base.invalidate();
    }
    
    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(key_event) => {
                if key_event.pressed {
                    // TODO: Handle keyboard input
                    // For now, just mark as handled if we have focus
                    if self.has_focus() {
                        return EventResult::Handled;
                    }
                }
            }
            _ => {}
        }
        
        EventResult::Propagate
    }
    
    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }
    
    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }
    
    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }
}