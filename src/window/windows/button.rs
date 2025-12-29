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
}

impl Button {
    /// Create a new button with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, label: &str) -> Self {
        Button {
            base: WindowBase::new_with_id(id, bounds),
            label: String::from(label),
            pressed: false,
            on_click: None,
            bg_color: Color::LIGHT_GRAY,
            text_color: Color::BLACK,
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

        // Colors for 3D effect
        let highlight = Color::WHITE;
        let shadow = Color::DARK_GRAY;

        // Draw button background
        device.fill_rect(x, y, width, height, self.bg_color);

        if self.pressed {
            // Pressed state: shadow on top/left, highlight on bottom/right
            // Top edge
            device.draw_line(x, y, x + width - 1, y, shadow);
            // Left edge
            device.draw_line(x, y, x, y + height - 1, shadow);
            // Bottom edge
            device.draw_line(x, y + height - 1, x + width - 1, y + height - 1, highlight);
            // Right edge
            device.draw_line(x + width - 1, y, x + width - 1, y + height - 1, highlight);
        } else {
            // Normal state: highlight on top/left, shadow on bottom/right
            // Top edge
            device.draw_line(x, y, x + width - 1, y, highlight);
            // Left edge
            device.draw_line(x, y, x, y + height - 1, highlight);
            // Bottom edge
            device.draw_line(x, y + height - 1, x + width - 1, y + height - 1, shadow);
            // Right edge
            device.draw_line(x + width - 1, y, x + width - 1, y + height - 1, shadow);
        }

        // Draw label centered
        if !self.label.is_empty() {
            let font = get_default_font();
            let char_width = 8; // Default font is 8x8
            let char_height = 8;
            let text_width = self.label.len() * char_width;

            // Center text in button
            let text_x = if text_width < width {
                x + (width - text_width) / 2
            } else {
                x + 2
            };
            let text_y = if char_height < height {
                y + (height - char_height) / 2
            } else {
                y + 2
            };

            // Offset text slightly when pressed for visual feedback
            let (draw_x, draw_y) = if self.pressed {
                (text_x + 1, text_y + 1)
            } else {
                (text_x, text_y)
            };

            device.draw_text(draw_x, draw_y, &self.label, font.as_font(), self.text_color);
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let in_bounds = self.contains_point(mouse_event.position);

                match mouse_event.event_type {
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

    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }

    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }

    fn can_focus(&self) -> bool {
        // Buttons can receive focus for future keyboard support
        false
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
