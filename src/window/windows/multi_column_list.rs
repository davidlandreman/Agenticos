//! Multi-column list widget with headers

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use crate::window::{Window, WindowId, Rect, Event, EventResult, GraphicsDevice, Point};
use crate::window::event::MouseEventType;
use super::base::WindowBase;

/// Callback type for selection change events
pub type MultiColumnSelectionCallback = Box<dyn FnMut(usize) + Send>;

/// Callback type for right-click events (row_index, global_position)
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

/// A multi-column list widget with headers
pub struct MultiColumnList {
    /// Base window functionality
    base: WindowBase,
    /// Column definitions
    columns: Vec<Column>,
    /// Row data (each row is a vector of strings, one per column)
    rows: Vec<Vec<String>>,
    /// Currently selected row index
    selected_index: Option<usize>,
    /// Scroll offset (first visible row index)
    scroll_offset: usize,
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
            selected_index: None,
            scroll_offset: 0,
            header_height: 20,
            row_height: 16,
            on_select: None,
            on_right_click: None,
            bg_color: Color::WHITE,
            text_color: Color::BLACK,
            header_bg_color: Color::LIGHT_GRAY,
            header_text_color: Color::BLACK,
            selected_bg_color: Color::BLUE,
            selected_text_color: Color::WHITE,
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
        self.selected_index = None;
        self.scroll_offset = 0;
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

    /// Get the currently selected row index
    pub fn selected(&self) -> Option<usize> {
        self.selected_index
    }

    /// Get the selected row data
    pub fn selected_row(&self) -> Option<&Vec<String>> {
        self.selected_index.map(|i| &self.rows[i])
    }

    /// Set the selected row index
    pub fn set_selected(&mut self, index: Option<usize>) {
        let new_index = index.filter(|&i| i < self.rows.len());
        if self.selected_index != new_index {
            self.selected_index = new_index;
            self.base.invalidate();
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

    /// Set the right-click callback
    ///
    /// The callback receives the row index and the global mouse position,
    /// which can be used to position a context menu.
    pub fn on_right_click<F>(&mut self, callback: F)
    where
        F: FnMut(usize, Point) + Send + 'static,
    {
        self.on_right_click = Some(Box::new(callback));
    }

    /// Calculate how many rows can be displayed
    fn visible_rows(&self) -> usize {
        let bounds = self.base.bounds();
        let data_height = (bounds.height as usize).saturating_sub(self.header_height);
        data_height / self.row_height
    }

    /// Ensure a row is visible by adjusting scroll offset
    fn ensure_visible(&mut self, index: usize) {
        let visible = self.visible_rows();
        if index < self.scroll_offset {
            self.scroll_offset = index;
            self.base.invalidate();
        } else if index >= self.scroll_offset + visible {
            self.scroll_offset = index.saturating_sub(visible - 1);
            self.base.invalidate();
        }
    }

    /// Convert y coordinate to row index
    fn y_to_row_index(&self, y: i32) -> Option<usize> {
        let bounds = self.base.bounds();
        let relative_y = (y - bounds.y) as usize;

        // Check if in header
        if relative_y < self.header_height {
            return None;
        }

        let row_relative_y = relative_y - self.header_height;
        let index = self.scroll_offset + row_relative_y / self.row_height;

        if index < self.rows.len() {
            Some(index)
        } else {
            None
        }
    }
}

impl Window for MultiColumnList {
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

        let font = get_default_font();
        let padding = 4;

        // Draw header row
        device.fill_rect(x + 1, y + 1, width - 2, self.header_height, self.header_bg_color);

        let mut col_x = x + 1;
        for column in &self.columns {
            // Draw header text
            let text_y = y + (self.header_height - 8) / 2;
            device.draw_text(
                col_x + padding,
                text_y,
                &column.header,
                font.as_font(),
                self.header_text_color,
            );

            // Draw column separator
            col_x += column.width;
            if col_x < x + width - 1 {
                device.draw_line(col_x, y + 1, col_x, y + self.header_height, Color::GRAY);
            }
        }

        // Draw separator line below header
        let header_bottom = y + self.header_height;
        device.draw_line(x + 1, header_bottom, x + width - 2, header_bottom, Color::GRAY);

        // Draw data rows
        let visible_count = self.visible_rows();
        for i in 0..visible_count {
            let row_index = self.scroll_offset + i;
            if row_index >= self.rows.len() {
                break;
            }

            let row_y = y + self.header_height + 1 + i * self.row_height;
            let is_selected = self.selected_index == Some(row_index);

            // Draw row background
            if is_selected {
                device.fill_rect(
                    x + 1,
                    row_y,
                    width - 2,
                    self.row_height,
                    self.selected_bg_color,
                );
            }

            // Draw row cells
            let text_color = if is_selected {
                self.selected_text_color
            } else {
                self.text_color
            };

            let row_data = &self.rows[row_index];
            let mut cell_x = x + 1;
            for (col_idx, column) in self.columns.iter().enumerate() {
                let text = row_data.get(col_idx).map(|s| s.as_str()).unwrap_or("");
                let text_y = row_y + (self.row_height - 8) / 2;

                // Truncate text if it doesn't fit
                let max_chars = (column.width - padding * 2) / 8;
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

                cell_x += column.width;
            }
        }

        // Draw scrollbar if needed
        let visible = self.visible_rows();
        if self.rows.len() > visible && visible > 0 {
            let scrollbar_width = 8;
            let scrollbar_x = x + width - scrollbar_width - 1;
            let scrollbar_y = y + self.header_height + 1;
            let scrollbar_height = height - self.header_height - 2;

            // Scrollbar track
            device.fill_rect(
                scrollbar_x,
                scrollbar_y,
                scrollbar_width,
                scrollbar_height,
                Color::LIGHT_GRAY,
            );

            // Scrollbar thumb
            let thumb_height = (visible * scrollbar_height) / self.rows.len();
            let thumb_height = thumb_height.max(10);
            let thumb_pos = (self.scroll_offset * scrollbar_height) / self.rows.len();
            device.fill_rect(
                scrollbar_x,
                scrollbar_y + thumb_pos,
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
                        if let Some(index) = self.y_to_row_index(mouse_event.position.y) {
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
                    MouseEventType::ButtonDown if mouse_event.buttons.right => {
                        if let Some(index) = self.y_to_row_index(mouse_event.position.y) {
                            // Select the row on right-click too
                            if self.selected_index != Some(index) {
                                self.selected_index = Some(index);
                                self.base.invalidate();
                            }
                            // Trigger right-click callback with global position for menu placement
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
                match kbd_event.key_code {
                    KeyCode::Up => {
                        if let Some(idx) = self.selected_index {
                            if idx > 0 {
                                self.set_selected(Some(idx - 1));
                                if let Some(ref mut callback) = self.on_select {
                                    callback(idx - 1);
                                }
                            }
                        } else if !self.rows.is_empty() {
                            self.set_selected(Some(0));
                            if let Some(ref mut callback) = self.on_select {
                                callback(0);
                            }
                        }
                        EventResult::Handled
                    }
                    KeyCode::Down => {
                        if let Some(idx) = self.selected_index {
                            if idx + 1 < self.rows.len() {
                                self.set_selected(Some(idx + 1));
                                if let Some(ref mut callback) = self.on_select {
                                    callback(idx + 1);
                                }
                            }
                        } else if !self.rows.is_empty() {
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
