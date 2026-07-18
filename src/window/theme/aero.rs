use crate::graphics::color::Color;
use crate::window::theme::{FrameChrome, AERO_METRICS};
use crate::window::GraphicsDevice;

pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    draw_shadow(chrome, device);
    draw_glass(chrome, device);
    punch_rounded_corners(chrome, device);
    draw_close_button(chrome, device);
    draw_title(chrome, device);
}

fn draw_shadow(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let margin = AERO_METRICS.shadow_margin as i32;
    let peak = if chrome.active { 96u32 } else { 56u32 };
    for y in bounds.y - margin..bounds.bottom() + margin {
        for x in bounds.x - margin..bounds.right() + margin {
            let dx = if x < bounds.x {
                bounds.x - x
            } else if x >= bounds.right() {
                x - bounds.right() + 1
            } else {
                0
            };
            let dy = if y < bounds.y {
                bounds.y - y
            } else if y >= bounds.bottom() {
                y - bounds.bottom() + 1
            } else {
                0
            };
            if dx == 0 && dy == 0 {
                continue;
            }
            let edge_x = shadow_falloff(dx, margin, peak);
            let edge_y = shadow_falloff(dy, margin, peak);
            let alpha = match (dx == 0, dy == 0) {
                (true, false) => edge_y,
                (false, true) => edge_x,
                (false, false) => (edge_x as u32 * edge_y as u32 / peak.max(1)) as u8,
                (true, true) => 0,
            };
            device.draw_pixel_argb(x, y, Color::BLACK, alpha);
        }
    }
}

fn shadow_falloff(distance: i32, margin: i32, peak: u32) -> u8 {
    if distance <= 0 || distance > margin {
        return 0;
    }
    let remaining = (margin - distance + 1) as u32;
    (peak * remaining * remaining / (margin as u32 * margin as u32)).min(255) as u8
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
    draw_rect_argb(
        device,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        Color::BLACK,
        120,
    );
    if bounds.width > 2 && bounds.height > 2 {
        draw_rect_argb(
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

fn punch_rounded_corners(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    clip_corner_pair(
        device,
        bounds,
        AERO_METRICS.corner_radius_top,
        true,
        chrome.active,
    );
    clip_corner_pair(
        device,
        bounds,
        AERO_METRICS.corner_radius_bottom,
        false,
        chrome.active,
    );
}

fn clip_corner_pair(
    device: &mut dyn GraphicsDevice,
    bounds: crate::window::Rect,
    radius: u32,
    top: bool,
    active: bool,
) {
    if radius == 0 {
        return;
    }
    let r = radius as i32;
    let inner_sq = (r - 1).max(0) * (r - 1).max(0);
    let outer_sq = r * r;
    let fringe = (outer_sq - inner_sq).max(1);
    for oy in 0..r {
        for ox in 0..r {
            let dx = r - 1 - ox;
            let dy = r - 1 - oy;
            let distance_sq = dx * dx + dy * dy;
            if distance_sq <= inner_sq {
                continue;
            }
            let alpha = if distance_sq > outer_sq {
                0
            } else {
                (((outer_sq - distance_sq) * 150) / fringe).clamp(1, 150) as u8
            };
            let y = if top {
                bounds.y + oy
            } else {
                bounds.bottom() - 1 - oy
            };
            let color = if active {
                Color::new(185, 210, 235)
            } else {
                Color::new(185, 190, 195)
            };
            device.draw_pixel_argb(bounds.x + ox, y, color, alpha);
            device.draw_pixel_argb(bounds.right() - 1 - ox, y, color, alpha);
        }
    }
}

fn draw_close_button(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let button = chrome.close_button_rect;
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
            if outside_rounded_rect(
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

fn draw_title(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let font = crate::graphics::fonts::core_font::get_default_font();
    let line_h = font.line_height() as i32;
    let x = chrome.bounds.x + AERO_METRICS.border_width as i32 + 8;
    let y = chrome.bounds.y
        + AERO_METRICS.border_width as i32
        + (AERO_METRICS.title_bar_height as i32 - line_h) / 2;
    draw_text_argb(
        device,
        x + 1,
        y + 1,
        chrome.title,
        font.as_font(),
        Color::BLACK,
        140,
    );
    device.draw_text(x, y, chrome.title, font.as_font(), Color::WHITE);
}

fn draw_text_argb(
    device: &mut dyn GraphicsDevice,
    x: i32,
    y: i32,
    text: &str,
    font: &dyn crate::graphics::fonts::core_font::Font,
    color: Color,
    alpha: u8,
) {
    let baseline = y + font.ascent() as i32;
    let mut pen_x = x;
    for ch in text.chars() {
        if ch == '\n' {
            break;
        }
        let Some(glyph) = font.glyph(ch) else {
            continue;
        };
        let glyph_x = pen_x + glyph.x_offset;
        let glyph_y = baseline + glyph.y_offset;
        for row in 0..glyph.height as i32 {
            for col in 0..glyph.width as i32 {
                let coverage = glyph.coverage[(row * glyph.width as i32 + col) as usize];
                let effective = ((coverage as u16 * alpha as u16 + 127) / 255) as u8;
                if effective != 0 {
                    device.draw_pixel_argb(glyph_x + col, glyph_y + row, color, effective);
                }
            }
        }
        pen_x += glyph.advance as i32;
    }
}

fn draw_rect_argb(
    device: &mut dyn GraphicsDevice,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
    alpha: u8,
) {
    if width == 0 || height == 0 {
        return;
    }
    device.fill_rect_argb(x, y, width, 1, color, alpha);
    device.fill_rect_argb(x, y + height as i32 - 1, width, 1, color, alpha);
    device.fill_rect_argb(x, y, 1, height, color, alpha);
    device.fill_rect_argb(x + width as i32 - 1, y, 1, height, color, alpha);
}

fn outside_rounded_rect(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
    let cx = if x < radius {
        radius - 1
    } else if x >= width - radius {
        width - radius
    } else {
        return false;
    };
    let cy = if y < radius {
        radius - 1
    } else if y >= height - radius {
        height - radius
    } else {
        return false;
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy > radius * radius
}

fn lerp_u8(start: u8, end: u8, position: u32, span: u32) -> u8 {
    let position = position.min(span);
    ((start as u32 * (span - position) + end as u32 * position) / span.max(1)) as u8
}

fn lerp_color(start: Color, end: Color, position: u32, span: u32) -> Color {
    Color::new(
        lerp_u8(start.red, end.red, position, span),
        lerp_u8(start.green, end.green, position, span),
        lerp_u8(start.blue, end.blue, position, span),
    )
}
