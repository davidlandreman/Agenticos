//! Button widget with mouse click support

use alloc::boxed::Box;
use alloc::string::String;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Point, Event, EventResult, GraphicsDevice};
use crate::window::event::MouseEventType;
use super::base::WindowBase;

/// Callback type for button click events
pub type ButtonCallback = Box<dyn FnMut() + Send>;

/// A clickable button widget
pub struct Button {
    /// Base window functionality
    base: WindowBase,
    /// Button label text
    label: String,
    /// Whether the button is currently pressed (mouse down)
    pressed: bool,
    /// Click callback
    on_click: Option<ButtonCallback>,
    /// Normal background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Whether the button is enabled. Disabled buttons paint in a
    /// greyed-out state and ignore `ButtonDown` / `ButtonUp` events
    /// (the click callback never fires).
    enabled: bool,
}

impl Button {
    /// Create a new button with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, label: &str) -> Self {
        Button {
            base: WindowBase::new_with_id(id, bounds),
            label: String::from(label),
            pressed: false,
            on_click: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            enabled: true,
        }
    }

    /// Create a new button (generates its own ID)
    pub fn new(bounds: Rect, label: &str) -> Self {
        Self::new_with_id(WindowId::new(), bounds, label)
    }

    /// Set the click callback
    pub fn on_click<F>(&mut self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.on_click = Some(Box::new(callback));
    }

    /// Set the button label
    pub fn set_label(&mut self, label: &str) {
        if self.label != label {
            self.label = String::from(label);
            self.base.invalidate();
        }
    }

    /// Get the button label
    pub fn label(&self) -> &str {
        &self.label
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

    /// Set whether the button is enabled. When disabled, the button
    /// paints in a greyed-out state and ignores `ButtonDown` / `ButtonUp`
    /// events (the click callback never fires).
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            // Drop any in-flight pressed state when disabling so the
            // visual doesn't get stuck mid-click.
            if !enabled {
                self.pressed = false;
            }
            self.base.invalidate();
        }
    }

    /// Returns whether the button is currently enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Check if a point is within the button bounds
    /// Note: point is expected to be in local coordinates (relative to button's top-left)
    fn contains_point(&self, point: Point) -> bool {
        let bounds = self.base.bounds();
        // Check against a local rect at (0,0) with the button's size
        point.x >= 0
            && point.y >= 0
            && point.x < bounds.width as i32
            && point.y < bounds.height as i32
    }
}

impl Window for Button {
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
        if !self.base.needs_repaint() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;
        let right = x + width as i32 - 1;
        let bottom = y + height as i32 - 1;

        // Colors for 3D effect
        let highlight = Color::WHITE;
        let shadow = Color::DARK_GRAY;

        // Disabled buttons paint with a lighter background and a
        // mid-grey label; the 3D edge highlight/shadow still draws so
        // the bounds remain visible, just muted.
        let (bg, label_color) = if self.enabled {
            (self.bg_color, self.text_color)
        } else {
            // Lighter than the default LIGHT_GRAY (192/192/192).
            let disabled_bg = Color {
                red: 224,
                green: 224,
                blue: 224,
            };
            (disabled_bg, Color::GRAY)
        };

        // Draw button background
        device.fill_rect(x, y, width, height, bg);

        if self.pressed {
            // Pressed state: shadow on top/left, highlight on bottom/right
            device.draw_line(x, y, right, y, shadow);
            device.draw_line(x, y, x, bottom, shadow);
            device.draw_line(x, bottom, right, bottom, highlight);
            device.draw_line(right, y, right, bottom, highlight);
        } else {
            // Normal state: highlight on top/left, shadow on bottom/right
            device.draw_line(x, y, right, y, highlight);
            device.draw_line(x, y, x, bottom, highlight);
            device.draw_line(x, bottom, right, bottom, shadow);
            device.draw_line(right, y, right, bottom, shadow);
        }

        // Draw label centered
        if !self.label.is_empty() {
            let font = get_default_font();
            let char_width = font.cell_width();
            let char_height = font.line_height();
            let text_width = (self.label.len() as u32) * char_width;

            // Center text in button
            let text_x = if text_width < width {
                x + ((width - text_width) / 2) as i32
            } else {
                x + 2
            };
            let text_y = if char_height < height {
                y + ((height - char_height) / 2) as i32
            } else {
                y + 2
            };

            // Offset text slightly when pressed for visual feedback
            let (draw_x, draw_y) = if self.pressed {
                (text_x + 1, text_y + 1)
            } else {
                (text_x, text_y)
            };

            device.draw_text(draw_x, draw_y, &self.label, font.as_font(), label_color);
        }

        self.base.clear_needs_repaint();
    }

    fn as_button_mut(&mut self) -> Option<&mut Button> {
        Some(self)
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let in_bounds = self.contains_point(mouse_event.position);

                match mouse_event.event_type {
                    // Disabled buttons ignore press/release entirely so
                    // they neither flip pressed state nor fire callbacks.
                    MouseEventType::ButtonDown | MouseEventType::ButtonUp
                        if !self.enabled =>
                    {
                        EventResult::Ignored
                    }
                    MouseEventType::ButtonDown if in_bounds && mouse_event.buttons.left => {
                        self.pressed = true;
                        self.base.invalidate();
                        EventResult::Handled
                    }
                    MouseEventType::ButtonUp if self.pressed => {
                        let was_pressed = self.pressed;
                        self.pressed = false;
                        self.base.invalidate();

                        // Only trigger click if mouse is still over button
                        if was_pressed && in_bounds {
                            if let Some(ref mut callback) = self.on_click {
                                callback();
                            }
                        }
                        EventResult::Handled
                    }
                    MouseEventType::Move => {
                        // If we're in pressed state and mouse moves outside, we might want to show visual feedback
                        // For now, we keep pressed state until mouse up
                        EventResult::Ignored
                    }
                    _ => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }

}
