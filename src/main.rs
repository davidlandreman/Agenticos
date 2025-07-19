#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod memory;
mod debug;
mod color;
mod frame_buffer;
mod double_buffer;
mod double_buffered_text;
mod core_font;
mod embedded_font;
mod vfnt;
mod truetype_font;
mod font_data;
mod core_text;
mod core_gfx;
mod text_buffer;
mod display;
mod interrupts;

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

    // Initialize interrupt descriptor table
    interrupts::init_idt();
    
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
    
    // Initialize framebuffer if available
    debug_info!("Checking for framebuffer...");
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        if display::USE_DOUBLE_BUFFER {
            debug_info!("Framebuffer found! Initializing double buffered text...");
            double_buffered_text::init(framebuffer);
            debug_info!("Double buffered text initialized successfully!");
        } else {
            debug_info!("Framebuffer found! Initializing text buffer...");
            text_buffer::init(framebuffer);
            debug_info!("Text buffer initialized successfully!");
        }
        
        // Demonstrate the print! and println! macros
        let buffer_type = if display::USE_DOUBLE_BUFFER { " (Double Buffered)" } else { "" };
        println!("Welcome to AgenticOS!{}", buffer_type);
        println!("======================");
        println!();
        
        // Print memory information
        println!("Memory Statistics:");
        println!("  Total usable memory: {} MB", stats.usable_memory / (1024 * 1024));
        println!("  Total memory: {} MB", stats.total_memory / (1024 * 1024));
        println!();
        
        // Demonstrate color support
        display::set_color(color::Color::CYAN);
        println!("This text is in cyan!");
        
        display::set_color(color::Color::GREEN);
        println!("This text is in green!");
        
        display::set_color(color::Color::YELLOW);
        println!("This text is in yellow!");
        
        display::set_color(color::Color::WHITE);
        println!();
        
        // Demonstrate scrolling by printing many lines
        println!("Testing scrolling functionality:");
        println!("================================");
        
        for i in 0..300 {
            display::set_color(if i % 2 == 0 { color::Color::WHITE } else { color::Color::GRAY });
            println!("Line {}: This is a test of the scrolling text buffer", i + 1);
        }
        
        display::set_color(color::Color::MAGENTA);
        println!();
        println!("Scrolling test complete!");
        
        // Demonstrate tab support
        display::set_color(color::Color::WHITE);
        println!();

        println!("Tilde Test: ~");
        println!("Tab test:");
        println!("Column:\t1\t2\t3\t4");
        println!("Value:\tA\tB\tC\tD");
        
        // Final message
        println!();
        display::set_color(color::Color::CYAN);
        println!("AgenticOS kernel initialized successfully!");
        display::set_color(color::Color::WHITE);
        println!("System ready.");
        

        
    } else {
        debug_warn!("No framebuffer available from bootloader");
    }
    
    debug_info!("Kernel initialization complete. Entering idle loop...");
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    debug_error!("KERNEL PANIC: {}", info);
    
    // Try to display panic on screen if text buffer is available
    text_buffer::set_color(color::Color::RED);
    println!();
    println!("!!! KERNEL PANIC !!!");
    println!("{}", info);
    
    loop {}
}