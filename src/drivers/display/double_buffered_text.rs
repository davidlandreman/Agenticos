//! Double-buffered framebuffer text terminal. Used by the legacy `print!`
//! path before the window system is up. Sizes its character grid from the
//! current default font.

use crate::graphics::color::Color;
use super::double_buffer::DoubleBufferedFrameBuffer;
use bootloader_api::info::FrameBuffer;
use core::fmt;
use spin::Mutex;
use crate::debug_info;
use crate::graphics::fonts::core_font::{get_default_font, FontRef};

const DEFAULT_COLOR: Color = Color::WHITE;
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

impl DoubleBufferedText {
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let info = framebuffer.info();
        let width = info.width;
        let height = info.height;

        let required_size = info.height * info.stride * info.bytes_per_pixel;
        if required_size > MAX_BUFFER_SIZE {
            panic!("Framebuffer too large for static buffer: {} bytes needed", required_size);
        }

        let back_buffer = unsafe { &mut BACK_BUFFER[..required_size] };
        let buffer = DoubleBufferedFrameBuffer::new(framebuffer, back_buffer);

        let font = get_default_font();
        let cell_width = font.cell_width() as usize;
        let line_height = font.line_height() as usize;
        debug_info!("DoubleBufferedText font cell: {}x{}", cell_width, line_height);

        let text_cols = width / cell_width.max(1);
        let text_rows = height / line_height.max(1);

        debug_info!(
            "DoubleBufferedText dimensions: {}x{} pixels, {}x{} chars",
            width, height, text_cols, text_rows
        );

        let mut text_buffer = Self {
            buffer,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            text_cols,
            text_rows,
            current_color: DEFAULT_COLOR,
            cell_width,
            line_height,
        };

        text_buffer.clear();

        text_buffer
    }

    pub fn clear(&mut self) {
        self.buffer.clear(BACKGROUND_COLOR);
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.buffer.swap_buffers();
    }

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
                    self.buffer.draw_pixel(dst_x, dst_y, bg.blend(&color, alpha));
                }
            }
        }
    }

    pub fn write_char(&mut self, ch: char) {
        let font = get_default_font();

        match ch {
            '\n' => self.newline(),
            '\r' => self.cursor_x = 0,
            ch => {
                if self.cursor_x >= self.text_cols {
                    self.newline();
                }

                let x = self.cursor_x * self.cell_width;
                let y = self.cursor_y * self.line_height;

                self.buffer.fill_rect(x, y, self.cell_width, self.line_height, BACKGROUND_COLOR);
                self.draw_glyph(&font, ch, x, y);

                self.cursor_x += 1;
            }
        }
        // Caller is responsible for swapping buffers if needed.
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

                    self.buffer.fill_rect(x, y, self.cell_width, self.line_height, BACKGROUND_COLOR);
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

pub fn init(framebuffer: &'static mut FrameBuffer) {
    let mut writer = WRITER.lock();
    *writer = Some(DoubleBufferedText::new(framebuffer));
}

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

pub fn clear_screen() {
    let mut writer = WRITER.lock();
    if let Some(ref mut w) = *writer {
        w.clear();
    }
}

pub fn with_buffer<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut DoubleBufferedFrameBuffer) -> R,
{
    let mut writer = WRITER.lock();
    if let Some(ref mut w) = *writer {
        Some(f(&mut w.buffer))
    } else {
        None
    }
}

pub fn set_cursor_y(y: usize) {
    if let Some(ref mut writer) = *WRITER.lock() {
        writer.cursor_y = y;
    }
}

pub fn with_double_buffer<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut DoubleBufferedFrameBuffer) -> R,
{
    if let Some(ref mut writer) = *WRITER.lock() {
        Some(f(&mut writer.buffer))
    } else {
        None
    }
}
