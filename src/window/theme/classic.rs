use crate::window::theme::{lerp_color, FrameChrome, CLASSIC_METRICS};
use crate::window::{GraphicsDevice, Rect};

/// Windows 98 "Windows Standard" palette. The bevel colors do not follow focus;
/// only the caption gradient and caption text do.
mod colors {
    use crate::graphics::color::Color;

    /// 3D light — outer top/left bevel ring.
    pub const BEVEL_LIGHT: Color = Color::new(223, 223, 223); // #DFDFDF
    /// 3D dark shadow — outer bottom/right bevel ring.
    pub const BEVEL_DARK: Color = Color::new(0, 0, 0); // #000000
    /// 3D highlight — inner top/left bevel ring.
    pub const BEVEL_HIGHLIGHT: Color = Color::WHITE; // #FFFFFF
    /// 3D shadow — inner bottom/right bevel ring.
    pub const BEVEL_SHADOW: Color = Color::new(128, 128, 128); // #808080
    /// 3D face — border fill and button face.
    pub const FACE: Color = Color::new(192, 192, 192); // #C0C0C0

    pub const CAPTION_ACTIVE_LEFT: Color = Color::new(0, 0, 128); // #000080
    pub const CAPTION_ACTIVE_RIGHT: Color = Color::new(16, 132, 208); // #1084D0
    pub const CAPTION_INACTIVE_LEFT: Color = Color::new(128, 128, 128); // #808080
    pub const CAPTION_INACTIVE_RIGHT: Color = Color::new(181, 181, 181); // #B5B5B5

    pub const CAPTION_TEXT_ACTIVE: Color = Color::WHITE;
    pub const CAPTION_TEXT_INACTIVE: Color = Color::new(192, 192, 192); // #C0C0C0

    pub const GLYPH: Color = Color::new(0, 0, 0);
}

/// An 8×7 close glyph (Marlett-style ✕) with 2px-thick diagonal strokes. Bit 7
/// (0x80) is the leftmost column. Rendered as a hardcoded bitmap rather than
/// `draw_line` diagonals so it is pixel-stable across sizes.
const CLOSE_GLYPH_W: i32 = 8;
const CLOSE_GLYPH: [u8; 7] = [
    0b11000011, // ##....##
    0b01100110, // .##..##.
    0b00111100, // ..####..
    0b00011000, // ...##...
    0b00111100, // ..####..
    0b01100110, // .##..##.
    0b11000011, // ##....##
];

/// Windows 98 classic frame painter. The raised 3D bevel gives the window its
/// depth (no soft drop shadow); the caption gradient and caption text follow
/// focus, the bevel does not.
pub(super) fn draw(chrome: &FrameChrome<'_>, device: &mut dyn GraphicsDevice) {
    let bounds = chrome.bounds;
    let border = CLASSIC_METRICS.border_width as i32;
    let title_height = CLASSIC_METRICS.title_bar_height as i32;

    draw_bevel_frame(bounds, device);
    draw_caption_gradient(bounds, border, title_height, chrome.active, device);
    draw_close_button(chrome.close_button_rect, device);
    draw_caption_text(chrome, bounds, border, title_height, device);
}

/// Two raised bevel rings (light/dark outer, highlight/shadow inner) plus the
/// 2px ButtonFace fill ring, per the GDI `DrawEdge` cross-section. The
/// bottom/right (dark) edges are drawn over the full side length so they own
/// the top-right and bottom-left corner pixels. Identical for active/inactive.
fn draw_bevel_frame(bounds: Rect, device: &mut dyn GraphicsDevice) {
    let x = bounds.x;
    let y = bounds.y;
    let w = bounds.width;
    let h = bounds.height;

    for (ring, top_left, bottom_right) in [
        (0i32, colors::BEVEL_LIGHT, colors::BEVEL_DARK),
        (1i32, colors::BEVEL_HIGHLIGHT, colors::BEVEL_SHADOW),
    ] {
        let side_w = w.saturating_sub(2 * ring as u32);
        let side_h = h.saturating_sub(2 * ring as u32);
        // Top and left first…
        device.fill_rect(x + ring, y + ring, side_w, 1, top_left);
        device.fill_rect(x + ring, y + ring, 1, side_h, top_left);
        // …then bottom and right over the full span so dark owns the corners.
        device.fill_rect(x + ring, y + h as i32 - 1 - ring, side_w, 1, bottom_right);
        device.fill_rect(x + w as i32 - 1 - ring, y + ring, 1, side_h, bottom_right);
    }

    // 2px ButtonFace fill ring inside the bevel (rings 2 and 3).
    let inner_w = w.saturating_sub(4);
    let inner_h = h.saturating_sub(4);
    device.fill_rect(x + 2, y + 2, inner_w, 2, colors::FACE);
    device.fill_rect(x + 2, y + h as i32 - 4, inner_w, 2, colors::FACE);
    device.fill_rect(x + 2, y + 2, 2, inner_h, colors::FACE);
    device.fill_rect(x + w as i32 - 4, y + 2, 2, inner_h, colors::FACE);
}

/// Horizontal left→right caption gradient across the caption band.
fn draw_caption_gradient(
    bounds: Rect,
    border: i32,
    title_height: i32,
    active: bool,
    device: &mut dyn GraphicsDevice,
) {
    let (left, right) = if active {
        (colors::CAPTION_ACTIVE_LEFT, colors::CAPTION_ACTIVE_RIGHT)
    } else {
        (
            colors::CAPTION_INACTIVE_LEFT,
            colors::CAPTION_INACTIVE_RIGHT,
        )
    };
    let caption_width = bounds.width.saturating_sub(2 * border as u32);
    let span = caption_width.saturating_sub(1).max(1);
    for i in 0..caption_width {
        let color = lerp_color(left, right, i, span);
        device.fill_rect(
            bounds.x + border + i as i32,
            bounds.y + border,
            1,
            title_height as u32,
            color,
        );
    }
}

/// Raised ButtonFace push button with a black ✕. The button edge order is the
/// standard control bevel (highlight outermost, light inner) — the inverse of
/// the window edge above.
fn draw_close_button(button: Rect, device: &mut dyn GraphicsDevice) {
    let x = button.x;
    let y = button.y;
    let w = button.width;
    let h = button.height;

    device.fill_rect(x, y, w, h, colors::FACE);

    for (ring, top_left, bottom_right) in [
        (0i32, colors::BEVEL_HIGHLIGHT, colors::BEVEL_DARK),
        (1i32, colors::BEVEL_LIGHT, colors::BEVEL_SHADOW),
    ] {
        let side_w = w.saturating_sub(2 * ring as u32);
        let side_h = h.saturating_sub(2 * ring as u32);
        device.fill_rect(x + ring, y + ring, side_w, 1, top_left);
        device.fill_rect(x + ring, y + ring, 1, side_h, top_left);
        device.fill_rect(x + ring, y + h as i32 - 1 - ring, side_w, 1, bottom_right);
        device.fill_rect(x + w as i32 - 1 - ring, y + ring, 1, side_h, bottom_right);
    }

    // Center the glyph in the button face.
    let glyph_h = CLOSE_GLYPH.len() as i32;
    let ox = x + (w as i32 - CLOSE_GLYPH_W) / 2;
    let oy = y + (h as i32 - glyph_h) / 2;
    for (row, bits) in CLOSE_GLYPH.iter().enumerate() {
        for col in 0..CLOSE_GLYPH_W {
            if bits & (0b1000_0000 >> col) != 0 {
                device.draw_pixel(ox + col, oy + row as i32, colors::GLYPH);
            }
        }
    }
}

/// Bold, focus-colored caption text, clipped so it never draws into the close
/// button. Bold is synthesized by double-striking (draw at `x` and `x+1`).
fn draw_caption_text(
    chrome: &FrameChrome<'_>,
    bounds: Rect,
    border: i32,
    title_height: i32,
    device: &mut dyn GraphicsDevice,
) {
    if chrome.title.is_empty() {
        return;
    }
    let color = if chrome.active {
        colors::CAPTION_TEXT_ACTIVE
    } else {
        colors::CAPTION_TEXT_INACTIVE
    };
    let font = crate::graphics::fonts::core_font::get_caption_font();
    let line_h = font.line_height() as i32;
    let text_x = bounds.x + border + 3;
    let text_y = bounds.y + border + (title_height - line_h) / 2;

    // Elide the caption text before it reaches the close button.
    let clip_right = chrome.close_button_rect.x - 2;
    let clip_width = (clip_right - text_x).max(0) as u32;
    device.set_clip_rect(Some(Rect::new(
        text_x,
        bounds.y + border,
        clip_width,
        title_height as u32,
    )));
    device.draw_text(text_x, text_y, chrome.title, font.as_font(), color);
    device.draw_text(text_x + 1, text_y, chrome.title, font.as_font(), color);
    device.set_clip_rect(None);
}
