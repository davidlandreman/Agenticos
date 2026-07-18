//! Tests for the no_std SVG icon parser and destination-size rasterizer.

use crate::graphics::color::Color;
use crate::graphics::images::image::{Image, ImageFormat};
use crate::graphics::images::SvgImage;

fn test_svg_dimensions_format_and_transparency() {
    let svg = SvgImage::from_bytes(
        br##"<svg width="8" height="6" viewBox="0 0 8 6">
             <rect x="2" y="1" width="4" height="4" fill="#123456"/>
             </svg>"##,
    )
    .unwrap();
    assert_eq!(svg.width(), 8);
    assert_eq!(svg.height(), 6);
    assert_eq!(svg.format(), ImageFormat::Svg);
    assert_eq!(svg.get_pixel(0, 0), None);
    assert_eq!(svg.get_pixel(3, 2), Some(Color::new(0x12, 0x34, 0x56)));
}

fn test_svg_shapes_obey_document_order() {
    let svg = SvgImage::from_bytes(
        br##"<svg viewBox="0 0 10 10">
             <circle cx="5" cy="5" r="4" fill="#ff0000"/>
             <rect x="4" y="4" width="2" height="2" fill="#0000ff"/>
             </svg>"##,
    )
    .unwrap();
    assert_eq!(svg.get_pixel(5, 5), Some(Color::new(0, 0, 255)));
    assert_eq!(svg.get_pixel(2, 5), Some(Color::new(255, 0, 0)));
}

fn test_svg_scaled_sampling_is_vector_native() {
    let svg = SvgImage::from_bytes(
        br##"<svg width="4" height="4" viewBox="0 0 4 4">
             <circle cx="2" cy="2" r="1" fill="#00aa44"/>
             </svg>"##,
    )
    .unwrap();

    // At native 4x4 this location samples outside the circle. At 40x40 the
    // corresponding high-resolution sample lies inside its curved edge. That
    // distinguishes direct vector sampling from nearest-neighbor enlargement.
    assert_eq!(svg.get_pixel(1, 0), None);
    assert_eq!(
        svg.get_scaled_pixel(16, 11, 40, 40),
        Some(Color::new(0, 0xaa, 0x44))
    );
}

fn test_svg_polygon_path_and_stroke() {
    let svg = SvgImage::from_bytes(
        br##"<svg width="12" height="12" viewBox="0 0 12 12">
             <path d="M 2 10 L 6 2 L 10 10 Z" fill="#ffee00"/>
             <line x1="1" y1="1" x2="11" y2="1" stroke="#112233" stroke-width="2"/>
             </svg>"##,
    )
    .unwrap();
    assert_eq!(svg.get_pixel(6, 6), Some(Color::new(255, 238, 0)));
    assert_eq!(svg.get_pixel(5, 0), Some(Color::new(0x11, 0x22, 0x33)));
}

pub fn get_tests() -> &'static [&'static dyn crate::lib::test_utils::Testable] {
    &[
        &test_svg_dimensions_format_and_transparency,
        &test_svg_shapes_obey_document_order,
        &test_svg_scaled_sampling_is_vector_native,
        &test_svg_polygon_path_and_stroke,
    ]
}
