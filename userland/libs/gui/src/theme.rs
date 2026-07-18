//! Ring-3 mirror of the kernel's control theme.
//!
//! The kernel publishes the resolved theme as `/etc/theme` (`classic`,
//! `aero`, or `futurism`); this module reads it once, caches it, and exposes
//! the same control palette + surface helpers the kernel's
//! `window::theme::controls` uses, adapted to the toolkit's opaque XRGB
//! [`Canvas`]. Painters dispatch on the theme's [`Finish`] (its surface
//! construction family) rather than on identity, mirroring the kernel's
//! `ControlStyle`. Classic/Aero color values are normative in
//! `docs/plans/2026-07-18-003-feat-theme-aware-controls-plan.md`; Futurism's
//! in `docs/plans/2026-07-18-007-feat-futurism-theme-plan.md`.
//!
//! Missing or malformed `/etc/theme` degrades to Classic — apps never fail
//! to start because of theming, and an old binary meeting an unknown future
//! theme token falls back safely.

use core::sync::atomic::{AtomicU8, Ordering};

use crate::Canvas;

/// The active system theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Classic,
    Aero,
    Futurism,
}

impl Theme {
    /// Whether this is one of the modern (retained-compositor) themes.
    pub fn is_modern(self) -> bool {
        !matches!(self, Theme::Classic)
    }

    fn code(self) -> u8 {
        match self {
            Theme::Classic => 1,
            Theme::Aero => 2,
            Theme::Futurism => 3,
        }
    }

    fn from_code(code: u8) -> Option<Theme> {
        match code {
            1 => Some(Theme::Classic),
            2 => Some(Theme::Aero),
            3 => Some(Theme::Futurism),
            _ => None,
        }
    }
}

/// Surface-construction family used by the drawing helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// Win98 raised/sunken bevels (Classic).
    Bevel98,
    /// Win7 rounded gradient glass (Aero).
    GlassKd4,
    /// Flat rounded surfaces with hairline borders (Futurism).
    SoftRounded,
}

/// Visual state of a push-button-like control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    /// Default / accent button (Aero: blue border + glow; Classic: black
    /// rim; Futurism: accent border + tinted fill).
    Hot,
    Pressed,
    Disabled,
}

/// Theme-scoped colors widgets read directly (XRGB8888 `0x00RRGGBB`).
pub struct Palette {
    /// Window / panel content background.
    pub content_bg: u32,
    /// Text on `content_bg` and button faces.
    pub text: u32,
    /// Greyed-out label text.
    pub disabled_text: u32,
    /// Generic border / divider color.
    pub border: u32,
    /// Interior of text fields and lists.
    pub field_bg: u32,
    /// Text drawn on `field_bg`.
    pub field_text: u32,
    /// Selection / hover highlight fill.
    pub selection_bg: u32,
    /// Text on `selection_bg`.
    pub selection_text: u32,
    pub scrollbar_track: u32,
    pub scrollbar_thumb: u32,
    pub scrollbar_hot: u32,
    pub scrollbar_pressed: u32,
}

const CLASSIC_PALETTE: Palette = Palette {
    content_bg: 0xC0C0C0,
    text: 0x000000,
    disabled_text: 0x808080,
    border: 0x808080,
    field_bg: 0xFFFFFF,
    field_text: 0x000000,
    selection_bg: 0x000080,
    selection_text: 0xFFFFFF,
    scrollbar_track: 0xDFDFDF,
    scrollbar_thumb: 0xC0C0C0,
    scrollbar_hot: 0xD4D0C8,
    scrollbar_pressed: 0xA0A0A0,
};

const AERO_PALETTE: Palette = Palette {
    content_bg: 0xF0F0F0,
    text: 0x000000,
    disabled_text: 0x838383,
    border: 0x707070,
    field_bg: 0xFFFFFF,
    field_text: 0x000000,
    selection_bg: 0xCBE8F6,
    selection_text: 0x000000,
    scrollbar_track: 0xF0F0F0,
    scrollbar_thumb: 0xCDCDCD,
    scrollbar_hot: 0xA9D4F0,
    scrollbar_pressed: 0x7FB6D8,
};

const FUTURISM_PALETTE: Palette = Palette {
    content_bg: 0xF7F9FC,
    text: 0x1F2937,
    disabled_text: 0x94A3B8,
    border: 0xD3DBE8,
    field_bg: 0xFFFFFF,
    field_text: 0x1F2937,
    selection_bg: 0xDCE9FC,
    selection_text: 0x1D4ED8,
    scrollbar_track: 0xEEF2F8,
    scrollbar_thumb: 0xC3CEDF,
    scrollbar_hot: 0x9EC3F5,
    scrollbar_pressed: 0x7FA9E8,
};

// Classic (Win98) bevel constants, shared with the kernel classic theme.
const BEVEL_HIGHLIGHT: u32 = 0xFFFFFF;
const BEVEL_LIGHT: u32 = 0xDFDFDF;
const BEVEL_SHADOW: u32 = 0x808080;
const BEVEL_DARK: u32 = 0x000000;
const CLASSIC_FACE: u32 = 0xC0C0C0;
const CLASSIC_FACE_DISABLED: u32 = 0xD4D0C8;

// Aero constants (KD4 in the plan; from the reference screenshot + Win7).
const AERO_BORDER_NORMAL: u32 = 0x707070;
const AERO_BORDER_HOT: u32 = 0x3C7FB1;
const AERO_BORDER_PRESSED: u32 = 0x2C628B;
const AERO_BORDER_DISABLED: u32 = 0xADB2B5;
const AERO_GLOW: u32 = 0xA9D4F0;
const AERO_INNER_HIGHLIGHT: u32 = 0xFCFCFC;
const AERO_INNER_SHADOW: u32 = 0x9DB6C8;
const AERO_FILL_NORMAL: [u32; 4] = [0xF2F2F2, 0xEBEBEB, 0xDDDDDD, 0xCFCFCF];
const AERO_FILL_HOT: [u32; 4] = [0xEAF6FD, 0xD9F0FC, 0xBEE6FD, 0xA7D9F5];
const AERO_FILL_PRESSED: [u32; 4] = [0xE5F4FC, 0xC4E5F6, 0x98D1EF, 0x68B3DB];
const AERO_FILL_DISABLED: u32 = 0xF4F4F4;
const AERO_FIELD_BORDER: u32 = 0xABABAB;
const AERO_MENU_BORDER: u32 = 0x979797;
const AERO_SELECTION_BORDER: u32 = 0x26A0DA;

// Futurism constants (from the reference mock).
const FUT_ACCENT: u32 = 0x3C8CF0;
const FUT_BORDER: u32 = 0xD3DBE8;
const FUT_BORDER_HOT: u32 = 0x9EC3F5;
const FUT_BORDER_PRESSED: u32 = 0x7FA9E8;
const FUT_BORDER_DISABLED: u32 = 0xE1E7F0;
const FUT_FILL_NORMAL: u32 = 0xFFFFFF;
const FUT_FILL_HOT: u32 = 0xF3F7FE;
const FUT_FILL_PRESSED: u32 = 0xE4ECF8;
const FUT_FILL_DISABLED: u32 = 0xF1F4F9;
const FUT_MENU_SURFACE: u32 = 0xFAFBFE;
const FUT_MENU_BORDER: u32 = 0xC9D2E4;
const FUT_SELECTION_BORDER: u32 = 0x8FB7F2;

/// 0 = not yet loaded; else `Theme::code()`.
static THEME: AtomicU8 = AtomicU8::new(0);

/// The active theme, loaded from `/etc/theme` on first use.
pub fn current() -> Theme {
    match Theme::from_code(THEME.load(Ordering::Acquire)) {
        Some(theme) => theme,
        None => {
            let theme = load();
            THEME.store(theme.code(), Ordering::Release);
            theme
        }
    }
}

/// The active theme's surface-construction family.
pub fn finish() -> Finish {
    match current() {
        Theme::Classic => Finish::Bevel98,
        Theme::Aero => Finish::GlassKd4,
        Theme::Futurism => Finish::SoftRounded,
    }
}

/// Force a specific theme (test / preview hook; normal apps never call this).
pub fn set(theme: Theme) {
    THEME.store(theme.code(), Ordering::Release);
}

/// Apply a process-global theme notification before an app handles the event.
pub fn apply_system_event(event: &runtime::GuiEvent) -> bool {
    if event.kind != runtime::GUI_EVENT_THEME_CHANGED {
        return false;
    }
    let code = u8::try_from(event.payload[0]).ok();
    set(code.and_then(Theme::from_code).unwrap_or(Theme::Classic));
    true
}

pub fn palette_for(theme: Theme) -> &'static Palette {
    match theme {
        Theme::Classic => &CLASSIC_PALETTE,
        Theme::Aero => &AERO_PALETTE,
        Theme::Futurism => &FUTURISM_PALETTE,
    }
}

fn load() -> Theme {
    let path = b"/etc/theme\0";
    let fd = runtime::openat(runtime::AT_FDCWD, path, runtime::O_RDONLY, 0);
    if fd < 0 {
        return Theme::Classic;
    }
    let fd = fd as i32;
    let mut buffer = [0u8; 16];
    let count = runtime::read(fd, &mut buffer);
    let _ = runtime::close(fd);
    if count <= 0 {
        return Theme::Classic;
    }
    let contents = &buffer[..count as usize];
    let token = contents
        .split(|&byte| byte == b'\n' || byte == b'\r' || byte == b' ' || byte == 0)
        .next()
        .unwrap_or(&[]);
    match token {
        b"futurism" => Theme::Futurism,
        b"aero" => Theme::Aero,
        // Unknown tokens degrade to Classic so stale binaries stay usable.
        _ => Theme::Classic,
    }
}

/// The palette for the active theme.
pub fn palette() -> &'static Palette {
    palette_for(current())
}

/// Label color for a button in `state`.
pub fn button_text(state: ButtonState) -> u32 {
    match state {
        ButtonState::Disabled => palette().disabled_text,
        _ => palette().text,
    }
}

/// When a pressed control shifts its label down-right by 1px (Classic only).
pub fn pressed_label_shift(state: ButtonState) -> i32 {
    if finish() == Finish::Bevel98 && state == ButtonState::Pressed {
        1
    } else {
        0
    }
}

/// Per-channel linear interpolation between two XRGB colors.
fn lerp(start: u32, end: u32, position: u32, span: u32) -> u32 {
    let span = span.max(1);
    let position = position.min(span);
    let mix = |shift: u32| {
        let a = (start >> shift) & 0xFF;
        let b = (end >> shift) & 0xFF;
        ((a * (span - position) + b * position) / span) & 0xFF
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Corner inset (pixels skipped per side) quantizing the Aero radius-3
/// rounded corner. Pixels outside the inset are left untouched, so the
/// caller's existing background shows through the corners.
fn aero_corner_inset(edge_distance: i32) -> i32 {
    match edge_distance {
        0 => 2,
        1 => 1,
        _ => 0,
    }
}

/// Quantized radius-5 rounding used by Futurism's larger corner radii.
fn soft_corner_inset(edge_distance: i32) -> i32 {
    match edge_distance {
        0 => 3,
        1 => 2,
        2 => 1,
        _ => 0,
    }
}

/// Fill a rect with rounded corners quantized by `inset`.
fn fill_rounded_rect(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: u32,
    inset: fn(i32) -> i32,
) {
    let hi = h as i32;
    let wi = w as i32;
    for row in 0..hi {
        let inset = inset(row.min(hi - 1 - row));
        canvas.fill_rect(x + inset, y + row, (wi - 2 * inset).max(0) as u32, 1, color);
    }
}

/// 1px border following the rounded boundary quantized by `inset`, including
/// the diagonal step pixels that keep the corner contiguous.
fn draw_rounded_outline(
    canvas: &mut Canvas,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: u32,
    inset: fn(i32) -> i32,
) {
    let hi = h as i32;
    let wi = w as i32;
    for row in 0..hi {
        let row_inset = inset(row.min(hi - 1 - row));
        if row == 0 || row == hi - 1 {
            canvas.fill_rect(
                x + row_inset,
                y + row,
                (wi - 2 * row_inset).max(0) as u32,
                1,
                color,
            );
        } else {
            canvas.pixel(x + row_inset, y + row, color);
            canvas.pixel(x + wi - 1 - row_inset, y + row, color);
            let neighbor = inset((row - 1).min(hi - 2 - row));
            if neighbor > row_inset {
                canvas.pixel(x + neighbor, y + row, color);
                canvas.pixel(x + wi - 1 - neighbor, y + row, color);
            }
        }
    }
}

/// Paint a push-button surface (face + edges, no label).
pub fn draw_button(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, state: ButtonState) {
    match finish() {
        Finish::Bevel98 => draw_classic_button(canvas, x, y, w, h, state),
        Finish::GlassKd4 => draw_aero_button(canvas, x, y, w, h, state),
        Finish::SoftRounded => draw_futurism_button(canvas, x, y, w, h, state),
    }
}

fn draw_classic_button(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, state: ButtonState) {
    let face = if state == ButtonState::Disabled {
        CLASSIC_FACE_DISABLED
    } else {
        CLASSIC_FACE
    };
    canvas.fill_rect(x, y, w, h, face);

    let rings: [(u32, u32); 2] = match state {
        ButtonState::Pressed => [(BEVEL_SHADOW, BEVEL_HIGHLIGHT), (BEVEL_DARK, BEVEL_LIGHT)],
        _ => [(BEVEL_HIGHLIGHT, BEVEL_DARK), (BEVEL_LIGHT, BEVEL_SHADOW)],
    };
    draw_bevel_rings(canvas, x, y, w, h, &rings);

    // Default button: extra 1px black rim, kept inside the bounds.
    if state == ButtonState::Hot {
        canvas.rect(x, y, w, h, BEVEL_DARK);
    }
}

/// Two concentric 1px bevel rings; `rings[i] = (top_left, bottom_right)`.
fn draw_bevel_rings(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, rings: &[(u32, u32)]) {
    for (ring, (top_left, bottom_right)) in rings.iter().enumerate() {
        let ring = ring as i32;
        let side_w = w.saturating_sub(2 * ring as u32);
        let side_h = h.saturating_sub(2 * ring as u32);
        canvas.fill_rect(x + ring, y + ring, side_w, 1, *top_left);
        canvas.fill_rect(x + ring, y + ring, 1, side_h, *top_left);
        canvas.fill_rect(x + ring, y + h as i32 - 1 - ring, side_w, 1, *bottom_right);
        canvas.fill_rect(x + w as i32 - 1 - ring, y + ring, 1, side_h, *bottom_right);
    }
}

/// Futurism flat rounded button: state-colored fill, hairline border, no
/// gradient and no pressed label shift.
fn draw_futurism_button(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, state: ButtonState) {
    let (fill, border) = match state {
        ButtonState::Normal => (FUT_FILL_NORMAL, FUT_BORDER),
        ButtonState::Hot => (FUT_FILL_HOT, FUT_BORDER_HOT),
        ButtonState::Pressed => (FUT_FILL_PRESSED, FUT_BORDER_PRESSED),
        ButtonState::Disabled => (FUT_FILL_DISABLED, FUT_BORDER_DISABLED),
    };
    let wi = w as i32;
    let hi = h as i32;
    if wi < 8 || hi < 8 {
        canvas.fill_rect(x, y, w, h, fill);
        canvas.rect(x, y, w, h, border);
        return;
    }
    fill_rounded_rect(canvas, x, y, w, h, fill, soft_corner_inset);
    draw_rounded_outline(canvas, x, y, w, h, border, soft_corner_inset);
    // Accent focus ring: a second inner border for the default button.
    if state == ButtonState::Hot {
        draw_rounded_outline(
            canvas,
            x + 1,
            y + 1,
            w.saturating_sub(2),
            h.saturating_sub(2),
            FUT_ACCENT,
            aero_corner_inset,
        );
    }
}

/// Aero rounded-gradient button, from the reference screenshot: rounded
/// corners, thin border (blue + glow when hot), 1px inner highlight, and a
/// two-band vertical gradient with the hard mid transition.
fn draw_aero_button(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, state: ButtonState) {
    let wi = w as i32;
    let hi = h as i32;
    if wi < 6 || hi < 6 {
        canvas.fill_rect(x, y, w, h, AERO_FILL_NORMAL[1]);
        canvas.rect(x, y, w, h, AERO_BORDER_NORMAL);
        return;
    }

    let border = match state {
        ButtonState::Normal => AERO_BORDER_NORMAL,
        ButtonState::Hot => AERO_BORDER_HOT,
        ButtonState::Pressed => AERO_BORDER_PRESSED,
        ButtonState::Disabled => AERO_BORDER_DISABLED,
    };

    // Fill: two vertical gradient bands with the mid-line transition.
    let mid = hi / 2;
    for row in 0..hi {
        let inset = aero_corner_inset(row.min(hi - 1 - row));
        let color = match state {
            ButtonState::Disabled => AERO_FILL_DISABLED,
            _ => {
                let stops = match state {
                    ButtonState::Hot => &AERO_FILL_HOT,
                    ButtonState::Pressed => &AERO_FILL_PRESSED,
                    _ => &AERO_FILL_NORMAL,
                };
                if row < mid {
                    lerp(stops[0], stops[1], row as u32, (mid - 1).max(1) as u32)
                } else {
                    lerp(
                        stops[2],
                        stops[3],
                        (row - mid) as u32,
                        (hi - 1 - mid).max(1) as u32,
                    )
                }
            }
        };
        canvas.fill_rect(x + inset, y + row, (wi - 2 * inset).max(0) as u32, 1, color);
    }

    // Border along the corner-rounded boundary.
    draw_rounded_outline(canvas, x, y, w, h, border, aero_corner_inset);

    // Inner ring: highlight (normal), glow (hot), or inner shadow (pressed).
    let inner = match state {
        ButtonState::Normal => Some(AERO_INNER_HIGHLIGHT),
        ButtonState::Hot => Some(AERO_GLOW),
        ButtonState::Pressed => Some(AERO_INNER_SHADOW),
        ButtonState::Disabled => None,
    };
    if let Some(inner) = inner {
        let ix = x + 1;
        let iy = y + 1;
        let iw = wi - 2;
        let ih = hi - 2;
        for row in 0..ih {
            let inset = aero_corner_inset(row.min(ih - 1 - row)).min(1);
            if row == 0 || row == ih - 1 {
                canvas.fill_rect(
                    ix + inset,
                    iy + row,
                    (iw - 2 * inset).max(0) as u32,
                    1,
                    inner,
                );
            } else {
                canvas.pixel(ix, iy + row, inner);
                canvas.pixel(ix + iw - 1, iy + row, inner);
            }
        }
    }
}

/// Paint a data well (text field / list) background + border with focus
/// feedback. Classic keeps its sunken bevel regardless of focus; Aero and
/// Futurism swap the border for the accent focus ring.
pub fn draw_field(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, focused: bool) {
    canvas.fill_rect(x, y, w, h, palette().field_bg);
    draw_field_border(canvas, x, y, w, h, focused);
}

/// Border-only variant for wells whose interior the widget paints itself
/// (e.g. a list that fills rows first and borders last).
pub fn draw_field_border(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, focused: bool) {
    match finish() {
        Finish::Bevel98 => {
            draw_bevel_rings(
                canvas,
                x,
                y,
                w,
                h,
                &[(BEVEL_SHADOW, BEVEL_HIGHLIGHT), (BEVEL_DARK, BEVEL_LIGHT)],
            );
        }
        Finish::GlassKd4 => {
            canvas.rect(
                x,
                y,
                w,
                h,
                if focused {
                    AERO_BORDER_HOT
                } else {
                    AERO_FIELD_BORDER
                },
            );
        }
        Finish::SoftRounded => {
            canvas.rect(x, y, w, h, if focused { FUT_ACCENT } else { FUT_BORDER });
        }
    }
}

/// Paint a selection / hover highlight band.
pub fn draw_selection(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
    if finish() == Finish::SoftRounded && w >= 8 && h >= 8 {
        fill_rounded_rect(
            canvas,
            x,
            y,
            w,
            h,
            palette().selection_bg,
            aero_corner_inset,
        );
        draw_rounded_outline(canvas, x, y, w, h, FUT_SELECTION_BORDER, aero_corner_inset);
        return;
    }
    canvas.fill_rect(x, y, w, h, palette().selection_bg);
    if finish() == Finish::GlassKd4 {
        canvas.rect(x, y, w, h, AERO_SELECTION_BORDER);
    }
}

/// Paint a popup-menu surface: themed background plus popup border.
pub fn draw_menu_surface(canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
    match finish() {
        Finish::Bevel98 => {
            canvas.fill_rect(x, y, w, h, palette().content_bg);
            draw_bevel_rings(
                canvas,
                x,
                y,
                w,
                h,
                &[(BEVEL_HIGHLIGHT, BEVEL_DARK), (BEVEL_LIGHT, BEVEL_SHADOW)],
            );
        }
        Finish::GlassKd4 => {
            canvas.fill_rect(x, y, w, h, palette().content_bg);
            canvas.rect(x, y, w, h, AERO_MENU_BORDER);
        }
        Finish::SoftRounded => {
            canvas.fill_rect(x, y, w, h, FUT_MENU_SURFACE);
            canvas.rect(x, y, w, h, FUT_MENU_BORDER);
        }
    }
}

pub fn draw_scrollbar_track(canvas: &mut Canvas, rect: gui_core::Rect) {
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, palette().scrollbar_track);
}

pub fn draw_scrollbar_part(
    canvas: &mut Canvas,
    rect: gui_core::Rect,
    enabled: bool,
    hot: bool,
    pressed: bool,
) {
    let fill = if !enabled {
        palette().content_bg
    } else if pressed {
        palette().scrollbar_pressed
    } else if hot {
        palette().scrollbar_hot
    } else {
        palette().scrollbar_thumb
    };
    canvas.fill_rect(rect.x, rect.y, rect.w, rect.h, fill);
    match current() {
        Theme::Classic => {
            let rings = if pressed {
                [(BEVEL_SHADOW, BEVEL_HIGHLIGHT), (BEVEL_DARK, BEVEL_LIGHT)]
            } else {
                [(BEVEL_HIGHLIGHT, BEVEL_DARK), (BEVEL_LIGHT, BEVEL_SHADOW)]
            };
            draw_bevel_rings(canvas, rect.x, rect.y, rect.w, rect.h, &rings);
        }
        Theme::Aero | Theme::Futurism => {
            canvas.rect(rect.x, rect.y, rect.w, rect.h, palette().border)
        }
    }
}
