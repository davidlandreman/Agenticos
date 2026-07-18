//! Cursor rendering with background save/restore

use super::{GraphicsDevice, Rect};
use crate::graphics::color::Color;

/// Size of the cursor save buffer (cursor footprint + outline margin)
const CURSOR_BUFFER_SIZE: usize = 17;

/// The cursor shape as relative pixel offsets (white fill)
const CURSOR_PIXELS: &[(i32, i32)] = &[
    (0, 0),
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (0, 5),
    (0, 6),
    (0, 7),
    (0, 8),
    (0, 9),
    (0, 10),
    (1, 0),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
    (1, 7),
    (1, 8),
    (1, 9),
    (2, 2),
    (2, 3),
    (2, 4),
    (2, 5),
    (2, 6),
    (2, 7),
    (2, 8),
    (3, 3),
    (3, 4),
    (3, 5),
    (3, 6),
    (3, 7),
    (4, 4),
    (4, 5),
    (4, 6),
    (5, 5),
];

/// Handles cursor rendering with background save/restore to avoid trails.
///
/// Coordinates are signed: callers may pass negative or beyond-screen
/// positions and the device adapter handles clipping. The save buffer always
/// covers `CURSOR_BUFFER_SIZE × CURSOR_BUFFER_SIZE` pixels around the cursor;
/// pixels outside the device fall back to `Color::BLACK` on read and are
/// skipped on restore by the adapter's own clipping.
pub struct CursorRenderer {
    /// Saved pixels under the cursor
    background: [[Color; CURSOR_BUFFER_SIZE]; CURSOR_BUFFER_SIZE],
    /// Last save position (top-left corner of the save region)
    last_x: i32,
    last_y: i32,
    /// Whether we have valid saved background
    background_valid: bool,
}

impl CursorRenderer {
    /// Create a new cursor renderer
    pub fn new() -> Self {
        Self {
            background: [[Color::BLACK; CURSOR_BUFFER_SIZE]; CURSOR_BUFFER_SIZE],
            last_x: 0,
            last_y: 0,
            background_valid: false,
        }
    }

    /// Bounds of the background currently saved under the visible cursor.
    pub fn saved_bounds(&self) -> Option<Rect> {
        self.background_valid.then(|| {
            Rect::new(
                self.last_x,
                self.last_y,
                CURSOR_BUFFER_SIZE as u32,
                CURSOR_BUFFER_SIZE as u32,
            )
        })
    }

    /// Complete cursor save/draw footprint at a position.
    pub fn bounds_at(cursor_x: i32, cursor_y: i32) -> Rect {
        Rect::new(
            cursor_x - 1,
            cursor_y - 1,
            CURSOR_BUFFER_SIZE as u32,
            CURSOR_BUFFER_SIZE as u32,
        )
    }

    /// Save background pixels at the cursor position before drawing.
    /// Accounts for outline by starting save 1 pixel before cursor position.
    pub fn save_background(&mut self, cursor_x: i32, cursor_y: i32, device: &dyn GraphicsDevice) {
        let save_x = cursor_x - 1;
        let save_y = cursor_y - 1;

        for dy in 0..CURSOR_BUFFER_SIZE as i32 {
            for dx in 0..CURSOR_BUFFER_SIZE as i32 {
                let px = save_x + dx;
                let py = save_y + dy;
                self.background[dy as usize][dx as usize] = device.read_pixel(px, py);
            }
        }
        self.last_x = save_x;
        self.last_y = save_y;
        self.background_valid = true;
    }

    /// Restore previously saved background pixels
    pub fn restore_background(&mut self, device: &mut dyn GraphicsDevice) {
        if !self.background_valid {
            return;
        }

        let x = self.last_x;
        let y = self.last_y;

        for dy in 0..CURSOR_BUFFER_SIZE as i32 {
            for dx in 0..CURSOR_BUFFER_SIZE as i32 {
                let px = x + dx;
                let py = y + dy;
                device.draw_pixel(px, py, self.background[dy as usize][dx as usize]);
            }
        }
        self.background_valid = false;
    }

    /// Draw the cursor at the given position
    pub fn draw(&self, x: i32, y: i32, device: &mut dyn GraphicsDevice) {
        let cursor_color = Color::WHITE;
        let outline_color = Color::BLACK;

        // Draw black outline first
        for &(dx, dy) in CURSOR_PIXELS {
            let px = x + dx;
            let py = y + dy;

            // Draw outline pixels in all 4 directions; the device clips.
            device.draw_pixel(px - 1, py, outline_color);
            device.draw_pixel(px + 1, py, outline_color);
            device.draw_pixel(px, py - 1, outline_color);
            device.draw_pixel(px, py + 1, outline_color);
        }

        // Draw white cursor on top
        for &(dx, dy) in CURSOR_PIXELS {
            let px = x + dx;
            let py = y + dy;
            device.draw_pixel(px, py, cursor_color);
        }
    }
}

impl Default for CursorRenderer {
    fn default() -> Self {
        Self::new()
    }
}
