//! Multi-line text editor widget with full cursor navigation
//!
//! Uses `Vec<String>` lines and supports full cursor movement, insertion, and
//! editing.
//!
//! Per U7, `TextEditor` no longer manages its own scroll state. Callers
//! wrap it in a [`ScrollView`](super::scroll_view::ScrollView) and feed
//! [`TextEditor::content_size`] into [`ScrollView::set_content_size`]
//! whenever the content changes. When the cursor moves, `TextEditor`
//! stages an [`Event::EnsureVisible`] payload via
//! [`TextEditor::take_pending_ensure_visible`]; the window manager picks
//! it up after every event dispatch and forwards it to the nearest
//! enclosing `ScrollView` ancestor.

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::{KeyCode, KeyboardEvent, MouseEventType};
use crate::window::keyboard::keycode_to_char;
use crate::window::{
    CursorIcon, Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowId,
};
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
    /// Text has been modified since last save
    modified: bool,
    /// Character width (from font)
    char_width: usize,
    /// Character height (from font)
    char_height: usize,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Cursor color
    cursor_color: Color,
    /// Set whenever a cursor-moving operation runs. The window manager
    /// drains this via `take_pending_ensure_visible` after each event
    /// dispatch and forwards an `Event::EnsureVisible(rect)` to the
    /// nearest `ScrollView` ancestor. The rect is in the editor's local
    /// coordinate space (origin = top-left of the content area).
    pending_ensure_visible: Option<Rect>,
}

impl TextEditor {
    /// Create a new text editor with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let font = get_default_font();
        let char_width = font.cell_width() as usize;
        let char_height = font.line_height() as usize;

        let mut base = WindowBase::new_with_id(id, bounds);
        base.set_can_focus(true);

        TextEditor {
            base,
            lines: vec![String::new()], // Start with one empty line
            cursor_col: 0,
            cursor_row: 0,
            modified: false,
            char_width,
            char_height,
            bg_color: crate::window::theme::controls::palette().field_bg,
            text_color: crate::window::theme::controls::palette().field_text,
            cursor_color: crate::window::theme::controls::palette().field_text,
            pending_ensure_visible: None,
        }
    }

    /// Create a new text editor (generates its own ID)
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Get the full text content

    /// Set the text content
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.lines().map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_col = 0;
        self.cursor_row = 0;
        self.modified = false;
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Clear all text

    /// Check if text has been modified

    /// Set modified state

    /// Get number of lines

    /// Get cursor position (col, row)
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn cursor_position(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    /// Natural content extent in pixels.
    ///
    /// Width is the longest line in characters times character width;
    /// height is the line count times line height. Callers that wrap the
    /// editor in a [`ScrollView`](super::scroll_view::ScrollView) feed
    /// this into [`ScrollView::set_content_size`] whenever the content
    /// may have changed.
    pub fn content_size(&self) -> (u32, u32) {
        let max_line_chars = self
            .lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        // Reserve at least the bounds width so an empty editor still
        // fills its viewport (the ScrollView clamps content_w upward to
        // the viewport size anyway, but reporting zero for an empty
        // editor would let the caller mistake "empty" for "needs no
        // content").
        let w = (max_line_chars * self.char_width) as u32;
        let h = (self.lines.len() * self.char_height) as u32;
        (w, h)
    }

    /// Drain the pending `Event::EnsureVisible` rect, if any. Called by
    /// the window manager after each event dispatch; if the value is
    /// `Some`, the manager forwards an `Event::EnsureVisible(rect)` to
    /// the nearest enclosing `ScrollView` ancestor.

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
        self.queue_cursor_into_view();
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
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor up
    pub fn move_cursor_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.clamp_cursor_col();
        }
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor down
    pub fn move_cursor_down(&mut self) {
        if self.cursor_row < self.lines.len() - 1 {
            self.cursor_row += 1;
            self.clamp_cursor_col();
        }
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor to start of line
    pub fn move_cursor_home(&mut self) {
        self.cursor_col = 0;
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor to end of line
    pub fn move_cursor_end(&mut self) {
        self.cursor_col = self.current_line_len();
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor to start of document
    pub fn move_cursor_doc_start(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Move cursor to end of document
    pub fn move_cursor_doc_end(&mut self) {
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = self.current_line_len();
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Page up. With scroll handled by the enclosing `ScrollView`, "page"
    /// here means "the editor's viewport height in lines" — derived from
    /// the current bounds. We keep the same semantics (advance cursor by
    /// roughly one viewport) and let `EnsureVisible` align the result.
    pub fn page_up(&mut self) {
        let visible_rows = self.visible_rows_estimate();
        if self.cursor_row >= visible_rows {
            self.cursor_row -= visible_rows;
        } else {
            self.cursor_row = 0;
        }
        self.clamp_cursor_col();
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Page down
    pub fn page_down(&mut self) {
        let visible_rows = self.visible_rows_estimate();
        let max_row = self.lines.len().saturating_sub(1);
        self.cursor_row = (self.cursor_row + visible_rows).min(max_row);
        self.clamp_cursor_col();
        self.queue_cursor_into_view();
        self.base.invalidate();
    }

    /// Page-up/page-down step in lines. Approximated from the editor's
    /// outer bounds — accurate when bounds match the viewport. If the
    /// editor is currently translated by a `ScrollView` paint pass, this
    /// will read the translated bounds instead, but page-up/down only
    /// runs from `handle_event`, which executes outside paint.
    fn visible_rows_estimate(&self) -> usize {
        let h = self.base.bounds().height as usize;
        if self.char_height == 0 {
            1
        } else {
            (h / self.char_height).max(1)
        }
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
            self.queue_cursor_into_view();
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
            self.queue_cursor_into_view();
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
                self.queue_cursor_into_view();
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
            self.queue_cursor_into_view();
            self.base.invalidate();
        }
    }

    /// Cursor rect in local content coordinates. Used by both
    /// `queue_cursor_into_view` and the painter so the on-screen cursor
    /// and the EnsureVisible payload always agree on geometry.
    fn cursor_rect_local(&self) -> Rect {
        let x = (self.cursor_col * self.char_width) as i32;
        let y = (self.cursor_row * self.char_height) as i32;
        Rect::new(x, y, self.char_width as u32, self.char_height as u32)
    }

    /// Stage an `Event::EnsureVisible(cursor_rect)` payload for the
    /// window manager to forward upward after this event dispatch.
    fn queue_cursor_into_view(&mut self) {
        self.pending_ensure_visible = Some(self.cursor_rect_local());
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

    /// Handle a mouse click. Coordinates are in the editor's local
    /// frame (post-scroll-translation, courtesy of `ScrollView`), so
    /// row/col map directly without scroll-offset arithmetic.
    fn handle_click(&mut self, x: i32, y: i32) {
        if x < 0 || y < 0 {
            return;
        }
        if self.char_width == 0 || self.char_height == 0 {
            return;
        }

        let col = (x as usize) / self.char_width;
        let row = (y as usize) / self.char_height;

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

        self.queue_cursor_into_view();
        self.base.invalidate();
    }
}

impl Window for TextEditor {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn cursor_icon_at(&self, _point: Point) -> CursorIcon {
        CursorIcon::Text
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        // `bounds` here is the editor's *content* rect — when the editor
        // is wrapped in a ScrollView, the manager-side render path
        // temporarily rewrites `bounds` to the full content extent so we
        // paint everything; the active clip rect (set by the parent
        // ScrollView) limits the visible pixels to the viewport.
        let bounds = self.bounds();
        let font = get_default_font();

        // Fill background across the whole content rect.
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            self.bg_color,
        );

        // Draw every line at its content-space y. The clip rect handles
        // off-screen lines.
        for (row, line) in self.lines.iter().enumerate() {
            let y = bounds.y + (row * self.char_height) as i32;
            if !line.is_empty() {
                device.draw_text(bounds.x, y, line, font.as_font(), self.text_color);
            }
        }

        // Draw cursor if focused.
        if self.has_focus() {
            let cursor_rect = self.cursor_rect_local();
            let cursor_x = bounds.x + cursor_rect.x;
            let cursor_y = bounds.y + cursor_rect.y;
            device.fill_rect(
                cursor_x,
                cursor_y,
                2,
                self.char_height as u32,
                self.cursor_color,
            );
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

    fn take_pending_ensure_visible(&mut self) -> Option<Rect> {
        self.pending_ensure_visible.take()
    }
}
