//! `GraphicsDevice` adapter that rasterizes existing widgets into a surface.

use bootloader_api::info::PixelFormat;

use crate::graphics::color::Color;
use crate::graphics::surface::{PremulArgb, Surface};
use crate::window::{ColorDepth, GraphicsDevice, Rect};

pub struct SurfaceCanvas<'a> {
    surface: &'a mut Surface,
    origin_x: i32,
    origin_y: i32,
    logical_width: usize,
    logical_height: usize,
    clip_rect: Option<Rect>,
}

impl<'a> SurfaceCanvas<'a> {
    pub fn new(surface: &'a mut Surface, origin: (i32, i32), logical_size: (usize, usize)) -> Self {
        Self {
            surface,
            origin_x: origin.0,
            origin_y: origin.1,
            logical_width: logical_size.0,
            logical_height: logical_size.1,
            clip_rect: None,
        }
    }

    fn local(&self, x: i32, y: i32) -> Option<(u32, u32)> {
        if let Some(clip) = self.clip_rect {
            if !clip.contains_point(crate::window::Point::new(x, y)) {
                return None;
            }
        }
        let x = x - self.origin_x;
        let y = y - self.origin_y;
        if x < 0 || y < 0 || x >= self.surface.width() as i32 || y >= self.surface.height() as i32 {
            return None;
        }
        Some((x as u32, y as u32))
    }
}

impl GraphicsDevice for SurfaceCanvas<'_> {
    fn width(&self) -> usize {
        self.logical_width
    }
    fn height(&self) -> usize {
        self.logical_height
    }
    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32
    }

    fn clear(&mut self, color: Color) {
        self.surface.clear(
            Rect::new(0, 0, self.surface.width(), self.surface.height()),
            PremulArgb::from_rgba(color.red, color.green, color.blue, u8::MAX),
        );
    }

    fn draw_pixel(&mut self, x: i32, y: i32, color: Color) {
        if let Some((x, y)) = self.local(x, y) {
            self.surface.set_pixel(
                x,
                y,
                PremulArgb::from_rgba(color.red, color.green, color.blue, u8::MAX),
            );
        }
    }

    fn read_pixel(&self, x: i32, y: i32) -> Color {
        let Some((x, y)) = self.local(x, y) else {
            return Color::BLACK;
        };
        let Some(pixel) = self.surface.pixel(x, y) else {
            return Color::BLACK;
        };
        let (r, g, b, _) = pixel.to_rgba();
        Color::new(r, g, b)
    }

    fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Color) {
        let dx = (x2 - x1).abs();
        let dy = -(y2 - y1).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let (mut x, mut y, mut err) = (x1, y1, dx + dy);
        loop {
            self.draw_pixel(x, y, color);
            if x == x2 && y == y2 {
                break;
            }
            let twice = err.saturating_mul(2);
            if twice >= dy {
                err += dy;
                x += sx;
            }
            if twice <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
        let right = x.saturating_add(width as i32 - 1);
        let bottom = y.saturating_add(height as i32 - 1);
        self.draw_line(x, y, right, y, color);
        self.draw_line(right, y, right, bottom, color);
        self.draw_line(right, bottom, x, bottom, color);
        self.draw_line(x, bottom, x, y, color);
    }

    fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        for py in y..y.saturating_add(height as i32) {
            for px in x..x.saturating_add(width as i32) {
                self.draw_pixel(px, py, color);
            }
        }
    }

    fn set_clip_rect(&mut self, rect: Option<Rect>) {
        self.clip_rect = rect;
    }
    fn flush(&mut self) {}
    fn pixel_format(&self) -> PixelFormat {
        PixelFormat::Bgr
    }
    fn bytes_per_pixel(&self) -> usize {
        4
    }
    fn stride(&self) -> usize {
        self.surface.width() as usize
    }
}
