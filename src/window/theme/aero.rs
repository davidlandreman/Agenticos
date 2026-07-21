use crate::graphics::color::Color;
use crate::window::theme::frame_util::{self, ShadowSpec};
use crate::window::theme::{lerp_color, lerp_u8, FrameChrome, AERO_METRICS};
use crate::window::{GraphicsDevice, Rect};

pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    frame_util::draw_shadow(device, chrome.bounds, &shadow_spec(chrome.active));
    draw_glass(chrome, device);
    finish_rounded_corners(chrome, device);
    if let Some(button) = chrome.buttons.minimize {
        draw_neutral_button(button, NeutralGlyph::Minimize, device);
    }
    if let Some(button) = chrome.buttons.maximize {
        draw_neutral_button(
            button,
            if chrome.maximized {
                NeutralGlyph::Restore
            } else {
                NeutralGlyph::Maximize
            },
            device,
        );
    }
    draw_close_button(chrome, device);
    draw_title(chrome, device);
}

fn shadow_spec(active: bool) -> ShadowSpec {
    ShadowSpec {
        margin: AERO_METRICS.shadow_margin as i32,
        peak: if active { 96 } else { 56 },
        top_radius: AERO_METRICS.corner_radius_top as i32,
        bottom_radius: AERO_METRICS.corner_radius_bottom as i32,
    }
}

fn draw_glass(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let border = AERO_METRICS.border_width as i32;
    let title_bottom = bounds.y + border + AERO_METRICS.title_bar_height as i32;
    let (top, bottom, top_alpha, bottom_alpha) = if chrome.active {
        (
            Color::new(220, 235, 250),
            Color::new(160, 190, 220),
            180u8,
            150u8,
        )
    } else {
        (
            Color::new(200, 205, 210),
            Color::new(170, 175, 180),
            190u8,
            170u8,
        )
    };

    for y in bounds.y..bounds.bottom() {
        let in_title = y < title_bottom;
        let in_bottom = y >= bounds.bottom() - border;
        let (color, alpha) = if in_title {
            let row = (y - bounds.y).max(0) as u32;
            let span = (title_bottom - bounds.y - 1).max(1) as u32;
            (
                lerp_color(top, bottom, row, span),
                lerp_u8(top_alpha, bottom_alpha, row, span),
            )
        } else {
            (bottom, bottom_alpha)
        };
        if in_title || in_bottom {
            device.fill_rect_argb(bounds.x, y, bounds.width, 1, color, alpha);
        } else {
            device.fill_rect_argb(bounds.x, y, border as u32, 1, color, alpha);
            device.fill_rect_argb(bounds.right() - border, y, border as u32, 1, color, alpha);
        }
    }

    // Dark outside rim and bright inside rim.
    frame_util::draw_rect_argb(
        device,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        Color::BLACK,
        120,
    );
    if bounds.width > 2 && bounds.height > 2 {
        frame_util::draw_rect_argb(
            device,
            bounds.x + 1,
            bounds.y + 1,
            bounds.width - 2,
            bounds.height - 2,
            Color::WHITE,
            90,
        );
    }
    let client_top = title_bottom;
    device.fill_rect_argb(
        bounds.x + border,
        client_top - 1,
        bounds.width.saturating_sub((border * 2) as u32),
        1,
        Color::WHITE,
        80,
    );
}

fn finish_rounded_corners(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let fringe = if chrome.active {
        Color::new(185, 210, 235)
    } else {
        Color::new(185, 190, 195)
    };
    let shadow = shadow_spec(chrome.active);
    frame_util::clip_corner_pair(
        device,
        bounds,
        AERO_METRICS.corner_radius_top,
        true,
        fringe,
        150,
        &shadow,
    );
    frame_util::clip_corner_pair(
        device,
        bounds,
        AERO_METRICS.corner_radius_bottom,
        false,
        fringe,
        150,
        &shadow,
    );
}

fn draw_close_button(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let button = chrome.buttons.close;
    let radius = 3i32;
    for y in button.y..button.bottom() {
        let row = (y - button.y) as u32;
        let color = lerp_color(
            Color::new(232, 17, 35),
            Color::new(140, 10, 20),
            row,
            button.height.saturating_sub(1).max(1),
        );
        for x in button.x..button.right() {
            let local_x = x - button.x;
            let local_y = y - button.y;
            if frame_util::outside_rounded_rect(
                local_x,
                local_y,
                button.width as i32,
                button.height as i32,
                radius,
            ) {
                continue;
            }
            let edge = local_x == 0
                || local_y == 0
                || local_x == button.width as i32 - 1
                || local_y == button.height as i32 - 1;
            device.draw_pixel_argb(x, y, if edge { Color::new(105, 5, 12) } else { color }, 245);
        }
    }

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

#[derive(Clone, Copy)]
enum NeutralGlyph {
    Minimize,
    Maximize,
    Restore,
}

fn draw_neutral_button(button: Rect, glyph: NeutralGlyph, device: &mut dyn GraphicsDevice) {
    let radius = 3i32;
    for y in button.y..button.bottom() {
        let row = (y - button.y) as u32;
        let color = lerp_color(
            Color::new(226, 239, 250),
            Color::new(132, 170, 205),
            row,
            button.height.saturating_sub(1).max(1),
        );
        for x in button.x..button.right() {
            let local_x = x - button.x;
            let local_y = y - button.y;
            if frame_util::outside_rounded_rect(
                local_x,
                local_y,
                button.width as i32,
                button.height as i32,
                radius,
            ) {
                continue;
            }
            let edge = local_x == 0
                || local_y == 0
                || local_x == button.width as i32 - 1
                || local_y == button.height as i32 - 1;
            device.draw_pixel_argb(
                x,
                y,
                if edge {
                    Color::new(70, 105, 135)
                } else {
                    color
                },
                220,
            );
        }
    }
    draw_neutral_glyph(button, glyph, device);
}

fn draw_neutral_glyph(button: Rect, glyph: NeutralGlyph, device: &mut dyn GraphicsDevice) {
    let color = Color::new(25, 45, 65);
    let left = button.x + (button.width as i32 - 8) / 2;
    let top = button.y + (button.height as i32 - 7) / 2;
    match glyph {
        NeutralGlyph::Minimize => device.fill_rect(left, top + 5, 8, 2, color),
        NeutralGlyph::Maximize => {
            device.draw_line(left, top, left + 7, top, color);
            device.draw_line(left, top + 1, left, top + 6, color);
            device.draw_line(left + 7, top + 1, left + 7, top + 6, color);
            device.draw_line(left, top + 6, left + 7, top + 6, color);
        }
        NeutralGlyph::Restore => {
            device.draw_line(left + 2, top, left + 7, top, color);
            device.draw_line(left + 7, top, left + 7, top + 4, color);
            device.draw_line(left, top + 2, left + 5, top + 2, color);
            device.draw_line(left, top + 2, left, top + 6, color);
            device.draw_line(left + 5, top + 2, left + 5, top + 6, color);
            device.draw_line(left, top + 6, left + 5, top + 6, color);
        }
    }
}

fn draw_title(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let font = crate::graphics::fonts::core_font::get_default_font();
    let line_h = font.line_height() as i32;
    let x = chrome.bounds.x + AERO_METRICS.border_width as i32 + 8;
    let y = chrome.bounds.y
        + AERO_METRICS.border_width as i32
        + (AERO_METRICS.title_bar_height as i32 - line_h) / 2;
    let clip_right = chrome.buttons.leftmost_x() - 3;
    device.set_clip_rect(Some(Rect::new(
        x,
        chrome.bounds.y + AERO_METRICS.border_width as i32,
        (clip_right - x).max(0) as u32,
        AERO_METRICS.title_bar_height,
    )));
    device.draw_text(x, y, chrome.title, font.as_font(), Color::BLACK);
    device.set_clip_rect(None);
}
