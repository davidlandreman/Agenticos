//! Shared geometry for frame painters that draw translucent, rounded,
//! shadowed chrome (Aero, Futurism).
//!
//! Extracted verbatim from the original Aero painter and parameterized on
//! metrics/colors so painters share one implementation of the drop shadow,
//! the anti-aliased corner clipping, and the fringe-over-shadow blend.

use crate::graphics::color::Color;
use crate::window::{GraphicsDevice, Rect};

/// Drop-shadow parameters for one frame paint.
pub(super) struct ShadowSpec {
    pub margin: i32,
    pub peak: u32,
    pub top_radius: i32,
    pub bottom_radius: i32,
}

/// Paint the translucent drop shadow in the `margin` gutter around `bounds`.
pub(super) fn draw_shadow(device: &mut dyn GraphicsDevice, bounds: Rect, spec: &ShadowSpec) {
    let margin = spec.margin;
    for y in bounds.y - margin..bounds.bottom() + margin {
        for x in bounds.x - margin..bounds.right() + margin {
            if bounds.contains_point(crate::window::Point::new(x, y)) {
                continue;
            }
            let distance = shadow_distance_from_frame(x, y, bounds, spec);
            let alpha = shadow_falloff(distance, margin, spec.peak);
            device.draw_pixel_argb(x, y, Color::BLACK, alpha);
        }
    }
}

/// Distance from a pixel to the rounded frame outline.
///
/// Straight sides retain the original one-dimensional falloff. In each corner
/// region the distance is measured from the corresponding corner circle, so
/// the shadow expands concentrically around the same arcs that clip the frame.
fn shadow_distance_from_frame(x: i32, y: i32, bounds: Rect, spec: &ShadowSpec) -> i32 {
    let top_radius = spec.top_radius;
    let bottom_radius = spec.bottom_radius;
    let margin = spec.margin;

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

pub(super) fn ceil_sqrt(value: i32, upper_bound: i32) -> i32 {
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

pub(super) fn shadow_falloff(distance: i32, margin: i32, peak: u32) -> u8 {
    if distance <= 0 || distance > margin {
        return 0;
    }
    let remaining = (margin - distance + 1) as u32;
    (peak * remaining * remaining / (margin as u32 * margin as u32)).min(255) as u8
}

/// Replace the corner pixels of `bounds` with an anti-aliased arc fringe in
/// `fringe` (peak alpha `fringe_peak`), blended over the drop shadow so the
/// notch between the shadow bands stays filled.
pub(super) fn clip_corner_pair(
    device: &mut dyn GraphicsDevice,
    bounds: Rect,
    radius: u32,
    top: bool,
    fringe: Color,
    fringe_peak: i32,
    shadow: &ShadowSpec,
) {
    if radius == 0 {
        return;
    }
    let r = radius as i32;
    let inner_sq = (r - 1).max(0) * (r - 1).max(0);
    let outer_sq = r * r;
    let fringe_span = (outer_sq - inner_sq).max(1);
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
                (((outer_sq - distance_sq) * fringe_peak) / fringe_span).clamp(1, fringe_peak) as u8
            };
            let y = if top {
                bounds.y + oy
            } else {
                bounds.bottom() - 1 - oy
            };
            let shadow_distance = (ceil_sqrt(distance_sq, r + shadow.margin) - r).max(1);
            let shadow_alpha = shadow_falloff(shadow_distance, shadow.margin, shadow.peak);
            let (color, alpha) = fringe_over_shadow(fringe, alpha, shadow_alpha);
            device.draw_pixel_argb(bounds.x + ox, y, color, alpha);
            device.draw_pixel_argb(bounds.right() - 1 - ox, y, color, alpha);
        }
    }
}

/// Combine the anti-aliased frame fringe with the black shadow behind it.
/// Pixels fully outside the frame arc retain the shadow instead of becoming a
/// transparent notch between the horizontal and vertical shadow bands.
fn fringe_over_shadow(fringe: Color, fringe_alpha: u8, shadow_alpha: u8) -> (Color, u8) {
    let fringe_alpha = fringe_alpha as u32;
    let shadow_alpha = shadow_alpha as u32;
    let inverse_fringe = 255 - fringe_alpha;
    let output_alpha = fringe_alpha + (shadow_alpha * inverse_fringe + 127) / 255;
    if output_alpha == 0 {
        return (Color::BLACK, 0);
    }
    let channel = |value: u8| {
        ((value as u32 * fringe_alpha + output_alpha / 2) / output_alpha).min(255) as u8
    };
    (
        Color::new(
            channel(fringe.red),
            channel(fringe.green),
            channel(fringe.blue),
        ),
        output_alpha.min(255) as u8,
    )
}

/// One-pixel translucent rectangle outline.
pub(super) fn draw_rect_argb(
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

/// Whether a local pixel lies outside a rounded rect of uniform `radius`.
pub(super) fn outside_rounded_rect(x: i32, y: i32, width: i32, height: i32, radius: i32) -> bool {
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
