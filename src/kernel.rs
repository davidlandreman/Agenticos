use bootloader_api::BootInfo;
use crate::lib::debug::{self, DebugLevel};
use crate::{debug_info, debug_debug, debug_warn};
use crate::arch::x86_64::interrupts;
use crate::mm::memory;
use crate::drivers::display::{display, text_buffer, double_buffered_text};
use crate::drivers::ps2_controller;
use crate::process::{Process, ShellProcess};

pub fn init(boot_info: &'static mut BootInfo) {
    // Initialize debug subsystem
    debug::init();
    debug::set_debug_level(DebugLevel::Debug);
    
    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_info!("Kernel entry point reached successfully!");
    debug_debug!("Boot info address: {:p}", boot_info);

    // Initialize interrupt descriptor table
    interrupts::init_idt();
    
    // Initialize PS/2 controller configuration for keyboard
    ps2_controller::init();
    
    // Initialize memory manager
    memory::init(&boot_info.memory_regions, boot_info.physical_memory_offset.into_option());
    
    // Print memory information
    memory::print_memory_info();
    
    // Print boot information
    if let Some(rsdp_addr) = boot_info.rsdp_addr.into_option() {
        debug_debug!("RSDP address: 0x{:016x}", rsdp_addr);
    }
    
    // Initialize display
    init_display(boot_info);
    
    // Initialize mouse driver
    crate::drivers::mouse::init();
    
}

fn init_display(boot_info: &'static mut BootInfo) {
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
        
    } else {
        debug_warn!("No framebuffer available from bootloader");
    }
}


pub fn run() -> ! {
    debug_info!("Kernel initialization complete.");

    // Run shell process
    let mut shell_process = ShellProcess::new();
    debug_info!("Running shell process (PID: {})", shell_process.get_id());
    shell_process.run();

    debug_info!("Entering idle loop with mouse cursor...");
    
    // Main kernel loop
    loop {
        // Process any pending keyboard input (outside of interrupt context)
        crate::drivers::keyboard::process_pending_input();
        
        // Draw mouse cursor if double buffering is enabled
        if display::USE_DOUBLE_BUFFER {
            double_buffered_text::with_buffer(|buffer| {
                // Draw the mouse cursor
                crate::graphics::mouse_cursor::draw_mouse_cursor(buffer);
                // Swap buffers to show the cursor
                buffer.swap_buffers();
            });
        }
        
        x86_64::instructions::hlt();
    }
}

