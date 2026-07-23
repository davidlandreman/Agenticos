//! Minimal RGB color value, replacing the kernel's `graphics::color::Color`.
//!
//! The emulator stores `ColorSpec` per cell and only resolves to a concrete
//! `Color` at paint time (`colors::resolve`). The renderer converts a resolved
//! `Color` to little-endian XRGB8888 for `gui_win_present` via [`Color::to_xrgb`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Color {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub const BLACK: Color = Color::new(0, 0, 0);
    pub const WHITE: Color = Color::new(255, 255, 255);

    /// Pack into the little-endian XRGB8888 word (`0x00RRGGBB`) that the GUI
    /// present ABI expects.
    pub const fn to_xrgb(self) -> u32 {
        ((self.red as u32) << 16) | ((self.green as u32) << 8) | (self.blue as u32)
    }
}
