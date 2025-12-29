//! Label widget for displaying static text

use alloc::string::String;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use super::base::WindowBase;

/// A simple label widget for displaying text
pub struct Label {
    /// Base window functionality
    base: WindowBase,
    /// Text to display
    text: String,
    /// Text color
    color: Color,
    /// Optional background color (None = transparent)
    background: Option<Color>,
    /// Horizontal alignment
    align: TextAlign,
}

/// Text alignment options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

impl Label {
    /// Create a new label with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, text: &str) -> Self {
        Label {
            base: WindowBase::new_with_id(id, bounds),
            text: String::from(text),
            color: Color::BLACK,
            background: None,
            align: TextAlign::Left,
        }
    }

    /// Create a new label (generates its own ID)
    pub fn new(bounds: Rect, text: &str) -> Self {
        Self::new_with_id(WindowId::new(), bounds, text)
    }

    /// Set the label text
    pub fn set_text(&mut self, text: &str) {
        if self.text != text {
            self.text = String::from(text);
            self.base.invalidate();
        }
    }

    /// Get the current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text color
    pub fn set_color(&mut self, color: Color) {
        if self.color != color {
            self.color = color;
            self.base.invalidate();
        }
    }

    /// Set the background color (None for transparent)
    pub fn set_background(&mut self, background: Option<Color>) {
        if self.background != background {
            self.background = background;
            self.base.invalidate();
        }
    }

    /// Set text alignment
    pub fn set_align(&mut self, align: TextAlign) {
        if self.align != align {
            self.align = align;
            self.base.invalidate();
        }
    }
}

impl Window for Label {
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

        // Draw background if set
        if let Some(bg) = self.background {
            device.fill_rect(x, y, width, height, bg);
        }

        // Draw text
        if !self.text.is_empty() {
            let font = get_default_font();
            let char_width = 8; // Default font is 8x8
            let text_width = self.text.len() * char_width;

            // Calculate x position based on alignment
            let text_x = match self.align {
                TextAlign::Left => x + 2, // Small padding
                TextAlign::Center => {
                    if text_width < width {
                        x + (width - text_width) / 2
                    } else {
                        x + 2
                    }
                }
                TextAlign::Right => {
                    if text_width < width {
                        x + width - text_width - 2
                    } else {
                        x + 2
                    }
                }
            };

            // Center vertically
            let text_y = y + (height.saturating_sub(8)) / 2;

            device.draw_text(text_x, text_y, &self.text, font.as_font(), self.color);
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Labels don't handle events
        EventResult::Ignored
    }

    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }

    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }

    fn can_focus(&self) -> bool {
        // Labels cannot receive focus
        false
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
