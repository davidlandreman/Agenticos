//! Label widget for displaying static text

use super::base::WindowBase;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::string::String;

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
    #[expect(dead_code, reason = "intentional kernel API surface")]
    Center,
    Right,
}

impl Label {
    /// Create a new label with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, text: &str) -> Self {
        Label {
            base: WindowBase::new_with_id(id, bounds),
            text: String::from(text),
            color: crate::window::PALETTE_TEXT,
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
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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

        // Draw background if set
        if let Some(bg) = self.background {
            device.fill_rect(x, y, width, height, bg);
        }

        // Draw text
        if !self.text.is_empty() {
            let font = get_default_font();
            let char_width = font.cell_width();
            let text_width = (self.text.len() as u32) * char_width;

            // Calculate x position based on alignment
            let text_x = match self.align {
                TextAlign::Left => x + 2, // Small padding
                TextAlign::Center => {
                    if text_width < width {
                        x + ((width - text_width) / 2) as i32
                    } else {
                        x + 2
                    }
                }
                TextAlign::Right => {
                    if text_width < width {
                        x + (width - text_width) as i32 - 2
                    } else {
                        x + 2
                    }
                }
            };

            // Center vertically
            let text_y = y + (height.saturating_sub(font.line_height()) / 2) as i32;

            device.draw_text(text_x, text_y, &self.text, font.as_font(), self.color);
        }

        self.base.clear_needs_repaint();
    }

    fn as_label_mut(&mut self) -> Option<&mut Label> {
        Some(self)
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Labels don't handle events
        EventResult::Ignored
    }
}
