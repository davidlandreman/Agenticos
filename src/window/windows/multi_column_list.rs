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
    pub fn new(bounds: Rect, columns: Vec<Column>) -> Self {
        Self::new_with_id(WindowId::new(), bounds, columns)
    }

    /// Add a row to the list
    pub fn add_row(&mut self, values: Vec<String>) {
        self.rows.push(values);
        self.base.invalidate();
    }

    /// Add a row from string slices
    pub fn add_row_strs(&mut self, values: &[&str]) {
        self.rows
            .push(values.iter().map(|s| String::from(*s)).collect());
        self.base.invalidate();
    }

    /// Clear all rows (keep columns)
    pub fn clear_rows(&mut self) {
        self.rows.clear();
        self.selection = Selection::None;
        self.base.invalidate();
    }

    /// Get the number of rows
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Check if the list is empty
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Get the currently selected row index. For multi-select this returns
    /// the first selected index in ascending order.
    pub fn selected(&self) -> Option<usize> {
        self.selection.iter().next()
    }

    /// Borrow the underlying selection state.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Get the selected row data (first selected row in ascending order).
    pub fn selected_row(&self) -> Option<&Vec<String>> {
        self.selected().and_then(|i| self.rows.get(i))
    }

    /// Set the selected row index. Passing `None` clears the selection.
    pub fn set_selected(&mut self, index: Option<usize>) {
        let new_sel = match index.filter(|&i| i < self.rows.len()) {
            Some(i) => Selection::Single(i),
            None => Selection::None,
        };
        if self.selection != new_sel {
            self.selection = new_sel;
            self.base.invalidate();
        }
    }

    /// Configure the selection mode. Switching from `Multi` back to `Single`
    /// collapses any existing multi-selection to its first index in
    /// ascending order.
    pub fn set_selection_mode(&mut self, mode: SelectionMode) {
        if self.selection_mode == mode {
            return;
        }
        self.selection_mode = mode;
        if matches!(mode, SelectionMode::Single) {
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

    /// Header row height in pixels.
    pub fn header_height(&self) -> usize {
        self.header_height
    }

    /// Data row height in pixels.
    pub fn row_height(&self) -> usize {
        self.row_height
    }

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
                        if let Some(index) = self.y_to_row_index(mouse_event.position.y) {
                            let mods = ClickMods::new(
                                mouse_event.modifiers.shift,
                                mouse_event.modifiers.ctrl,
                            );
                            self.apply_click(index, mods);
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
                    _ => EventResult::Ignored,
                }
            }
            _ => EventResult::Ignored,
        }
    }
}
