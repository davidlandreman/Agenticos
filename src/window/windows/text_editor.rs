//! Multi-line text editor widget with full cursor navigation
//!
//! Unlike TextWindow (grid-based), TextEditor uses Vec<String> for lines
//! and supports full cursor movement, text insertion, and editing.

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::{KeyCode, KeyboardEvent, MouseEventType};
use crate::window::keyboard::keycode_to_char;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::{string::String, vec, vec::Vec};

use super::base::WindowBase;

/// A multi-line text editor with full cursor navigation
pub struct TextEditor {
    /// Base window functionality
    base: WindowBase,
    /// Text content as lines
    lines: Vec<String>,
    /// Cursor column position
    cursor_col: usize,
    /// Cursor row position
    cursor_row: usize,
    /// Scroll offset (first visible row)
    scroll_y: usize,
    /// Scroll offset (first visible column)
    scroll_x: usize,
    /// Text has been modified since last save
    modified: bool,
    /// Character width (from font)
    char_width: usize,
    /// Character height (from font)
    char_height: usize,
    /// Visible columns
    visible_cols: usize,
    /// Visible rows
    visible_rows: usize,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Cursor color
    cursor_color: Color,
}

impl TextEditor {
    /// Create a new text editor with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let font = get_default_font();
        let char_width = font.char_width();
        let char_height = font.char_height();

        let visible_cols = (bounds.width as usize) / char_width;
        let visible_rows = (bounds.height as usize) / char_height;

        let mut base = WindowBase::new_with_id(id, bounds);
        base.set_can_focus(true);

        TextEditor {
            base,
            lines: vec![String::new()], // Start with one empty line
            cursor_col: 0,
            cursor_row: 0,
            scroll_y: 0,
            scroll_x: 0,
            modified: false,
            char_width,
            char_height,
            visible_cols,
            visible_rows,
            bg_color: Color::WHITE,
            text_color: Color::BLACK,
            cursor_color: Color::BLACK,
        }
    }

    /// Create a new text editor (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Get the full text content
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Set the text content
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.lines().map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.scroll_x = 0;
        self.scroll_y = 0;
        self.modified = false;
        self.base.invalidate();
    }

    /// Clear all text
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.scroll_x = 0;
        self.scroll_y = 0;
        self.modified = false;
        self.base.invalidate();
    }

    /// Check if text has been modified
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Set modified state
    pub fn set_modified(&mut self, modified: bool) {
        self.modified = modified;
    }

    /// Get number of lines
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Get cursor position (col, row)
    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    /// Get current line length
    fn current_line_len(&self) -> usize {
        self.lines.get(self.cursor_row).map_or(0, |l| l.len())
    }

    /// Clamp cursor column to line length
    fn clamp_cursor_col(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
    }

    /// Move cursor left
    pub fn move_cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            // Move to end of previous line
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
        }
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor right
    pub fn move_cursor_right(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row < self.lines.len() - 1 {
            // Move to start of next line
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor up
    pub fn move_cursor_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_cursor_col();
        }
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor down
    pub fn move_cursor_down(&mut self) {
        if self.cursor_row < self.lines.len() - 1 {
            self.cursor_row += 1;
            self.clamp_cursor_col();
        }
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor to start of line
    pub fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor to end of line
    pub fn move_cursor_end(&mut self) {
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor to start of document
    pub fn move_cursor_doc_start(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Move cursor to end of document
    pub fn move_cursor_doc_end(&mut self) {
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Page up
    pub fn page_up(&mut self) {
        if self.cursor_row >= self.visible_rows {
            self.cursor_row -= self.visible_rows;
        } else {
            self.cursor_row = 0;
        }
        self.clamp_cursor_col();
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Page down
    pub fn page_down(&mut self) {
        let max_row = self.lines.len().saturating_sub(1);
        self.cursor_row = (self.cursor_row + self.visible_rows).min(max_row);
        self.clamp_cursor_col();
        self.ensure_cursor_visible();
        self.base.invalidate();
    }

    /// Insert a character at cursor position
    pub fn insert_char(&mut self, ch: char) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            // Handle tab as spaces
            if ch == '\t' {
                let spaces = 4 - (self.cursor_col % 4);
                for _ in 0..spaces {
                    line.insert(self.cursor_col, ' ');
                    self.cursor_col += 1;
                }
            } else {
                line.insert(self.cursor_col, ch);
                self.cursor_col += 1;
            }
            self.modified = true;
            self.ensure_cursor_visible();
            self.base.invalidate();
        }
    }

    /// Insert a newline at cursor position
    pub fn insert_newline(&mut self) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            // Split line at cursor
            let rest = line.split_off(self.cursor_col);
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.lines.insert(self.cursor_row, rest);
            self.modified = true;
            self.ensure_cursor_visible();
            self.base.invalidate();
        }
    }

    /// Delete character at cursor (Delete key)
    pub fn delete_char(&mut self) {
        if let Some(line) = self.lines.get_mut(self.cursor_row) {
            if self.cursor_col < line.len() {
                // Delete character at cursor
                line.remove(self.cursor_col);
                self.modified = true;
                self.base.invalidate();
            } else if self.cursor_row < self.lines.len() - 1 {
                // Join with next line
                let next_line = self.lines.remove(self.cursor_row + 1);
                if let Some(current) = self.lines.get_mut(self.cursor_row) {
                    current.push_str(&next_line);
                }
                self.modified = true;
                self.base.invalidate();
            }
        }
    }

    /// Delete character before cursor (Backspace)
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            // Delete character before cursor
            if let Some(line) = self.lines.get_mut(self.cursor_row) {
                self.cursor_col -= 1;
                line.remove(self.cursor_col);
                self.modified = true;
                self.base.invalidate();
            }
        } else if self.cursor_row > 0 {
            // Join with previous line
            let current_line = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            if let Some(prev_line) = self.lines.get_mut(self.cursor_row) {
                self.cursor_col = prev_line.len();
                prev_line.push_str(&current_line);
            }
            self.modified = true;
            self.ensure_cursor_visible();
            self.base.invalidate();
        }
    }

    /// Ensure cursor is visible (adjust scroll)
    fn ensure_cursor_visible(&mut self) {
        // Vertical scroll
        if self.cursor_row < self.scroll_y {
            self.scroll_y = self.cursor_row;
        } else if self.cursor_row >= self.scroll_y + self.visible_rows {
            self.scroll_y = self.cursor_row - self.visible_rows + 1;
        }

        // Horizontal scroll
        if self.cursor_col < self.scroll_x {
            self.scroll_x = self.cursor_col;
        } else if self.cursor_col >= self.scroll_x + self.visible_cols {
            self.scroll_x = self.cursor_col - self.visible_cols + 1;
        }
    }

    /// Handle a keyboard event
    fn handle_key(&mut self, event: &KeyboardEvent) -> EventResult {
        if !event.pressed {
            return EventResult::Ignored;
        }

        // Handle Ctrl+key shortcuts
        if event.modifiers.ctrl {
            match event.key_code {
                KeyCode::Home => {
                    self.move_cursor_doc_start();
                    return EventResult::Handled;
                }
                KeyCode::End => {
                    self.move_cursor_doc_end();
                    return EventResult::Handled;
                }
                _ => {}
            }
        }

        match event.key_code {
            // Navigation
            KeyCode::Left => self.move_cursor_left(),
            KeyCode::Right => self.move_cursor_right(),
            KeyCode::Up => self.move_cursor_up(),
            KeyCode::Down => self.move_cursor_down(),
            KeyCode::Home => self.move_cursor_home(),
            KeyCode::End => self.move_cursor_end(),
            KeyCode::PageUp => self.page_up(),
            KeyCode::PageDown => self.page_down(),

            // Editing
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_char(),
            KeyCode::Enter => self.insert_newline(),
            KeyCode::Tab => self.insert_char('\t'),

            // Character input
            _ => {
                if let Some(ch) = keycode_to_char(event.key_code, event.modifiers) {
                    if ch != '\n' && ch != '\t' {
                        self.insert_char(ch);
                    }
                    return EventResult::Handled;
                }
                return EventResult::Ignored;
            }
        }

        EventResult::Handled
    }

    /// Handle a mouse click
    fn handle_click(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 {
            return;
        }

        // Calculate clicked position
        let col = (x as usize) / self.char_width + self.scroll_x;
        let row = (y as usize) / self.char_height + self.scroll_y;

        // Set cursor position (clamped to valid range)
        if row < self.lines.len() {
            self.cursor_row = row;
            let line_len = self.current_line_len();
            self.cursor_col = col.min(line_len);
        } else {
            // Clicked below last line - go to end
            self.cursor_row = self.lines.len().saturating_sub(1);
            self.cursor_col = self.current_line_len();
        }

        self.base.invalidate();
    }

    /// Update visible dimensions when bounds change
    fn update_dimensions(&mut self) {
        let bounds = self.base.bounds();
        self.visible_cols = (bounds.width as usize) / self.char_width;
        self.visible_rows = (bounds.height as usize) / self.char_height;
    }
}

impl Window for TextEditor {
    fn id(&self) -> WindowId {
        self.base.id()
    }

    fn bounds(&self) -> Rect {
        self.base.bounds()
    }

    fn visible(&self) -> bool {
        self.base.visible()
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.base.set_bounds(bounds);
        self.update_dimensions();
    }

    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.base.set_bounds_no_invalidate(bounds);
        self.update_dimensions();
    }

    fn set_visible(&mut self, visible: bool) {
        self.base.set_visible(visible);
    }

    fn parent(&self) -> Option<WindowId> {
        self.base.parent()
    }

    fn children(&self) -> &[WindowId] {
        self.base.children()
    }

    fn set_parent(&mut self, parent: Option<WindowId>) {
        self.base.set_parent(parent);
    }

    fn add_child(&mut self, child: WindowId) {
        self.base.add_child(child);
    }

    fn remove_child(&mut self, child: WindowId) {
        self.base.remove_child(child);
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        let bounds = self.bounds();
        let font = get_default_font();

        // Fill background
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            bounds.height as usize,
            self.bg_color,
        );

        // Draw visible lines
        for row in 0..self.visible_rows {
            let line_idx = self.scroll_y + row;
            if line_idx >= self.lines.len() {
                break;
            }

            let line = &self.lines[line_idx];
            let y = bounds.y as usize + row * self.char_height;

            // Get visible portion of line
            let start_col = self.scroll_x;
            let end_col = (self.scroll_x + self.visible_cols).min(line.len());

            if start_col < line.len() {
                let visible_text: String = line
                    .chars()
                    .skip(start_col)
                    .take(end_col - start_col)
                    .collect();

                device.draw_text(
                    bounds.x as usize,
                    y,
                    &visible_text,
                    font.as_font(),
                    self.text_color,
                );
            }
        }

        // Draw cursor if focused
        if self.has_focus() {
            let cursor_screen_row = self.cursor_row as isize - self.scroll_y as isize;
            let cursor_screen_col = self.cursor_col as isize - self.scroll_x as isize;

            if cursor_screen_row >= 0
                && cursor_screen_row < self.visible_rows as isize
                && cursor_screen_col >= 0
                && cursor_screen_col < self.visible_cols as isize
            {
                let cursor_x = bounds.x as usize + cursor_screen_col as usize * self.char_width;
                let cursor_y = bounds.y as usize + cursor_screen_row as usize * self.char_height;

                // Draw cursor as vertical bar
                device.fill_rect(cursor_x, cursor_y, 2, self.char_height, self.cursor_color);
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(key_event) => self.handle_key(&key_event),
            Event::Mouse(mouse_event) => {
                if mouse_event.event_type == MouseEventType::ButtonDown && mouse_event.buttons.left
                {
                    self.handle_click(mouse_event.position.x, mouse_event.position.y);
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            Event::Focus(focus_event) => {
                self.set_focus(focus_event.gained);
                self.base.invalidate();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
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

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
