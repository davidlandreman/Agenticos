use core::panic::PanicInfo;
use crate::drivers::display::text_buffer;
use crate::graphics::color::Color;
use crate::{debug_error, println};

#[cfg(not(feature = "test"))]
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

#[cfg(feature = "test")]
#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    use crate::lib::test_utils::{exit_qemu, QemuExitCode};
    
    debug_error!("TEST PANIC: {}", info);
    
    // Exit QEMU with failure code for tests
    exit_qemu(QemuExitCode::Failed);
    
    // Just in case QEMU doesn't exit
    loop {}
}