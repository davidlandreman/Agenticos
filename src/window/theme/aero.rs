use crate::graphics::color::Color;
use crate::window::theme::{lerp_color, lerp_u8, FrameChrome, AERO_METRICS};
use crate::window::GraphicsDevice;

pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    draw_shadow(chrome, device);
    draw_glass(chrome, device);
    finish_rounded_corners(chrome, device);
    draw_close_button(chrome, device);
    draw_title(chrome, device);
}

fn draw_shadow(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let margin = AERO_METRICS.shadow_margin as i32;
    let peak = if chrome.active { 96u32 } else { 56u32 };
    for y in bounds.y - margin..bounds.bottom() + margin {
        for x in bounds.x - margin..bounds.right() + margin {
            if bounds.contains_point(crate::window::Point::new(x, y)) {
                continue;
            }
            let distance = shadow_distance_from_frame(x, y, bounds, margin);
            let alpha = shadow_falloff(distance, margin, peak);
            device.draw_pixel_argb(x, y, Color::BLACK, alpha);
        }
    }
}

/// Distance from a pixel to the rounded Aero frame outline.
///
/// Straight sides retain the original one-dimensional falloff. In each corner
/// region the distance is measured from the corresponding corner circle, so
/// the shadow expands concentrically around the same arcs that clip the frame.
fn shadow_distance_from_frame(x: i32, y: i32, bounds: crate::window::Rect, margin: i32) -> i32 {
    let top_radius = AERO_METRICS.corner_radius_top as i32;
    let bottom_radius = AERO_METRICS.corner_radius_bottom as i32;

    if top_radius > 0 && y < bounds.y + top_radius {
        let center_y = bounds.y + top_radius - 1;
        if x < bounds.x + top_radius {
            return distance_from_corner(
                x,
                y,
                bounds.x + top_radius - 1,
                center_y,
                top_radius,
                margin,
            );
        }
        if x >= bounds.right() - top_radius {
            return distance_from_corner(
                x,
                y,
                bounds.right() - top_radius,
                center_y,
                top_radius,
                margin,
            );
        }
    }

    if bottom_radius > 0 && y >= bounds.bottom() - bottom_radius {
        let center_y = bounds.bottom() - bottom_radius;
        if x < bounds.x + bottom_radius {
            return distance_from_corner(
                x,
                y,
                bounds.x + bottom_radius - 1,
                center_y,
                bottom_radius,
                margin,
            );
        }
        if x >= bounds.right() - bottom_radius {
            return distance_from_corner(
                x,
                y,
                bounds.right() - bottom_radius,
                center_y,
                bottom_radius,
                margin,
            );
        }
    }

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
    dx.max(dy)
}

fn distance_from_corner(
    x: i32,
    y: i32,
    center_x: i32,
    center_y: i32,
    radius: i32,
    margin: i32,
) -> i32 {
    let dx = x - center_x;
    let dy = y - center_y;
    let distance_sq = dx * dx + dy * dy;
    let outer_radius = radius + margin;
    if distance_sq > outer_radius * outer_radius {
        return margin + 1;
    }
    (ceil_sqrt(distance_sq, outer_radius) - radius).max(1)
}

fn ceil_sqrt(value: i32, upper_bound: i32) -> i32 {
    if value <= 0 {
        return 0;
    }
    let mut low = 1;
    let mut high = upper_bound.max(1);
    while low < high {
        let middle = low + (high - low) / 2;
        if middle * middle >= value {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
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

fn finish_rounded_corners(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
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
    let shadow_margin = AERO_METRICS.shadow_margin as i32;
    let shadow_peak = if active { 96u32 } else { 56u32 };
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
            let shadow_distance = (ceil_sqrt(distance_sq, r + shadow_margin) - r).max(1);
            let shadow_alpha = shadow_falloff(shadow_distance, shadow_margin, shadow_peak);
            let (color, alpha) = glass_over_shadow(color, alpha, shadow_alpha);
            device.draw_pixel_argb(bounds.x + ox, y, color, alpha);
            device.draw_pixel_argb(bounds.right() - 1 - ox, y, color, alpha);
        }
    }
}

/// Combine the anti-aliased glass fringe with the black shadow behind it.
/// Pixels fully outside the frame arc retain the shadow instead of becoming a
/// transparent notch between the horizontal and vertical shadow bands.
fn glass_over_shadow(glass: Color, glass_alpha: u8, shadow_alpha: u8) -> (Color, u8) {
    let glass_alpha = glass_alpha as u32;
    let shadow_alpha = shadow_alpha as u32;
    let inverse_glass = 255 - glass_alpha;
    let output_alpha = glass_alpha + (shadow_alpha * inverse_glass + 127) / 255;
    if output_alpha == 0 {
        return (Color::BLACK, 0);
    }
    let channel =
        |value: u8| ((value as u32 * glass_alpha + output_alpha / 2) / output_alpha).min(255) as u8;
    (
        Color::new(
            channel(glass.red),
            channel(glass.green),
            channel(glass.blue),
        ),
        output_alpha.min(255) as u8,
    )
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
