//! Tests for `WindowBuffer` — the framebuffer-native backing store
//! repurposed for the opt-in compositor (Phase 2 of the rendering
//! refactor).

use bootloader_api::info::PixelFormat;

use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::window::WindowBuffer;

fn test_new_allocates_correct_byte_length() {
    let buf = WindowBuffer::new(8, 4, PixelFormat::Rgb, 4);
    assert_eq!(buf.width, 8);
    assert_eq!(buf.height, 4);
    assert_eq!(buf.bytes_per_pixel, 4);
    assert_eq!(buf.stride_pixels, 8);
    assert_eq!(buf.pixel_format, PixelFormat::Rgb);
    assert_eq!(buf.byte_len(), 8 * 4 * 4);
    assert_eq!(buf.pixels.len(), 8 * 4 * 4);
    assert!(buf.pixels.iter().all(|&b| b == 0));
}

fn test_write_pixel_rgb_byte_order() {
    let mut buf = WindowBuffer::new(2, 2, PixelFormat::Rgb, 4);
    buf.write_pixel(0, 0, Color::new(0xAA, 0xBB, 0xCC));
    // Rgb: red, green, blue, _padding_
    assert_eq!(buf.pixels[0], 0xAA);
    assert_eq!(buf.pixels[1], 0xBB);
    assert_eq!(buf.pixels[2], 0xCC);
}

fn test_write_pixel_bgr_byte_order() {
    let mut buf = WindowBuffer::new(2, 2, PixelFormat::Bgr, 4);
    buf.write_pixel(0, 0, Color::new(0xAA, 0xBB, 0xCC));
    // Bgr: blue, green, red, _padding_
    assert_eq!(buf.pixels[0], 0xCC);
    assert_eq!(buf.pixels[1], 0xBB);
    assert_eq!(buf.pixels[2], 0xAA);
}

fn test_write_pixel_offset_row_indexing() {
    // 4 wide, 4 high. Pixel at (1, 2) lives at byte offset (2 * 4 + 1) * 4 = 36.
    let mut buf = WindowBuffer::new(4, 4, PixelFormat::Rgb, 4);
    buf.write_pixel(1, 2, Color::new(0x10, 0x20, 0x30));
    assert_eq!(buf.pixels[36], 0x10);
    assert_eq!(buf.pixels[37], 0x20);
    assert_eq!(buf.pixels[38], 0x30);
    // Surrounding pixels untouched.
    assert_eq!(buf.pixels[0], 0);
    assert_eq!(buf.pixels[35], 0);
    assert_eq!(buf.pixels[39], 0);
    assert_eq!(buf.pixels[40], 0);
}

fn test_write_pixel_out_of_range_is_silent_noop() {
    let mut buf = WindowBuffer::new(4, 4, PixelFormat::Rgb, 4);
    buf.write_pixel(99, 0, Color::new(0xAA, 0xBB, 0xCC));
    buf.write_pixel(0, 99, Color::new(0xAA, 0xBB, 0xCC));
    assert!(buf.pixels.iter().all(|&b| b == 0));
}

fn test_resize_to_same_dims_does_not_reallocate() {
    let mut buf = WindowBuffer::new(8, 4, PixelFormat::Bgr, 4);
    buf.write_pixel(0, 0, Color::new(0xAA, 0xBB, 0xCC));
    let allocated = buf.resize_to(8, 4);
    assert!(!allocated, "resize to same dims must not reallocate");
    // Pixels preserved.
    assert_eq!(buf.pixels[0], 0xCC);
    assert_eq!(buf.pixels[1], 0xBB);
    assert_eq!(buf.pixels[2], 0xAA);
}

fn test_resize_to_different_dims_reallocates_and_zeroes() {
    let mut buf = WindowBuffer::new(8, 4, PixelFormat::Bgr, 4);
    buf.write_pixel(0, 0, Color::new(0xAA, 0xBB, 0xCC));
    let allocated = buf.resize_to(16, 8);
    assert!(allocated);
    assert_eq!(buf.width, 16);
    assert_eq!(buf.height, 8);
    assert_eq!(buf.byte_len(), 16 * 8 * 4);
    assert_eq!(buf.pixels.len(), 16 * 8 * 4);
    assert!(buf.pixels.iter().all(|&b| b == 0));
}

fn test_row_bytes_returns_correct_slice() {
    let mut buf = WindowBuffer::new(4, 3, PixelFormat::Rgb, 4);
    buf.write_pixel(0, 1, Color::new(0xDE, 0xAD, 0xBE));
    buf.write_pixel(3, 1, Color::new(0xCA, 0xFE, 0xEE));

    let row = buf.row_bytes(1);
    assert_eq!(row.len(), 4 * 4);
    // Pixel 0 of row 1: 0xDE, 0xAD, 0xBE.
    assert_eq!(row[0], 0xDE);
    assert_eq!(row[1], 0xAD);
    assert_eq!(row[2], 0xBE);
    // Pixel 3 of row 1 starts at byte 12.
    assert_eq!(row[12], 0xCA);
    assert_eq!(row[13], 0xFE);
    assert_eq!(row[14], 0xEE);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_new_allocates_correct_byte_length,
        &test_write_pixel_rgb_byte_order,
        &test_write_pixel_bgr_byte_order,
        &test_write_pixel_offset_row_indexing,
        &test_write_pixel_out_of_range_is_silent_noop,
        &test_resize_to_same_dims_does_not_reallocate,
        &test_resize_to_different_dims_reallocates_and_zeroes,
        &test_row_bytes_returns_correct_slice,
    ]
}
