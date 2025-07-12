#![no_std]
#![no_main]

mod vga_buffer;
mod memory;

use core::panic::PanicInfo;
use bootloader_api::{entry_point, BootInfo};
use qemu_print::qemu_println;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // Debug output to QEMU serial console
    qemu_println!("=== AgenticOS Kernel Starting ===");
    qemu_println!("Kernel entry point reached successfully!");
    qemu_println!("Boot info address: {:p}", boot_info);
    
    // Initialize memory manager
    memory::init(&boot_info.memory_regions, boot_info.physical_memory_offset.into_option());
    
    // Print memory information through memory manager
    memory::print_memory_info();
    
    // Print other boot information
    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        qemu_println!("\nRSDP address: 0x{:016x}", rsdp_addr);
    }
    
    // Get memory statistics
    let stats = memory::get_memory_stats();
    qemu_println!("\nMemory manager reports {} MB of usable memory available", 
        stats.usable_memory / (1024 * 1024));
    
    // Clear the screen and print Hello World
    qemu_println!("\nInitializing VGA buffer...");
    vga_buffer::print_hello();
    qemu_println!("VGA buffer initialized and Hello World printed!");
    
    qemu_println!("\nKernel initialization complete. Entering idle loop...");
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    qemu_println!("KERNEL PANIC: {}", info);
    loop {}
}
