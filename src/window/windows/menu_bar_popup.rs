//! Popup window for MenuBar dropdown menus
//!
//! A popup window that displays menu items with labels, shortcuts, and separators.

use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};

use super::base::WindowBase;
use super::menu_bar::MenuItemDef;


/// A popup window for displaying menu bar dropdowns
pub struct MenuBarPopup {
    /// Base window functionality
    base: WindowBase,
    /// Menu items to display
    items: Vec<MenuItemDef>,
    /// Currently hovered item index
    hover_index: Option<usize>,
    /// ID of the menu bar that owns this popup
    menu_bar_id: WindowId,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Pending selection (item index) to be processed
    pending_selection: Option<usize>,
}

impl MenuBarPopup {
    /// Create a new popup window
    pub fn new_with_id(id: WindowId, bounds: Rect, menu_bar_id: WindowId, items: Vec<MenuItemDef>) -> Self {
        MenuBarPopup {
            base: WindowBase::new_with_id(id, bounds),
            items,
            hover_index: None,
            menu_bar_id,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            pending_selection: None,
        }
    }

    /// Poll for pending selection

    /// Get the menu bar ID this popup belongs to

    /// Get the currently hovered item index

    /// Get popup item index at y position
    fn item_at_y(&self, y: i32) -> Option<usize> {
        if y < 2 {
            return None;
        }

        let mut current_y = 2usize;
        for (i, item) in self.items.iter().enumerate() {
            let item_height = match item {
                MenuItemDef::Separator => 8,
                _ => 24,
            };

            if (y as usize) >= current_y && (y as usize) < current_y + item_height {
                // Don't select separators
                if matches!(item, MenuItemDef::Separator) {
                    return None;
                }
                return Some(i);
            }

            current_y += item_height;
        }
        None
    }
}

impl Window for MenuBarPopup {
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

        let bounds = self.bounds();
        let font = get_default_font();
        let char_height = font.line_height() as usize;

        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;

        // Background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Border
        device.draw_rect(x, y, width, height, crate::window::PALETTE_BORDER);

        // Draw items
        let mut item_y: i32 = y + 2;
        for (i, item) in self.items.iter().enumerate() {
            match item {
                MenuItemDef::Item { label, shortcut, .. } => {
                    let item_height: u32 = 24;

                    // Highlight if hovered
                    if self.hover_index == Some(i) {
                        device.fill_rect(
                            x + 2,
                            item_y,
                            width - 4,
                            item_height,
                            crate::window::PALETTE_HIGHLIGHT_BG,
                        );
                    }

                    // Draw label
                    let text_color = if self.hover_index == Some(i) {
                        crate::window::PALETTE_HIGHLIGHT_TEXT
                    } else {
                        self.text_color
                    };

                    device.draw_text(
                        x + 8,
                        item_y + (item_height as i32 - char_height as i32) / 2,
                        label,
                        font.as_font(),
                        text_color,
                    );

                    // Draw shortcut if present
                    if let Some(shortcut) = shortcut {
                        let shortcut_color = if self.hover_index == Some(i) {
                            Color::new(200, 200, 200)
                        } else {
                            Color::new(128, 128, 128)
                        };
                        let shortcut_x = x + width as i32 - 8
                            - (shortcut.len() as i32 * font.cell_width() as i32);
                        device.draw_text(
                            shortcut_x,
                            item_y + (item_height as i32 - char_height as i32) / 2,
                            shortcut,
                            font.as_font(),
                            shortcut_color,
                        );
                    }

                    item_y += item_height as i32;
                }
                MenuItemDef::Separator => {
                    item_y += 4;
                    device.fill_rect(
                        x + 4,
                        item_y,
                        width - 8,
                        1,
                        Color::new(180, 180, 180),
                    );
                    item_y += 4;
                }
            }
        }

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(mouse_event) => {
                let local_x = mouse_event.position.x;
                let local_y = mouse_event.position.y;
                let bounds = self.base.bounds();

                // Check if in popup bounds
                let in_popup = local_x >= 0
                    && local_x < bounds.width as i32
                    && local_y >= 0
                    && local_y < bounds.height as i32;

                if in_popup {
                    match mouse_event.event_type {
                        MouseEventType::Move => {
                            let new_hover = self.item_at_y(local_y);
                            if new_hover != self.hover_index {
                                self.hover_index = new_hover;
                                self.base.invalidate();
                            }
                        }
                        MouseEventType::ButtonUp => {
                            // Set pending selection for the window manager to process
                            // Note: on ButtonUp, buttons.left is false since the button was just released
                            crate::debug_info!("MenuBarPopup: ButtonUp, hover_index={:?}", self.hover_index);
                            if let Some(idx) = self.hover_index {
                                self.pending_selection = Some(idx);
                                crate::debug_info!("MenuBarPopup: pending_selection set to {}", idx);
                            }
                            return EventResult::Handled;
                        }
                        MouseEventType::ButtonDown => {
                            // Capture the click
                            return EventResult::Handled;
                        }
                        _ => {}
                    }
                    return EventResult::Handled;
                }

                EventResult::Propagate
            }
            _ => EventResult::Ignored,
        }
    }

    // Popup is never focusable; override the default delegation so
    // `set_focus(true)` cannot flip `WindowBase::has_focus`.
    fn has_focus(&self) -> bool {
        false
    }

    fn set_focus(&mut self, _focused: bool) {}

    fn poll_pending_popup_selection(&mut self) -> Option<(WindowId, usize)> {
        self.pending_selection.take().map(|idx| (self.menu_bar_id, idx))
    }
}
