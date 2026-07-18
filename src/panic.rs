#[cfg(not(feature = "test"))]
use crate::drivers::display::text_buffer;
#[cfg(not(feature = "test"))]
use crate::graphics::color::Color;
#[cfg(not(feature = "test"))]
use crate::println;
use core::panic::PanicInfo;

#[cfg(not(feature = "test"))]
#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    crate::arch::x86_64::smp::freeze_other_cpus();
    crate::lib::debug::write_panic_line("[ERROR] ", format_args!("KERNEL PANIC: {}", info));

    // Try to display panic on screen if text buffer is available
    text_buffer::set_color(Color::RED);
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);

    // Freeze this CPU as well; no scheduler or device handler may interleave
    // with panic diagnostics.
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(feature = "test")]
#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    use crate::lib::test_utils::{exit_qemu, QemuExitCode};

    x86_64::instructions::interrupts::disable();
    crate::arch::x86_64::smp::freeze_other_cpus();
    crate::lib::debug::write_panic_line("[ERROR] ", format_args!("TEST PANIC: {}", info));

    // Exit QEMU with failure code for tests
    exit_qemu(QemuExitCode::Failed);

    // Just in case QEMU doesn't exit
    loop {}
}
