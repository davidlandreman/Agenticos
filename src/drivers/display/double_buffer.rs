use bootloader_api::info::{FrameBuffer, PixelFormat};
use core::fmt;
use core::ptr;
use crate::graphics::color::Color;

pub struct DoubleBufferedFrameBuffer {
    framebuffer: &'static mut FrameBuffer,
    back_buffer: &'static mut [u8],
    x_pos: usize,
    y_pos: usize,
    #[expect(dead_code, reason = "intentional kernel API surface")]
    color: Color,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
    stride: usize,
    pixel_format: PixelFormat,
}

const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 16;
const LINE_SPACING: usize = 2;

impl DoubleBufferedFrameBuffer {
    pub fn new(framebuffer: &'static mut FrameBuffer, back_buffer: &'static mut [u8]) -> Self {
        use crate::debug_debug;
        
        let info = framebuffer.info();
        let buffer_size = info.height * info.stride * info.bytes_per_pixel;
        
        if back_buffer.len() < buffer_size {
            panic!("Back buffer too small! Need {} bytes, got {}", buffer_size, back_buffer.len());
        }
        
        debug_debug!("Creating DoubleBufferedFrameBuffer...");
        debug_debug!("  Framebuffer size: {} bytes", buffer_size);
        debug_debug!("  Back buffer size: {} bytes", back_buffer.len());
        
        let mut writer = Self {
            framebuffer,
            back_buffer,
            x_pos: 0,
            y_pos: 0,
            color: Color::WHITE,
            width: info.width,
            height: info.height,
            bytes_per_pixel: info.bytes_per_pixel,
            stride: info.stride,
            pixel_format: info.pixel_format,
        };
        
        writer.clear(Color::BLACK);
        writer.swap_buffers();
        debug_debug!("DoubleBufferedFrameBuffer created successfully!");
        
        writer
    }
    
    /// Owned snapshot of the back buffer (DRAM) plus metadata. Reading the
    /// back buffer is fast and avoids tearing against the compositor's
    /// `swap_buffers`. Caller must hold the device-level mutex around this
    /// call to serialize against renders.
    



    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    pub fn bytes_per_pixel(&self) -> usize {
        self.bytes_per_pixel
    }

    #[expect(dead_code, reason = "required by dormant GraphicsDevice API")]
    pub fn stride(&self) -> usize {
        self.stride
    }


    /// Mutable access to a single row of the back buffer, returned as a
    /// byte slice of length `width * bytes_per_pixel`. Used by adapter-
    /// level bulk blits (see `DoubleBufferedDevice::blit_buffer`) that
    /// need to memcpy whole rows from a same-format source.
    pub fn back_buffer_row_mut(&mut self, y: usize) -> Option<&mut [u8]> {
        if y >= self.height {
            return None;
        }
        let row_start = y * self.stride * self.bytes_per_pixel;
        let row_end = row_start + self.width * self.bytes_per_pixel;
        Some(&mut self.back_buffer[row_start..row_end])
    }


    pub fn clear(&mut self, color: Color) {
        self.fill_rect(0, 0, self.width, self.height, color);
        self.x_pos = 0;
        self.y_pos = 0;
    }
    
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        if x >= self.width || y >= self.height {
            return;
        }
        
        let byte_offset = y * self.stride + x;
        let pixel_offset = byte_offset * self.bytes_per_pixel;
        
        match self.pixel_format {
            PixelFormat::Rgb => {
                self.back_buffer[pixel_offset] = color.red;
                self.back_buffer[pixel_offset + 1] = color.green;
                self.back_buffer[pixel_offset + 2] = color.blue;
            }
            PixelFormat::Bgr => {
                self.back_buffer[pixel_offset] = color.blue;
                self.back_buffer[pixel_offset + 1] = color.green;
                self.back_buffer[pixel_offset + 2] = color.red;
            }
            _ => {
                self.back_buffer[pixel_offset] = color.blue;
                self.back_buffer[pixel_offset + 1] = color.green;
                self.back_buffer[pixel_offset + 2] = color.red;
            }
        }
    }
    
    pub fn get_pixel(&self, x: usize, y: usize) -> Color {
        if x >= self.width || y >= self.height {
            return Color::BLACK;
        }
        
        let byte_offset = y * self.stride + x;
        let pixel_offset = byte_offset * self.bytes_per_pixel;
        
        match self.pixel_format {
            PixelFormat::Rgb => {
                Color::new(
                    self.back_buffer[pixel_offset],
                    self.back_buffer[pixel_offset + 1],
                    self.back_buffer[pixel_offset + 2],
                )
            }
            PixelFormat::Bgr => {
                Color::new(
                    self.back_buffer[pixel_offset + 2],
                    self.back_buffer[pixel_offset + 1],
                    self.back_buffer[pixel_offset],
                )
            }
            _ => {
                Color::new(
                    self.back_buffer[pixel_offset + 2],
                    self.back_buffer[pixel_offset + 1],
                    self.back_buffer[pixel_offset],
                )
            }
        }
    }
    
    pub fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        let x_end = (x + width).min(self.width);
        let y_end = (y + height).min(self.height);
        
        for dy in y..y_end {
            let row_start = dy * self.stride + x;
            let row_end = dy * self.stride + x_end;
            let pixel_start = row_start * self.bytes_per_pixel;
            let pixel_end = row_end * self.bytes_per_pixel;
            
            match self.pixel_format {
                PixelFormat::Rgb => {
                    for pixel_offset in (pixel_start..pixel_end).step_by(self.bytes_per_pixel) {
                        self.back_buffer[pixel_offset] = color.red;
                        self.back_buffer[pixel_offset + 1] = color.green;
                        self.back_buffer[pixel_offset + 2] = color.blue;
                    }
                }
                PixelFormat::Bgr => {
                    for pixel_offset in (pixel_start..pixel_end).step_by(self.bytes_per_pixel) {
                        self.back_buffer[pixel_offset] = color.blue;
                        self.back_buffer[pixel_offset + 1] = color.green;
                        self.back_buffer[pixel_offset + 2] = color.red;
                    }
                }
                _ => {
                    for pixel_offset in (pixel_start..pixel_end).step_by(self.bytes_per_pixel) {
                        self.back_buffer[pixel_offset] = color.blue;
                        self.back_buffer[pixel_offset + 1] = color.green;
                        self.back_buffer[pixel_offset + 2] = color.red;
                    }
                }
            }
        }
    }
    
    pub fn draw_char(&mut self, _ch: char, _x: usize, _y: usize) {
        // Character drawing will be handled by the font system
        // This is a placeholder for now
    }
    
    pub fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.x_pos = 0,
            ch => {
                if self.x_pos + CHAR_WIDTH >= self.width {
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
        
        if self.y_pos + CHAR_HEIGHT >= self.height {
            self.scroll();
        }
    }
    
    pub fn scroll_by_pixels(&mut self, pixels: usize) {
        if pixels >= self.height {
            // Clear entire screen if scrolling more than screen height
            self.back_buffer.fill(0);
            return;
        }
        
        let copy_height = self.height - pixels;
        let copy_size = copy_height * self.stride * self.bytes_per_pixel;
        
        unsafe {
            ptr::copy(
                self.back_buffer.as_ptr().add(pixels * self.stride * self.bytes_per_pixel),
                self.back_buffer.as_mut_ptr(),
                copy_size,
            );
        }
        
        // Clear the bottom portion
        let clear_start = copy_height * self.stride * self.bytes_per_pixel;
        let clear_size = pixels * self.stride * self.bytes_per_pixel;
        self.back_buffer[clear_start..clear_start + clear_size].fill(0);
    }
    
    fn scroll(&mut self) {
        let scroll_height = CHAR_HEIGHT + LINE_SPACING;
        self.scroll_by_pixels(scroll_height);
        self.y_pos -= scroll_height;
    }
    
    pub fn swap_buffers(&mut self) {
        let buffer_size = self.height * self.stride * self.bytes_per_pixel;
        let front_buffer = self.framebuffer.buffer_mut();
        
        unsafe {
            ptr::copy_nonoverlapping(
                self.back_buffer.as_ptr(),
                front_buffer.as_mut_ptr(),
                buffer_size.min(front_buffer.len())
            );
        }
    }
    
    /// Swap only a specific rectangular region
    pub fn swap_region(&mut self, x: usize, y: usize, width: usize, height: usize) {
        let front_buffer = self.framebuffer.buffer_mut();
        
        // Clamp the region to screen bounds
        let x = x.min(self.width);
        let y = y.min(self.height);
        let width = width.min(self.width - x);
        let height = height.min(self.height - y);
        
        // Copy only the specified region row by row
        unsafe {
            for row in 0..height {
                let y_offset = (y + row) * self.stride * self.bytes_per_pixel;
                let x_offset = x * self.bytes_per_pixel;
                let offset = y_offset + x_offset;
                let row_size = width * self.bytes_per_pixel;
                
                ptr::copy_nonoverlapping(
                    self.back_buffer.as_ptr().add(offset),
                    front_buffer.as_mut_ptr().add(offset),
                    row_size.min(front_buffer.len() - offset)
                );
            }
        }
    }
    
    // Image drawing methods
        
    }

impl fmt::Write for DoubleBufferedFrameBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
