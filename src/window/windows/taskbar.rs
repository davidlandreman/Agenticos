//! Taskbar, task-button geometry, and right-side notification tray.

use super::base::WindowBase;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_caption_font;
use crate::time::DateTime;
use crate::window::{Event, EventResult, GraphicsDevice, Rect, Window, WindowId};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Height of the taskbar in pixels
pub const TASKBAR_HEIGHT: u32 = 32;
/// Width of the Start button
pub const START_BUTTON_WIDTH: u32 = 60;
/// Maximum width of window buttons
pub const MAX_WINDOW_BUTTON_WIDTH: u32 = 150;
/// Gap between buttons
pub const BUTTON_GAP: u32 = 4;
/// Button height (with some padding from top/bottom)
pub const BUTTON_HEIGHT: u32 = 24;
/// Vertical offset for buttons
pub const BUTTON_Y_OFFSET: u32 = 4;
/// Width reserved for the right-side notification tray.
pub const TRAY_WIDTH: u32 = 96;
/// Tray height within the 32px taskbar.
pub const TRAY_HEIGHT: u32 = 28;
/// Insets between the tray and the taskbar's right/bottom edges.
pub const TRAY_EDGE_INSET: u32 = 2;

/// Left edge of the area available to task-window buttons.
pub const fn window_button_start_x() -> u32 {
    BUTTON_GAP + START_BUTTON_WIDTH + BUTTON_GAP
}

/// Right-anchored tray bounds in taskbar-local coordinates. On displays too
/// narrow to fit Start and the tray, the tray collapses instead of overlapping
/// Start or underflowing its coordinates.
pub fn tray_bounds(taskbar_width: u32) -> Rect {
    let right = taskbar_width.saturating_sub(TRAY_EDGE_INSET);
    let minimum_left = window_button_start_x();
    let width = TRAY_WIDTH.min(right.saturating_sub(minimum_left));
    Rect::new(
        right.saturating_sub(width) as i32,
        TRAY_EDGE_INSET as i32,
        width,
        TRAY_HEIGHT.min(TASKBAR_HEIGHT.saturating_sub(TRAY_EDGE_INSET)),
    )
}

/// Bounds for one task-window button, reserving the tray's complete right-side
/// span. This pure helper is shared by initial creation, relayout, and tests.
pub fn window_button_bounds(taskbar_width: u32, button_count: usize, index: usize) -> Rect {
    let start = window_button_start_x();
    let tray = tray_bounds(taskbar_width);
    let end = (tray.x.max(0) as u32).saturating_sub(BUTTON_GAP);
    if button_count == 0 || index >= button_count {
        return Rect::new(
            start.min(end) as i32,
            BUTTON_Y_OFFSET as i32,
            0,
            BUTTON_HEIGHT,
        );
    }

    let count = button_count as u32;
    let span = end.saturating_sub(start);
    let total_gaps = count.saturating_sub(1).saturating_mul(BUTTON_GAP);
    let width = span
        .saturating_sub(total_gaps)
        .checked_div(count)
        .unwrap_or(0)
        .min(MAX_WINDOW_BUTTON_WIDTH);
    let x = start
        .saturating_add((index as u32).saturating_mul(width.saturating_add(BUTTON_GAP)))
        .min(end);
    let clipped_width = width.min(end.saturating_sub(x));
    Rect::new(
        x as i32,
        BUTTON_Y_OFFSET as i32,
        clipped_width,
        BUTTON_HEIGHT,
    )
}

pub(crate) fn format_clock(datetime: DateTime) -> (String, String) {
    (
        format!("{:02}:{:02} UTC", datetime.hour, datetime.minute),
        format!(
            "{:04}-{:02}-{:02}",
            datetime.year, datetime.month, datetime.day
        ),
    )
}

/// Tracks a window button on the taskbar
#[derive(Debug, Clone)]
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub struct TaskbarButton {
    /// ID of the button widget
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub button_id: WindowId,
    /// ID of the frame window this button represents
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub frame_id: WindowId,
    /// Title of the window
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub title: String,
}

/// The taskbar window
pub struct TaskbarWindow {
    /// Base window functionality
    base: WindowBase,
    /// Background color
    bg_color: Color,
    /// ID of the Start button
    #[expect(dead_code, reason = "intentional kernel API surface")]
    start_button_id: Option<WindowId>,
    /// Window buttons for open frame windows
    #[expect(dead_code, reason = "intentional kernel API surface")]
    window_buttons: Vec<TaskbarButton>,
    /// Currently active window (for highlighting its button)
    #[expect(dead_code, reason = "intentional kernel API surface")]
    active_frame_id: Option<WindowId>,
    /// Screen width (for layout calculations)
    #[expect(dead_code, reason = "intentional kernel API surface")]
    screen_width: u32,
}

impl TaskbarWindow {
    /// Create a new taskbar with a specific ID
    pub fn new_with_id(id: WindowId, screen_width: u32, screen_height: u32) -> Self {
        let bounds = Rect::new(
            0,
            (screen_height - TASKBAR_HEIGHT) as i32,
            screen_width,
            TASKBAR_HEIGHT,
        );

        TaskbarWindow {
            base: WindowBase::new_with_id(id, bounds),
            bg_color: crate::window::PALETTE_CONTENT_BG,
            start_button_id: None,
            window_buttons: Vec::new(),
            active_frame_id: None,
            screen_width,
        }
    }
}

impl Window for TaskbarWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        let x = bounds.x;
        let y = bounds.y;
        let width = bounds.width;
        let height = bounds.height;

        // Draw taskbar background
        device.fill_rect(x, y, width, height, self.bg_color);

        // Draw top border (highlight)
        device.draw_line(x, y, x + width as i32 - 1, y, Color::WHITE);

        // Note: Child buttons will paint themselves

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        // Taskbar doesn't handle events directly - they go to child buttons
        match event {
            Event::Mouse(_) => EventResult::Propagate,
            _ => EventResult::Ignored,
        }
    }
}

/// Right-side notification area. The v1 tray contains only a UTC date/time
/// display but remains a separate taskbar child for independent invalidation
/// and future notification icons.
pub struct TaskbarTrayWindow {
    base: WindowBase,
    time_text: String,
    date_text: String,
    last_displayed_minute: Option<u64>,
}

impl TaskbarTrayWindow {
    pub fn new_with_id(id: WindowId, bounds: Rect) -> Self {
        let mut tray = Self {
            base: WindowBase::new_with_id(id, bounds),
            time_text: String::from("--:-- UTC"),
            date_text: String::from("----------"),
            last_displayed_minute: None,
        };
        tray.refresh_clock(crate::time::wall_clock_ns(), crate::time::utc_now());
        tray
    }

    pub(crate) fn refresh_clock(&mut self, wall_clock_ns: Option<u64>, datetime: Option<DateTime>) {
        let minute = wall_clock_ns.map(|ns| ns / 60_000_000_000);
        if minute == self.last_displayed_minute {
            return;
        }

        self.last_displayed_minute = minute;
        let (time_text, date_text) = datetime.map_or_else(
            || (String::from("--:-- UTC"), String::from("----------")),
            format_clock,
        );
        if self.time_text != time_text || self.date_text != date_text {
            self.time_text = time_text;
            self.date_text = date_text;
            self.base.invalidate();
        }
    }

    #[cfg(feature = "test")]
    pub(crate) fn clock_text(&self) -> (&str, &str) {
        (&self.time_text, &self.date_text)
    }
}

impl Window for TaskbarTrayWindow {
    fn base(&self) -> &WindowBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut WindowBase {
        &mut self.base
    }

    fn prepare_for_render(&mut self) {
        self.refresh_clock(crate::time::wall_clock_ns(), crate::time::utc_now());
    }

    fn paint(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.base.visible() {
            return;
        }

        let bounds = self.base.bounds();
        if bounds.width == 0 || bounds.height == 0 {
            self.base.clear_needs_repaint();
            return;
        }

        // Windows 98-style sunken panel: dark top/left, bright bottom/right.
        device.fill_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            Color::new(192, 192, 192),
        );
        let right = bounds.x + bounds.width as i32 - 1;
        let bottom = bounds.y + bounds.height as i32 - 1;
        device.draw_line(
            bounds.x,
            bounds.y,
            right,
            bounds.y,
            Color::new(128, 128, 128),
        );
        device.draw_line(
            bounds.x,
            bounds.y,
            bounds.x,
            bottom,
            Color::new(128, 128, 128),
        );
        device.draw_line(bounds.x, bottom, right, bottom, Color::WHITE);
        device.draw_line(right, bounds.y, right, bottom, Color::WHITE);

        let font = get_caption_font();
        let line_height = font.line_height();
        let total_text_height = line_height.saturating_mul(2);
        let first_y = bounds.y
            + bounds
                .height
                .saturating_sub(total_text_height)
                .checked_div(2)
                .unwrap_or(0) as i32;
        draw_centered_text(device, bounds, first_y, &self.time_text, font);
        draw_centered_text(
            device,
            bounds,
            first_y + line_height as i32,
            &self.date_text,
            font,
        );

        self.base.clear_needs_repaint();
    }

    fn handle_event(&mut self, event: Event) -> EventResult {
        match event {
            Event::Mouse(_) => EventResult::Propagate,
            _ => EventResult::Ignored,
        }
    }
}

fn draw_centered_text(
    device: &mut dyn GraphicsDevice,
    bounds: Rect,
    y: i32,
    text: &str,
    font: crate::graphics::fonts::core_font::FontRef,
) {
    let text_width = (text.len() as u32).saturating_mul(font.cell_width());
    let x = bounds.x
        + bounds
            .width
            .saturating_sub(text_width)
            .checked_div(2)
            .unwrap_or(0) as i32;
    device.draw_text(x, y, text, font.as_font(), Color::BLACK);
}
