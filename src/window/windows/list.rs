//! List widget for displaying selectable items.
//!
//! After the U6 migration this widget no longer manages its own scroll
//! offset or paints a scrollbar. Callers wrap it in a
//! [`ScrollView`](crate::window::windows::scroll_view::ScrollView) for
//! scrolling. Selection state is delegated to the shared
//! [`Selection`](crate::window::selection::Selection) model so click and
//! arrow-key semantics stay consistent across list-shaped widgets.
//!
//! Key behavior:
//! - Default `selection_mode` is `Single` — preserves the prior
//!   single-select API for existing dialog consumers.
//! - The selection callback signature is `FnMut(&Selection)`. Use
//!   `Selection::iter().next()` (or the `selected()` helper) to get the
//!   first index in the simple case.
//! - The list paints its full content rect (`width × item_count *
//!   item_height`); a wrapping `ScrollView` clips and translates as
//!   needed.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::selection::{ArrowDirection, ClickMods, Selection, SelectionMode};
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use super::base::WindowBase;

/// Callback invoked when the user changes the selection.
///
/// The callback receives a reference to the new [`Selection`]. Most
/// single-select consumers can convert via `selection.iter().next()`.
pub type SelectionCallback = Box<dyn FnMut(&Selection) + Send>;

/// A simple single-column list widget with selection.
pub struct List {
    /// Base window functionality
    base: WindowBase,
    /// List items
    items: Vec<String>,
    /// Selection state (delegated to the shared model).
    selection: Selection,
    /// Selection mode (Single by default; opt-in Multi).
    selection_mode: SelectionMode,
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
            selection: Selection::None,
            selection_mode: SelectionMode::Single,
            item_height: 16, // 8px font + 8px padding
            on_select: None,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            selected_bg_color: crate::window::PALETTE_HIGHLIGHT_BG,
            selected_text_color: crate::window::PALETTE_HIGHLIGHT_TEXT,
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
        self.selection = Selection::None;
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

    /// Get the currently selected index. For multi-select this returns the
    /// first selected index in ascending order. Returns `None` when nothing
    /// is selected.
    pub fn selected(&self) -> Option<usize> {
        self.selection.iter().next()
    }

    /// Borrow the underlying selection state.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Get the selected item text (first selected item in ascending order).
    pub fn selected_item(&self) -> Option<&str> {
        self.selected().and_then(|i| self.items.get(i).map(|s| s.as_str()))
    }

    /// Set the selected index. Passing `None` clears the selection. The
    /// resulting selection is always `Single(idx)` or `None`; switching to
    /// multi-select selection state should go through the click/arrow paths.
    pub fn set_selected(&mut self, index: Option<usize>) {
        let new_sel = match index.filter(|&i| i < self.items.len()) {
            Some(i) => Selection::Single(i),
            None => Selection::None,
        };
        if self.selection != new_sel {
            self.selection = new_sel;
            self.base.invalidate();
        }
    }

    /// Configure the selection mode. Switching from `Multi` back to `Single`
    /// collapses any existing multi-selection to its first index in ascending
    /// order so the widget is left in a consistent state.
    pub fn set_selection_mode(&mut self, mode: SelectionMode) {
        if self.selection_mode == mode {
            return;
        }
        self.selection_mode = mode;
        if matches!(mode, SelectionMode::Single) {
            // Collapse any existing multi/range selection to its first index.
            let first = self.selection.iter().next();
            self.selection = match first {
                Some(i) => Selection::Single(i),
                None => Selection::None,
            };
            self.base.invalidate();
        }
    }

    /// Current selection mode.
    pub fn selection_mode(&self) -> SelectionMode {
        self.selection_mode
    }

    /// Set the selection change callback.
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(&Selection) + Send + 'static,
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

    /// Item height in pixels (used by callers that wrap this widget in a
    /// `ScrollView` to compute the content size).
    pub fn item_height(&self) -> usize {
        self.item_height
    }

    /// Natural content height in pixels (`item_count * item_height`).
    /// Use to feed `ScrollView::set_content_size` from the caller.
    pub fn content_height(&self) -> u32 {
        (self.items.len() * self.item_height) as u32
    }

    /// Convert a y coordinate (in the list's local frame, same frame as
    /// `MouseEvent::position`) to an item index, if any.
    fn y_to_index(&self, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let relative_y = y - bounds.y;
        if relative_y < 0 {
            return None;
        }
        if self.item_height == 0 {
            return None;
        }
        let index = (relative_y as usize) / self.item_height;
        if index < self.items.len() {
            Some(index)
        } else {
            None
        }
    }

    /// Apply a click to the selection model and fire the callback. Returns
    /// `true` if the callback was invoked.
    fn apply_click(&mut self, idx: usize, mods: ClickMods) {
        let before = self.selection.clone();
        self.selection.click(idx, mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
        }
        // Fire callback even when selection didn't change; callers may
        // rely on per-click signaling (e.g., for double-click semantics).
        if let Some(ref mut callback) = self.on_select {
            callback(&self.selection);
        }
    }

    /// Apply an arrow-key navigation step to the selection.
    fn apply_arrow(&mut self, direction: ArrowDirection, mods: ClickMods) {
        if self.items.is_empty() {
            return;
        }
        let before = self.selection.clone();
        self.selection.arrow(direction, self.items.len(), mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
            if let Some(ref mut callback) = self.on_select {
                callback(&self.selection);
            }
        }
    }
}

impl Window for List {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }
        if !self.base.needs_repaint() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let item_height = self.item_height as i32;

        // Background covers the full content rect (when wrapped in
        // ScrollView the bounds are temporarily extended to cover the
        // content). When standalone, this still paints the visible area.
        device.fill_rect(x, y, width, bounds.height, self.bg_color);

        // Border around the visible area only when not embedded in a
        // ScrollView. We can't tell here, so draw none — ScrollView paints
        // its own border-equivalent and standalone callers can wrap in a
        // ContainerWindow if they want a border.

        let font = get_default_font();
        let line_h = font.line_height() as usize;
        let padding: i32 = 4;

        // Draw all items. Clipping (when wrapped in ScrollView) ensures
        // only visible rows hit pixels.
        for (item_index, item_text) in self.items.iter().enumerate() {
            let item_y = y + (item_index as i32) * item_height;
            let is_selected = self.selection.is_selected(item_index);

            if is_selected {
                device.fill_rect(
                    x + 1,
                    item_y,
                    width.saturating_sub(2),
                    self.item_height as u32,
                    self.selected_bg_color,
                );
            }

            let text_color = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            let text_y = item_y + (item_height - line_h as i32) / 2;
            device.draw_text(
                x + padding,
                text_y,
                item_text,
                font.as_font(),
                text_color,
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
                            let mods = ClickMods::new(
                                mouse_event.modifiers.shift,
                                mouse_event.modifiers.ctrl,
                            );
                            self.apply_click(index, mods);
                        }
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            Event::Keyboard(kbd_event) if kbd_event.pressed => {
                use crate::window::event::KeyCode;
                let mods = ClickMods::new(kbd_event.modifiers.shift, kbd_event.modifiers.ctrl);
                match kbd_event.key_code {
                    KeyCode::Up => {
                        self.apply_arrow(ArrowDirection::Up, mods);
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        self.apply_arrow(ArrowDirection::Down, mods);
                        EventResult::Handled
                    }
                    _ => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
