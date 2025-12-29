//! Menu window for popup menus (like Start menu)

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Point, Event, EventResult, GraphicsDevice};
use crate::window::event::MouseEventType;
use super::base::WindowBase;

/// Height of each menu item in pixels
pub const MENU_ITEM_HEIGHT: u32 = 24;
/// Horizontal padding for menu items
pub const MENU_ITEM_PADDING: u32 = 8;

/// Callback type for menu item selection
pub type MenuSelectCallback = Box<dyn FnMut(usize) + Send>;

/// A menu item with a label
pub struct MenuItem {
    /// The display text for this item
    pub label: String,
}

impl MenuItem {
    /// Create a new menu item
    pub fn new(label: &str) -> Self {
        MenuItem {
            label: String::from(label),
        }
    }
}

/// A popup menu window
pub struct MenuWindow {
    /// Base window functionality
    base: WindowBase,
    /// Menu items
    items: Vec<MenuItem>,
    /// Currently hovered item index
    hover_index: Option<usize>,
    /// Callback when an item is selected
    on_select: Option<MenuSelectCallback>,
    /// Background color
    bg_color: Color,
    /// Border color
    border_color: Color,
    /// Text color
    text_color: Color,
    /// Hover background color
    hover_bg_color: Color,
}

impl MenuWindow {
    /// Create a new menu window with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        MenuWindow {
            base: WindowBase::new_with_id(id, bounds),
            items: Vec::new(),
            hover_index: None,
            on_select: None,
            bg_color: Color::new(240, 240, 240),
            border_color: Color::new(100, 100, 100),
            text_color: Color::BLACK,
            hover_bg_color: Color::new(0, 120, 215),
        }
    }

    /// Create a new menu window (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Add an item to the menu
    pub fn add_item(&mut self, label: &str) {
        self.items.push(MenuItem::new(label));
        self.base.invalidate();
    }

    /// Clear all items
    pub fn clear_items(&mut self) {
        self.items.clear();
        self.hover_index = None;
        self.base.invalidate();
    }

    /// Get the number of items
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Set the selection callback
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(usize) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Calculate the required height for all items
    pub fn calculate_height(&self) -> u32 {
        let item_count = self.items.len() as u32;
        item_count * MENU_ITEM_HEIGHT + 4 // 2px border top and bottom
    }

    /// Get the item index at a given y position (local coordinates)
    fn item_at_position(&self, y: i32) -> Option<usize> {
        if y < 2 || y >= (self.base.bounds().height as i32 - 2) {
            return None;
        }

        let item_y = (y - 2) as u32;
        let index = (item_y / MENU_ITEM_HEIGHT) as usize;

        if index < self.items.len() {
            Some(index)
        } else {
            None
        }
    }

    /// Check if a point is within the menu bounds
    fn contains_point(&self, point: Point) -> bool {
        let bounds = self.base.bounds();
        point.x >= 0
            && point.y >= 0
            && point.x < bounds.width as i32
            && point.y < bounds.height as i32
    }
}

impl Window for MenuWindow {
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

        let bounds = self.base.bounds();
        let x = bounds.x as usize;
        let y = bounds.y as usize;
        let width = bounds.width as usize;
        let height = bounds.height as usize;

        // Draw background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw border
        device.draw_rect(x, y, width, height, self.border_color);

        // Draw items
        let font = get_default_font();
        let char_height = 8;

        for (i, item) in self.items.iter().enumerate() {
            let item_y = y + 2 + i * MENU_ITEM_HEIGHT as usize;
            let item_height = MENU_ITEM_HEIGHT as usize;

            // Draw hover background if this item is hovered
            if self.hover_index == Some(i) {
                device.fill_rect(
                    x + 2,
                    item_y,
                    width - 4,
                    item_height,
                    self.hover_bg_color,
                );
            }

            // Draw item text
            let text_x = x + MENU_ITEM_PADDING as usize;
            let text_y = item_y + (item_height - char_height) / 2;

            let text_color = if self.hover_index == Some(i) {
                Color::WHITE
            } else {
                self.text_color
            };

            device.draw_text(text_x, text_y, &item.label, font.as_font(), text_color);
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let in_bounds = self.contains_point(mouse_event.position);

                match mouse_event.event_type {
                    MouseEventType::Move => {
                        if in_bounds {
                            let new_hover = self.item_at_position(mouse_event.position.y);
                            if new_hover != self.hover_index {
                                self.hover_index = new_hover;
                                self.base.invalidate();
                            }
                        } else {
                            if self.hover_index.is_some() {
                                self.hover_index = None;
                                self.base.invalidate();
                            }
                        }
                        EventResult::Handled
                    }
                    MouseEventType::ButtonDown if mouse_event.buttons.left => {
                        if in_bounds {
                            // Item will be selected on button up
                            EventResult::Handled
                        } else {
                            // Click outside - signal to close menu
                            // Return Propagate so the window manager knows to dismiss
                            EventResult::Propagate
                        }
                    }
                    MouseEventType::ButtonUp => {
                        // On button up, check if we're over an item and select it
                        if in_bounds {
                            if let Some(index) = self.item_at_position(mouse_event.position.y) {
                                // Trigger callback
                                if let Some(ref mut callback) = self.on_select {
                                    callback(index);
                                }
                            }
                        }
                        EventResult::Handled
                    }
                    _ => EventResult::Handled,
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
        false
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
