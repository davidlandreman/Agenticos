// Unified display module that automatically selects between single and double buffering

use crate::graphics::color::Color;
use core::fmt;

// Configuration: Set to true to enable double buffering
pub const USE_DOUBLE_BUFFER: bool = true;

// Unified print function that routes to the appropriate implementation
pub fn _print(args: fmt::Arguments) {
    if USE_DOUBLE_BUFFER {
        super::double_buffered_text::_print(args);
    } else {
        super::text_buffer::_print(args);
    }
}

// Unified color setting function
pub fn set_color(color: Color) {
    if USE_DOUBLE_BUFFER {
        super::double_buffered_text::set_color(color);
    } else {
        super::text_buffer::set_color(color);
    }
}

// Export the macros that use the unified print function
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::drivers::display::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}