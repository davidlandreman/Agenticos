#![allow(dead_code)]
//! Button widget with mouse click support

use super::base::WindowBase;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::theme::controls::{self, ControlState};
use crate::window::{Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowId};
use alloc::boxed::Box;
use alloc::string::String;

/// Callback type for button click events
pub type ButtonCallback = Box<dyn FnMut() + Send>;

/// A clickable button widget. The surface (face, bevel/gradient, border)
/// comes from the active Classic/Aero theme via `theme::controls`.
pub struct Button {
    /// Base window functionality
    base: WindowBase,
    /// Button label text
    label: String,
    /// Whether the button is currently pressed (mouse down)
    pressed: bool,
    /// Click callback
    on_click: Option<ButtonCallback>,
    /// Default / accent button (Aero: blue border + glow; Classic: black rim).
    default_button: bool,
    /// Whether the button is enabled. Disabled buttons paint in a
    /// greyed-out state and ignore `ButtonDown` / `ButtonUp` events
    /// (the click callback never fires).
    enabled: bool,
    /// Taskbar-hosted button: painted through the chrome surface helpers
    /// (frosted pill under Futurism, ordinary button otherwise).
    taskbar_style: bool,
    /// Accent-tinted chrome button (the Start button).
    taskbar_accent: bool,
}

impl Button {
    /// Create a new button with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, label: &str) -> Self {
        Button {
            base: WindowBase::new_with_id(id, bounds),
            label: String::from(label),
            pressed: false,
            on_click: None,
            default_button: false,
            enabled: true,
            taskbar_style: false,
            taskbar_accent: false,
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

    /// Mark this button as the dialog's default / accent button. The theme
    /// renders it distinctly (Aero: blue border + glow; Classic: black rim).
    pub fn set_default(&mut self, default_button: bool) {
        if self.default_button != default_button {
            self.default_button = default_button;
            self.base.invalidate();
        }
    }

    /// Mark this button as taskbar chrome. Classic/Aero keep the ordinary
    /// button surface; Futurism paints translucent rounded pills (the accent
    /// pill is the Start button).
    pub fn set_taskbar_style(&mut self, accent: bool) {
        self.taskbar_style = true;
        self.taskbar_accent = accent;
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
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;

        let state = if !self.enabled {
            ControlState::Disabled
        } else if self.pressed {
            ControlState::Pressed
        } else if self.default_button {
            ControlState::Hot
        } else {
            ControlState::Normal
        };

        if self.taskbar_style {
            controls::draw_task_button(device, bounds, state, self.taskbar_accent);
        } else {
            controls::draw_button(device, bounds, state);
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

            // Classic shifts the label down-right while pressed.
            let shift = controls::pressed_label_shift(state);
            let color = if self.taskbar_style {
                controls::task_button_text(state, self.taskbar_accent)
            } else {
                controls::button_text(state)
            };
            device.draw_text(
                text_x + shift,
                text_y + shift,
                &self.label,
                font.as_font(),
                color,
            );
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
                    MouseEventType::ButtonDown | MouseEventType::ButtonUp if !self.enabled => {
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
