//! Futurism frame painter: frosted dark title bar over backdrop blur meeting
//! the content well directly, content flush to the window edge inside a dark
//! hairline rim, large rounded corners, a soft drop shadow, and a rounded
//! soft-red close button.
//!
//! The bottom corners are carved in [`draw_overlay`], which the window
//! manager runs *after* the content child paints — the client fills the
//! whole well edge-to-edge (border is one hairline pixel) and the overlay
//! replaces the corner pixels with the shadow-blended arc again.

use crate::graphics::color::Color;
use crate::window::theme::frame_util::{self, ShadowSpec};
use crate::window::theme::{lerp_color, lerp_u8, FrameChrome, FUTURISM_METRICS};
use crate::window::GraphicsDevice;

const TITLE_TOP_ACTIVE: Color = Color::new(30, 42, 82); // #1E2A52
const TITLE_BOTTOM_ACTIVE: Color = Color::new(46, 60, 104); // #2E3C68
const TITLE_TOP_INACTIVE: Color = Color::new(56, 64, 88); // #384058
const TITLE_BOTTOM_INACTIVE: Color = Color::new(70, 78, 102); // #464E66
const TITLE_ALPHA_TOP: u8 = 228;
const TITLE_ALPHA_BOTTOM: u8 = 208;
const RIM: Color = Color::new(16, 24, 48); // dark hairline rim
const RIM_ALPHA: u8 = 88;
const CLOSE_FILL_TOP: Color = Color::new(238, 100, 88); // #EE6458
const CLOSE_FILL_BOTTOM: Color = Color::new(216, 70, 58); // #D8463A
const CLOSE_RIM: Color = Color::new(178, 52, 44); // #B2342C
const TITLE_TEXT_ACTIVE: Color = Color::WHITE;
const TITLE_TEXT_INACTIVE: Color = Color::new(196, 202, 218); // ~65% white

pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    frame_util::draw_shadow(device, chrome.bounds, &shadow_spec(chrome.active));
    draw_body(chrome, device);
    finish_top_corners(chrome, device);
    draw_close_button(chrome, device);
    draw_title(chrome, device);
}

/// Post-children pass: re-carve the rounded bottom corners over whatever the
/// content child painted there. Surface ARGB writes are exact replacement,
/// so pixels outside the arc become shadow/transparent again.
pub(super) fn draw_overlay(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    frame_util::clip_corner_pair(
        device,
        chrome.bounds,
        FUTURISM_METRICS.corner_radius_bottom,
        false,
        RIM,
        110,
        &shadow_spec(chrome.active),
    );
}

fn shadow_spec(active: bool) -> ShadowSpec {
    ShadowSpec {
        margin: FUTURISM_METRICS.shadow_margin as i32,
        peak: if active { 80 } else { 44 },
        top_radius: FUTURISM_METRICS.corner_radius_top as i32,
        bottom_radius: FUTURISM_METRICS.corner_radius_bottom as i32,
    }
}

/// Frosted title-bar gradient and the dark hairline rim. The client well
/// itself is left transparent — the content child paints it edge-to-edge
/// (inset only by the 1px rim), directly against the title bar.
fn draw_body(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let border = FUTURISM_METRICS.border_width as i32;
    let title_bottom = bounds.y + border + FUTURISM_METRICS.title_bar_height as i32;
    let (top, bottom) = if chrome.active {
        (TITLE_TOP_ACTIVE, TITLE_BOTTOM_ACTIVE)
    } else {
        (TITLE_TOP_INACTIVE, TITLE_BOTTOM_INACTIVE)
    };

    let span = (title_bottom - bounds.y - 1).max(1) as u32;
    for y in bounds.y..title_bottom {
        let row = (y - bounds.y).max(0) as u32;
        device.fill_rect_argb(
            bounds.x,
            y,
            bounds.width,
            1,
            lerp_color(top, bottom, row, span),
            lerp_u8(TITLE_ALPHA_TOP, TITLE_ALPHA_BOTTOM, row, span),
        );
    }

    // Dark hairline rim around the whole frame — no light borders; the
    // content child paints flush against it.
    frame_util::draw_rect_argb(
        device,
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height,
        RIM,
        RIM_ALPHA,
    );
    // Soft light top edge on the title bar only.
    if bounds.width > 2 {
        device.fill_rect_argb(
            bounds.x + 1,
            bounds.y + 1,
            bounds.width - 2,
            1,
            Color::WHITE,
            48,
        );
    }
}

fn finish_top_corners(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let top_fringe = if chrome.active {
        TITLE_TOP_ACTIVE
    } else {
        TITLE_TOP_INACTIVE
    };
    frame_util::clip_corner_pair(
        device,
        chrome.bounds,
        FUTURISM_METRICS.corner_radius_top,
        true,
        top_fringe,
        TITLE_ALPHA_TOP as i32,
        &shadow_spec(chrome.active),
    );
}

fn draw_close_button(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let button = chrome.close_button_rect;
    let radius = 7i32;
    let alpha = if chrome.active { 245 } else { 205 };
    for y in button.y..button.bottom() {
        let row = (y - button.y) as u32;
        let color = lerp_color(
            CLOSE_FILL_TOP,
            CLOSE_FILL_BOTTOM,
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
            device.draw_pixel_argb(x, y, if edge { CLOSE_RIM } else { color }, alpha);
        }
    }

    // Centered × glyph, independent of the button's aspect ratio.
    let size = 8i32
        .min(button.width as i32 - 4)
        .min(button.height as i32 - 4);
    if size < 3 {
        return;
    }
    let x1 = button.x + (button.width as i32 - size) / 2;
    let y1 = button.y + (button.height as i32 - size) / 2;
    let x2 = x1 + size - 1;
    let y2 = y1 + size - 1;
    device.draw_line(x1, y1, x2, y2, Color::WHITE);
    device.draw_line(x2, y1, x1, y2, Color::WHITE);
    device.draw_line(x1 + 1, y1, x2 + 1, y2, Color::WHITE);
    device.draw_line(x2 - 1, y1, x1 - 1, y2, Color::WHITE);
}

fn draw_title(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let font = crate::graphics::fonts::core_font::get_default_font();
    let line_h = font.line_height() as i32;
    let x = chrome.bounds.x + FUTURISM_METRICS.border_width as i32 + 12;
    let y = chrome.bounds.y
        + FUTURISM_METRICS.border_width as i32
        + (FUTURISM_METRICS.title_bar_height as i32 - line_h) / 2;
    let color = if chrome.active {
        TITLE_TEXT_ACTIVE
    } else {
        TITLE_TEXT_INACTIVE
    };
    device.draw_text(x, y, chrome.title, font.as_font(), color);
}
