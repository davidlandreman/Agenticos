//! Text input widget for editable text

use super::base::WindowBase;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::{KeyCode, MouseEventType};
use crate::window::keyboard::keycode_to_char;
use crate::window::theme::controls;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::boxed::Box;
use alloc::string::String;

/// Callback type for text change events
pub type TextChangeCallback = Box<dyn FnMut(&str) + Send>;
/// Callback type for Enter while the input is focused.
pub type TextSubmitCallback = Box<dyn FnMut(&str) + Send>;
/// Callback type for Escape while the input is focused.
pub type TextCancelCallback = Box<dyn FnMut() + Send>;

/// A single-line text input widget. The well surface (background + border,
/// focus feedback) comes from the active theme via `theme::controls`.
pub struct TextInput {
    /// Base window functionality
    base: WindowBase,
    /// Current text content
    text: String,
    /// Maximum text length (None = unlimited)
    max_length: Option<usize>,
    /// Callback for text changes
    on_change: Option<TextChangeCallback>,
    /// Callback for Enter/submit
    on_submit: Option<TextSubmitCallback>,
    /// Callback for Escape/cancel
    on_cancel: Option<TextCancelCallback>,
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
            on_submit: None,
            on_cancel: None,
        }
    }

    /// Get the current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text content
    pub fn set_text(&mut self, text: &str) {
        let new_text = String::from(self.limit_text(text));

        if self.text != new_text {
            self.text = new_text;
            self.base.invalidate();
            self.notify_change();
        }
    }

    /// Clear the text.
    pub fn clear(&mut self) {
        self.set_text("");
    }

    /// Set the maximum UTF-8 byte length. Existing text is truncated at a
    /// character boundary when the limit shrinks.
    pub fn set_max_length(&mut self, max_length: Option<usize>) {
        self.max_length = max_length;
        let text = self.text.clone();
        self.set_text(&text);
    }

    /// Set the text change callback.
    pub fn on_change<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_change = Some(Box::new(callback));
    }

    /// Set the Enter/submit callback.
    pub fn on_submit<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_submit = Some(Box::new(callback));
    }

    /// Set the Escape/cancel callback.
    pub fn on_cancel<F>(&mut self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.on_cancel = Some(Box::new(callback));
    }

    /// Notify change callback
    fn notify_change(&mut self) {
        if let Some(ref mut callback) = self.on_change {
            callback(&self.text);
        }
    }

    fn limit_text<'a>(&self, text: &'a str) -> &'a str {
        let Some(max) = self.max_length else {
            return text;
        };
        if text.len() <= max {
            return text;
        }
        let mut end = max;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }

    /// Append a character to the text
    fn append_char(&mut self, ch: char) {
        // Check max length
        if let Some(max) = self.max_length {
            if self.text.len().saturating_add(ch.len_utf8()) > max {
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
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;

        // Themed well: background + border with focus feedback.
        controls::draw_field(device, bounds, self.base.has_focus());
        let text_color = controls::palette().field_text;

        // Draw text with padding
        let padding: i32 = 4;
        let font = get_default_font();
        let char_width = font.cell_width() as i32;
        let char_height = font.line_height() as i32;

        // Calculate text position (vertically centered)
        let text_x = x + padding;
        let text_y = y + (height.saturating_sub(char_height as u32) / 2) as i32;

        // Draw text
        if !self.text.is_empty() {
            device.draw_text(text_x, text_y, &self.text, font.as_font(), text_color);
        }

        // Draw cursor when focused
        if self.base.has_focus() {
            let cursor_x = text_x + self.text.len() as i32 * char_width;
            // Draw a vertical line as cursor
            if cursor_x < x + width as i32 - padding {
                let cursor_color = text_color;
                device.draw_line(
                    cursor_x,
                    text_y,
                    cursor_x,
                    text_y + char_height - 1,
                    cursor_color,
                );
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                match kbd_event.key_code {
                    KeyCode::Enter => {
                        if let Some(ref mut callback) = self.on_submit {
                            callback(&self.text);
                        }
                        EventResult::Handled
                    }
                    KeyCode::Escape => {
                        if let Some(ref mut callback) = self.on_cancel {
                            callback();
                        }
                        EventResult::Handled
                    }
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

    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }
}
