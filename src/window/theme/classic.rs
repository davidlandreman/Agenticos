use crate::graphics::color::Color;
use crate::window::theme::{FrameChrome, CLASSIC_METRICS};
use crate::window::GraphicsDevice;

/// Historical frame painter, kept pixel-for-pixel for the fallback path.
pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let border = CLASSIC_METRICS.border_width;
    let title_height = CLASSIC_METRICS.title_bar_height;
    let border_color = if chrome.active {
        crate::window::PALETTE_CHROME_ACTIVE
    } else {
        Color::new(150, 150, 150)
    };

    device.fill_rect(bounds.x, bounds.y, bounds.width, border, border_color);
    device.fill_rect(
        bounds.x,
        bounds.y + bounds.height as i32 - border as i32,
        bounds.width,
        border,
        border_color,
    );
    device.fill_rect(bounds.x, bounds.y, border, bounds.height, border_color);
    device.fill_rect(
        bounds.x + bounds.width as i32 - border as i32,
        bounds.y,
        border,
        bounds.height,
        border_color,
    );

    let title_bar_color = if chrome.active {
        crate::window::PALETTE_CHROME_ACTIVE
    } else {
        crate::window::PALETTE_CHROME_INACTIVE
    };
    device.fill_rect(
        bounds.x + border as i32,
        bounds.y + border as i32,
        bounds.width - 2 * border,
        title_height,
        title_bar_color,
    );

    let font = crate::graphics::fonts::core_font::get_default_font();
    let line_h = font.line_height() as i32;
    let text_y = bounds.y + border as i32 + (title_height as i32 - line_h) / 2;
    device.draw_text(
        bounds.x + border as i32 + 8,
        text_y,
        chrome.title,
        font.as_font(),
        Color::WHITE,
    );

    let button = chrome.close_button_rect;
    device.fill_rect(
        button.x,
        button.y,
        button.width,
        button.height,
        Color::new(192, 0, 0),
    );
    let padding = 4;
    let x1 = button.x + padding;
    let y1 = button.y + padding;
    let x2 = button.right() - padding - 1;
    let y2 = button.bottom() - padding - 1;
    device.draw_line(x1, y1, x2, y2, Color::WHITE);
    device.draw_line(x2, y1, x1, y2, Color::WHITE);
    device.draw_line(x1 + 1, y1, x2 + 1, y2, Color::WHITE);
    device.draw_line(x2 - 1, y1, x1 - 1, y2, Color::WHITE);
}
