use crate::color::Color;
use bootloader_api::info::{FrameBuffer, PixelFormat};
use core::fmt;
use spin::Mutex;
use crate::debug_info;
use crate::core_font::IBM_PLEX_FONT;

const DEFAULT_COLOR: Color = Color::WHITE;
const BACKGROUND_COLOR: Color = Color::BLACK;
// IBM Plex standard font is 9x17
const CHAR_WIDTH: usize = 9;
const CHAR_HEIGHT: usize = 17;


pub struct TextBuffer {
    framebuffer: &'static mut FrameBuffer,
    width: usize,
    height: usize,
    cursor_x: usize,
    cursor_y: usize,
    text_cols: usize,
    text_rows: usize,
    current_color: Color,
}

impl TextBuffer {
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let info = framebuffer.info();
        let width = info.width;
        let height = info.height;
        let text_cols = width / CHAR_WIDTH;
        let text_rows = height / CHAR_HEIGHT;
        
        debug_info!("TextBuffer dimensions: {}x{} pixels, {}x{} chars", width, height, text_cols, text_rows);
        
        let mut buffer = Self {
            framebuffer,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            text_cols,
            text_rows,
            current_color: DEFAULT_COLOR,
        };
        
        buffer.clear();
        buffer
    }
    
    pub fn set_color(&mut self, color: Color) {
        self.current_color = color;
    }
    
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        let info = self.framebuffer.info();
        let byte_offset = (y * info.stride + x) * info.bytes_per_pixel;
        let pixel_offset = byte_offset;

        let pixel_buffer = self.framebuffer.buffer_mut();

        match info.pixel_format {
            PixelFormat::Rgb => {
                pixel_buffer[pixel_offset] = color.red;
                pixel_buffer[pixel_offset + 1] = color.green;
                pixel_buffer[pixel_offset + 2] = color.blue;
            }
            PixelFormat::Bgr => {
                pixel_buffer[pixel_offset] = color.blue;
                pixel_buffer[pixel_offset + 1] = color.green;
                pixel_buffer[pixel_offset + 2] = color.red;
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
        let byte_offset = (y * info.stride + x) * info.bytes_per_pixel;
        let pixel_offset = byte_offset;

        let pixel_buffer = self.framebuffer.buffer();

        match info.pixel_format {
            PixelFormat::Rgb => {
                Color::new(
                    pixel_buffer[pixel_offset],
                    pixel_buffer[pixel_offset + 1],
                    pixel_buffer[pixel_offset + 2],
                )
            }
            PixelFormat::Bgr => {
                Color::new(
                    pixel_buffer[pixel_offset + 2],
                    pixel_buffer[pixel_offset + 1],
                    pixel_buffer[pixel_offset],
                )
            }
            _ => {
                Color::new(
                    pixel_buffer[pixel_offset + 2],
                    pixel_buffer[pixel_offset + 1],
                    pixel_buffer[pixel_offset],
                )
            }
        }
    }
    
    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        for dy in 0..height {
            for dx in 0..width {
                self.draw_pixel(x + dx, y + dy, color);
            }
        }
    }
    
    fn draw_char(&mut self, ch: char, x: usize, y: usize, color: Color) {
        // Get the IBM Plex font
        if let Some(ref font) = *IBM_PLEX_FONT {
            if let Some(bitmap) = font.get_char_bitmap(ch) {
                // Draw background
                self.fill_rect(x, y, CHAR_WIDTH, CHAR_HEIGHT, BACKGROUND_COLOR);
                
                // Draw character using VFNT bitmap
                let width = font.width as usize;
                let height = font.height as usize;
                let bytes_per_row = (width + 7) / 8;
                
                for row in 0..height {
                    let row_offset = row * bytes_per_row;
                    for byte_idx in 0..bytes_per_row {
                        if row_offset + byte_idx >= bitmap.len() {
                            break;
                        }
                        let byte = bitmap[row_offset + byte_idx];
                        let pixels_in_byte = if byte_idx == bytes_per_row - 1 && width % 8 != 0 {
                            width % 8
                        } else {
                            8
                        };
                        
                        for bit in 0..pixels_in_byte {
                            if (byte >> (7 - bit)) & 1 == 1 {
                                let px = x + byte_idx * 8 + bit;
                                if px < x + width {
                                    self.draw_pixel(px, y + row, color);
                                }
                            }
                        }
                    }
                }
            } else {
                // Character not found in font
                debug_info!("Character '{}' (0x{:02X}) not found in IBM Plex font", ch, ch as u8);
                // Draw a placeholder rectangle
                self.fill_rect(x, y, CHAR_WIDTH, CHAR_HEIGHT, BACKGROUND_COLOR);
                // Draw border to indicate missing character
                for i in 0..CHAR_WIDTH {
                    self.draw_pixel(x + i, y, color);
                    self.draw_pixel(x + i, y + CHAR_HEIGHT - 1, color);
                }
                for i in 0..CHAR_HEIGHT {
                    self.draw_pixel(x, y + i, color);
                    self.draw_pixel(x + CHAR_WIDTH - 1, y + i, color);
                }
            }
        } else {
            debug_info!("IBM Plex font not loaded!");
        }
    }
    
    fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.cursor_x = 0,
            '\t' => {
                // Tab to next 4-character boundary
                let spaces = 4 - (self.cursor_x % 4);
                for _ in 0..spaces {
                    self.write_char(' ');
                }
            }
            _ => {
                if self.cursor_x >= self.text_cols {
                    self.new_line();
                }
                
                // Draw the character at current position
                let x = self.cursor_x * CHAR_WIDTH;
                let y = self.cursor_y * CHAR_HEIGHT;
                
                self.draw_char(ch, x, y, self.current_color);
                
                self.cursor_x += 1;
            }
        }
    }
    
    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        
        // Check if we need to scroll
        if self.cursor_y >= self.text_rows {
            self.scroll_up();
            self.cursor_y = self.text_rows - 1;
        }
    }
    
    fn scroll_up(&mut self) {
        // Copy each row up by one
        for row in 1..self.text_rows {
            let src_y = row * CHAR_HEIGHT;
            let dst_y = (row - 1) * CHAR_HEIGHT;
            
            // Copy row data
            for y in 0..CHAR_HEIGHT {
                for x in 0..self.width {
                    let pixel = self.get_pixel(x, src_y + y);
                    self.draw_pixel(x, dst_y + y, pixel);
                }
            }
        }
        
        // Clear the last row
        let last_row_y = (self.text_rows - 1) * CHAR_HEIGHT;
        self.fill_rect(0, last_row_y, self.width, CHAR_HEIGHT, BACKGROUND_COLOR);
    }
    
    pub fn clear(&mut self) {
        self.fill_rect(0, 0, self.width, self.height, BACKGROUND_COLOR);
        self.cursor_x = 0;
        self.cursor_y = 0;
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

// Global text buffer instance
static TEXT_BUFFER: Mutex<Option<TextBuffer>> = Mutex::new(None);

pub fn init(framebuffer: &'static mut FrameBuffer) {
    let mut buffer = TEXT_BUFFER.lock();
    *buffer = Some(TextBuffer::new(framebuffer));
}

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    
    if let Some(ref mut buffer) = *TEXT_BUFFER.lock() {
        buffer.write_fmt(args).unwrap();
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::text_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn set_color(color: Color) {
    if let Some(ref mut buffer) = *TEXT_BUFFER.lock() {
        buffer.set_color(color);
    }
}

pub fn clear_screen() {
    if let Some(ref mut buffer) = *TEXT_BUFFER.lock() {
        buffer.clear();
    }
}