use crate::graphics::color::Color;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::vec::Vec;
use super::base::WindowBase;

/// A desktop window that provides a background
pub struct DesktopWindow {
    base: WindowBase,
    background_color: Color,
}

impl DesktopWindow {
    pub fn new(_id: WindowId, bounds: Rect) -> Self {
        Self {
            base: WindowBase::new(bounds),
            background_color: Color::new(0, 50, 100), // Nice blue desktop color
        }
    }
}

impl Window for DesktopWindow {
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


    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        // Fill the entire desktop with the background color
        let bounds = self.base.bounds();
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            bounds.height as usize,
            self.background_color,
        );

        self.base.clear_needs_repaint();
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }

    fn handle_event(&mut self, _event: Event) -> EventResult {
        EventResult::Ignored
    }

    fn can_focus(&self) -> bool {
        false
    }

    fn has_focus(&self) -> bool {
        false
    }

    fn parent(&self) -> Option<WindowId> {
        self.base.parent()  // Should always be None for desktop
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

    fn set_focus(&mut self, _focused: bool) {
        // Desktop never has focus
    }
}