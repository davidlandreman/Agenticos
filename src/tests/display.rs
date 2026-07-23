use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::lib::test_utils::Testable;
use crate::println;
use crate::window::cursor::{sprite, sprite_is_valid, CursorIcon, CursorRenderer};
use crate::window::{Point, Rect};

fn test_display_colors() {
    display::set_color(Color::RED);
    println!("This text should be red");
    display::set_color(Color::GREEN);
    println!("This text should be green");
    display::set_color(Color::BLUE);
    println!("This text should be blue");
    display::set_color(Color::WHITE);
}

fn test_color_values() {
    assert_eq!(Color::RED.red, 255);
    assert_eq!(Color::RED.green, 0);
    assert_eq!(Color::RED.blue, 0);

    assert_eq!(Color::GREEN.red, 0);
    assert_eq!(Color::GREEN.green, 255);
    assert_eq!(Color::GREEN.blue, 0);

    assert_eq!(Color::BLUE.red, 0);
    assert_eq!(Color::BLUE.green, 0);
    assert_eq!(Color::BLUE.blue, 255);

    assert_eq!(Color::WHITE.red, 255);
    assert_eq!(Color::WHITE.green, 255);
    assert_eq!(Color::WHITE.blue, 255);

    assert_eq!(Color::BLACK.red, 0);
    assert_eq!(Color::BLACK.green, 0);
    assert_eq!(Color::BLACK.blue, 0);
}

fn test_color_print() {
    display::set_color(Color::CYAN);
    println!("Testing cyan color");
    display::set_color(Color::MAGENTA);
    println!("Testing magenta color");
    display::set_color(Color::YELLOW);
    println!("Testing yellow color");
    display::set_color(Color::GRAY);
    println!("Testing gray color");
    display::set_color(Color::WHITE);
}

fn test_cursor_sprites_and_hotspots() {
    for icon in [CursorIcon::Arrow, CursorIcon::Wait, CursorIcon::Text] {
        assert!(sprite_is_valid(icon));
        let image = sprite(icon);
        let position = Point::new(100, 80);
        assert_eq!(
            CursorRenderer::bounds_at(icon, position),
            Rect::new(
                position.x - i32::from(image.hot_x),
                position.y - i32::from(image.hot_y),
                u32::from(image.width),
                u32::from(image.height),
            )
        );
        let hardware = CursorRenderer::hardware_argb_64(icon);
        assert_eq!(hardware.len(), 64 * 64);
        assert!(hardware.iter().any(|pixel| *pixel == 0xff00_0000));
        assert!(hardware.iter().any(|pixel| *pixel == 0xffff_ffff));
        assert!(hardware.iter().any(|pixel| *pixel == 0));
    }
    assert_eq!(CursorRenderer::hotspot(CursorIcon::Arrow), (0, 0));
    assert_ne!(CursorRenderer::hotspot(CursorIcon::Wait), (0, 0));
    assert_ne!(CursorRenderer::hotspot(CursorIcon::Text), (0, 0));
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_display_colors,
        &test_color_values,
        &test_color_print,
        &test_cursor_sprites_and_hotspots,
    ]
}
