//! Terminal subsystem — VT100/xterm-compatible terminal emulation.
//!
//! Sits between the window manager (display surface + keyboard events)
//! and userland (process fds), owning the character grid, the ANSI/VT
//! parser, and the PTY pair.
//!
//! Layout follows the plan
//! `docs/plans/2026-05-24-001-feat-terminal-ansi-vt-pty-and-caret-plan.md`:
//!
//! - `vte` — Williams DEC state machine producing `VteEvent`s.
//! - `screen` — character grid, attributes, scrollback, cursor.
//! - `pty` — master/slave fd pair, per-process termios, winsize.
//! - `caret` — caret state (visible, shape, blink).
//! - `colors` — SGR `ColorSpec` and 256-color palette.
//! - `keys` — `KeyCode` → escape-sequence encoding.
//! - `config` — compile-time constants.
//!
//! Units land in plan order. Anything not yet implemented is gated to a
//! stub module so the rest of the kernel sees a stable surface.

pub mod caret;
pub mod colors;
pub mod config;
pub mod keys;
pub mod pty;
pub mod screen;
pub mod vte;

#[cfg(feature = "test")]
pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &tests::test_color_default_fg_is_white,
        &tests::test_color_default_bg_is_dark_grey,
        &tests::test_indexed_palette_ansi_red,
        &tests::test_indexed_palette_ansi_bright_white,
        &tests::test_indexed_palette_cube_corner,
        &tests::test_indexed_palette_grayscale_first_and_last,
        &tests::test_rgb_spec_resolves_directly,
        &tests::test_config_defaults_match_legacy_winsize,
    ]
}

#[cfg(feature = "test")]
mod tests {
    use super::colors::{self, ColorSpec, DEFAULT_BG, DEFAULT_FG, INDEXED_PALETTE};
    use super::config;
    use crate::graphics::color::Color;

    pub(super) fn test_color_default_fg_is_white() {
        assert_eq!(colors::resolve(ColorSpec::Default, true), DEFAULT_FG);
        assert_eq!(DEFAULT_FG, Color::WHITE);
    }

    pub(super) fn test_color_default_bg_is_dark_grey() {
        assert_eq!(colors::resolve(ColorSpec::Default, false), DEFAULT_BG);
        assert_eq!(DEFAULT_BG, Color::new(32, 32, 32));
    }

    pub(super) fn test_indexed_palette_ansi_red() {
        // Index 1 = ANSI red.
        assert_eq!(INDEXED_PALETTE[1], Color::new(170, 0, 0));
        assert_eq!(
            colors::resolve(ColorSpec::Indexed(1), true),
            Color::new(170, 0, 0),
        );
    }

    pub(super) fn test_indexed_palette_ansi_bright_white() {
        // Index 15 = ANSI bright white.
        assert_eq!(INDEXED_PALETTE[15], Color::new(255, 255, 255));
    }

    pub(super) fn test_indexed_palette_cube_corner() {
        // Index 16 = cube origin (0,0,0). Index 231 = cube far corner
        // (255,255,255).
        assert_eq!(INDEXED_PALETTE[16], Color::new(0, 0, 0));
        assert_eq!(INDEXED_PALETTE[231], Color::new(255, 255, 255));
    }

    pub(super) fn test_indexed_palette_grayscale_first_and_last() {
        // Index 232 = first grayscale step (8,8,8).
        assert_eq!(INDEXED_PALETTE[232], Color::new(8, 8, 8));
        // Index 255 = last grayscale step (8 + 23*10 = 238).
        assert_eq!(INDEXED_PALETTE[255], Color::new(238, 238, 238));
    }

    pub(super) fn test_rgb_spec_resolves_directly() {
        assert_eq!(
            colors::resolve(ColorSpec::Rgb(12, 34, 56), true),
            Color::new(12, 34, 56),
        );
    }

    pub(super) fn test_config_defaults_match_legacy_winsize() {
        assert_eq!(config::DEFAULT_COLS, 80);
        assert_eq!(config::DEFAULT_ROWS, 24);
        assert_eq!(config::TAB_WIDTH, 8);
    }
}
