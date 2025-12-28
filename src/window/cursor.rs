//! Cursor rendering with background save/restore

use crate::graphics::color::Color;
use super::GraphicsDevice;

/// Size of the cursor save buffer (cursor footprint + outline margin)
const CURSOR_BUFFER_SIZE: usize = 17;

/// The cursor shape as relative pixel offsets (white fill)
const CURSOR_PIXELS: &[(usize, usize)] = &[
    (0, 0), (0, 1), (0, 2), (0, 3), (0, 4), (0, 5), (0, 6), (0, 7), (0, 8), (0, 9), (0, 10),
    (1, 0), (1, 1), (1, 2), (1, 3), (1, 4), (1, 5), (1, 6), (1, 7), (1, 8), (1, 9),
    (2, 2), (2, 3), (2, 4), (2, 5), (2, 6), (2, 7), (2, 8),
    (3, 3), (3, 4), (3, 5), (3, 6), (3, 7),
    (4, 4), (4, 5), (4, 6),
    (5, 5),
];

/// Handles cursor rendering with background save/restore to avoid trails
pub struct CursorRenderer {
    /// Saved pixels under the cursor
    background: [[Color; CURSOR_BUFFER_SIZE]; CURSOR_BUFFER_SIZE],
    /// Last save position (adjusted for outline)
    last_x: usize,
    last_y: usize,
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

    /// Save background pixels at the cursor position before drawing
    /// Accounts for outline by starting save 1 pixel before cursor position
    pub fn save_background(&mut self, cursor_x: usize, cursor_y: usize, device: &dyn GraphicsDevice) {
        let width = device.width();
        let height = device.height();

        // Start save position - offset by 1 to include outline on left/top
        let save_x = if cursor_x > 0 { cursor_x - 1 } else { 0 };
        let save_y = if cursor_y > 0 { cursor_y - 1 } else { 0 };

        for dy in 0..CURSOR_BUFFER_SIZE {
            for dx in 0..CURSOR_BUFFER_SIZE {
                let px = save_x + dx;
                let py = save_y + dy;
                if px < width && py < height {
                    self.background[dy][dx] = device.read_pixel(px, py);
                }
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
        let width = device.width();
        let height = device.height();

        for dy in 0..CURSOR_BUFFER_SIZE {
            for dx in 0..CURSOR_BUFFER_SIZE {
                let px = x + dx;
                let py = y + dy;
                if px < width && py < height {
                    device.draw_pixel(px, py, self.background[dy][dx]);
                }
            }
        }
        self.background_valid = false;
    }

    /// Draw the cursor at the given position
    pub fn draw(&self, x: usize, y: usize, device: &mut dyn GraphicsDevice) {
        let cursor_color = Color::WHITE;
        let outline_color = Color::BLACK;
        let width = device.width();
        let height = device.height();

        // Draw black outline first
        for &(dx, dy) in CURSOR_PIXELS {
            let px = x + dx;
            let py = y + dy;

            // Draw outline pixels in all 4 directions
            if px > 0 {
                device.draw_pixel(px - 1, py, outline_color);
            }
            if px < width - 1 {
                device.draw_pixel(px + 1, py, outline_color);
            }
            if py > 0 {
                device.draw_pixel(px, py - 1, outline_color);
            }
            if py < height - 1 {
                device.draw_pixel(px, py + 1, outline_color);
            }
        }

        // Draw white cursor on top
        for &(dx, dy) in CURSOR_PIXELS {
            let px = x + dx;
            let py = y + dy;

            if px < width && py < height {
                device.draw_pixel(px, py, cursor_color);
            }
        }
    }
}

impl Default for CursorRenderer {
    fn default() -> Self {
        Self::new()
    }
}
