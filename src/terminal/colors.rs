//! Color resolution for SGR escape sequences.
//!
//! A terminal cell's foreground and background each carry a `ColorSpec`,
//! not a concrete RGB value. The spec is one of:
//!
//! - `Default` — defer to the terminal's configured default foreground /
//!   background (typically white on dark grey).
//! - `Indexed(u8)` — a 256-color palette entry. Indices 0..16 are the
//!   eight ANSI colors + their bright variants; 16..232 are the 6×6×6
//!   color cube; 232..256 are 24 grayscale steps.
//! - `Rgb(r, g, b)` — direct 24-bit color from `\x1B[38;2;r;g;b m` /
//!   `\x1B[48;2;r;g;b m`.
//!
//! `resolve` collapses a spec to a concrete `Color` using xterm's
//! standard palette. Resolution happens at paint time, not at SGR parse
//! time, so future changes to the palette don't require re-walking the
//! grid.

use crate::graphics::color::Color;

/// Default foreground used when `ColorSpec::Default` is resolved for FG.
/// Matches the prior `TextWindow` foreground.
pub const DEFAULT_FG: Color = Color::WHITE;

/// Default background used when `ColorSpec::Default` is resolved for BG.
/// Matches the dark-grey terminal background painted by `TextWindow`.
pub const DEFAULT_BG: Color = Color::new(32, 32, 32);

/// A per-cell color specification. Copied/compared by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpec {
    /// Use the terminal's configured default.
    Default,
    /// 256-color palette index.
    Indexed(u8),
    /// 24-bit direct RGB.
    Rgb(u8, u8, u8),
}

impl ColorSpec {
    pub const fn default_fg() -> Self {
        ColorSpec::Default
    }

    pub const fn default_bg() -> Self {
        ColorSpec::Default
    }
}

/// Resolve a `ColorSpec` to a concrete `Color`. `is_foreground` selects
/// which default to use when the spec is `Default`.
pub fn resolve(spec: ColorSpec, is_foreground: bool) -> Color {
    match spec {
        ColorSpec::Default => {
            if is_foreground {
                DEFAULT_FG
            } else {
                DEFAULT_BG
            }
        }
        ColorSpec::Indexed(i) => INDEXED_PALETTE[i as usize],
        ColorSpec::Rgb(r, g, b) => Color::new(r, g, b),
    }
}

/// The 256-color xterm palette as a precomputed const table.
///
/// - 0..8: standard ANSI colors (matches `xterm-256color` terminfo defaults)
/// - 8..16: bright ANSI colors
/// - 16..232: 6×6×6 RGB cube; the steps are 0, 95, 135, 175, 215, 255
/// - 232..256: grayscale, RGB(8 + 10*i, …) for i in 0..24
pub const INDEXED_PALETTE: [Color; 256] = build_palette();

const fn build_palette() -> [Color; 256] {
    let mut palette = [Color::BLACK; 256];

    // ANSI 0..8
    palette[0] = Color::new(0, 0, 0); // black
    palette[1] = Color::new(170, 0, 0); // red
    palette[2] = Color::new(0, 170, 0); // green
    palette[3] = Color::new(170, 85, 0); // yellow (brown in classic VGA)
    palette[4] = Color::new(0, 0, 170); // blue
    palette[5] = Color::new(170, 0, 170); // magenta
    palette[6] = Color::new(0, 170, 170); // cyan
    palette[7] = Color::new(170, 170, 170); // white (light grey)

    // ANSI bright 8..16
    palette[8] = Color::new(85, 85, 85); // bright black (dark grey)
    palette[9] = Color::new(255, 85, 85); // bright red
    palette[10] = Color::new(85, 255, 85); // bright green
    palette[11] = Color::new(255, 255, 85); // bright yellow
    palette[12] = Color::new(85, 85, 255); // bright blue
    palette[13] = Color::new(255, 85, 255); // bright magenta
    palette[14] = Color::new(85, 255, 255); // bright cyan
    palette[15] = Color::new(255, 255, 255); // bright white

    // 6×6×6 RGB cube at 16..232
    let cube_steps: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut idx = 16usize;
    let mut r = 0usize;
    while r < 6 {
        let mut g = 0usize;
        while g < 6 {
            let mut b = 0usize;
            while b < 6 {
                palette[idx] = Color::new(cube_steps[r], cube_steps[g], cube_steps[b]);
                idx += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }

    // Grayscale ramp 232..256
    let mut i = 0usize;
    while i < 24 {
        let v = 8 + 10 * i as u8;
        palette[232 + i] = Color::new(v, v, v);
        i += 1;
    }

    palette
}
