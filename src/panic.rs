use core::panic::PanicInfo;
use crate::drivers::display::text_buffer;
use crate::graphics::color::Color;
use crate::{debug_error, println};

#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    debug_error!("KERNEL PANIC: {}", info);
    
    // Try to display panic on screen if text buffer is available
    text_buffer::set_color(Color::RED);
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);
    
    loop {}
}