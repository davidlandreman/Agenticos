//! Single-buffered framebuffer text terminal. Used by the legacy `print!`
//! path before the window system is up. Sizes its character grid from the
//! current default font.

use crate::graphics::color::Color;
use crate::graphics::fonts::core_font::get_default_font;
use bootloader_api::info::{FrameBuffer, PixelFormat};
use core::fmt;
use spin::Mutex;

const BACKGROUND_COLOR: Color = Color::BLACK;

pub struct TextBuffer {
    framebuffer: &'static mut FrameBuffer,
    width: usize,
    height: usize,
    cursor_x: usize,
    cursor_y: usize,
    text_cols: usize,
    text_rows: usize,
    current_color: Color,
    cell_width: usize,
    line_height: usize,
}

impl TextBuffer {
    pub fn set_color(&mut self, color: Color) {
        self.current_color = color;
    }

    fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        let info = self.framebuffer.info();
        let pixel_offset = (y * info.stride + x) * info.bytes_per_pixel;
        let pixel_buffer = self.framebuffer.buffer_mut();

        match info.pixel_format {
            PixelFormat::Rgb => {
                pixel_buffer[pixel_offset] = color.red;
                pixel_buffer[pixel_offset + 1] = color.green;
                pixel_buffer[pixel_offset + 2] = color.blue;
            }
            _ => {
                pixel_buffer[pixel_offset] = color.blue;
                pixel_buffer[pixel_offset + 1] = color.green;
                pixel_buffer[pixel_offset + 2] = color.red;
            }
        }
    }

    fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color::BLACK;
        }

        let info = self.framebuffer.info();
        let pixel_offset = (y * info.stride + x) * info.bytes_per_pixel;
        let pixel_buffer = self.framebuffer.buffer();

        match info.pixel_format {
            PixelFormat::Rgb => Color::new(
                pixel_buffer[pixel_offset],
                pixel_buffer[pixel_offset + 1],
                pixel_buffer[pixel_offset + 2],
            ),
            _ => Color::new(
                pixel_buffer[pixel_offset + 2],
                pixel_buffer[pixel_offset + 1],
                pixel_buffer[pixel_offset],
            ),
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        for dy in 0..height {
            for dx in 0..width {
                self.draw_pixel(x + dx, y + dy, color);
            }
        }
    }

    fn draw_char(&mut self, ch: char, cell_x_left: usize, cell_y_top: usize, color: Color) {
        // Always paint the full cell background first so the previous char
        // doesn't bleed through.
        self.fill_rect(
            cell_x_left,
            cell_y_top,
            self.cell_width,
            self.line_height,
            BACKGROUND_COLOR,
        );

        let font = get_default_font();
        let Some(glyph) = font.glyph(ch) else {
            return;
        };

        let baseline_y = cell_y_top as i32 + font.ascent() as i32;
        let bitmap_x = cell_x_left as i32 + glyph.x_offset;
        let bitmap_y = baseline_y + glyph.y_offset;
        let gw = glyph.width as i32;
        let gh = glyph.height as i32;

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
                    self.draw_pixel(dst_x, dst_y, color);
                } else {
                    let bg = self.get_pixel(dst_x, dst_y);
                    self.draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
                }
            }
        }
    }

    fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.cursor_x = 0,
            '\t' => {
                let spaces = 4 - (self.cursor_x % 4);
                for _ in 0..spaces {
                    self.write_char(' ');
                }
            }
            _ => {
                if self.cursor_x >= self.text_cols {
                    self.new_line();
                }

                let x = self.cursor_x * self.cell_width;
                let y = self.cursor_y * self.line_height;

                self.draw_char(ch, x, y, self.current_color);

                self.cursor_x += 1;
            }
        }
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;

        if self.cursor_y >= self.text_rows {
            self.scroll_up();
            self.cursor_y = self.text_rows - 1;
        }
    }

    fn scroll_up(&mut self) {
        for row in 1..self.text_rows {
            let src_y = row * self.line_height;
            let dst_y = (row - 1) * self.line_height;

            for y in 0..self.line_height {
                for x in 0..self.width {
                    let pixel = self.get_pixel(x, src_y + y);
                    self.draw_pixel(x, dst_y + y, pixel);
                }
            }
        }

        let last_row_y = (self.text_rows - 1) * self.line_height;
        self.fill_rect(
            0,
            last_row_y,
            self.width,
            self.line_height,
            BACKGROUND_COLOR,
        );
    }
}

impl fmt::Write for TextBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            self.write_char(ch);
        }
        Ok(())
    }
}

static TEXT_BUFFER: Mutex<Option<TextBuffer>> = Mutex::new(None);

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    if let Some(ref mut buffer) = *TEXT_BUFFER.lock() {
        buffer.write_fmt(args).unwrap();
    }
}

pub fn set_color(color: Color) {
    if let Some(ref mut buffer) = *TEXT_BUFFER.lock() {
        buffer.set_color(color);
    }
}
