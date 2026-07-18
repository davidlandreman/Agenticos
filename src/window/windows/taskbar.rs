//! Taskbar window that displays Start button and window buttons

use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use super::base::WindowBase;

/// Height of the taskbar in pixels
pub const TASKBAR_HEIGHT: u32 = 32;
/// Width of the Start button
pub const START_BUTTON_WIDTH: u32 = 60;
/// Maximum width of window buttons
pub const MAX_WINDOW_BUTTON_WIDTH: u32 = 150;
/// Gap between buttons
pub const BUTTON_GAP: u32 = 4;
/// Button height (with some padding from top/bottom)
pub const BUTTON_HEIGHT: u32 = 24;
/// Vertical offset for buttons
pub const BUTTON_Y_OFFSET: u32 = 4;

/// Tracks a window button on the taskbar
#[derive(Debug, Clone)]
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub struct TaskbarButton {
    /// ID of the button widget
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub button_id: WindowId,
    /// ID of the frame window this button represents
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub frame_id: WindowId,
    /// Title of the window
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub title: String,
}

/// The taskbar window
pub struct TaskbarWindow {
    /// Base window functionality
    base: WindowBase,
    /// Background color
    bg_color: Color,
    /// ID of the Start button
    #[expect(dead_code, reason = "intentional kernel API surface")]
    start_button_id: Option<WindowId>,
    /// Window buttons for open frame windows
    #[expect(dead_code, reason = "intentional kernel API surface")]
    window_buttons: Vec<TaskbarButton>,
    /// Currently active window (for highlighting its button)
    #[expect(dead_code, reason = "intentional kernel API surface")]
    active_frame_id: Option<WindowId>,
    /// Screen width (for layout calculations)
    #[expect(dead_code, reason = "intentional kernel API surface")]
    screen_width: u32,
}

impl TaskbarWindow {
    /// Create a new taskbar with a specific ID
    pub fn new_with_id(id: WindowId, screen_width: u32, screen_height: u32) -> Self {
        let bounds = Rect::new(
            0,
            (screen_height - TASKBAR_HEIGHT) as i32,
            screen_width,
            TASKBAR_HEIGHT,
        );

        TaskbarWindow {
            base: WindowBase::new_with_id(id, bounds),
            bg_color: crate::window::PALETTE_CONTENT_BG,
            start_button_id: None,
            window_buttons: Vec::new(),
            active_frame_id: None,
            screen_width,
        }
    }

    }

impl Window for TaskbarWindow {
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

        // Draw taskbar background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw top border (highlight)
        device.draw_line(x, y, x + width as i32 - 1, y, Color::WHITE);

        // Note: Child buttons will paint themselves

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        // Taskbar doesn't handle events directly - they go to child buttons
        match event {
            Event::Mouse(_) => EventResult::Propagate,
            _ => EventResult::Ignored,
        }
    }
}
