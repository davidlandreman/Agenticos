//! Theme-aware control surfaces (buttons, fields, panels, selection).
//!
//! Widgets own layout, labels, and hit-testing; they delegate *surface
//! rendering* to these helpers so every control follows the boot-selected
//! Classic / Aero theme. The theme is fixed at boot (`theme::active()`), so
//! helpers dispatch at paint time with no invalidation machinery.
//!
//! Normative color values live in
//! `docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md`; the ring-3
//! toolkit (`userland/libs/gui`) mirrors the same tables.

use crate::graphics::color::Color;
use crate::window::theme::{self, classic::colors as classic, lerp_color, ThemeKind};
use crate::window::{GraphicsDevice, Rect};

/// Visual state of a push-button-like control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    Normal,
    /// Default / accent button (Aero: blue border + glow; Classic: black rim).
    Hot,
    Pressed,
    Disabled,
}

/// Theme-scoped colors that widgets read directly (text, selection, wells).
/// Surface *shapes* (bevels, gradients, rounded corners) go through the
/// drawing helpers below instead.
pub struct ControlPalette {
    /// Window / panel content background.
    pub content_bg: Color,
    /// Text on `content_bg` and on button faces.
    pub text: Color,
    /// Greyed-out label text.
    pub disabled_text: Color,
    /// Generic border / divider color.
    pub border: Color,
    /// Interior of text fields, lists, and other data wells.
    pub field_bg: Color,
    /// Text drawn on `field_bg`.
    pub field_text: Color,
    /// Selection / hover highlight fill.
    pub selection_bg: Color,
    /// Text on `selection_bg`.
    pub selection_text: Color,
    /// Filled portion of a progress bar. (`ProgressBar` is currently only
    /// constructed by QEMU tests, so the read is invisible to production
    /// dead-code analysis.)
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub progress_fill: Color,
    /// Scrollbar thumb fill.
    pub scrollbar_thumb: Color,
    /// Scrollbar track fill.
    pub scrollbar_track: Color,
}

const CLASSIC_PALETTE: ControlPalette = ControlPalette {
    content_bg: classic::FACE,
    text: Color::BLACK,
    disabled_text: classic::BEVEL_SHADOW,
    border: classic::BEVEL_SHADOW,
    field_bg: Color::WHITE,
    field_text: Color::BLACK,
    selection_bg: Color::new(0, 0, 128), // #000080 navy
    selection_text: Color::WHITE,
    progress_fill: Color::new(0, 0, 128), // navy blocks
    scrollbar_thumb: classic::FACE,
    scrollbar_track: Color::new(223, 223, 223), // #DFDFDF checker-ish track
};

const AERO_PALETTE: ControlPalette = ControlPalette {
    content_bg: Color::new(240, 240, 240), // #F0F0F0
    text: Color::BLACK,
    disabled_text: Color::new(131, 131, 131), // #838383
    border: Color::new(112, 112, 112),        // #707070
    field_bg: Color::WHITE,
    field_text: Color::BLACK,
    selection_bg: Color::new(203, 232, 246), // #CBE8F6
    selection_text: Color::BLACK,
    progress_fill: Color::new(6, 176, 37),      // Win7 green
    scrollbar_thumb: Color::new(205, 205, 205), // #CDCDCD
    scrollbar_track: Color::new(240, 240, 240),
};

/// The palette for the boot-selected theme.
pub fn palette() -> &'static ControlPalette {
    palette_for(theme::active())
}

pub const fn palette_for(kind: ThemeKind) -> &'static ControlPalette {
    match kind {
        ThemeKind::Classic => &CLASSIC_PALETTE,
        ThemeKind::Aero => &AERO_PALETTE,
    }
}

// ---------------------------------------------------------------------
// Aero constants (KD4 in the plan; from the reference screenshot + Win7)
// ---------------------------------------------------------------------

const AERO_RADIUS: i32 = 3;
const AERO_BORDER_NORMAL: Color = Color::new(112, 112, 112); // #707070
const AERO_BORDER_HOT: Color = Color::new(60, 127, 177); // #3C7FB1
const AERO_BORDER_PRESSED: Color = Color::new(44, 98, 139); // #2C628B
const AERO_BORDER_DISABLED: Color = Color::new(173, 178, 181); // #ADB2B5
const AERO_GLOW: Color = Color::new(169, 212, 240); // #A9D4F0
const AERO_INNER_HIGHLIGHT: Color = Color::new(252, 252, 252); // #FCFCFC
const AERO_INNER_SHADOW: Color = Color::new(157, 182, 200); // #9DB6C8
const AERO_FILL_NORMAL: [Color; 4] = [
    Color::new(242, 242, 242), // #F2F2F2
    Color::new(235, 235, 235), // #EBEBEB
    Color::new(221, 221, 221), // #DDDDDD
    Color::new(207, 207, 207), // #CFCFCF
];
const AERO_FILL_HOT: [Color; 4] = [
    Color::new(234, 246, 253), // #EAF6FD
    Color::new(217, 240, 252), // #D9F0FC
    Color::new(190, 230, 253), // #BEE6FD
    Color::new(167, 217, 245), // #A7D9F5
];
const AERO_FILL_PRESSED: [Color; 4] = [
    Color::new(229, 244, 252), // #E5F4FC
    Color::new(196, 229, 246), // #C4E5F6
    Color::new(152, 209, 239), // #98D1EF
    Color::new(104, 179, 219), // #68B3DB
];
const AERO_FILL_DISABLED: Color = Color::new(244, 244, 244); // #F4F4F4
const AERO_FIELD_BORDER: Color = Color::new(171, 171, 171); // #ABABAB
const AERO_PANEL_TOP: Color = Color::new(240, 245, 250); // #F0F5FA
const AERO_PANEL_BOTTOM: Color = Color::new(207, 217, 228); // #CFD9E4
const AERO_PANEL_EDGE: Color = Color::new(182, 188, 198); // #B6BCC6

/// Corner inset (pixels skipped from each side) for a given distance from the
/// top/bottom edge, quantizing a radius-3 rounded corner.
const fn aero_corner_inset(edge_distance: i32) -> i32 {
    match edge_distance {
        0 => 2,
        1 => 1,
        _ => 0,
    }
}

/// Label color for a button in `state`.
pub fn button_text(state: ControlState) -> Color {
    match state {
        ControlState::Disabled => palette().disabled_text,
        _ => palette().text,
    }
}

/// When a pressed control shifts its label down-right by 1px (Classic only).
pub fn pressed_label_shift(state: ControlState) -> i32 {
    if theme::active() == ThemeKind::Classic && state == ControlState::Pressed {
        1
    } else {
        0
    }
}

/// Paint a push-button surface (face + edges, no label) into `rect`.
pub fn draw_button(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    match theme::active() {
        ThemeKind::Classic => draw_classic_button(device, rect, state),
        ThemeKind::Aero => draw_aero_button(device, rect, state),
    }
}

/// Classic raised / sunken button per the Win98 `DrawEdge` cross-section.
fn draw_classic_button(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    let face = if state == ControlState::Disabled {
        Color::new(212, 208, 200) // slightly lighter disabled face
    } else {
        classic::FACE
    };
    device.fill_rect(rect.x, rect.y, rect.width, rect.height, face);

    let rings: [(Color, Color); 2] = match state {
        ControlState::Pressed => [
            (classic::BEVEL_SHADOW, classic::BEVEL_HIGHLIGHT),
            (classic::BEVEL_DARK, classic::BEVEL_LIGHT),
        ],
        _ => [
            (classic::BEVEL_HIGHLIGHT, classic::BEVEL_DARK),
            (classic::BEVEL_LIGHT, classic::BEVEL_SHADOW),
        ],
    };
    draw_bevel_rings(device, rect, &rings);

    // Default button: extra 1px black rim outside the bevel (drawn just
    // inside the bounds, displacing nothing — classic dialogs reserve the
    // pixel). Kept inside `rect` so footprints never grow.
    if state == ControlState::Hot {
        outline(device, rect, classic::BEVEL_DARK);
    }
}

/// Two concentric 1px bevel rings; `rings[i] = (top_left, bottom_right)`.
fn draw_bevel_rings(device: &mut dyn GraphicsDevice, rect: Rect, rings: &[(Color, Color)]) {
    let x = rect.x;
    let y = rect.y;
    let w = rect.width;
    let h = rect.height;
    for (ring, (top_left, bottom_right)) in rings.iter().enumerate() {
        let ring = ring as i32;
        let side_w = w.saturating_sub(2 * ring as u32);
        let side_h = h.saturating_sub(2 * ring as u32);
        device.fill_rect(x + ring, y + ring, side_w, 1, *top_left);
        device.fill_rect(x + ring, y + ring, 1, side_h, *top_left);
        device.fill_rect(x + ring, y + h as i32 - 1 - ring, side_w, 1, *bottom_right);
        device.fill_rect(x + w as i32 - 1 - ring, y + ring, 1, side_h, *bottom_right);
    }
}

fn outline(device: &mut dyn GraphicsDevice, rect: Rect, color: Color) {
    device.fill_rect(rect.x, rect.y, rect.width, 1, color);
    device.fill_rect(
        rect.x,
        rect.y + rect.height as i32 - 1,
        rect.width,
        1,
        color,
    );
    device.fill_rect(rect.x, rect.y, 1, rect.height, color);
    device.fill_rect(
        rect.x + rect.width as i32 - 1,
        rect.y,
        1,
        rect.height,
        color,
    );
}

/// Aero rounded-gradient button. Everything stays inside `rect`: the Hot glow
/// is an inner ring, not an outer one, so footprints never grow.
fn draw_aero_button(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    let h = rect.height as i32;
    let w = rect.width as i32;
    if h < 6 || w < 6 {
        // Too small for rounding; flat fallback.
        device.fill_rect(rect.x, rect.y, rect.width, rect.height, AERO_FILL_NORMAL[1]);
        outline(device, rect, AERO_BORDER_NORMAL);
        return;
    }

    let border = match state {
        ControlState::Normal => AERO_BORDER_NORMAL,
        ControlState::Hot => AERO_BORDER_HOT,
        ControlState::Pressed => AERO_BORDER_PRESSED,
        ControlState::Disabled => AERO_BORDER_DISABLED,
    };

    // Fill: two vertical gradient bands (stops 0→1 above the midline, 2→3
    // below) with the characteristic hard transition in the middle.
    let mid = h / 2;
    for row in 0..h {
        let inset = aero_corner_inset(row.min(h - 1 - row));
        let color = match state {
            ControlState::Disabled => AERO_FILL_DISABLED,
            _ => {
                let stops = match state {
                    ControlState::Hot => &AERO_FILL_HOT,
                    ControlState::Pressed => &AERO_FILL_PRESSED,
                    _ => &AERO_FILL_NORMAL,
                };
                if row < mid {
                    lerp_color(stops[0], stops[1], row as u32, (mid - 1).max(1) as u32)
                } else {
                    lerp_color(
                        stops[2],
                        stops[3],
                        (row - mid) as u32,
                        (h - 1 - mid).max(1) as u32,
                    )
                }
            }
        };
        device.fill_rect(
            rect.x + inset,
            rect.y + row,
            (w - 2 * inset).max(0) as u32,
            1,
            color,
        );
    }

    // Border: per-row edge pixels at the corner-rounded boundary, plus full
    // horizontal runs on the top/bottom rows.
    for row in 0..h {
        let inset = aero_corner_inset(row.min(h - 1 - row));
        if row == 0 || row == h - 1 {
            device.fill_rect(
                rect.x + inset,
                rect.y + row,
                (w - 2 * inset).max(0) as u32,
                1,
                border,
            );
        } else {
            device.fill_rect(rect.x + inset, rect.y + row, 1, 1, border);
            device.fill_rect(rect.x + w - 1 - inset, rect.y + row, 1, 1, border);
            // Diagonal step pixels keep the rounded corner contiguous.
            let neighbor = aero_corner_inset((row - 1).min(h - 2 - row));
            if neighbor > inset {
                device.fill_rect(rect.x + neighbor, rect.y + row, 1, 1, border);
                device.fill_rect(rect.x + w - 1 - neighbor, rect.y + row, 1, 1, border);
            }
        }
    }

    // Inner ring: white highlight (normal), light-blue glow (hot), or inner
    // shadow (pressed). Disabled buttons stay flat.
    let inner = match state {
        ControlState::Normal => Some(AERO_INNER_HIGHLIGHT),
        ControlState::Hot => Some(AERO_GLOW),
        ControlState::Pressed => Some(AERO_INNER_SHADOW),
        ControlState::Disabled => None,
    };
    if let Some(inner) = inner {
        let inner_rect = Rect::new(
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        let ih = inner_rect.height as i32;
        let iw = inner_rect.width as i32;
        for row in 0..ih {
            let inset = aero_corner_inset(row.min(ih - 1 - row)).min(1);
            if row == 0 || row == ih - 1 {
                device.fill_rect(
                    inner_rect.x + inset,
                    inner_rect.y + row,
                    (iw - 2 * inset).max(0) as u32,
                    1,
                    inner,
                );
            } else {
                device.fill_rect(inner_rect.x, inner_rect.y + row, 1, 1, inner);
                device.fill_rect(inner_rect.x + iw - 1, inner_rect.y + row, 1, 1, inner);
            }
        }
    }
    let _ = AERO_RADIUS; // documented radius; the inset table quantizes it
}

/// Paint a data well (text field) with focus feedback. Classic keeps its
/// sunken bevel regardless of focus (the caret carries focus); Aero swaps
/// the border for the blue focus ring.
pub fn draw_field(device: &mut dyn GraphicsDevice, rect: Rect, focused: bool) {
    device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette().field_bg);
    draw_field_border_focused(device, rect, focused);
}

fn draw_field_border_focused(device: &mut dyn GraphicsDevice, rect: Rect, focused: bool) {
    match theme::active() {
        ThemeKind::Classic => {
            draw_bevel_rings(
                device,
                rect,
                &[
                    (classic::BEVEL_SHADOW, classic::BEVEL_HIGHLIGHT),
                    (classic::BEVEL_DARK, classic::BEVEL_LIGHT),
                ],
            );
        }
        ThemeKind::Aero => {
            outline(
                device,
                rect,
                if focused {
                    AERO_BORDER_HOT
                } else {
                    AERO_FIELD_BORDER
                },
            );
        }
    }
}

/// Paint a raised panel (taskbar, toolbar, status bar): Classic gets a
/// ButtonFace fill with a raised top edge; Aero a soft vertical gradient with
/// a 1px edge.
pub fn draw_raised_panel(device: &mut dyn GraphicsDevice, rect: Rect) {
    match theme::active() {
        ThemeKind::Classic => {
            device.fill_rect(rect.x, rect.y, rect.width, rect.height, classic::FACE);
            device.fill_rect(rect.x, rect.y, rect.width, 1, classic::BEVEL_HIGHLIGHT);
        }
        ThemeKind::Aero => {
            let h = rect.height as i32;
            let span = (h - 1).max(1) as u32;
            for row in 0..h {
                device.fill_rect(
                    rect.x,
                    rect.y + row,
                    rect.width,
                    1,
                    lerp_color(AERO_PANEL_TOP, AERO_PANEL_BOTTOM, row as u32, span),
                );
            }
            device.fill_rect(rect.x, rect.y, rect.width, 1, AERO_PANEL_EDGE);
        }
    }
}

/// Paint a popup-menu surface: themed background plus popup border (Classic:
/// raised two-ring bevel; Aero: flat 1px border).
pub fn draw_menu_surface(device: &mut dyn GraphicsDevice, rect: Rect) {
    let palette = palette();
    device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette.content_bg);
    match theme::active() {
        ThemeKind::Classic => {
            draw_bevel_rings(
                device,
                rect,
                &[
                    (classic::BEVEL_HIGHLIGHT, classic::BEVEL_DARK),
                    (classic::BEVEL_LIGHT, classic::BEVEL_SHADOW),
                ],
            );
        }
        ThemeKind::Aero => {
            outline(device, rect, Color::new(151, 151, 151)); // #979797
        }
    }
}

/// Paint a selection / hover highlight band.
pub fn draw_selection(device: &mut dyn GraphicsDevice, rect: Rect) {
    let palette = palette();
    device.fill_rect(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        palette.selection_bg,
    );
    if theme::active() == ThemeKind::Aero {
        // Aero selections carry a slightly saturated border.
        outline(device, rect, Color::new(38, 160, 218)); // #26A0DA
    }
}

#[cfg(feature = "test")]
pub mod tests {
    use super::*;

    fn test_palette_dispatch_per_theme() {
        assert_eq!(
            palette_for(ThemeKind::Classic).selection_bg,
            Color::new(0, 0, 128)
        );
        assert_eq!(
            palette_for(ThemeKind::Aero).selection_bg,
            Color::new(203, 232, 246)
        );
        assert_eq!(palette_for(ThemeKind::Classic).content_bg, classic::FACE);
    }

    fn test_aero_corner_inset_table() {
        assert_eq!(aero_corner_inset(0), 2);
        assert_eq!(aero_corner_inset(1), 1);
        assert_eq!(aero_corner_inset(2), 0);
        assert_eq!(aero_corner_inset(100), 0);
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_palette_dispatch_per_theme,
            &test_aero_corner_inset_table,
        ]
    }
}
