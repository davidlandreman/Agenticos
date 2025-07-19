use super::color::Color;
use super::core_gfx::Graphics;
use crate::drivers::display::double_buffer::DoubleBufferedFrameBuffer;

const CURSOR_SIZE: usize = 12;

// Classic arrow cursor shape
const CURSOR_SHAPE: [[u8; CURSOR_SIZE]; CURSOR_SIZE] = [
    [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 1, 0, 0, 0, 0],
    [1, 2, 2, 2, 2, 2, 2, 2, 1, 0, 0, 0],
    [1, 2, 2, 2, 2, 1, 1, 1, 1, 1, 0, 0],
    [1, 2, 2, 1, 2, 1, 0, 0, 0, 0, 0, 0],
    [1, 1, 1, 0, 1, 1, 0, 0, 0, 0, 0, 0],
];

pub struct MouseCursor {
    saved_pixels: [[Color; CURSOR_SIZE]; CURSOR_SIZE],
    last_x: i32,
    last_y: i32,
    visible: bool,
}

impl MouseCursor {
    pub const fn new() -> Self {
        Self {
            saved_pixels: [[Color::BLACK; CURSOR_SIZE]; CURSOR_SIZE],
            last_x: -1,
            last_y: -1,
            visible: false,
        }
    }
    
    pub fn draw(&mut self, frame_buffer: &mut DoubleBufferedFrameBuffer, x: i32, y: i32) {
        // First restore the area under the previous cursor position
        if self.visible && self.last_x >= 0 && self.last_y >= 0 {
            self.restore_background(frame_buffer);
        }
        
        // Save the area under the new cursor position
        self.save_background(frame_buffer, x, y);
        
        // Draw the cursor at the new position
        let (width, height) = frame_buffer.get_dimensions();
        
        for row in 0..CURSOR_SIZE {
            for col in 0..CURSOR_SIZE {
                let pixel_x = x + col as i32;
                let pixel_y = y + row as i32;
                
                if pixel_x >= 0 && pixel_x < width as i32 && 
                   pixel_y >= 0 && pixel_y < height as i32 {
                    match CURSOR_SHAPE[row][col] {
                        1 => frame_buffer.draw_pixel(pixel_x as usize, pixel_y as usize, Color::BLACK),
                        2 => frame_buffer.draw_pixel(pixel_x as usize, pixel_y as usize, Color::WHITE),
                        _ => {} // Transparent
                    }
                }
            }
        }
        
        self.last_x = x;
        self.last_y = y;
        self.visible = true;
    }
    
    pub fn hide(&mut self, frame_buffer: &mut DoubleBufferedFrameBuffer) {
        if self.visible && self.last_x >= 0 && self.last_y >= 0 {
            self.restore_background(frame_buffer);
            self.visible = false;
        }
    }
    
    fn save_background(&mut self, frame_buffer: &mut DoubleBufferedFrameBuffer, x: i32, y: i32) {
        let (width, height) = frame_buffer.get_dimensions();
        
        for row in 0..CURSOR_SIZE {
            for col in 0..CURSOR_SIZE {
                let pixel_x = x + col as i32;
                let pixel_y = y + row as i32;
                
                if pixel_x >= 0 && pixel_x < width as i32 && 
                   pixel_y >= 0 && pixel_y < height as i32 {
                    self.saved_pixels[row][col] = frame_buffer.get_pixel(pixel_x as usize, pixel_y as usize);
                }
            }
        }
    }
    
    fn restore_background(&mut self, frame_buffer: &mut DoubleBufferedFrameBuffer) {
        let (width, height) = frame_buffer.get_dimensions();
        
        for row in 0..CURSOR_SIZE {
            for col in 0..CURSOR_SIZE {
                let pixel_x = self.last_x + col as i32;
                let pixel_y = self.last_y + row as i32;
                
                if pixel_x >= 0 && pixel_x < width as i32 && 
                   pixel_y >= 0 && pixel_y < height as i32 {
                    frame_buffer.draw_pixel(pixel_x as usize, pixel_y as usize, self.saved_pixels[row][col]);
                }
            }
        }
    }
}

// For use with the Graphics API
pub fn draw_cursor_with_graphics(gfx: &mut Graphics, x: i32, y: i32) {
    // Draw a simple crosshair cursor using the graphics API
    gfx.set_stroke_color(Color::WHITE);
    gfx.set_stroke_width(1);
    
    // Horizontal line
    gfx.draw_line((x - 5) as isize, y as isize, (x + 5) as isize, y as isize);
    // Vertical line  
    gfx.draw_line(x as isize, (y - 5) as isize, x as isize, (y + 5) as isize);
    
    // Black outline for visibility
    gfx.set_stroke_color(Color::BLACK);
    gfx.draw_rect((x - 6) as usize, (y - 6) as usize, 12, 12);
}

// Global mouse cursor instance
use spin::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    static ref MOUSE_CURSOR: Mutex<MouseCursor> = Mutex::new(MouseCursor::new());
}

/// Draw the mouse cursor at the current mouse position
pub fn draw_mouse_cursor(buffer: &mut DoubleBufferedFrameBuffer) {
    // Get current mouse position
    let (x, y, _buttons) = crate::drivers::mouse::get_state();
    
    // Draw the cursor
    let mut cursor = MOUSE_CURSOR.lock();
    cursor.draw(buffer, x, y);
}