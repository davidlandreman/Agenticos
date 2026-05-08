use core::panic::PanicInfo;
use crate::drivers::display::text_buffer;
use crate::graphics::color::Color;
use crate::{debug_error, println};

#[cfg(not(feature = "test"))]
#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    // CRITICAL: Force-enable interrupts immediately.
    // If we panicked while holding a lock with interrupts disabled,
    // this ensures the timer keeps running so the system doesn't completely freeze.
    // This allows the watchdog to eventually detect the hung state.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }

    debug_error!("KERNEL PANIC: {}", info);

    // Try to display panic on screen if text buffer is available
    text_buffer::set_color(Color::RED);
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);

    // Loop with hlt to save power and allow interrupts to fire
    loop {
        x86_64::instructions::hlt();
    }
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