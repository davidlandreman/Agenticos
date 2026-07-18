//! Container window that can hold child windows

use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use crate::graphics::color::Color;
use super::base::WindowBase;

/// A window that can contain other windows
pub struct ContainerWindow {
    /// Base window functionality
    base: WindowBase,
    /// Background color
    background_color: Color,
}

impl ContainerWindow {
    /// Create a new container window
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect) -> Self {
        ContainerWindow {
            base: WindowBase::new(bounds),
            background_color: crate::window::PALETTE_CONTENT_BG,
        }
    }

    /// Create a new container window with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        ContainerWindow {
            base: WindowBase::new_with_id(id, bounds),
            background_color: crate::window::PALETTE_CONTENT_BG,
        }
    }
    
    /// Set the background color
    pub fn set_background_color(&mut self, color: Color) {
        self.background_color = color;
        self.base.invalidate();
    }
}

impl Window for ContainerWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.visible() {
            return;
        }

        // Fill background
        let bounds = self.bounds();
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            self.background_color,
        );

        // Clear repaint flag
        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Container doesn't handle events by default
        EventResult::Propagate
    }

    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }
}