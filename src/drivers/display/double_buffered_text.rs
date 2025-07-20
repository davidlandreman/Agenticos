use crate::graphics::color::Color;
use super::double_buffer::DoubleBufferedFrameBuffer;
use bootloader_api::info::FrameBuffer;
use core::fmt;
use spin::Mutex;
use crate::debug_info;
use crate::graphics::fonts::core_font::get_default_font;

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
    &mut BACK_BUFFER
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
    char_width: usize,
    char_height: usize,
}

impl DoubleBufferedText {
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let info = framebuffer.info();
        let width = info.width;
        let height = info.height;
        
        // Calculate required buffer size
        let required_size = info.height * info.stride * info.bytes_per_pixel;
        if required_size > MAX_BUFFER_SIZE {
            panic!("Framebuffer too large for static buffer: {} bytes needed", required_size);
        }
        
        // Get back buffer slice
        let back_buffer = unsafe {
            &mut BACK_BUFFER[..required_size]
        };
        
        // Create double buffered framebuffer
        let buffer = DoubleBufferedFrameBuffer::new(framebuffer, back_buffer);
        
        // Get font dimensions
        let font = get_default_font();
        let char_width = font.char_width();
        let char_height = font.char_height();
        debug_info!("Font dimensions: {}x{}", char_width, char_height);
        
        let text_cols = width / char_width;
        let text_rows = height / char_height;
        
        debug_info!("DoubleBufferedText dimensions: {}x{} pixels, {}x{} chars", width, height, text_cols, text_rows);
        
        let mut text_buffer = Self {
            buffer,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            text_cols,
            text_rows,
            current_color: DEFAULT_COLOR,
            char_width,
            char_height,
        };
        
        // Clear screen
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
    
    pub fn write_char(&mut self, ch: char) {
        // For single character writes, we still need to get the font
        let font = get_default_font();
        let bytes_per_row = font.bytes_per_row();
        
        match ch {
            '\n' => {
                self.newline();
            }
            '\r' => {
                self.cursor_x = 0;
            }
            ch => {
                if self.cursor_x >= self.text_cols {
                    self.newline();
                }
                
                // Draw character inline
                let x = self.cursor_x * self.char_width;
                let y = self.cursor_y * self.char_height;
                
                // Clear character background
                self.buffer.fill_rect(x, y, self.char_width, self.char_height, BACKGROUND_COLOR);
                
                // Draw character using font
                if let Some(bitmap) = font.get_char_bitmap(ch) {
                    for row in 0..self.char_height.min(font.char_height()) {
                        let row_start = row * bytes_per_row;
                        let row_data = &bitmap[row_start..row_start + bytes_per_row];
                        
                        for col in 0..self.char_width.min(font.char_width()) {
                            let byte_index = col / 8;
                            let bit_offset = 7 - (col % 8);
                            
                            if byte_index < row_data.len() && (row_data[byte_index] >> bit_offset) & 1 == 1 {
                                self.buffer.draw_pixel(x + col, y + row, self.current_color);
                            }
                        }
                    }
                }
                
                self.cursor_x += 1;
            }
        }
        // Note: For single character writes, caller is responsible for swapping buffers if needed
    }
    
    pub fn write_string(&mut self, s: &str) {
        // Get font once for the entire string
        let font = get_default_font();
        let bytes_per_row = font.bytes_per_row();
        
        for ch in s.chars() {
            match ch {
                '\n' => {
                    self.newline();
                }
                '\r' => {
                    self.cursor_x = 0;
                }
                ch => {
                    if self.cursor_x >= self.text_cols {
                        self.newline();
                    }
                    
                    // Draw character inline to avoid calling get_default_font repeatedly
                    let x = self.cursor_x * self.char_width;
                    let y = self.cursor_y * self.char_height;
                    
                    // Clear character background
                    self.buffer.fill_rect(x, y, self.char_width, self.char_height, BACKGROUND_COLOR);
                    
                    // Draw character using font
                    if let Some(bitmap) = font.get_char_bitmap(ch) {
                        for row in 0..self.char_height.min(font.char_height()) {
                            let row_start = row * bytes_per_row;
                            let row_data = &bitmap[row_start..row_start + bytes_per_row];
                            
                            for col in 0..self.char_width.min(font.char_width()) {
                                let byte_index = col / 8;
                                let bit_offset = 7 - (col % 8);
                                
                                if byte_index < row_data.len() && (row_data[byte_index] >> bit_offset) & 1 == 1 {
                                    self.buffer.draw_pixel(x + col, y + row, self.current_color);
                                }
                            }
                        }
                    }
                    
                    self.cursor_x += 1;
                }
            }
        }
        // Swap buffers after writing the entire string
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
        // Scroll the buffer contents up by one line height
        self.buffer.scroll_by_pixels(self.char_height);
        
        // Move cursor up one line
        self.cursor_y = self.text_rows - 1;
        
        // Note: Don't swap buffers here - let write_string handle it
    }
}

impl fmt::Write for DoubleBufferedText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

// Global instance
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

// Access the underlying buffer for mouse cursor drawing
pub fn with_buffer<F, R>(f: F) -> Option<R>
where 
    F: FnOnce(&mut DoubleBufferedFrameBuffer) -> R
{
    let mut writer = WRITER.lock();
    if let Some(ref mut w) = *writer {
        Some(f(&mut w.buffer))
    } else {
        None
    }
}

// Set cursor Y position
pub fn set_cursor_y(y: usize) {
    if let Some(ref mut writer) = *WRITER.lock() {
        writer.cursor_y = y;
    }
}

// Get access to the double buffer for direct operations
pub fn with_double_buffer<F, R>(f: F) -> Option<R>
where 
    F: FnOnce(&mut DoubleBufferedFrameBuffer) -> R
{
    if let Some(ref mut writer) = *WRITER.lock() {
        Some(f(&mut writer.buffer))
    } else {
        None
    }
}

// Macros are now exported from display.rs