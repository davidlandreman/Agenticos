//! Double-buffered framebuffer text terminal. Used by the legacy `print!`
//! path before the window system is up. Sizes its character grid from the
//! current default font.

use super::double_buffer::DoubleBufferedFrameBuffer;
use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::{get_default_font, FontRef};
use core::fmt;
use spin::Mutex;

const BACKGROUND_COLOR: Color = Color::BLACK;
const MAX_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8MB static buffer

// Static buffer for double buffering
static mut BACK_BUFFER: [u8; MAX_BUFFER_SIZE] = [0; MAX_BUFFER_SIZE];

/// Get access to the static back buffer
///
/// # Safety
/// This function is unsafe because it returns a mutable reference to a static buffer.
/// The caller must ensure no other code is accessing this buffer.
pub unsafe fn get_static_back_buffer() -> &'static mut [u8] {
    &mut *(&raw mut BACK_BUFFER)
}

pub struct DoubleBufferedText {
    buffer: DoubleBufferedFrameBuffer,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    width: usize,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    height: usize,
    cursor_x: usize,
    cursor_y: usize,
    text_cols: usize,
    text_rows: usize,
    current_color: Color,
    cell_width: usize,
    line_height: usize,
}

impl DoubleBufferedText {
    pub fn set_color(&mut self, color: Color) {
        self.current_color = color;
    }

    fn draw_glyph(&mut self, font: &FontRef, ch: char, cell_x_left: usize, cell_y_top: usize) {
        // Background fill is the caller's responsibility — see write_char.
        let Some(glyph) = font.glyph(ch) else {
            return;
        };

        let baseline_y = cell_y_top as i32 + font.ascent() as i32;
        let bitmap_x = cell_x_left as i32 + glyph.x_offset;
        let bitmap_y = baseline_y + glyph.y_offset;
        let gw = glyph.width as i32;
        let gh = glyph.height as i32;
        let color = self.current_color;

        for row in 0..gh {
            let dst_y = bitmap_y + row;
            if dst_y < 0 {
                continue;
            }
            for col in 0..gw {
                let dst_x = bitmap_x + col;
                if dst_x < 0 {
                    continue;
                }
                let alpha = glyph.coverage[(row * gw + col) as usize];
                if alpha == 0 {
                    continue;
                }
                let dst_x = dst_x as usize;
                let dst_y = dst_y as usize;
                if alpha == 0xFF {
                    self.buffer.draw_pixel(dst_x, dst_y, color);
                } else {
                    let bg = self.buffer.get_pixel(dst_x, dst_y);
                    self.buffer
                        .draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
                }
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        let font = get_default_font();

        for ch in s.chars() {
            match ch {
                '\n' => self.newline(),
                '\r' => self.cursor_x = 0,
                ch => {
                    if self.cursor_x >= self.text_cols {
                        self.newline();
                    }

                    let x = self.cursor_x * self.cell_width;
                    let y = self.cursor_y * self.line_height;

                    self.buffer.fill_rect(
                        x,
                        y,
                        self.cell_width,
                        self.line_height,
                        BACKGROUND_COLOR,
                    );
                    self.draw_glyph(&font, ch, x, y);

                    self.cursor_x += 1;
                }
            }
        }
        self.buffer.swap_buffers();
    }

    fn newline(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;

        if self.cursor_y >= self.text_rows {
            self.scroll();
        }
    }

    fn scroll(&mut self) {
        self.buffer.scroll_by_pixels(self.line_height);
        self.cursor_y = self.text_rows - 1;
    }
}

impl fmt::Write for DoubleBufferedText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

static WRITER: Mutex<Option<DoubleBufferedText>> = Mutex::new(None);

pub fn _print(args: fmt::Arguments) {
    use fmt::Write;

    let mut writer = WRITER.lock();
    if let Some(ref mut w) = *writer {
        w.write_fmt(args).unwrap();
    }
}

pub fn set_color(color: Color) {
    let mut writer = WRITER.lock();
    if let Some(ref mut w) = *writer {
        w.set_color(color);
    }
}
