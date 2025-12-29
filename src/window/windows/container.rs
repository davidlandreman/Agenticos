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
    pub fn new(bounds: Rect) -> Self {
        ContainerWindow {
            base: WindowBase::new(bounds),
            background_color: Color::new(240, 240, 240), // Light gray
        }
    }

    /// Create a new container window with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        ContainerWindow {
            base: WindowBase::new_with_id(id, bounds),
            background_color: Color::new(240, 240, 240), // Light gray
        }
    }
    
    /// Set the background color
    pub fn set_background_color(&mut self, color: Color) {
        self.background_color = color;
        self.base.invalidate();
    }
}

impl Window for ContainerWindow {
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
        if !self.visible() {
            return;
        }
        
        // Fill background
        let bounds = self.bounds();
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            bounds.height as usize,
            self.background_color,
        );
        
        // Clear repaint flag
        self.base.clear_needs_repaint();
    }
    
    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }
    
    fn invalidate(&mut self) {
        self.base.invalidate();
    }
    
    fn handle_event(&mut self, _event: Event) -> EventResult {
        // Container doesn't handle events by default
        EventResult::Propagate
    }
    
    fn can_focus(&self) -> bool {
        self.base.can_focus()
    }
    
    fn has_focus(&self) -> bool {
        self.base.has_focus()
    }
    
    fn set_focus(&mut self, focused: bool) {
        self.base.set_focus(focused);
    }
}