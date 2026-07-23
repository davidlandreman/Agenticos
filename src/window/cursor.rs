//! Typed mouse-pointer sprites and legacy background save/restore.

use alloc::vec::Vec;

use super::{GraphicsDevice, Point, Rect};
use crate::graphics::color::Color;

const HARDWARE_CURSOR_SIDE: usize = 64;
const MAX_CURSOR_WIDTH: usize = 18;
const MAX_CURSOR_HEIGHT: usize = 20;

/// Stable cursor kinds shared with the ring-3 GUI ABI.
#[repr(u32)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorIcon {
    #[default]
    Arrow = 0,
    Wait = 1,
    Text = 2,
}

impl CursorIcon {
    pub const fn from_abi(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Arrow),
            1 => Some(Self::Wait),
            2 => Some(Self::Text),
            _ => None,
        }
    }
}

/// One immutable integer-pixel pointer image.
///
/// Rows use `B` for black, `W` for white, `Y` for the wait-cursor sand, and
/// `.` for transparent. Encoding the outline explicitly keeps the software and
/// hardware cursor paths pixel-identical.
#[derive(Debug, Clone, Copy)]
pub struct CursorSprite {
    pub width: u8,
    pub height: u8,
    pub hot_x: u8,
    pub hot_y: u8,
    rows: &'static [&'static [u8]],
}

const ARROW_ROWS: &[&[u8]] = &[
    b"B.................",
    b"BB................",
    b"BWB...............",
    b"BWWB..............",
    b"BWWWB.............",
    b"BWWWWB............",
    b"BWWWWWB...........",
    b"BWWWWWWB..........",
    b"BWWWWWWWB.........",
    b"BWWWWWWWWB........",
    b"BWWWWBBBBBB.......",
    b"BWWBWWB...........",
    b"BWB.BWWB..........",
    b"BB..BWWB..........",
    b"B....BWWB.........",
    b".....BWWB.........",
    b"......BB..........",
];

const WAIT_ROWS: &[&[u8]] = &[
    b"..BBBBBBBBBBBB....",
    b"..BWWWWWWWWWWB....",
    b"...BWWWWWWWWB.....",
    b"....BYYYYYYB......",
    b".....BYYYYB.......",
    b"......BYYB........",
    b".......BB.........",
    b".......BB.........",
    b"......BWWB........",
    b".....BWWWWB.......",
    b"....BWWYYWWB......",
    b"...BWWYYYYWWB.....",
    b"..BWWYYYYYYWWB....",
    b"..BWWWWWWWWWWB....",
    b"..BBBBBBBBBBBB....",
];

const TEXT_ROWS: &[&[u8]] = &[
    b"...BBBBBBB........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b".....BWB..........",
    b"...BBBBBBB........",
];

const ARROW: CursorSprite = CursorSprite {
    width: 18,
    height: 17,
    hot_x: 0,
    hot_y: 0,
    rows: ARROW_ROWS,
};

const WAIT: CursorSprite = CursorSprite {
    width: 18,
    height: 15,
    hot_x: 8,
    hot_y: 7,
    rows: WAIT_ROWS,
};

const TEXT: CursorSprite = CursorSprite {
    width: 18,
    height: 17,
    hot_x: 6,
    hot_y: 8,
    rows: TEXT_ROWS,
};

pub const fn sprite(icon: CursorIcon) -> &'static CursorSprite {
    match icon {
        CursorIcon::Arrow => &ARROW,
        CursorIcon::Wait => &WAIT,
        CursorIcon::Text => &TEXT,
    }
}

#[cfg(feature = "test")]
pub fn sprite_is_valid(icon: CursorIcon) -> bool {
    let image = sprite(icon);
    image.width > 0
        && image.height > 0
        && usize::from(image.width) <= MAX_CURSOR_WIDTH
        && usize::from(image.height) <= MAX_CURSOR_HEIGHT
        && image.hot_x < image.width
        && image.hot_y < image.height
        && image.rows.len() == usize::from(image.height)
        && image
            .rows
            .iter()
            .all(|row| row.len() == usize::from(image.width))
}

fn sprite_color(byte: u8) -> Option<Color> {
    match byte {
        b'B' => Some(Color::BLACK),
        b'W' => Some(Color::WHITE),
        b'Y' => Some(Color::new(226, 174, 45)),
        _ => None,
    }
}

fn argb(color: Color) -> u32 {
    0xff00_0000
        | (u32::from(color.red) << 16)
        | (u32::from(color.green) << 8)
        | u32::from(color.blue)
}

/// Handles cursor rendering with background save/restore for the legacy
/// framebuffer path.
pub struct CursorRenderer {
    background: [[Color; MAX_CURSOR_WIDTH]; MAX_CURSOR_HEIGHT],
    last_bounds: Rect,
    background_valid: bool,
}

impl CursorRenderer {
    pub fn new() -> Self {
        Self {
            background: [[Color::BLACK; MAX_CURSOR_WIDTH]; MAX_CURSOR_HEIGHT],
            last_bounds: Rect::new(0, 0, 0, 0),
            background_valid: false,
        }
    }

    pub fn saved_bounds(&self) -> Option<Rect> {
        self.background_valid.then_some(self.last_bounds)
    }

    /// Exact image bounds when `position` identifies the sprite hotspot.
    pub fn bounds_at(icon: CursorIcon, position: Point) -> Rect {
        let image = sprite(icon);
        Rect::new(
            position.x - i32::from(image.hot_x),
            position.y - i32::from(image.hot_y),
            u32::from(image.width),
            u32::from(image.height),
        )
    }

    pub const fn hotspot(icon: CursorIcon) -> (u32, u32) {
        let image = sprite(icon);
        (image.hot_x as u32, image.hot_y as u32)
    }

    /// Fixed 64×64 VirtIO-GPU image generated from the canonical sprite.
    pub fn hardware_argb_64(icon: CursorIcon) -> Vec<u32> {
        let image = sprite(icon);
        let mut pixels = alloc::vec![0; HARDWARE_CURSOR_SIDE * HARDWARE_CURSOR_SIDE];
        for (y, row) in image.rows.iter().enumerate() {
            for (x, byte) in row.iter().copied().enumerate() {
                if let Some(color) = sprite_color(byte) {
                    pixels[y * HARDWARE_CURSOR_SIDE + x] = argb(color);
                }
            }
        }
        pixels
    }

    pub fn save_background(
        &mut self,
        icon: CursorIcon,
        position: Point,
        device: &dyn GraphicsDevice,
    ) {
        let bounds = Self::bounds_at(icon, position);
        debug_assert!(bounds.width as usize <= MAX_CURSOR_WIDTH);
        debug_assert!(bounds.height as usize <= MAX_CURSOR_HEIGHT);
        for dy in 0..bounds.height as i32 {
            for dx in 0..bounds.width as i32 {
                self.background[dy as usize][dx as usize] =
                    device.read_pixel(bounds.x + dx, bounds.y + dy);
            }
        }
        self.last_bounds = bounds;
        self.background_valid = true;
    }

    pub fn restore_background(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.background_valid {
            return;
        }
        for dy in 0..self.last_bounds.height as i32 {
            for dx in 0..self.last_bounds.width as i32 {
                device.draw_pixel(
                    self.last_bounds.x + dx,
                    self.last_bounds.y + dy,
                    self.background[dy as usize][dx as usize],
                );
            }
        }
        self.background_valid = false;
    }

    pub fn draw(&self, icon: CursorIcon, position: Point, device: &mut dyn GraphicsDevice) {
        let image = sprite(icon);
        let origin_x = position.x - i32::from(image.hot_x);
        let origin_y = position.y - i32::from(image.hot_y);
        for (dy, row) in image.rows.iter().enumerate() {
            for (dx, byte) in row.iter().copied().enumerate() {
                if let Some(color) = sprite_color(byte) {
                    device.draw_pixel(origin_x + dx as i32, origin_y + dy as i32, color);
                }
            }
        }
    }
}

impl Default for CursorRenderer {
    fn default() -> Self {
        Self::new()
    }
}
