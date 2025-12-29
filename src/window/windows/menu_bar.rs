//! Menu bar widget with dropdown menus
//!
//! A horizontal bar at the top of a window containing clickable menu titles
//! that open dropdown menus. Popups are displayed as separate windows for
//! proper z-ordering and event handling.

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::{boxed::Box, string::String, vec::Vec};

use super::base::WindowBase;

/// Height of the menu bar in pixels
pub const MENU_BAR_HEIGHT: u32 = 24;

/// Padding around menu titles
const MENU_TITLE_PADDING: usize = 12;

/// Callback type for menu item selection (menu_index, item_id)
pub type MenuSelectCallback = Box<dyn FnMut(usize, usize) + Send>;

/// A menu item definition
#[derive(Debug, Clone)]
pub enum MenuItemDef {
    /// Regular menu item with label, optional shortcut, and id
    Item {
        label: String,
        shortcut: Option<String>,
        id: usize,
    },
    /// Separator line between items
    Separator,
}

impl MenuItemDef {
    /// Create a new menu item
    pub fn item(label: &str, id: usize) -> Self {
        MenuItemDef::Item {
            label: String::from(label),
            shortcut: None,
            id,
        }
    }

    /// Create a new menu item with shortcut
    pub fn item_with_shortcut(label: &str, shortcut: &str, id: usize) -> Self {
        MenuItemDef::Item {
            label: String::from(label),
            shortcut: Some(String::from(shortcut)),
            id,
        }
    }

    /// Create a separator
    pub fn separator() -> Self {
        MenuItemDef::Separator
    }
}

/// A menu in the menu bar
pub struct Menu {
    /// Menu title (displayed in the bar)
    pub title: String,
    /// Menu items
    pub items: Vec<MenuItemDef>,
    /// Calculated width in pixels
    width: usize,
    /// X position in the bar
    x: usize,
}

impl Menu {
    /// Create a new menu
    pub fn new(title: &str, items: Vec<MenuItemDef>) -> Self {
        let font = get_default_font();
        let char_width = font.char_width();
        let width = title.len() * char_width + MENU_TITLE_PADDING * 2;

        Menu {
            title: String::from(title),
            items,
            width,
            x: 0,
        }
    }
}

/// Information about an open popup that needs to be created by the window manager
#[derive(Clone)]
pub struct PendingPopup {
    /// Absolute screen position for the popup
    pub x: i32,
    pub y: i32,
    /// Popup dimensions
    pub width: u32,
    pub height: u32,
    /// Menu items to display
    pub items: Vec<MenuItemDef>,
    /// Menu index this popup belongs to
    pub menu_index: usize,
}

/// A horizontal menu bar
pub struct MenuBar {
    /// Base window functionality
    base: WindowBase,
    /// Menus in the bar
    menus: Vec<Menu>,
    /// Currently open menu index (if any)
    open_menu_index: Option<usize>,
    /// Currently hovered menu index in the bar
    hover_index: Option<usize>,
    /// Callback when menu item is selected
    on_select: Option<MenuSelectCallback>,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Hover background color
    hover_bg_color: Color,
    /// ID of the popup window (if open)
    popup_window_id: Option<WindowId>,
    /// Pending popup to be created (set by open_menu, consumed by poll_pending_popup)
    pending_popup: Option<PendingPopup>,
    /// Global offset for calculating popup position (set during paint)
    global_offset: (i32, i32),
}

impl MenuBar {
    /// Create a new menu bar with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let base = WindowBase::new_with_id(id, bounds);

        MenuBar {
            base,
            menus: Vec::new(),
            open_menu_index: None,
            hover_index: None,
            on_select: None,
            bg_color: Color::new(240, 240, 240),
            text_color: Color::BLACK,
            hover_bg_color: Color::new(200, 200, 200),
            popup_window_id: None,
            pending_popup: None,
            global_offset: (0, 0),
        }
    }

    /// Create a new menu bar (generates its own ID)
    pub fn new(bounds: Rect) -> Self {
        Self::new_with_id(WindowId::new(), bounds)
    }

    /// Add a menu to the bar
    pub fn add_menu(&mut self, title: &str, items: Vec<MenuItemDef>) {
        let mut menu = Menu::new(title, items);

        // Calculate x position based on existing menus
        let x = self.menus.iter().map(|m| m.width).sum();
        menu.x = x;

        self.menus.push(menu);
        self.base.invalidate();
    }

    /// Set the selection callback
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(usize, usize) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Close any open popup menu
    pub fn close_menu(&mut self) {
        if self.open_menu_index.is_some() {
            self.open_menu_index = None;
            self.popup_window_id = None;
            self.base.invalidate();
        }
    }

    /// Set the popup window ID (called by window manager after creating popup)
    pub fn set_popup_window_id(&mut self, id: Option<WindowId>) {
        self.popup_window_id = id;
    }

    /// Get the popup window ID
    pub fn get_popup_window_id(&self) -> Option<WindowId> {
        self.popup_window_id
    }

    /// Check if there's a pending popup that needs to be created
    pub fn poll_pending_popup(&mut self) -> Option<PendingPopup> {
        self.pending_popup.take()
    }

    /// Handle menu item selection from popup
    pub fn handle_popup_selection(&mut self, item_index: usize) {
        if let Some(menu_index) = self.open_menu_index {
            if let Some(menu) = self.menus.get(menu_index) {
                if let Some(item) = menu.items.get(item_index) {
                    if let MenuItemDef::Item { id, .. } = item {
                        let item_id = *id;
                        self.close_menu();
                        if let Some(ref mut callback) = self.on_select {
                            callback(menu_index, item_id);
                        }
                    }
                }
            }
        }
    }

    /// Open a menu dropdown - creates pending popup info
    fn open_menu(&mut self, index: usize) {
        crate::debug_info!("MenuBar::open_menu called with index {}", index);

        if index >= self.menus.len() {
            crate::debug_info!("MenuBar::open_menu - index out of bounds");
            return;
        }

        let menu = &self.menus[index];
        self.open_menu_index = Some(index);
        crate::debug_info!("MenuBar::open_menu - global_offset = {:?}", self.global_offset);

        // Calculate popup dimensions
        let font = get_default_font();
        let char_width = font.char_width();
        let item_height = 24usize;

        let max_label_width = menu
            .items
            .iter()
            .map(|item| match item {
                MenuItemDef::Item { label, shortcut, .. } => {
                    let shortcut_len = shortcut.as_ref().map_or(0, |s| s.len() + 2);
                    label.len() + shortcut_len
                }
                MenuItemDef::Separator => 0,
            })
            .max()
            .unwrap_or(10);

        let popup_width = (max_label_width * char_width + 32).max(100) as u32;
        let popup_height = menu
            .items
            .iter()
            .map(|item| match item {
                MenuItemDef::Separator => 8,
                _ => item_height,
            })
            .sum::<usize>() as u32
            + 4; // 2px border

        // Calculate popup position using stored global offset
        let popup_x = self.global_offset.0 + menu.x as i32;
        let popup_y = self.global_offset.1 + MENU_BAR_HEIGHT as i32;

        // Create pending popup for window manager to handle
        self.pending_popup = Some(PendingPopup {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
            items: menu.items.clone(),
            menu_index: index,
        });

        crate::debug_info!("MenuBar::open_menu - pending_popup created at ({}, {}), size {}x{}",
            popup_x, popup_y, popup_width, popup_height);

        self.base.invalidate();
    }

    /// Get menu index at x position
    fn menu_at_x(&self, x: i32) -> Option<usize> {
        if x < 0 {
            return None;
        }

        let x = x as usize;
        for (i, menu) in self.menus.iter().enumerate() {
            if x >= menu.x && x < menu.x + menu.width {
                return Some(i);
            }
        }
        None
    }
}

impl Window for MenuBar {
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

        let bounds = self.bounds();
        let font = get_default_font();
        let char_height = font.char_height();

        // Store global offset for popup positioning
        // During paint, bounds are temporarily set to absolute coordinates
        self.global_offset = (bounds.x, bounds.y);

        // Draw menu bar background
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            MENU_BAR_HEIGHT as usize,
            self.bg_color,
        );

        // Draw bottom border
        device.fill_rect(
            bounds.x as usize,
            (bounds.y + MENU_BAR_HEIGHT as i32 - 1) as usize,
            bounds.width as usize,
            1,
            Color::new(180, 180, 180),
        );

        // Draw menu titles
        for (i, menu) in self.menus.iter().enumerate() {
            let x = bounds.x as usize + menu.x;
            let text_y = bounds.y as usize + (MENU_BAR_HEIGHT as usize - char_height) / 2;

            // Highlight if hovered or open
            let is_active = self.hover_index == Some(i) || self.open_menu_index == Some(i);
            if is_active {
                device.fill_rect(
                    x,
                    bounds.y as usize,
                    menu.width,
                    MENU_BAR_HEIGHT as usize,
                    self.hover_bg_color,
                );
            }

            // Draw title text
            device.draw_text(
                x + MENU_TITLE_PADDING,
                text_y,
                &menu.title,
                font.as_font(),
                self.text_color,
            );
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let local_x = mouse_event.position.x;
                let local_y = mouse_event.position.y;

                // Check if in menu bar
                let bounds = self.base.bounds();
                let in_bar = local_x >= 0
                    && local_x < bounds.width as i32
                    && local_y >= 0
                    && local_y < MENU_BAR_HEIGHT as i32;

                if in_bar {
                    match mouse_event.event_type {
                        MouseEventType::Move => {
                            let new_hover = self.menu_at_x(local_x);
                            if new_hover != self.hover_index {
                                self.hover_index = new_hover;
                                // If a menu is open and we hover a different menu, open that one
                                if self.open_menu_index.is_some() {
                                    if let Some(idx) = new_hover {
                                        if self.open_menu_index != Some(idx) {
                                            self.open_menu(idx);
                                        }
                                    }
                                }
                                self.base.invalidate();
                            }
                        }
                        MouseEventType::ButtonDown if mouse_event.buttons.left => {
                            crate::debug_info!("MenuBar: ButtonDown at x={}, menu_at_x={:?}",
                                local_x, self.menu_at_x(local_x));
                            if let Some(idx) = self.menu_at_x(local_x) {
                                if self.open_menu_index == Some(idx) {
                                    self.close_menu();
                                } else {
                                    self.open_menu(idx);
                                }
                            }
                        }
                        _ => {}
                    }
                    return EventResult::Handled;
                }

                // Clear hover if mouse leaves
                if self.hover_index.is_some() {
                    self.hover_index = None;
                    self.base.invalidate();
                }

                EventResult::Propagate
            }
            _ => EventResult::Ignored,
        }
    }

    fn can_focus(&self) -> bool {
        false
    }

    fn has_focus(&self) -> bool {
        false
    }

    fn set_focus(&mut self, _focused: bool) {}

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }

    fn poll_pending_popup(&mut self) -> Option<super::super::windows::PendingPopup> {
        let result = self.pending_popup.take();
        if result.is_some() {
            crate::debug_info!("MenuBar::poll_pending_popup - returning pending popup");
        }
        result
    }

    fn handle_popup_selection(&mut self, item_index: usize) {
        if let Some(menu_index) = self.open_menu_index {
            if let Some(menu) = self.menus.get(menu_index) {
                if let Some(item) = menu.items.get(item_index) {
                    if let MenuItemDef::Item { id, .. } = item {
                        let item_id = *id;
                        self.close_menu();
                        if let Some(ref mut callback) = self.on_select {
                            callback(menu_index, item_id);
                        }
                    }
                }
            }
        }
    }

    fn close_popup_menu(&mut self) {
        self.close_menu();
    }
}
