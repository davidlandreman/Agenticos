//! Tests for `GraphicsDevice::draw_image` and `draw_image_scaled` defaults.
//!
//! These exercise the trait's default implementations through a minimal
//! recording device that backs `draw_pixel`/`read_pixel` with a heap-allocated
//! pixel grid. The real adapters use the same `draw_pixel` clipping path, so
//! coverage of the trait defaults plus the existing adapter clipping suite in
//! `window_clipping.rs` is sufficient — no need to spin up a fake framebuffer.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::graphics::images::image::{Image, ImageFormat, PixelFormat};
use crate::lib::test_utils::Testable;
use crate::window::{ColorDepth, GraphicsDevice, Rect};

/// Recording graphics device backed by a flat pixel grid.
struct GridDevice {
    width: usize,
    height: usize,
    pixels: Vec<Color>,
    clip: Option<Rect>,
}

impl GridDevice {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![Color::BLACK; width * height],
            clip: None,
        }
    }

    fn pixel_at(&self, x: usize, y: usize) -> Color {
        self.pixels[y * self.width + x]
    }
}

impl GraphicsDevice for GridDevice {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }
    fn color_depth(&self) -> ColorDepth { ColorDepth::Bit32 }

    fn clear(&mut self, color: Color) {
        for px in self.pixels.iter_mut() { *px = color; }
    }

    fn draw_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 { return; }
        let (x, y) = (x as usize, y as usize);
        if x >= self.width || y >= self.height { return; }
        if let Some(clip) = self.clip {
            if (x as i32) < clip.x
                || (y as i32) < clip.y
                || (x as i32) >= clip.x + clip.width as i32
                || (y as i32) >= clip.y + clip.height as i32
            {
                return;
            }
        }
        self.pixels[y * self.width + x] = color;
    }

    fn read_pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || y < 0 { return Color::BLACK; }
        let (x, y) = (x as usize, y as usize);
        if x >= self.width || y >= self.height { return Color::BLACK; }
        self.pixels[y * self.width + x]
    }

    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn fill_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}

    fn set_clip_rect(&mut self, rect: Option<Rect>) { self.clip = rect; }
    fn flush(&mut self) {}
}

/// In-memory image where each pixel encodes its `(x, y)` coordinate as RGB.
struct CoordImage {
    width: usize,
    height: usize,
}

impl CoordImage {
    fn new(width: usize, height: usize) -> Self { Self { width, height } }
}

impl Image for CoordImage {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }
    fn format(&self) -> ImageFormat { ImageFormat::Bmp }
    fn pixel_format(&self) -> PixelFormat { PixelFormat::Rgb888 }

    fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.width || y >= self.height { return None; }
        Some(Color::new(x as u8 + 1, y as u8 + 1, 0xAA))
    }

    fn get_pixel_data(&self) -> &[u8] { &[] }
}

// -- draw_image (1:1) -----------------------------------------------------

fn test_draw_image_blits_pixel_for_pixel() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(4, 4);

    device.draw_image(0, 0, &image);

    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                device.pixel_at(x, y),
                Color::new(x as u8 + 1, y as u8 + 1, 0xAA)
            );
        }
    }
    // Pixels outside the source rect remain unchanged (still BLACK).
    assert_eq!(device.pixel_at(4, 0), Color::BLACK);
    assert_eq!(device.pixel_at(0, 4), Color::BLACK);
}

fn test_draw_image_at_offset() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(2, 2);

    device.draw_image(3, 5, &image);

    assert_eq!(device.pixel_at(3, 5), Color::new(1, 1, 0xAA));
    assert_eq!(device.pixel_at(4, 5), Color::new(2, 1, 0xAA));
    assert_eq!(device.pixel_at(3, 6), Color::new(1, 2, 0xAA));
    assert_eq!(device.pixel_at(4, 6), Color::new(2, 2, 0xAA));
    assert_eq!(device.pixel_at(0, 0), Color::BLACK);
}

fn test_draw_image_negative_origin_clips_top_left() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(4, 4);

    device.draw_image(-2, -2, &image);

    // Source pixels (0,0)..(1,1) land off-screen and are dropped.
    // Source pixel (2, 2) lands at device (0, 0).
    assert_eq!(device.pixel_at(0, 0), Color::new(3, 3, 0xAA));
    assert_eq!(device.pixel_at(1, 1), Color::new(4, 4, 0xAA));
}

fn test_draw_image_beyond_right_edge_clips() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(4, 4);

    device.draw_image(6, 6, &image);

    // Source pixels (0,0)..(1,1) land at device (6,6)..(7,7); the rest fall
    // beyond the right/bottom edge and must be silently dropped.
    assert_eq!(device.pixel_at(6, 6), Color::new(1, 1, 0xAA));
    assert_eq!(device.pixel_at(7, 7), Color::new(2, 2, 0xAA));
    // No panics, no out-of-bounds writes; smoke-checked by reaching here.
}

// -- draw_image_scaled ----------------------------------------------------

fn test_draw_image_scaled_upsamples_nearest_neighbor() {
    let mut device = GridDevice::new(16, 16);
    let image = CoordImage::new(4, 4);

    // Scale 4x4 -> 16x16 (4x in each axis). Each source pixel covers a 4x4
    // destination block.
    device.draw_image_scaled(0, 0, 16, 16, &image);

    for sy in 0..4 {
        for sx in 0..4 {
            let expected = Color::new(sx as u8 + 1, sy as u8 + 1, 0xAA);
            for dy in 0..4 {
                for dx in 0..4 {
                    let px = device.pixel_at(sx * 4 + dx, sy * 4 + dy);
                    assert_eq!(px, expected);
                }
            }
        }
    }
}

fn test_draw_image_scaled_downsamples_nearest_neighbor() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(16, 16);

    // Scale 16x16 -> 4x4: each destination samples every fourth source pixel.
    device.draw_image_scaled(0, 0, 4, 4, &image);

    for dy in 0..4 {
        for dx in 0..4 {
            let sx = (dx as usize * 16) / 4;
            let sy = (dy as usize * 16) / 4;
            assert_eq!(
                device.pixel_at(dx, dy),
                Color::new(sx as u8 + 1, sy as u8 + 1, 0xAA)
            );
        }
    }
    // Pixels beyond the 4x4 destination stay BLACK.
    assert_eq!(device.pixel_at(4, 0), Color::BLACK);
    assert_eq!(device.pixel_at(0, 4), Color::BLACK);
}

fn test_draw_image_scaled_zero_size_is_noop() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(4, 4);

    device.draw_image_scaled(0, 0, 0, 8, &image);
    device.draw_image_scaled(0, 0, 8, 0, &image);

    for px in &device.pixels {
        assert_eq!(*px, Color::BLACK);
    }
}

fn test_draw_image_scaled_zero_source_is_noop() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(0, 4);

    device.draw_image_scaled(0, 0, 8, 8, &image);

    for px in &device.pixels {
        assert_eq!(*px, Color::BLACK);
    }
}

fn test_draw_image_scaled_clips_off_screen() {
    let mut device = GridDevice::new(8, 8);
    let image = CoordImage::new(4, 4);

    // Scale 4x4 -> 8x8 starting at (-4, -4): right/bottom 4x4 of destination
    // lands on device pixels (0..4, 0..4); the rest is clipped.
    device.draw_image_scaled(-4, -4, 8, 8, &image);

    // Sample the dest origin: dx=4, dy=4 -> sx=2, sy=2.
    assert_eq!(device.pixel_at(0, 0), Color::new(3, 3, 0xAA));
    // Should not have written beyond the visible region; pixel (5, 5) on the
    // device corresponds to dx=9 which is outside the 8-wide destination, so
    // it stays BLACK.
    assert_eq!(device.pixel_at(5, 5), Color::BLACK);
}

// -- registration ---------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_draw_image_blits_pixel_for_pixel,
        &test_draw_image_at_offset,
        &test_draw_image_negative_origin_clips_top_left,
        &test_draw_image_beyond_right_edge_clips,
        &test_draw_image_scaled_upsamples_nearest_neighbor,
        &test_draw_image_scaled_downsamples_nearest_neighbor,
        &test_draw_image_scaled_zero_size_is_noop,
        &test_draw_image_scaled_zero_source_is_noop,
        &test_draw_image_scaled_clips_off_screen,
    ]
}
