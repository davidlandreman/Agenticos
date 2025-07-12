use crate::color::Color;
use crate::frame_buffer::FrameBufferWriter;
use core::cmp::{max, min};

pub struct Graphics<'a> {
    frame_buffer: &'a mut FrameBufferWriter,
    stroke_color: Color,
    fill_color: Color,
    stroke_width: usize,
}

impl<'a> Graphics<'a> {
    pub fn new(frame_buffer: &'a mut FrameBufferWriter) -> Self {
        Self {
            frame_buffer,
            stroke_color: Color::WHITE,
            fill_color: Color::WHITE,
            stroke_width: 1,
        }
    }
    
    pub fn set_stroke_color(&mut self, color: Color) {
        self.stroke_color = color;
    }
    
    pub fn set_fill_color(&mut self, color: Color) {
        self.fill_color = color;
    }
    
    pub fn set_stroke_width(&mut self, width: usize) {
        self.stroke_width = width;
    }
    
    // Basic pixel drawing
    pub fn draw_pixel(&mut self, x: usize, y: usize, color: Color) {
        self.frame_buffer.draw_pixel(x, y, color);
    }
    
    // Line drawing using Bresenham's algorithm
    pub fn draw_line(&mut self, x0: isize, y0: isize, x1: isize, y1: isize) {
        self.draw_line_with_color(x0, y0, x1, y1, self.stroke_color);
    }
    
    pub fn draw_line_with_color(&mut self, x0: isize, y0: isize, x1: isize, y1: isize, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut x = x0;
        let mut y = y0;
        
        loop {
            if x >= 0 && y >= 0 {
                if self.stroke_width == 1 {
                    self.draw_pixel(x as usize, y as usize, color);
                } else {
                    // Draw a filled circle for thick lines
                    self.fill_circle_internal(x as usize, y as usize, self.stroke_width / 2, color);
                }
            }
            
            if x == x1 && y == y1 {
                break;
            }
            
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }
    
    // Rectangle drawing
    pub fn draw_rect(&mut self, x: usize, y: usize, width: usize, height: usize) {
        self.draw_rect_with_color(x, y, width, height, self.stroke_color);
    }
    
    pub fn draw_rect_with_color(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        // Top and bottom edges
        for i in 0..width {
            for t in 0..self.stroke_width {
                if y + t < self.frame_buffer.get_dimensions().1 {
                    self.draw_pixel(x + i, y + t, color);
                }
                if y + height > t && y + height - t - 1 < self.frame_buffer.get_dimensions().1 {
                    self.draw_pixel(x + i, y + height - t - 1, color);
                }
            }
        }
        
        // Left and right edges
        for i in 0..height {
            for t in 0..self.stroke_width {
                if x + t < self.frame_buffer.get_dimensions().0 {
                    self.draw_pixel(x + t, y + i, color);
                }
                if x + width > t && x + width - t - 1 < self.frame_buffer.get_dimensions().0 {
                    self.draw_pixel(x + width - t - 1, y + i, color);
                }
            }
        }
    }
    
    pub fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize) {
        self.frame_buffer.fill_rect(x, y, width, height, self.fill_color);
    }
    
    pub fn fill_rect_with_color(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color) {
        self.frame_buffer.fill_rect(x, y, width, height, color);
    }
    
    // Circle drawing using midpoint algorithm
    pub fn draw_circle(&mut self, center_x: usize, center_y: usize, radius: usize) {
        self.draw_circle_with_color(center_x, center_y, radius, self.stroke_color);
    }
    
    pub fn draw_circle_with_color(&mut self, center_x: usize, center_y: usize, radius: usize, color: Color) {
        let mut x = radius as isize;
        let mut y = 0isize;
        let mut err = 0isize;
        
        while x >= y {
            for t in 0..self.stroke_width {
                let r = radius as isize + t as isize;
                self.draw_circle_points(center_x as isize, center_y as isize, x + t as isize, y, color);
                self.draw_circle_points(center_x as isize, center_y as isize, y, x + t as isize, color);
            }
            
            y += 1;
            if err <= 0 {
                err += 2 * y + 1;
            }
            if err > 0 {
                x -= 1;
                err -= 2 * x + 1;
            }
        }
    }
    
    fn draw_circle_points(&mut self, cx: isize, cy: isize, x: isize, y: isize, color: Color) {
        if cx + x >= 0 && cy + y >= 0 {
            self.draw_pixel((cx + x) as usize, (cy + y) as usize, color);
        }
        if cx - x >= 0 && cy + y >= 0 {
            self.draw_pixel((cx - x) as usize, (cy + y) as usize, color);
        }
        if cx + x >= 0 && cy - y >= 0 {
            self.draw_pixel((cx + x) as usize, (cy - y) as usize, color);
        }
        if cx - x >= 0 && cy - y >= 0 {
            self.draw_pixel((cx - x) as usize, (cy - y) as usize, color);
        }
    }
    
    pub fn fill_circle(&mut self, center_x: usize, center_y: usize, radius: usize) {
        self.fill_circle_internal(center_x, center_y, radius, self.fill_color);
    }
    
    fn fill_circle_internal(&mut self, center_x: usize, center_y: usize, radius: usize, color: Color) {
        let mut x = radius as isize;
        let mut y = 0isize;
        let mut err = 0isize;
        
        while x >= y {
            self.draw_horizontal_line(center_x as isize - x, center_x as isize + x, center_y as isize + y, color);
            self.draw_horizontal_line(center_x as isize - x, center_x as isize + x, center_y as isize - y, color);
            self.draw_horizontal_line(center_x as isize - y, center_x as isize + y, center_y as isize + x, color);
            self.draw_horizontal_line(center_x as isize - y, center_x as isize + y, center_y as isize - x, color);
            
            y += 1;
            if err <= 0 {
                err += 2 * y + 1;
            }
            if err > 0 {
                x -= 1;
                err -= 2 * x + 1;
            }
        }
    }
    
    fn draw_horizontal_line(&mut self, x0: isize, x1: isize, y: isize, color: Color) {
        if y < 0 {
            return;
        }
        
        let (width, height) = self.frame_buffer.get_dimensions();
        let y_pos = y as usize;
        if y_pos >= height {
            return;
        }
        
        let start = max(0, min(x0, x1)) as usize;
        let end = min(width as isize - 1, max(x0, x1)) as usize;
        
        for x in start..=end {
            self.draw_pixel(x, y_pos, color);
        }
    }
    
    // Triangle drawing
    pub fn draw_triangle(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, x2: usize, y2: usize) {
        self.draw_line(x0 as isize, y0 as isize, x1 as isize, y1 as isize);
        self.draw_line(x1 as isize, y1 as isize, x2 as isize, y2 as isize);
        self.draw_line(x2 as isize, y2 as isize, x0 as isize, y0 as isize);
    }
    
    pub fn fill_triangle(&mut self, x0: isize, y0: isize, x1: isize, y1: isize, x2: isize, y2: isize) {
        // Sort vertices by y coordinate
        let mut v0 = (x0, y0);
        let mut v1 = (x1, y1);
        let mut v2 = (x2, y2);
        
        if v0.1 > v1.1 {
            core::mem::swap(&mut v0, &mut v1);
        }
        if v1.1 > v2.1 {
            core::mem::swap(&mut v1, &mut v2);
        }
        if v0.1 > v1.1 {
            core::mem::swap(&mut v0, &mut v1);
        }
        
        // Fill triangle using horizontal lines
        let total_height = v2.1 - v0.1;
        if total_height == 0 {
            return;
        }
        
        for y in v0.1..=v2.1 {
            let segment_height = if y <= v1.1 {
                v1.1 - v0.1
            } else {
                v2.1 - v1.1
            };
            
            if segment_height == 0 {
                continue;
            }
            
            let alpha = (y - v0.1) as f32 / total_height as f32;
            let beta = if y <= v1.1 {
                (y - v0.1) as f32 / segment_height as f32
            } else {
                (y - v1.1) as f32 / segment_height as f32
            };
            
            let xa = v0.0 + ((v2.0 - v0.0) as f32 * alpha) as isize;
            let xb = if y <= v1.1 {
                v0.0 + ((v1.0 - v0.0) as f32 * beta) as isize
            } else {
                v1.0 + ((v2.0 - v1.0) as f32 * beta) as isize
            };
            
            self.draw_horizontal_line(xa, xb, y, self.fill_color);
        }
    }
    
    // Polygon drawing
    pub fn draw_polygon(&mut self, points: &[(usize, usize)]) {
        if points.len() < 2 {
            return;
        }
        
        for i in 0..points.len() {
            let next = (i + 1) % points.len();
            self.draw_line(
                points[i].0 as isize,
                points[i].1 as isize,
                points[next].0 as isize,
                points[next].1 as isize,
            );
        }
    }
    
    // Ellipse drawing
    pub fn draw_ellipse(&mut self, center_x: usize, center_y: usize, a: usize, b: usize) {
        self.draw_ellipse_with_color(center_x, center_y, a, b, self.stroke_color);
    }
    
    pub fn draw_ellipse_with_color(&mut self, center_x: usize, center_y: usize, a: usize, b: usize, color: Color) {
        let mut x = a as isize;
        let mut y = 0isize;
        let a2 = (a * a) as isize;
        let b2 = (b * b) as isize;
        let mut err = a2 - b2 * x + b2 / 4;
        
        while b2 * x >= a2 * y {
            self.draw_ellipse_points(center_x as isize, center_y as isize, x, y, color);
            
            y += 1;
            if err < 0 {
                err += a2 * (2 * y + 1);
            } else {
                x -= 1;
                err += a2 * (2 * y + 1) - 2 * b2 * x;
            }
        }
        
        x = 0;
        y = b as isize;
        err = b2 - a2 * y + a2 / 4;
        
        while a2 * y >= b2 * x {
            self.draw_ellipse_points(center_x as isize, center_y as isize, x, y, color);
            
            x += 1;
            if err < 0 {
                err += b2 * (2 * x + 1);
            } else {
                y -= 1;
                err += b2 * (2 * x + 1) - 2 * a2 * y;
            }
        }
    }
    
    fn draw_ellipse_points(&mut self, cx: isize, cy: isize, x: isize, y: isize, color: Color) {
        if cx + x >= 0 && cy + y >= 0 {
            self.draw_pixel((cx + x) as usize, (cy + y) as usize, color);
        }
        if cx - x >= 0 && cy + y >= 0 {
            self.draw_pixel((cx - x) as usize, (cy + y) as usize, color);
        }
        if cx + x >= 0 && cy - y >= 0 {
            self.draw_pixel((cx + x) as usize, (cy - y) as usize, color);
        }
        if cx - x >= 0 && cy - y >= 0 {
            self.draw_pixel((cx - x) as usize, (cy - y) as usize, color);
        }
    }
}