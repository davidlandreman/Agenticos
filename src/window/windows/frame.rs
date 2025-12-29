use crate::graphics::color::Color;
use crate::window::{
    Event, EventResult, GraphicsDevice, Point, Rect, Window, WindowId,
};
use crate::window::types::HitTestResult;
use alloc::string::{String, ToString};
use super::base::WindowBase;

/// A window with decorations (title bar, borders)
pub struct FrameWindow {
    base: WindowBase,
    title: String,
    title_bar_height: usize,
    border_width: usize,
    close_button_size: usize,
    close_button_padding: usize,
    active: bool,
    content_window_id: Option<WindowId>,
}

impl FrameWindow {
    pub fn new(id: WindowId, title: &str) -> Self {
        Self {
            base: WindowBase::new_with_id(id, Rect::new(0, 0, 800, 600)),
            title: title.to_string(),
            title_bar_height: 24,
            border_width: 2,
            close_button_size: 16,
            close_button_padding: 4,
            active: false,
            content_window_id: None,
        }
    }

    pub fn set_content_window(&mut self, window_id: WindowId) {
        self.content_window_id = Some(window_id);
        self.base.add_child(window_id);
        self.base.invalidate();
    }

    pub fn content_area(&self) -> Rect {
        // Return area relative to parent (0,0 based)
        Rect::new(
            self.border_width as i32,
            (self.title_bar_height + self.border_width) as i32,
            self.base.bounds().width - 2 * self.border_width as u32,
            self.base.bounds().height - self.title_bar_height as u32 - 2 * self.border_width as u32,
        )
    }

    fn draw_title_bar(&self, device: &mut dyn GraphicsDevice) {
        let title_bar_color = if self.active {
            Color::new(0, 100, 200)  // Blue when active
        } else {
            Color::new(100, 100, 100)  // Grey when inactive
        };

        // Draw title bar background
        let bounds = self.base.bounds();
        device.fill_rect(
            bounds.x as usize + self.border_width,
            bounds.y as usize + self.border_width,
            (bounds.width - 2 * self.border_width as u32) as usize,
            self.title_bar_height,
            title_bar_color,
        );

        // Draw title text (left-aligned with padding)
        let text_y = bounds.y as usize + self.border_width + (self.title_bar_height - 8) / 2;
        let text_x = bounds.x as usize + self.border_width + 8;

        let font = crate::graphics::fonts::core_font::get_default_font();
        device.draw_text(
            text_x,
            text_y,
            &self.title,
            font.as_font(),
            Color::WHITE,
        );

        // Draw close button
        self.draw_close_button(device);
    }

    fn draw_close_button(&self, device: &mut dyn GraphicsDevice) {
        let bounds = self.base.bounds();

        // Calculate close button position (right side of titlebar, vertically centered)
        let btn_x = bounds.x as usize + bounds.width as usize
            - self.border_width - self.close_button_padding - self.close_button_size;
        let btn_y = bounds.y as usize + self.border_width
            + (self.title_bar_height - self.close_button_size) / 2;

        // Draw close button background (dark red)
        let btn_color = Color::new(192, 0, 0);
        device.fill_rect(btn_x, btn_y, self.close_button_size, self.close_button_size, btn_color);

        // Draw X symbol (white lines)
        let padding = 4; // Padding inside the button for the X
        let x1 = btn_x + padding;
        let y1 = btn_y + padding;
        let x2 = btn_x + self.close_button_size - padding - 1;
        let y2 = btn_y + self.close_button_size - padding - 1;

        // Draw the X using two diagonal lines
        device.draw_line(x1, y1, x2, y2, Color::WHITE);
        device.draw_line(x2, y1, x1, y2, Color::WHITE);
        // Draw second lines offset by 1 pixel to make it thicker
        device.draw_line(x1 + 1, y1, x2 + 1, y2, Color::WHITE);
        device.draw_line(x2 - 1, y1, x1 - 1, y2, Color::WHITE);
    }

    /// Perform a hit test at the given local coordinates.
    ///
    /// Returns what part of the window was hit.
    pub fn hit_test(&self, local_point: Point) -> HitTestResult {
        let bounds = self.base.bounds();
        let x = local_point.x;
        let y = local_point.y;

        // Check if point is within window bounds
        if x < 0 || y < 0 || x >= bounds.width as i32 || y >= bounds.height as i32 {
            return HitTestResult::None;
        }

        let border = self.border_width as i32;
        let title_height = self.title_bar_height as i32;

        // Check title bar area (excluding borders)
        if y >= border && y < border + title_height && x >= border && x < bounds.width as i32 - border {
            // Check if click is in close button area
            let close_btn_size = self.close_button_size as i32;
            let close_btn_padding = self.close_button_padding as i32;
            let close_btn_x = bounds.width as i32 - border - close_btn_padding - close_btn_size;
            let close_btn_y = border + (title_height - close_btn_size) / 2;

            if x >= close_btn_x && x < close_btn_x + close_btn_size
                && y >= close_btn_y && y < close_btn_y + close_btn_size
            {
                return HitTestResult::CloseButton;
            }

            return HitTestResult::TitleBar;
        }

        // Check borders for resize handles
        let at_left = x < border;
        let at_right = x >= bounds.width as i32 - border;
        let at_top = y < border;
        let at_bottom = y >= bounds.height as i32 - border;

        if at_top && at_left {
            return HitTestResult::Border(crate::window::types::ResizeEdge::TopLeft);
        }
        if at_top && at_right {
            return HitTestResult::Border(crate::window::types::ResizeEdge::TopRight);
        }
        if at_bottom && at_left {
            return HitTestResult::Border(crate::window::types::ResizeEdge::BottomLeft);
        }
        if at_bottom && at_right {
            return HitTestResult::Border(crate::window::types::ResizeEdge::BottomRight);
        }
        if at_top {
            return HitTestResult::Border(crate::window::types::ResizeEdge::Top);
        }
        if at_bottom {
            return HitTestResult::Border(crate::window::types::ResizeEdge::Bottom);
        }
        if at_left {
            return HitTestResult::Border(crate::window::types::ResizeEdge::Left);
        }
        if at_right {
            return HitTestResult::Border(crate::window::types::ResizeEdge::Right);
        }

        // Otherwise, it's the client area
        HitTestResult::Client
    }

    fn draw_borders(&self, device: &mut dyn GraphicsDevice) {
        let border_color = if self.active {
            Color::new(0, 100, 200)  // Blue when active
        } else {
            Color::new(150, 150, 150)  // Light grey when inactive
        };

        let bounds = self.base.bounds();
        // Top border
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            bounds.width as usize,
            self.border_width,
            border_color,
        );

        // Bottom border
        device.fill_rect(
            bounds.x as usize,
            (bounds.y + bounds.height as i32 - self.border_width as i32) as usize,
            bounds.width as usize,
            self.border_width,
            border_color,
        );

        // Left border
        device.fill_rect(
            bounds.x as usize,
            bounds.y as usize,
            self.border_width,
            bounds.height as usize,
            border_color,
        );

        // Right border
        device.fill_rect(
            (bounds.x + bounds.width as i32 - self.border_width as i32) as usize,
            bounds.y as usize,
            self.border_width,
            bounds.height as usize,
            border_color,
        );
    }
}

impl Window for FrameWindow {
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

        // Only paint if we actually need to repaint
        if !self.base.needs_repaint() {
            return;
        }

        // Draw frame decorations
        self.draw_borders(device);
        self.draw_title_bar(device);

        // The content window will be painted separately by the window manager
        self.base.clear_needs_repaint();
    }

    fn needs_repaint(&self) -> bool {
        self.base.needs_repaint()
    }

    fn invalidate(&mut self) {
        self.base.invalidate();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Focus(focus_event) => {
                self.active = focus_event.gained;
                self.base.invalidate();
                EventResult::Handled
            }
            _ => EventResult::Propagate,
        }
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn has_focus(&self) -> bool {
        self.active
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

    fn set_focus(&mut self, focused: bool) {
        self.active = focused;
        self.base.invalidate();
    }
}