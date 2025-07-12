#![no_std]
#![no_main]

mod vga_buffer;
mod memory;
mod debug;

use core::panic::PanicInfo;
use bootloader_api::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // Initialize debug subsystem
    debug::init();

    // Example: Change debug level to see more detailed output
    debug::set_debug_level(debug::DebugLevel::Trace);
    
    // Debug output to QEMU serial console
    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_info!("Kernel entry point reached successfully!");
    debug_debug!("Boot info address: {:p}", boot_info);

    // Initialize memory manager
    memory::init(&boot_info.memory_regions, boot_info.physical_memory_offset.into_option());
    
    // Print memory information through memory manager
    memory::print_memory_info();
    
    // Print other boot information
    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        debug_debug!("RSDP address: 0x{:016x}", rsdp_addr);
    }
    
    // Get memory statistics
    let stats = memory::get_memory_stats();
    debug_info!("Memory manager reports {} MB of usable memory available", 
        stats.usable_memory / (1024 * 1024));
    
    // Clear the screen and print Hello World
    debug_info!("Initializing VGA buffer...");
    vga_buffer::print_hello();
    debug_info!("VGA buffer initialized and Hello World printed!");

    debug_info!("Kernel initialization complete. Entering idle loop...");
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    debug_error!("KERNEL PANIC: {}", info);
    loop {}
}
