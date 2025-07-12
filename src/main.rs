#![no_std]
#![no_main]

mod memory;
mod debug;
mod color;
mod frame_buffer;
mod core_text;
mod core_gfx;

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
    
    // Initialize framebuffer if available
    debug_info!("Checking for framebuffer...");
    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        debug_info!("Framebuffer found! Initializing frame buffer driver...");
        let mut fb_writer = frame_buffer::init(framebuffer);
        debug_info!("Frame buffer initialized successfully!");
        
        // Get dimensions before creating text renderer
        let (fb_width, fb_height) = fb_writer.get_dimensions();
        
        // Create a text renderer and display welcome message
        let mut text_renderer = core_text::TextRenderer::with_default_font(&mut fb_writer);
        
        // Set a nice color for the welcome text (cyan)
        text_renderer.set_color(color::Color::CYAN);
        
        // Draw the welcome message centered on screen
        text_renderer.draw_text_centered("Hello", fb_width / 2, fb_height / 2);
        
        // Create graphics renderer and draw some shapes
        let mut graphics = core_gfx::Graphics::new(&mut fb_writer);
        
        // Draw a border around the screen
        graphics.set_stroke_color(color::Color::WHITE);
        graphics.set_stroke_width(2);
        graphics.draw_rect(10, 10, fb_width - 20, fb_height - 20);
        
        // Draw some colorful circles
        graphics.set_fill_color(color::Color::RED);
        graphics.fill_circle(100, 100, 30);
        
        graphics.set_fill_color(color::Color::GREEN);
        graphics.fill_circle(200, 100, 30);
        
        graphics.set_fill_color(color::Color::BLUE);
        graphics.fill_circle(300, 100, 30);
        
        // Draw a triangle
        graphics.set_stroke_color(color::Color::YELLOW);
        graphics.set_stroke_width(3);
        graphics.draw_triangle(150, 200, 250, 200, 200, 300);
        
        // Draw an ellipse
        graphics.set_stroke_color(color::Color::MAGENTA);
        graphics.draw_ellipse(fb_width / 2, fb_height - 100, 80, 40);
        
        debug_info!("Welcome message displayed on frame buffer");
    } else {
        debug_warn!("No framebuffer available from bootloader");
    }
    

    debug_info!("Kernel initialization complete. Entering idle loop...");
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    debug_error!("KERNEL PANIC: {}", info);
    loop {}
}
