use crate::{println};
use crate::lib::test_utils::Testable;
use crate::drivers::display::display;
use crate::graphics::color::Color;

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

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_display_colors,
        &test_color_values,
        &test_color_print,
    ]
}