//! Taskbar window that displays Start button and window buttons

use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::window::{Window, WindowId, Rect, Point, Event, EventResult, GraphicsDevice};
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
pub struct TaskbarButton {
    /// ID of the button widget
    pub button_id: WindowId,
    /// ID of the frame window this button represents
    pub frame_id: WindowId,
    /// Title of the window
    pub title: String,
}

/// The taskbar window
pub struct TaskbarWindow {
    /// Base window functionality
    base: WindowBase,
    /// Background color
    bg_color: Color,
    /// ID of the Start button
    start_button_id: Option<WindowId>,
    /// Window buttons for open frame windows
    window_buttons: Vec<TaskbarButton>,
    /// Currently active window (for highlighting its button)
    active_frame_id: Option<WindowId>,
    /// Screen width (for layout calculations)
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
            bg_color: Color::new(192, 192, 192),
            start_button_id: None,
            window_buttons: Vec::new(),
            active_frame_id: None,
            screen_width,
        }
    }

    /// Create a new taskbar (generates its own ID)
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self::new_with_id(WindowId::new(), screen_width, screen_height)
    }

    /// Set the Start button ID
    pub fn set_start_button(&mut self, button_id: WindowId) {
        self.start_button_id = Some(button_id);
    }

    /// Get the Start button ID
    pub fn start_button_id(&self) -> Option<WindowId> {
        self.start_button_id
    }

    /// Add a window button for a frame window
    pub fn add_window_button(&mut self, button_id: WindowId, frame_id: WindowId, title: &str) {
        self.window_buttons.push(TaskbarButton {
            button_id,
            frame_id,
            title: String::from(title),
        });
        self.base.invalidate();
    }

    /// Remove a window button by frame ID
    pub fn remove_window_button(&mut self, frame_id: WindowId) -> Option<WindowId> {
        if let Some(pos) = self.window_buttons.iter().position(|b| b.frame_id == frame_id) {
            let removed = self.window_buttons.remove(pos);
            self.base.invalidate();
            Some(removed.button_id)
        } else {
            None
        }
    }

    /// Get the button ID for a frame window
    pub fn get_button_for_frame(&self, frame_id: WindowId) -> Option<WindowId> {
        self.window_buttons
            .iter()
            .find(|b| b.frame_id == frame_id)
            .map(|b| b.button_id)
    }

    /// Get the frame ID for a button
    pub fn get_frame_for_button(&self, button_id: WindowId) -> Option<WindowId> {
        self.window_buttons
            .iter()
            .find(|b| b.button_id == button_id)
            .map(|b| b.frame_id)
    }

    /// Set the currently active window
    pub fn set_active_window(&mut self, frame_id: Option<WindowId>) {
        if self.active_frame_id != frame_id {
            self.active_frame_id = frame_id;
            self.base.invalidate();
        }
    }

    /// Get the active window
    pub fn active_window(&self) -> Option<WindowId> {
        self.active_frame_id
    }

    /// Get the window buttons
    pub fn window_buttons(&self) -> &[TaskbarButton] {
        &self.window_buttons
    }

    /// Calculate button bounds for layout
    /// Returns a vector of (button_id, bounds) for all window buttons
    pub fn calculate_button_layout(&self) -> Vec<(WindowId, Rect)> {
        let mut result = Vec::new();

        if self.window_buttons.is_empty() {
            return result;
        }

        // Start after the Start button
        let start_x = BUTTON_GAP + START_BUTTON_WIDTH + BUTTON_GAP;
        let available_width = self.screen_width.saturating_sub(start_x + BUTTON_GAP);
        let button_count = self.window_buttons.len() as u32;

        // Calculate width per button
        let total_gaps = (button_count.saturating_sub(1)) * BUTTON_GAP;
        let available_for_buttons = available_width.saturating_sub(total_gaps);
        let button_width = (available_for_buttons / button_count).min(MAX_WINDOW_BUTTON_WIDTH);

        for (i, btn) in self.window_buttons.iter().enumerate() {
            let x = start_x + (i as u32 * (button_width + BUTTON_GAP));
            let bounds = Rect::new(
                x as i32,
                BUTTON_Y_OFFSET as i32,
                button_width,
                BUTTON_HEIGHT,
            );
            result.push((btn.button_id, bounds));
        }

        result
    }

    /// Get the bounds for the Start button
    pub fn start_button_bounds(&self) -> Rect {
        Rect::new(
            BUTTON_GAP as i32,
            BUTTON_Y_OFFSET as i32,
            START_BUTTON_WIDTH,
            BUTTON_HEIGHT,
        )
    }

    /// Check if a frame is tracked by the taskbar
    pub fn has_window_button(&self, frame_id: WindowId) -> bool {
        self.window_buttons.iter().any(|b| b.frame_id == frame_id)
    }
}

impl Window for TaskbarWindow {
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

        // Draw taskbar background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw top border (highlight)
        device.draw_line(x, y, x + width - 1, y, Color::WHITE);

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

    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }

    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }

    fn can_focus(&self) -> bool {
        // Taskbar cannot receive focus
        false
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
