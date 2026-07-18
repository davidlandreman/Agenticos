//! Theme-aware control surfaces (buttons, fields, panels, selection).
//!
//! Widgets own layout, labels, and hit-testing; they delegate *surface
//! rendering* to these helpers so every control follows the active
//! Classic / Aero / Futurism theme. Painters dispatch on the theme's
//! [`ControlStyle`] — the surface-construction *finish* plus style flags —
//! rather than on theme identity, so a new theme only adds a palette and a
//! style (and a new painter only when it introduces a new finish).
//!
//! Normative Classic/Aero color values live in
//! `docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md`; Futurism's
//! in `docs/plans/2026-07-18-007-feat-futurism-theme-plan.md`. The ring-3
//! toolkit (`userland/libs/gui`) mirrors the same tables.

use crate::graphics::color::Color;
use crate::window::theme::{self, classic::colors as classic, lerp_color, ThemeKind};
use crate::window::{GraphicsDevice, Rect};

/// Visual state of a push-button-like control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlState {
    Normal,
    /// Default / accent button (Aero: blue border + glow; Classic: black rim;
    /// Futurism: accent border + tinted fill).
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

/// Surface-construction family used by the drawing helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFinish {
    /// Win98 raised/sunken bevels (Classic).
    Bevel98,
    /// Win7 rounded gradient glass (Aero).
    GlassKd4,
    /// Flat rounded surfaces with hairline borders (Futurism).
    SoftRounded,
}

/// Per-theme control styling beyond raw colors. Helpers branch on these
/// properties instead of on the theme's identity.
pub struct ControlStyle {
    pub finish: ControlFinish,
    /// Pressed controls shift their label down-right by 1px (Classic).
    pub pressed_label_shift: bool,
    /// Separators draw a second highlight line below the divider (Classic).
    pub separator_highlight: bool,
    /// Border drawn around selection highlights.
    pub selection_border: Option<Color>,
    /// Selection fills get quantized rounded corners.
    pub rounded_selection: bool,
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

const FUTURISM_PALETTE: ControlPalette = ControlPalette {
    content_bg: Color::new(247, 249, 252),    // #F7F9FC
    text: Color::new(31, 41, 55),             // #1F2937 slate
    disabled_text: Color::new(148, 163, 184), // #94A3B8
    border: FUT_BORDER,
    field_bg: Color::WHITE,
    field_text: Color::new(31, 41, 55),
    selection_bg: Color::new(220, 233, 252), // #DCE9FC
    selection_text: Color::new(29, 78, 216), // #1D4ED8
    progress_fill: FUT_ACCENT,
    scrollbar_thumb: Color::new(195, 206, 223), // #C3CEDF
    scrollbar_track: Color::new(238, 242, 248), // #EEF2F8
};

const CLASSIC_STYLE: ControlStyle = ControlStyle {
    finish: ControlFinish::Bevel98,
    pressed_label_shift: true,
    separator_highlight: true,
    selection_border: None,
    rounded_selection: false,
};

const AERO_STYLE: ControlStyle = ControlStyle {
    finish: ControlFinish::GlassKd4,
    pressed_label_shift: false,
    separator_highlight: false,
    selection_border: Some(Color::new(38, 160, 218)), // #26A0DA
    rounded_selection: false,
};

const FUTURISM_STYLE: ControlStyle = ControlStyle {
    finish: ControlFinish::SoftRounded,
    pressed_label_shift: false,
    separator_highlight: false,
    selection_border: Some(FUT_SELECTION_BORDER),
    rounded_selection: true,
};

/// The palette for the active theme.
pub fn palette() -> &'static ControlPalette {
    palette_for(theme::active())
}

pub const fn palette_for(kind: ThemeKind) -> &'static ControlPalette {
    match kind {
        ThemeKind::Classic => &CLASSIC_PALETTE,
        ThemeKind::Aero => &AERO_PALETTE,
        ThemeKind::Futurism => &FUTURISM_PALETTE,
    }
}

/// The control style for the active theme.
pub fn style() -> &'static ControlStyle {
    style_for(theme::active())
}

pub const fn style_for(kind: ThemeKind) -> &'static ControlStyle {
    match kind {
        ThemeKind::Classic => &CLASSIC_STYLE,
        ThemeKind::Aero => &AERO_STYLE,
        ThemeKind::Futurism => &FUTURISM_STYLE,
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

// ---------------------------------------------------------------------
// Futurism constants (KD in the Futurism plan; from the reference mock)
// ---------------------------------------------------------------------

const FUT_ACCENT: Color = Color::new(60, 140, 240); // #3C8CF0
const FUT_BORDER: Color = Color::new(211, 219, 232); // #D3DBE8
const FUT_BORDER_HOT: Color = Color::new(158, 195, 245); // #9EC3F5
const FUT_BORDER_PRESSED: Color = Color::new(127, 169, 232); // #7FA9E8
const FUT_BORDER_DISABLED: Color = Color::new(225, 231, 240); // #E1E7F0
const FUT_FILL_NORMAL: Color = Color::WHITE;
const FUT_FILL_HOT: Color = Color::new(243, 247, 254); // #F3F7FE
const FUT_FILL_PRESSED: Color = Color::new(228, 236, 248); // #E4ECF8
const FUT_FILL_DISABLED: Color = Color::new(241, 244, 249); // #F1F4F9
const FUT_PANEL: Color = Color::new(238, 242, 249); // #EEF2F9
const FUT_PANEL_EDGE: Color = Color::new(217, 225, 236); // #D9E1EC
const FUT_MENU_BORDER: Color = Color::new(201, 210, 228); // #C9D2E4
const FUT_SELECTION_BORDER: Color = Color::new(143, 183, 242); // #8FB7F2
/// Frosted taskbar tint (over the backdrop blur).
const FUT_TASKBAR_TINT: Color = Color::new(26, 36, 64); // #1A2440
const FUT_TASKBAR_TINT_ALPHA: u8 = 150;

/// Corner inset (pixels skipped from each side) for a given distance from the
/// top/bottom edge, quantizing a radius-3 rounded corner.
const fn aero_corner_inset(edge_distance: i32) -> i32 {
    match edge_distance {
        0 => 2,
        1 => 1,
        _ => 0,
    }
}

/// Quantized radius-5 rounding used by Futurism's larger corner radii.
const fn soft_corner_inset(edge_distance: i32) -> i32 {
    match edge_distance {
        0 => 3,
        1 => 2,
        2 => 1,
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
    if style().pressed_label_shift && state == ControlState::Pressed {
        1
    } else {
        0
    }
}

/// Paint a push-button surface (face + edges, no label) into `rect`.
pub fn draw_button(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    match style().finish {
        ControlFinish::Bevel98 => draw_classic_button(device, rect, state),
        ControlFinish::GlassKd4 => draw_aero_button(device, rect, state),
        ControlFinish::SoftRounded => draw_futurism_button(device, rect, state),
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

/// Fill `rect` with rounded corners quantized by `inset`.
fn fill_rounded_rect(
    device: &mut dyn GraphicsDevice,
    rect: Rect,
    color: Color,
    inset: fn(i32) -> i32,
) {
    let h = rect.height as i32;
    let w = rect.width as i32;
    for row in 0..h {
        let inset = inset(row.min(h - 1 - row));
        device.fill_rect(
            rect.x + inset,
            rect.y + row,
            (w - 2 * inset).max(0) as u32,
            1,
            color,
        );
    }
}

/// 1px border following the rounded boundary quantized by `inset`, including
/// the diagonal step pixels that keep the corner contiguous.
fn draw_rounded_outline(
    device: &mut dyn GraphicsDevice,
    rect: Rect,
    color: Color,
    inset: fn(i32) -> i32,
) {
    let h = rect.height as i32;
    let w = rect.width as i32;
    for row in 0..h {
        let row_inset = inset(row.min(h - 1 - row));
        if row == 0 || row == h - 1 {
            device.fill_rect(
                rect.x + row_inset,
                rect.y + row,
                (w - 2 * row_inset).max(0) as u32,
                1,
                color,
            );
        } else {
            device.fill_rect(rect.x + row_inset, rect.y + row, 1, 1, color);
            device.fill_rect(rect.x + w - 1 - row_inset, rect.y + row, 1, 1, color);
            let neighbor = inset((row - 1).min(h - 2 - row));
            if neighbor > row_inset {
                device.fill_rect(rect.x + neighbor, rect.y + row, 1, 1, color);
                device.fill_rect(rect.x + w - 1 - neighbor, rect.y + row, 1, 1, color);
            }
        }
    }
}

/// Futurism flat rounded button: state-colored fill, hairline border, no
/// gradient and no pressed label shift.
fn draw_futurism_button(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    let (fill, border) = match state {
        ControlState::Normal => (FUT_FILL_NORMAL, FUT_BORDER),
        ControlState::Hot => (FUT_FILL_HOT, FUT_BORDER_HOT),
        ControlState::Pressed => (FUT_FILL_PRESSED, FUT_BORDER_PRESSED),
        ControlState::Disabled => (FUT_FILL_DISABLED, FUT_BORDER_DISABLED),
    };
    let h = rect.height as i32;
    let w = rect.width as i32;
    if h < 8 || w < 8 {
        // Too small for rounding; flat fallback.
        device.fill_rect(rect.x, rect.y, rect.width, rect.height, fill);
        outline(device, rect, border);
        return;
    }
    fill_rounded_rect(device, rect, fill, soft_corner_inset);
    draw_rounded_outline(device, rect, border, soft_corner_inset);
    // Accent focus ring: a second inner border for the default button.
    if state == ControlState::Hot {
        let inner = Rect::new(
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        );
        draw_rounded_outline(device, inner, FUT_ACCENT, aero_corner_inset);
    }
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
    draw_rounded_outline(device, rect, border, aero_corner_inset);

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
/// sunken bevel regardless of focus (the caret carries focus); Aero and
/// Futurism swap the border for the accent focus ring.
pub fn draw_field(device: &mut dyn GraphicsDevice, rect: Rect, focused: bool) {
    device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette().field_bg);
    draw_field_border_focused(device, rect, focused);
}

fn draw_field_border_focused(device: &mut dyn GraphicsDevice, rect: Rect, focused: bool) {
    match style().finish {
        ControlFinish::Bevel98 => {
            draw_bevel_rings(
                device,
                rect,
                &[
                    (classic::BEVEL_SHADOW, classic::BEVEL_HIGHLIGHT),
                    (classic::BEVEL_DARK, classic::BEVEL_LIGHT),
                ],
            );
        }
        ControlFinish::GlassKd4 => {
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
        ControlFinish::SoftRounded => {
            outline(device, rect, if focused { FUT_ACCENT } else { FUT_BORDER });
        }
    }
}

/// Paint a raised panel (toolbar, status bar): Classic gets a ButtonFace
/// fill with a raised top edge; Aero a soft vertical gradient with a 1px
/// edge; Futurism a flat light strip with a hairline edge.
pub fn draw_raised_panel(device: &mut dyn GraphicsDevice, rect: Rect) {
    match style().finish {
        ControlFinish::Bevel98 => {
            device.fill_rect(rect.x, rect.y, rect.width, rect.height, classic::FACE);
            device.fill_rect(rect.x, rect.y, rect.width, 1, classic::BEVEL_HIGHLIGHT);
        }
        ControlFinish::GlassKd4 => {
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
        ControlFinish::SoftRounded => {
            device.fill_rect(rect.x, rect.y, rect.width, rect.height, FUT_PANEL);
            device.fill_rect(rect.x, rect.y, rect.width, 1, FUT_PANEL_EDGE);
        }
    }
}

/// Paint a recessed panel such as a status well. Classic keeps the Win98
/// sunken edge; Aero and Futurism use flat borders.
pub fn draw_recessed_panel(device: &mut dyn GraphicsDevice, rect: Rect) {
    let palette = palette();
    device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette.content_bg);
    match style().finish {
        ControlFinish::Bevel98 => {
            draw_bevel_rings(
                device,
                rect,
                &[(classic::BEVEL_SHADOW, classic::BEVEL_HIGHLIGHT)],
            );
        }
        ControlFinish::GlassKd4 | ControlFinish::SoftRounded => {
            outline(device, rect, palette.border)
        }
    }
}

/// Paint a popup-menu surface: themed background plus popup border (Classic:
/// raised two-ring bevel; Aero: flat 1px border; Futurism: frosted
/// translucent white over the chrome backdrop blur).
pub fn draw_menu_surface(device: &mut dyn GraphicsDevice, rect: Rect) {
    let palette = palette();
    match style().finish {
        ControlFinish::Bevel98 => {
            device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette.content_bg);
            draw_bevel_rings(
                device,
                rect,
                &[
                    (classic::BEVEL_HIGHLIGHT, classic::BEVEL_DARK),
                    (classic::BEVEL_LIGHT, classic::BEVEL_SHADOW),
                ],
            );
        }
        ControlFinish::GlassKd4 => {
            device.fill_rect(rect.x, rect.y, rect.width, rect.height, palette.content_bg);
            outline(device, rect, Color::new(151, 151, 151)); // #979797
        }
        ControlFinish::SoftRounded => {
            device.fill_rect_argb(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                Color::new(250, 251, 254),
                242,
            );
            outline(device, rect, FUT_MENU_BORDER);
        }
    }
}

/// Paint a menu separator using the active theme's divider treatment.
pub fn draw_menu_separator(device: &mut dyn GraphicsDevice, x: i32, y: i32, width: u32) {
    let palette = palette();
    device.fill_rect(x, y, width, 1, palette.border);
    if style().separator_highlight {
        device.fill_rect(x, y + 1, width, 1, classic::BEVEL_HIGHLIGHT);
    }
}

/// Paint a selection / hover highlight band.
pub fn draw_selection(device: &mut dyn GraphicsDevice, rect: Rect) {
    let palette = palette();
    let style = style();
    if style.rounded_selection && rect.width >= 8 && rect.height >= 8 {
        fill_rounded_rect(device, rect, palette.selection_bg, aero_corner_inset);
        if let Some(border) = style.selection_border {
            draw_rounded_outline(device, rect, border, aero_corner_inset);
        }
        return;
    }
    device.fill_rect(
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        palette.selection_bg,
    );
    if let Some(border) = style.selection_border {
        outline(device, rect, border);
    }
}

// ---------------------------------------------------------------------
// Desktop chrome (taskbar strip, tray well, task buttons)
// ---------------------------------------------------------------------

/// Paint the taskbar strip. Classic/Aero delegate to the raised panel;
/// Futurism paints a frosted translucent bar over the chrome backdrop blur.
pub fn draw_taskbar_surface(device: &mut dyn GraphicsDevice, rect: Rect) {
    match style().finish {
        ControlFinish::SoftRounded => {
            device.fill_rect_argb(
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                FUT_TASKBAR_TINT,
                FUT_TASKBAR_TINT_ALPHA,
            );
            device.fill_rect_argb(rect.x, rect.y, rect.width, 1, Color::WHITE, 56);
        }
        _ => draw_raised_panel(device, rect),
    }
}

/// Paint the tray notification well. Classic/Aero delegate to the recessed
/// panel; Futurism uses a translucent rounded-feeling well on the frosted bar.
pub fn draw_tray_well(device: &mut dyn GraphicsDevice, rect: Rect) {
    match style().finish {
        ControlFinish::SoftRounded => {
            device.fill_rect_argb(rect.x, rect.y, rect.width, rect.height, Color::WHITE, 30);
            argb_outline(device, rect, Color::WHITE, 56);
        }
        _ => draw_recessed_panel(device, rect),
    }
}

/// Text color for taskbar-hosted chrome (tray clock, task-button labels).
pub fn taskbar_text() -> Color {
    match style().finish {
        ControlFinish::SoftRounded => Color::WHITE,
        _ => palette().text,
    }
}

/// Paint a taskbar-hosted button (Start, task buttons). Classic/Aero use the
/// ordinary button surface; Futurism paints translucent rounded pills whose
/// fill tracks state (`accent` marks the gradient Start pill).
pub fn draw_task_button(
    device: &mut dyn GraphicsDevice,
    rect: Rect,
    state: ControlState,
    accent: bool,
) {
    if style().finish != ControlFinish::SoftRounded {
        draw_button(device, rect, state);
        return;
    }
    if accent {
        draw_futurism_start_pill(device, rect, state);
        return;
    }
    let (tint, fill_alpha, border_alpha) = match state {
        ControlState::Normal => (Color::WHITE, 36u8, 72u8),
        ControlState::Hot => (FUT_ACCENT, 150, 170),
        ControlState::Pressed => (Color::WHITE, 88, 110),
        ControlState::Disabled => (Color::WHITE, 18, 36),
    };
    let h = rect.height as i32;
    let w = rect.width as i32;
    if h < 8 || w < 8 {
        device.fill_rect_argb(rect.x, rect.y, rect.width, rect.height, tint, fill_alpha);
        argb_outline(device, rect, Color::WHITE, border_alpha);
        return;
    }
    for row in 0..h {
        let inset = soft_corner_inset(row.min(h - 1 - row));
        device.fill_rect_argb(
            rect.x + inset,
            rect.y + row,
            (w - 2 * inset).max(0) as u32,
            1,
            tint,
            fill_alpha,
        );
    }
    argb_rounded_outline(device, rect, Color::WHITE, border_alpha, soft_corner_inset);
}

/// Futurism Start pill: soft blue vertical gradient, light border, inner
/// highlight ring — the accent anchor of the frosted taskbar.
fn draw_futurism_start_pill(device: &mut dyn GraphicsDevice, rect: Rect, state: ControlState) {
    let (top, bottom) = match state {
        ControlState::Pressed => (Color::new(88, 124, 200), Color::new(54, 86, 168)),
        _ => (Color::new(124, 158, 232), Color::new(74, 112, 200)),
    };
    let h = rect.height as i32;
    let w = rect.width as i32;
    if h < 8 || w < 8 {
        device.fill_rect_argb(rect.x, rect.y, rect.width, rect.height, bottom, 235);
        argb_outline(device, rect, Color::new(214, 226, 250), 210);
        return;
    }
    let span = (h - 1).max(1) as u32;
    for row in 0..h {
        let inset = soft_corner_inset(row.min(h - 1 - row));
        device.fill_rect_argb(
            rect.x + inset,
            rect.y + row,
            (w - 2 * inset).max(0) as u32,
            1,
            lerp_color(top, bottom, row as u32, span),
            235,
        );
    }
    argb_rounded_outline(
        device,
        rect,
        Color::new(214, 226, 250),
        210,
        soft_corner_inset,
    );
    // Inner highlight ring for the glassy raised look.
    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    argb_rounded_outline(device, inner, Color::WHITE, 64, aero_corner_inset);
}

/// Label color for a taskbar-hosted button in `state`.
pub fn task_button_text(state: ControlState, _accent: bool) -> Color {
    match style().finish {
        ControlFinish::SoftRounded => match state {
            ControlState::Disabled => Color::new(170, 178, 198),
            _ => Color::WHITE,
        },
        _ => button_text(state),
    }
}

fn argb_outline(device: &mut dyn GraphicsDevice, rect: Rect, color: Color, alpha: u8) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    device.fill_rect_argb(rect.x, rect.y, rect.width, 1, color, alpha);
    device.fill_rect_argb(
        rect.x,
        rect.y + rect.height as i32 - 1,
        rect.width,
        1,
        color,
        alpha,
    );
    device.fill_rect_argb(rect.x, rect.y, 1, rect.height, color, alpha);
    device.fill_rect_argb(
        rect.x + rect.width as i32 - 1,
        rect.y,
        1,
        rect.height,
        color,
        alpha,
    );
}

fn argb_rounded_outline(
    device: &mut dyn GraphicsDevice,
    rect: Rect,
    color: Color,
    alpha: u8,
    inset: fn(i32) -> i32,
) {
    let h = rect.height as i32;
    let w = rect.width as i32;
    for row in 0..h {
        let row_inset = inset(row.min(h - 1 - row));
        if row == 0 || row == h - 1 {
            device.fill_rect_argb(
                rect.x + row_inset,
                rect.y + row,
                (w - 2 * row_inset).max(0) as u32,
                1,
                color,
                alpha,
            );
        } else {
            device.fill_rect_argb(rect.x + row_inset, rect.y + row, 1, 1, color, alpha);
            device.fill_rect_argb(rect.x + w - 1 - row_inset, rect.y + row, 1, 1, color, alpha);
            let neighbor = inset((row - 1).min(h - 2 - row));
            if neighbor > row_inset {
                device.fill_rect_argb(rect.x + neighbor, rect.y + row, 1, 1, color, alpha);
                device.fill_rect_argb(rect.x + w - 1 - neighbor, rect.y + row, 1, 1, color, alpha);
            }
        }
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
        assert_eq!(
            palette_for(ThemeKind::Futurism).selection_bg,
            Color::new(220, 233, 252)
        );
        assert_eq!(palette_for(ThemeKind::Classic).content_bg, classic::FACE);
        assert_eq!(
            palette_for(ThemeKind::Futurism).progress_fill,
            Color::new(60, 140, 240)
        );
    }

    fn test_aero_corner_inset_table() {
        assert_eq!(aero_corner_inset(0), 2);
        assert_eq!(aero_corner_inset(1), 1);
        assert_eq!(aero_corner_inset(2), 0);
        assert_eq!(aero_corner_inset(100), 0);
    }

    fn test_soft_corner_inset_table() {
        assert_eq!(soft_corner_inset(0), 3);
        assert_eq!(soft_corner_inset(1), 2);
        assert_eq!(soft_corner_inset(2), 1);
        assert_eq!(soft_corner_inset(3), 0);
        assert_eq!(soft_corner_inset(100), 0);
    }

    fn test_style_dispatch_per_theme() {
        assert_eq!(style_for(ThemeKind::Classic).finish, ControlFinish::Bevel98);
        assert!(style_for(ThemeKind::Classic).pressed_label_shift);
        assert_eq!(style_for(ThemeKind::Aero).finish, ControlFinish::GlassKd4);
        assert_eq!(
            style_for(ThemeKind::Futurism).finish,
            ControlFinish::SoftRounded
        );
        assert!(style_for(ThemeKind::Futurism).rounded_selection);
        assert!(!style_for(ThemeKind::Aero).rounded_selection);
    }

    pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
        &[
            &test_palette_dispatch_per_theme,
            &test_aero_corner_inset_table,
            &test_soft_corner_inset_table,
            &test_style_dispatch_per_theme,
        ]
    }
}
