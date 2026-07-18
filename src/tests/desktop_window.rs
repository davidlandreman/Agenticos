//! Tests for `DesktopWindow` paint behavior — wallpaper present, wallpaper
//! absent, and graceful fallback when wallpaper bytes fail to parse.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::windows::DesktopWindow;
use crate::window::{ColorDepth, GraphicsDevice, Rect, Window, WindowId};

/// Recording graphics device — counts the calls each `paint` triggers without
/// caring about pixel-level fidelity (the trait defaults are exercised by
/// `graphics_device_image.rs`).
#[derive(Default)]
struct RecordingDevice {
    fill_rect_calls: u32,
    draw_image_scaled_calls: u32,
    last_fill_color: Option<Color>,
    last_image_bounds: Option<(i32, i32, u32, u32)>,
}

impl GraphicsDevice for RecordingDevice {
    fn width(&self) -> usize {
        1280
    }
    fn height(&self) -> usize {
        720
    }
    fn color_depth(&self) -> ColorDepth {
        ColorDepth::Bit32
    }

    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color {
        Color::BLACK
    }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}

    fn fill_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, color: Color) {
        self.fill_rect_calls += 1;
        self.last_fill_color = Some(color);
    }

    // Override the trait default to record the call without walking the
    // image — the trait default is unit-tested in `graphics_device_image.rs`.
    fn draw_image_scaled(
        &mut self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        _image: &dyn crate::graphics::images::Image,
    ) {
        self.draw_image_scaled_calls += 1;
        self.last_image_bounds = Some((x, y, width, height));
    }

    fn set_clip_rect(&mut self, _rect: Option<Rect>) {}
    fn flush(&mut self) {}
}

/// Build a minimal valid 1x1 24-bit BMP (`Color::new(0xAA, 0xBB, 0xCC)`).
fn tiny_bmp() -> Vec<u8> {
    let data_offset: u32 = 14 + 40;
    // 1 row, 1 px * 3 bytes BGR, padded to 4-byte stride.
    let row_size: u32 = 4;
    let pixel_bytes: u32 = row_size; // one row
    let file_size: u32 = data_offset + pixel_bytes;

    let mut bmp = Vec::with_capacity(file_size as usize);
    // -- BITMAPFILEHEADER --
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes()); // reserved
    bmp.extend_from_slice(&data_offset.to_le_bytes());
    // -- BITMAPINFOHEADER (40) --
    bmp.extend_from_slice(&40u32.to_le_bytes()); // header size
    bmp.extend_from_slice(&1i32.to_le_bytes()); // width
    bmp.extend_from_slice(&1i32.to_le_bytes()); // height (positive = bottom-up)
    bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
    bmp.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    bmp.extend_from_slice(&0u32.to_le_bytes()); // compression
    bmp.extend_from_slice(&pixel_bytes.to_le_bytes()); // image size
    bmp.extend_from_slice(&0i32.to_le_bytes()); // x ppm
    bmp.extend_from_slice(&0i32.to_le_bytes()); // y ppm
    bmp.extend_from_slice(&0u32.to_le_bytes()); // colors used
    bmp.extend_from_slice(&0u32.to_le_bytes()); // colors important
                                                // -- pixel data: BGR, padded --
    bmp.extend_from_slice(&[0xCC, 0xBB, 0xAA, 0x00]);
    bmp
}

// -- tests ----------------------------------------------------------------

fn test_paint_with_wallpaper_calls_draw_image_scaled() {
    let bounds = Rect::new(0, 0, 1280, 720);
    let mut desktop = DesktopWindow::new_with_wallpaper(WindowId::new(), bounds, tiny_bmp());
    let mut device = RecordingDevice::default();

    desktop.paint(&mut device);

    assert_eq!(device.draw_image_scaled_calls, 1);
    assert_eq!(device.fill_rect_calls, 0);
    assert_eq!(device.last_image_bounds, Some((0, 0, 1280, 720)));
}

fn test_paint_without_wallpaper_falls_back_to_fill_rect() {
    let bounds = Rect::new(0, 0, 1280, 720);
    let mut desktop = DesktopWindow::new(WindowId::new(), bounds);
    let mut device = RecordingDevice::default();

    desktop.paint(&mut device);

    assert_eq!(device.draw_image_scaled_calls, 0);
    assert_eq!(device.fill_rect_calls, 1);
    assert_eq!(device.last_fill_color, Some(Color::new(0, 50, 100)));
}

fn test_paint_with_malformed_wallpaper_falls_back_to_fill_rect() {
    let bounds = Rect::new(0, 0, 1280, 720);
    // 16 bytes of 0xFF is too short to be a BMP header — `BmpImage::from_bytes`
    // returns `Err`, and paint should silently fall back.
    let garbage = vec![0xFFu8; 16];
    let mut desktop = DesktopWindow::new_with_wallpaper(WindowId::new(), bounds, garbage);
    let mut device = RecordingDevice::default();

    desktop.paint(&mut device);

    assert_eq!(device.draw_image_scaled_calls, 0);
    assert_eq!(device.fill_rect_calls, 1);
    assert_eq!(device.last_fill_color, Some(Color::new(0, 50, 100)));
}

fn test_paint_clears_needs_repaint_for_wallpaper_branch() {
    let bounds = Rect::new(0, 0, 1280, 720);
    let mut desktop = DesktopWindow::new_with_wallpaper(WindowId::new(), bounds, tiny_bmp());
    let mut device = RecordingDevice::default();

    assert!(
        desktop.needs_repaint(),
        "newly-created desktop should need a repaint"
    );
    desktop.paint(&mut device);
    assert!(!desktop.needs_repaint(), "paint must clear the dirty flag");
}

fn test_paint_clears_needs_repaint_for_fallback_branch() {
    let bounds = Rect::new(0, 0, 1280, 720);
    let mut desktop = DesktopWindow::new(WindowId::new(), bounds);
    let mut device = RecordingDevice::default();

    desktop.paint(&mut device);
    assert!(!desktop.needs_repaint());
}

fn test_repeated_paint_without_invalidate_redraws() {
    let bounds = Rect::new(0, 0, 1280, 720);
    let mut desktop = DesktopWindow::new_with_wallpaper(WindowId::new(), bounds, tiny_bmp());
    let mut device = RecordingDevice::default();

    desktop.paint(&mut device);
    desktop.paint(&mut device);
    desktop.paint(&mut device);

    // Per the `Window::paint` contract, the compositor decides whether to
    // call paint(); the window does not second-guess via needs_repaint.
    // Each call writes pixels (clipped by the device). Cost-control for
    // Desktop happens through the backing-store path
    // (`paint_into_backing_store`), not by skipping paint() itself.
    assert_eq!(device.draw_image_scaled_calls, 3);
}

// -- registration ---------------------------------------------------------

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_paint_with_wallpaper_calls_draw_image_scaled,
        &test_paint_without_wallpaper_falls_back_to_fill_rect,
        &test_paint_with_malformed_wallpaper_falls_back_to_fill_rect,
        &test_paint_clears_needs_repaint_for_wallpaper_branch,
        &test_paint_clears_needs_repaint_for_fallback_branch,
        &test_repeated_paint_without_invalidate_redraws,
    ]
}
