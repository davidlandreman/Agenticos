//! Text input widget for editable text

use alloc::boxed::Box;
use alloc::string::String;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use crate::window::event::{KeyCode, MouseEventType};
use crate::window::keyboard::keycode_to_char;
use super::base::WindowBase;

/// Callback type for text change events
pub type TextChangeCallback = Box<dyn FnMut(&str) + Send>;

/// A single-line text input widget
pub struct TextInput {
    /// Base window functionality
    base: WindowBase,
    /// Current text content
    text: String,
    /// Maximum text length (None = unlimited)
    max_length: Option<usize>,
    /// Callback for text changes
    on_change: Option<TextChangeCallback>,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Border color
    border_color: Color,
    /// Focused border color
    focus_border_color: Color,
}

impl TextInput {
    /// Create a new text input with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let mut base = WindowBase::new_with_id(id, bounds);
        base.set_can_focus(true);

        TextInput {
            base,
            text: String::new(),
            max_length: None,
            on_change: None,
            bg_color: Color::WHITE,
            text_color: Color::BLACK,
            border_color: Color::GRAY,
            focus_border_color: Color::BLUE,
        }
    }

    /// Create a new text input (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Get the current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text content
    pub fn set_text(&mut self, text: &str) {
        let new_text = if let Some(max) = self.max_length {
            if text.len() > max {
                String::from(&text[..max])
            } else {
                String::from(text)
            }
        } else {
            String::from(text)
        };

        if self.text != new_text {
            self.text = new_text;
            self.base.invalidate();
            self.notify_change();
        }
    }

    /// Clear the text
    pub fn clear(&mut self) {
        if !self.text.is_empty() {
            self.text.clear();
            self.base.invalidate();
            self.notify_change();
        }
    }

    /// Set maximum text length
    pub fn set_max_length(&mut self, max: Option<usize>) {
        self.max_length = max;
        // Truncate if current text exceeds new limit
        if let Some(max) = max {
            if self.text.len() > max {
                self.text.truncate(max);
                self.base.invalidate();
                self.notify_change();
            }
        }
    }

    /// Set the text change callback
    pub fn on_change<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_change = Some(Box::new(callback));
    }

    /// Set background color
    pub fn set_bg_color(&mut self, color: Color) {
        self.bg_color = color;
        self.base.invalidate();
    }

    /// Set text color
    pub fn set_text_color(&mut self, color: Color) {
        self.text_color = color;
        self.base.invalidate();
    }

    /// Notify change callback
    fn notify_change(&mut self) {
        if let Some(ref mut callback) = self.on_change {
            callback(&self.text);
        }
    }

    /// Append a character to the text
    fn append_char(&mut self, ch: char) {
        // Check max length
        if let Some(max) = self.max_length {
            if self.text.len() >= max {
                return;
            }
        }

        self.text.push(ch);
        self.base.invalidate();
        self.notify_change();
    }

    /// Remove the last character
    fn backspace(&mut self) {
        if self.text.pop().is_some() {
            self.base.invalidate();
            self.notify_change();
        }
    }
}

impl Window for TextInput {
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
    }

    fn set_bounds_no_invalidate(&mut self, bounds: Rect) {
        self.base.set_bounds_no_invalidate(bounds);
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
        if !self.base.visible() {
            return;
        }
        if !self.base.needs_repaint() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x as usize;
        let y = bounds.y as usize;
        let width = bounds.width as usize;
        let height = bounds.height as usize;

        // Draw background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw border (different color when focused)
        let border_color = if self.base.has_focus() {
            self.focus_border_color
        } else {
            self.border_color
        };

        // Top
        device.draw_line(x, y, x + width - 1, y, border_color);
        // Left
        device.draw_line(x, y, x, y + height - 1, border_color);
        // Bottom
        device.draw_line(x, y + height - 1, x + width - 1, y + height - 1, border_color);
        // Right
        device.draw_line(x + width - 1, y, x + width - 1, y + height - 1, border_color);

        // Draw text with padding
        let padding = 4;
        let font = get_default_font();
        let char_width = 8;
        let char_height = 8;

        // Calculate text position (vertically centered)
        let text_x = x + padding;
        let text_y = y + (height.saturating_sub(char_height)) / 2;

        // Draw text
        if !self.text.is_empty() {
            device.draw_text(text_x, text_y, &self.text, font.as_font(), self.text_color);
        }

        // Draw cursor when focused
        if self.base.has_focus() {
            let cursor_x = text_x + self.text.len() * char_width;
            // Draw a vertical line as cursor
            if cursor_x < x + width - padding {
                let cursor_color = self.text_color;
                device.draw_line(cursor_x, text_y, cursor_x, text_y + char_height - 1, cursor_color);
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                match kbd_event.key_code {
                    KeyCode::Backspace => {
                        self.backspace();
                        EventResult::Handled
                    }
                    KeyCode::Delete => {
                        // For basic input, delete works like backspace
                        self.backspace();
                        EventResult::Handled
                    }
                    _ => {
                        // Try to convert to a character
                        if let Some(ch) = keycode_to_char(kbd_event.key_code, kbd_event.modifiers) {
                            self.append_char(ch);
                            EventResult::Handled
                        } else {
                            EventResult::Ignored
                        }
                    }
                }
            }
            Event::Mouse(mouse_event) => {
                // Clicking on the input should focus it
                if mouse_event.event_type == MouseEventType::ButtonDown
                    && mouse_event.buttons.left
                    && self.bounds().contains_point(mouse_event.position)
                {
                    // Request focus - handled by window manager
                    EventResult::Handled
                } else {
                    EventResult::Ignored
                }
            }
            Event::Focus(focus_event) => {
                self.base.set_focus(focus_event.gained);
                self.base.invalidate();
                EventResult::Handled
            }
            _ => EventResult::Ignored,
        }
    }

    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }

    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }

    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
