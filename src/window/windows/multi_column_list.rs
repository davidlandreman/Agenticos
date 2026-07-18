//! Multi-column list widget with headers.
//!
//! After the U6 migration this widget no longer manages its own scroll
//! offset or paints a scrollbar. Callers wrap it in a
//! [`ScrollView`](crate::window::windows::scroll_view::ScrollView) for
//! scrolling. Selection state is delegated to the shared
//! [`Selection`](crate::window::selection::Selection) model so click and
//! arrow-key semantics stay consistent across list-shaped widgets.
//!
//! Right-click semantics are preserved verbatim: the right-clicked row
//! becomes selected and `on_right_click(row_index, global_position)` is
//! invoked, matching the prior contract.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::event::MouseEventType;
use crate::window::selection::{ArrowDirection, ClickMods, Selection, SelectionMode};
use crate::window::{Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowId};
use super::base::WindowBase;

/// Callback invoked when the user changes the selection.
pub type MultiColumnSelectionCallback = Box<dyn FnMut(&Selection) + Send>;

/// Callback type for right-click events `(row_index, global_position)`.
pub type RightClickCallback = Box<dyn FnMut(usize, Point) + Send>;

/// Callback fired when the user "activates" a row — clicking an
/// already-selected row, or pressing Enter while a row is selected.
pub type ActivateCallback = Box<dyn FnMut(usize) + Send>;

/// Column definition for the multi-column list
#[derive(Debug, Clone)]
pub struct Column {
    /// Column header text
    pub header: String,
    /// Column width in pixels
    pub width: usize,
}

impl Column {
    /// Create a new column definition
    pub fn new(header: &str, width: usize) -> Self {
        Column {
            header: String::from(header),
            width,
        }
    }
}

/// A multi-column list widget with headers.
pub struct MultiColumnList {
    /// Base window functionality
    base: WindowBase,
    /// Column definitions
    columns: Vec<Column>,
    /// Row data (each row is a vector of strings, one per column)
    rows: Vec<Vec<String>>,
    /// Selection state (delegated to the shared model).
    selection: Selection,
    /// Selection mode (Single by default; opt-in Multi).
    selection_mode: SelectionMode,
    /// Height of the header row
    header_height: usize,
    /// Height of each data row
    row_height: usize,
    /// Selection change callback
    on_select: Option<MultiColumnSelectionCallback>,
    /// Right-click callback (row_index, global_position)
    on_right_click: Option<RightClickCallback>,
    /// Row-activation callback (fires on click of already-selected row
    /// and on Enter key).
    on_activate: Option<ActivateCallback>,
    /// Last click bookkeeping for double-click detection. The tick
    /// value is the kernel tick (100 Hz) at click time; comparing
    /// against `get_timer_ticks()` gives elapsed time.
    last_click_row: Option<usize>,
    last_click_tick: u64,
    /// Background color
    bg_color: Color,
    /// Text color
    text_color: Color,
    /// Header background color
    header_bg_color: Color,
    /// Header text color
    header_text_color: Color,
    /// Selected row background color
    selected_bg_color: Color,
    /// Selected row text color
    selected_text_color: Color,
}

impl MultiColumnList {
    /// Create a new multi-column list with a specific ID
    pub fn new_with_id(id: WindowId, bounds: Rect, columns: Vec<Column>) -> Self {
        MultiColumnList {
            base: WindowBase::new_with_id(id, bounds),
            columns,
            rows: Vec::new(),
            selection: Selection::None,
            selection_mode: SelectionMode::Single,
            header_height: 20,
            row_height: 16,
            on_select: None,
            on_right_click: None,
            on_activate: None,
            last_click_row: None,
            last_click_tick: 0,
            bg_color: crate::window::PALETTE_CONTENT_BG,
            text_color: crate::window::PALETTE_TEXT,
            // Header is slightly distinct from the row body so the
            // header row reads as a separate band; LIGHT_GRAY is kept.
            header_bg_color: Color::LIGHT_GRAY,
            header_text_color: crate::window::PALETTE_TEXT,
            selected_bg_color: crate::window::PALETTE_HIGHLIGHT_BG,
            selected_text_color: crate::window::PALETTE_HIGHLIGHT_TEXT,
        }
    }

    /// Create a new multi-column list (generates its own ID)
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn new(bounds: Rect, columns: Vec<Column>) -> Self {
        Self::new_with_id(WindowId::new(), bounds, columns)
    }

    /// Add a row to the list
    pub fn add_row(&mut self, values: Vec<String>) {
        self.rows.push(values);
        self.base.invalidate();
    }

    /// Add a row from string slices

    /// Clear all rows (keep columns)
    pub fn clear_rows(&mut self) {
        self.rows.clear();
        self.selection = Selection::None;
        self.base.invalidate();
    }

    /// Get the number of rows

    /// Check if the list is empty

    /// Get the currently selected row index. For multi-select this returns
    /// the first selected index in ascending order.

    /// Borrow the underlying selection state.

    /// Get the selected row data (first selected row in ascending order).

    /// Set the selected row index. Passing `None` clears the selection.

    /// Configure the selection mode. Switching from `Multi` back to `Single`
    /// collapses any existing multi-selection to its first index in
    /// ascending order.

    /// Current selection mode.

    /// Set the selection change callback
    pub fn on_select<F>(&mut self, callback: F)
    where
        F: FnMut(&Selection) + Send + 'static,
    {
        self.on_select = Some(Box::new(callback));
    }

    /// Set the right-click callback.
    ///
    /// The callback receives the row index and the global mouse position,
    /// which can be used to position a context menu.
    pub fn on_right_click<F>(&mut self, callback: F)
    where
        F: FnMut(usize, Point) + Send + 'static,
    {
        self.on_right_click = Some(Box::new(callback));
    }

    /// Set the row-activation callback. Fires when the user clicks an
    /// already-selected row, or presses Enter while a row is selected.
    /// First click of an unselected row only updates selection; the
    /// second click (with the row still selected) activates.
    pub fn on_activate<F>(&mut self, callback: F)
    where
        F: FnMut(usize) + Send + 'static,
    {
        self.on_activate = Some(Box::new(callback));
    }

    /// Header row height in pixels.

    /// Data row height in pixels.

    /// Natural content height in pixels (`header + row_count * row_height`).
    /// Use to feed `ScrollView::set_content_size` from the caller.
    pub fn content_height(&self) -> u32 {
        (self.header_height + self.rows.len() * self.row_height) as u32
    }

    /// Convert a y coordinate (in the list's local frame, same frame as
    /// `MouseEvent::position`) to a row index, if any. Header clicks
    /// return `None`.
    fn y_to_row_index(&self, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let relative_y = y - bounds.y;
        if relative_y < 0 {
            return None;
        }
        let relative_y = relative_y as usize;

        // Check if in header
        if relative_y < self.header_height {
            return None;
        }

        if self.row_height == 0 {
            return None;
        }
        let row_relative_y = relative_y - self.header_height;
        let index = row_relative_y / self.row_height;

        if index < self.rows.len() {
            Some(index)
        } else {
            None
        }
    }

    /// Apply a click to the selection model and fire the callback.
    fn apply_click(&mut self, idx: usize, mods: ClickMods) {
        let before = self.selection.clone();
        self.selection.click(idx, mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
        }
        if let Some(ref mut callback) = self.on_select {
            callback(&self.selection);
        }
    }

    /// Apply an arrow-key navigation step to the selection.
    fn apply_arrow(&mut self, direction: ArrowDirection, mods: ClickMods) {
        if self.rows.is_empty() {
            return;
        }
        let before = self.selection.clone();
        self.selection.arrow(direction, self.rows.len(), mods, self.selection_mode);
        if before != self.selection {
            self.base.invalidate();
            if let Some(ref mut callback) = self.on_select {
                callback(&self.selection);
            }
        }
    }
}

impl Window for MultiColumnList {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn as_multi_column_list_mut(&mut self) -> Option<&mut MultiColumnList> {
        Some(self)
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let header_h = self.header_height as i32;
        let row_h = self.row_height as i32;

        // Background covers the full bounds (which, when wrapped in a
        // ScrollView, may equal the full content extent during paint).
        device.fill_rect(x, y, width, bounds.height, self.bg_color);

        let font = get_default_font();
        let line_h = font.line_height() as i32;
        let cell_w = font.cell_width() as usize;
        let padding: i32 = 4;

        // Draw header row
        device.fill_rect(x + 1, y + 1, width.saturating_sub(2), self.header_height as u32, self.header_bg_color);

        let mut col_x = x + 1;
        for column in &self.columns {
            // Draw header text
            let text_y = y + (header_h - line_h) / 2;
            device.draw_text(
                col_x + padding,
                text_y,
                &column.header,
                font.as_font(),
                self.header_text_color,
            );

            // Draw column separator
            col_x += column.width as i32;
            if col_x < x + width as i32 - 1 {
                device.draw_line(col_x, y + 1, col_x, y + header_h, Color::GRAY);
            }
        }

        // Draw separator line below header
        let header_bottom = y + header_h;
        device.draw_line(x + 1, header_bottom, x + width as i32 - 2, header_bottom, Color::GRAY);

        // Draw all data rows. When embedded in ScrollView, the active clip
        // rect limits visible pixels.
        for (row_index, row_data) in self.rows.iter().enumerate() {
            let row_y = y + header_h + 1 + (row_index as i32) * row_h;
            let is_selected = self.selection.is_selected(row_index);

            if is_selected {
                device.fill_rect(
                    x + 1,
                    row_y,
                    width.saturating_sub(2),
                    self.row_height as u32,
                    self.selected_bg_color,
                );
            }

            let text_color = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            let mut cell_x = x + 1;
            for (col_idx, column) in self.columns.iter().enumerate() {
                let text = row_data.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                let text_y = row_y + (row_h - line_h) / 2;

                // Truncate text if it doesn't fit
                let max_chars = column.width.saturating_sub((padding as usize) * 2) / cell_w.max(1);
                let display_text = if text.len() > max_chars {
                    &text[..max_chars]
                } else {
                    text
                };

                device.draw_text(
                    cell_x + padding,
                    text_y,
                    display_text,
                    font.as_font(),
                    text_color,
                );

                cell_x += column.width as i32;
            }
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
                        crate::debug_info!(
                            "MultiColumnList: ButtonDown at position=({}, {}) bounds=({}, {}, {}, {})",
                            mouse_event.position.x, mouse_event.position.y,
                            bounds.x, bounds.y, bounds.width, bounds.height
                        );
                        if let Some(index) = self.y_to_row_index(mouse_event.position.y) {
                            crate::debug_info!(
                                "MultiColumnList: hit row index={}, on_activate present={}",
                                index, self.on_activate.is_some()
                            );
                            let was_selected = self.selection.is_selected(index);
                            let mods = ClickMods::new(
                                mouse_event.modifiers.shift,
                                mouse_event.modifiers.ctrl,
                            );
                            self.apply_click(index, mods);
                            // Activation fires on either:
                            //   1. A rapid second click on the same row
                            //      (true double-click; <500ms apart),
                            //   2. A single click on an already-selected
                            //      row (matches "single-click open" mode).
                            // Modifier-driven toggles (ctrl-click) skip
                            // activation in both branches.
                            let now =
                                crate::arch::x86_64::interrupts::get_timer_ticks();
                            let is_double_click = self.last_click_row == Some(index)
                                && now.saturating_sub(self.last_click_tick) < 50;
                            let still_selected = self.selection.is_selected(index);
                            let activate_via_reclick =
                                was_selected && still_selected;
                            crate::debug_info!(
                                "MultiColumnList: was_selected={} still_selected={} double_click={} reclick={}",
                                was_selected, still_selected, is_double_click, activate_via_reclick
                            );
                            self.last_click_row = Some(index);
                            self.last_click_tick = now;
                            if !mods.ctrl && (is_double_click || activate_via_reclick) {
                                if let Some(ref mut callback) = self.on_activate {
                                    crate::debug_info!(
                                        "MultiColumnList: firing on_activate for row {}",
                                        index
                                    );
                                    callback(index);
                                }
                            }
                        } else {
                            crate::debug_info!(
                                "MultiColumnList: y_to_row_index returned None for y={}",
                                mouse_event.position.y
                            );
                        }
                        EventResult::Handled
                    }
                    MouseEventType::ButtonDown if mouse_event.buttons.right => {
                        if let Some(index) = self.y_to_row_index(mouse_event.position.y) {
                            // Right-click selects the row (preserve prior
                            // contract: selection moves to the right-clicked
                            // row regardless of selection mode). We force a
                            // collapse to Single(index) for symmetry with
                            // the prior behavior, where right-click never
                            // extended a multi-selection.
                            let new_sel = Selection::Single(index);
                            if self.selection != new_sel {
                                self.selection = new_sel;
                                self.base.invalidate();
                            }
                            if let Some(ref mut callback) = self.on_right_click {
                                callback(index, mouse_event.global_position);
                            }
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
                    KeyCode::Enter => {
                        if let Some(idx) = self.selection.iter().next() {
                            if let Some(ref mut callback) = self.on_activate {
                                callback(idx);
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
}
