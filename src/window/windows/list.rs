//! List widget for displaying selectable items

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice};
use crate::window::event::MouseEventType;
use super::base::WindowBase;

/// Callback type for selection change events
pub type SelectionCallback = Box<dyn FnMut(usize) + Send>;

/// A simple single-column list widget with selection
pub struct List {
    /// Base window functionality
    base: WindowBase,
    /// List items
    items: Vec<String>,
    /// Currently selected item index
    selected_index: Option<usize>,
    /// Scroll offset (first visible item index)
    scroll_offset: usize,
    /// Height of each item in pixels
    item_height: usize,
    /// Selection change callback
    on_select: Option<SelectionCallback>,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Selected item background color
    selected_bg_color: Color,
    /// Selected item text color
    selected_text_color: Color,
}

impl List {
    /// Create a new list with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        List {
            base: WindowBase::new_with_id(id, bounds),
            items: Vec::new(),
            selected_index: None,
            scroll_offset: 0,
            item_height: 16, // 8px font + 8px padding
            on_select: None,
            bg_color: Color::WHITE,
            text_color: Color::BLACK,
            selected_bg_color: Color::BLUE,
            selected_text_color: Color::WHITE,
        }
    }

    /// Create a new list (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Add an item to the list
    pub fn add_item(&mut self, text: &str) {
        self.items.push(String::from(text));
        self.base.invalidate();
    }

    /// Add multiple items at once
    pub fn add_items(&mut self, texts: &[&str]) {
        for text in texts {
            self.items.push(String::from(*text));
        }
        self.base.invalidate();
    }

    /// Clear all items
    pub fn clear(&mut self) {
        self.items.clear();
        self.selected_index = None;
        self.scroll_offset = 0;
        self.base.invalidate();
    }

    /// Get the number of items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if the list is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get the currently selected index
    pub fn selected(&self) -> Option<usize> {
        self.selected_index
    }

    /// Get the selected item text
    pub fn selected_item(&self) -> Option<&str> {
        self.selected_index.map(|i| self.items[i].as_str())
    }

    /// Set the selected index
    pub fn set_selected(&mut self, index: Option<usize>) {
        let new_index = index.filter(|&i| i < self.items.len());
        if self.selected_index != new_index {
            self.selected_index = new_index;
            self.base.invalidate();
            // Ensure selected item is visible
            if let Some(idx) = new_index {
                self.ensure_visible(idx);
            }
        }
    }

    /// Set the selection change callback
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(usize) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Set item height
    pub fn set_item_height(&mut self, height: usize) {
        if self.item_height != height {
            self.item_height = height;
            self.base.invalidate();
        }
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

    /// Set selected background color
    pub fn set_selected_bg_color(&mut self, color: Color) {
        self.selected_bg_color = color;
        self.base.invalidate();
    }

    /// Set selected text color
    pub fn set_selected_text_color(&mut self, color: Color) {
        self.selected_text_color = color;
        self.base.invalidate();
    }

    /// Calculate how many items can be displayed
    fn visible_items(&self) -> usize {
        let bounds = self.base.bounds();
        (bounds.height as usize) / self.item_height
    }

    /// Ensure an item is visible by adjusting scroll offset
    fn ensure_visible(&mut self, index: usize) {
        if index < self.scroll_offset {
            self.scroll_offset = index;
            self.base.invalidate();
        } else if index >= self.scroll_offset + self.visible_items() {
            self.scroll_offset = index.saturating_sub(self.visible_items() - 1);
            self.base.invalidate();
        }
    }

    /// Convert y coordinate to item index
    fn y_to_index(&self, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let relative_y = y - bounds.y;
        if relative_y < 0 {
            return None;
        }
        let index = self.scroll_offset + (relative_y as usize) / self.item_height;
        if index < self.items.len() {
            Some(index)
        } else {
            None
        }
    }
}

impl Window for List {
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

        // Draw background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw border
        device.draw_rect(x, y, width, height, Color::GRAY);

        // Draw items
        let font = get_default_font();
        let visible_count = self.visible_items();
        let padding = 4;

        for i in 0..visible_count {
            let item_index = self.scroll_offset + i;
            if item_index >= self.items.len() {
                break;
            }

            let item_y = y + i * self.item_height;
            let is_selected = self.selected_index == Some(item_index);

            // Draw item background
            if is_selected {
                device.fill_rect(x + 1, item_y, width - 2, self.item_height, self.selected_bg_color);
            }

            // Draw item text
            let text_color = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            let text_y = item_y + (self.item_height - 8) / 2; // Center vertically
            device.draw_text(
                x + padding,
                text_y,
                &self.items[item_index],
                font.as_font(),
                text_color,
            );
        }

        // Draw scrollbar if needed
        if self.items.len() > visible_count && visible_count > 0 {
            let scrollbar_width = 8;
            let scrollbar_x = x + width - scrollbar_width - 1;
            let scrollbar_height = height - 2;

            // Scrollbar track
            device.fill_rect(scrollbar_x, y + 1, scrollbar_width, scrollbar_height, Color::LIGHT_GRAY);

            // Scrollbar thumb
            let thumb_height = (visible_count * scrollbar_height) / self.items.len();
            let thumb_height = thumb_height.max(10); // Minimum thumb size
            let thumb_pos = (self.scroll_offset * scrollbar_height) / self.items.len();
            device.fill_rect(
                scrollbar_x,
                y + 1 + thumb_pos,
                scrollbar_width,
                thumb_height,
                Color::GRAY,
            );
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let bounds = self.base.bounds();
                if !bounds.contains_point(mouse_event.position) {
                    return EventResult::Ignored;
                }

                match mouse_event.event_type {
                    MouseEventType::ButtonDown if mouse_event.buttons.left => {
                        if let Some(index) = self.y_to_index(mouse_event.position.y) {
                            if self.selected_index != Some(index) {
                                self.selected_index = Some(index);
                                self.base.invalidate();
                                if let Some(ref mut callback) = self.on_select {
                                    callback(index);
                                }
                            }
                        }
                        EventResult::Handled
                    }
                    MouseEventType::Scroll => {
                        // Handle scroll wheel if implemented
                        EventResult::Ignored
                    }
                    _ => EventResult::Ignored,
                }
            }
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                use crate::window::event::KeyCode;
                match kbd_event.key_code {
                    KeyCode::Up => {
                        if let Some(idx) = self.selected_index {
                            if idx > 0 {
                                self.set_selected(Some(idx - 1));
                                if let Some(ref mut callback) = self.on_select {
                                    callback(idx - 1);
                                }
                            }
                        } else if !self.items.is_empty() {
                            self.set_selected(Some(0));
                            if let Some(ref mut callback) = self.on_select {
                                callback(0);
                            }
                        }
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        if let Some(idx) = self.selected_index {
                            if idx + 1 < self.items.len() {
                                self.set_selected(Some(idx + 1));
                                if let Some(ref mut callback) = self.on_select {
                                    callback(idx + 1);
                                }
                            }
                        } else if !self.items.is_empty() {
                            self.set_selected(Some(0));
                            if let Some(ref mut callback) = self.on_select {
                                callback(0);
                            }
                        }
                        EventResult::Handled
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
        true
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }
}
