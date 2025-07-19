use bootloader_api::info::{FrameBuffer, PixelFormat};
use core::fmt;
use core::ptr;
use crate::graphics::color::Color;

pub struct FrameBufferWriter {
    framebuffer: &'static mut FrameBuffer,
    x_pos: usize,
    y_pos: usize,
    color: Color,
}

const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 16;
const LINE_SPACING: usize = 2;

impl FrameBufferWriter {
    pub fn get_dimensions(&self) -> (usize, usize) {
        let info = self.framebuffer.info();
        (info.width, info.height)
    }
    
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        use crate::debug_debug;
        
        debug_debug!("Creating new FrameBufferWriter...");
        let mut writer = Self {
            framebuffer,
            x_pos: 0,
            y_pos: 0,
            color: Color::WHITE,
        };
        debug_debug!("Clearing framebuffer...");
        writer.clear(Color::BLACK);
        debug_debug!("FrameBufferWriter created successfully!");
        writer
    }

    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width(), self.height(), color);
        self.x_pos = 0;
        self.y_pos = 0;
    }
    
    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width() || y >= self.height() {
            return Color::BLACK;
        }

        let info = self.framebuffer.info();
        let byte_offset = y * info.stride + x;
        let pixel_offset = byte_offset * info.bytes_per_pixel;

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

    pub fn width(&self) -> usize {
        self.framebuffer.info().width
    }

    pub fn height(&self) -> usize {
        self.framebuffer.info().height
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width() || y >= self.height() {
            return;
        }

        let info = self.framebuffer.info();
        let byte_offset = y * info.stride + x;
        let pixel_offset = byte_offset * info.bytes_per_pixel;

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

    pub fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        for dy in 0..height {
            for dx in 0..width {
                self.draw_pixel(x + dx, y + dy, color);
            }
        }
    }

    pub fn draw_char(&mut self, ch: char, x: usize, y: usize) {
        if ch.is_ascii() {
            let char_index = ch as usize;
            if char_index < 128 {
                let char_data = &FONT_8X16[char_index * 16..][..16];
                
                for (row, &byte) in char_data.iter().enumerate() {
                    for col in 0..8 {
                        if (byte >> (7 - col)) & 1 == 1 {
                            self.draw_pixel(x + col, y + row, self.color);
                        }
                    }
                }
            }
        }
    }

    pub fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.x_pos = 0,
            ch => {
                if self.x_pos + CHAR_WIDTH >= self.width() {
                    self.new_line();
                }

                self.draw_char(ch, self.x_pos, self.y_pos);
                self.x_pos += CHAR_WIDTH;
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for ch in s.chars() {
            self.write_char(ch);
        }
    }

    fn new_line(&mut self) {
        self.x_pos = 0;
        self.y_pos += CHAR_HEIGHT + LINE_SPACING;

        if self.y_pos + CHAR_HEIGHT >= self.height() {
            self.scroll();
        }
    }

    fn scroll(&mut self) {
        let info = self.framebuffer.info();
        let bytes_per_pixel = info.bytes_per_pixel;
        let stride = info.stride;
        let scroll_height = CHAR_HEIGHT + LINE_SPACING;
        let height = info.height;
        
        let copy_height = height - scroll_height;
        let copy_size = copy_height * stride * bytes_per_pixel;
        
        let buffer = self.framebuffer.buffer_mut();
        
        unsafe {
            ptr::copy(
                buffer.as_ptr().add(scroll_height * stride * bytes_per_pixel),
                buffer.as_mut_ptr(),
                copy_size,
            );
        }
        
        let clear_start = copy_height * stride * bytes_per_pixel;
        let clear_size = scroll_height * stride * bytes_per_pixel;
        buffer[clear_start..clear_start + clear_size].fill(0);
        
        self.y_pos -= scroll_height;
    }
}

impl fmt::Write for FrameBufferWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

const FONT_8X16: &[u8] = &[0; 128 * 16]; // Temporary placeholder for 8x16 font data

pub fn init(framebuffer: &'static mut FrameBuffer) -> FrameBufferWriter {
    use crate::debug_info;
    
    let info = framebuffer.info();
    debug_info!("Framebuffer Information:");
    debug_info!("  Resolution: {}x{} pixels", info.width, info.height);
    debug_info!("  Pixel format: {:?}", info.pixel_format);
    debug_info!("  Bytes per pixel: {}", info.bytes_per_pixel);
    debug_info!("  Stride: {} pixels", info.stride);
    debug_info!("  Buffer size: {} bytes", framebuffer.buffer().len());
    
    FrameBufferWriter::new(framebuffer)
}