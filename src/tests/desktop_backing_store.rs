//! Tests for `DesktopWindow`'s opt-in backing-store path
//! (`wants_backing_store`, `paint_into_backing_store`, `backing_store`).
//!
//! Reuses the `tiny_bmp()` helper shape from `desktop_window.rs` to keep
//! BMP construction in one place mentally. A small `FakeDevice` provides
//! GraphicsDevice metadata (pixel format, dimensions) without doing
//! anything observable on draw calls — `paint_into_backing_store` only
//! reads metadata from the device.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use bootloader_api::info::PixelFormat;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::windows::DesktopWindow;
use crate::window::{ColorDepth, GraphicsDevice, Rect, Window, WindowId};

struct FakeDevice {
    width: usize,
    height: usize,
    pixel_format: PixelFormat,
    bytes_per_pixel: usize,
}

impl FakeDevice {
    fn bgr(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixel_format: PixelFormat::Bgr,
            bytes_per_pixel: 4,
        }
    }
}

impl GraphicsDevice for FakeDevice {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }
    fn color_depth(&self) -> ColorDepth { ColorDepth::Bit32 }
    fn clear(&mut self, _color: Color) {}
    fn draw_pixel(&mut self, _x: i32, _y: i32, _color: Color) {}
    fn read_pixel(&self, _x: i32, _y: i32) -> Color { Color::BLACK }
    fn draw_line(&mut self, _x1: i32, _y1: i32, _x2: i32, _y2: i32, _color: Color) {}
    fn draw_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn fill_rect(&mut self, _x: i32, _y: i32, _width: u32, _height: u32, _color: Color) {}
    fn set_clip_rect(&mut self, _rect: Option<Rect>) {}
    fn flush(&mut self) {}
    fn pixel_format(&self) -> PixelFormat { self.pixel_format }
    fn bytes_per_pixel(&self) -> usize { self.bytes_per_pixel }
}

/// Solid-color 1x1 BMP whose pixel is RGB (0xAA, 0xBB, 0xCC).
/// Same shape as the tiny_bmp() in desktop_window.rs.
fn tiny_bmp() -> Vec<u8> {
    let data_offset: u32 = 14 + 40;
    let row_size: u32 = 4;
    let pixel_bytes: u32 = row_size;
    let file_size: u32 = data_offset + pixel_bytes;

    let mut bmp = Vec::with_capacity(file_size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&data_offset.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&1i32.to_le_bytes());
    bmp.extend_from_slice(&1i32.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&pixel_bytes.to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0i32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    // BMP pixel storage is BGR; this represents RGB(0xAA, 0xBB, 0xCC).
    bmp.extend_from_slice(&[0xCC, 0xBB, 0xAA, 0x00]);
    bmp
}

// ---- core opt-in surface ------------------------------------------------

fn test_desktop_opts_in_to_backing_store() {
    let bounds = Rect::new(0, 0, 100, 50);
    let desktop = DesktopWindow::new(WindowId::new(), bounds);
    assert!(desktop.wants_backing_store());
    // No backing store yet — lazy.
    assert!(desktop.backing_store().is_none());
}

fn test_paint_into_backing_store_allocates_and_rasterizes_wallpaper() {
    let bounds = Rect::new(0, 0, 100, 50);
    let mut desktop = DesktopWindow::new_with_wallpaper(
        WindowId::new(), bounds, tiny_bmp(),
    );
    let device = FakeDevice::bgr(800, 600);

    desktop.paint_into_backing_store(&device);

    let buf = desktop.backing_store().expect("backing store should exist after rasterize");
    assert_eq!(buf.width, 100);
    assert_eq!(buf.height, 50);
    assert_eq!(buf.pixel_format, PixelFormat::Bgr);
    assert_eq!(buf.bytes_per_pixel, 4);

    // Wallpaper is 1x1 with color (0xAA, 0xBB, 0xCC). Nearest-neighbor scale
    // to 100x50 means every pixel is that color. Sample a few in BGR order.
    let stride = buf.stride_pixels * buf.bytes_per_pixel;
    assert_eq!(buf.pixels[0], 0xCC); // B
    assert_eq!(buf.pixels[1], 0xBB); // G
    assert_eq!(buf.pixels[2], 0xAA); // R
    let mid = stride * 25 + buf.bytes_per_pixel * 50;
    assert_eq!(buf.pixels[mid], 0xCC);
    assert_eq!(buf.pixels[mid + 1], 0xBB);
    assert_eq!(buf.pixels[mid + 2], 0xAA);
}

fn test_paint_into_backing_store_solid_fallback_when_no_wallpaper() {
    let bounds = Rect::new(0, 0, 32, 16);
    let mut desktop = DesktopWindow::new(WindowId::new(), bounds);
    let device = FakeDevice::bgr(800, 600);

    desktop.paint_into_backing_store(&device);

    let buf = desktop.backing_store().expect("backing store should exist");
    // Solid blue background = Color::new(0, 50, 100). In BGR byte order:
    // [blue=100, green=50, red=0].
    assert_eq!(buf.pixels[0], 100);
    assert_eq!(buf.pixels[1], 50);
    assert_eq!(buf.pixels[2], 0);
}

fn test_paint_into_backing_store_falls_back_on_malformed_wallpaper() {
    let bounds = Rect::new(0, 0, 32, 16);
    let mut desktop = DesktopWindow::new_with_wallpaper(
        WindowId::new(), bounds, vec![0xFFu8; 16],  // garbage too short to parse
    );
    let device = FakeDevice::bgr(800, 600);

    desktop.paint_into_backing_store(&device);

    let buf = desktop.backing_store().expect("backing store should exist");
    // Falls back to solid blue.
    assert_eq!(buf.pixels[0], 100);
    assert_eq!(buf.pixels[1], 50);
    assert_eq!(buf.pixels[2], 0);
}

fn test_rasterization_clears_needs_repaint() {
    let bounds = Rect::new(0, 0, 32, 16);
    let mut desktop = DesktopWindow::new_with_wallpaper(
        WindowId::new(), bounds, tiny_bmp(),
    );
    let device = FakeDevice::bgr(800, 600);

    assert!(desktop.needs_repaint(), "fresh desktop needs initial repaint");
    desktop.paint_into_backing_store(&device);
    assert!(!desktop.needs_repaint(),
        "rasterizing into the backing store satisfies the repaint contract — \
         the compositor only re-rasterizes when needs_repaint becomes true again");
}

fn test_invalidate_re_arms_rasterization() {
    let bounds = Rect::new(0, 0, 32, 16);
    let mut desktop = DesktopWindow::new_with_wallpaper(
        WindowId::new(), bounds, tiny_bmp(),
    );
    let device = FakeDevice::bgr(800, 600);

    desktop.paint_into_backing_store(&device);
    assert!(!desktop.needs_repaint());

    desktop.invalidate();
    assert!(desktop.needs_repaint());
}

fn test_resize_replaces_backing_store() {
    let mut desktop = DesktopWindow::new_with_wallpaper(
        WindowId::new(), Rect::new(0, 0, 32, 16), tiny_bmp(),
    );
    let device = FakeDevice::bgr(800, 600);

    desktop.paint_into_backing_store(&device);
    assert_eq!(desktop.backing_store().unwrap().width, 32);
    assert_eq!(desktop.backing_store().unwrap().height, 16);

    desktop.set_bounds(Rect::new(0, 0, 64, 32));
    assert!(desktop.needs_repaint(), "set_bounds must invalidate");

    desktop.paint_into_backing_store(&device);
    let buf = desktop.backing_store().unwrap();
    assert_eq!(buf.width, 64);
    assert_eq!(buf.height, 32);
    // Still rasterized (not stale zeroed pixels).
    assert_eq!(buf.pixels[0], 0xCC);
    assert_eq!(buf.pixels[1], 0xBB);
    assert_eq!(buf.pixels[2], 0xAA);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_desktop_opts_in_to_backing_store,
        &test_paint_into_backing_store_allocates_and_rasterizes_wallpaper,
        &test_paint_into_backing_store_solid_fallback_when_no_wallpaper,
        &test_paint_into_backing_store_falls_back_on_malformed_wallpaper,
        &test_rasterization_clears_needs_repaint,
        &test_invalidate_re_arms_rasterization,
        &test_resize_replaces_backing_store,
    ]
}
