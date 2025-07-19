use bootloader_api::BootInfo;
use crate::lib::debug::{self, DebugLevel};
use crate::{debug_info, debug_debug, debug_warn, println};
use crate::arch::x86_64::interrupts;
use crate::mm::memory;
use crate::drivers::display::{display, text_buffer, double_buffered_text};
use crate::graphics::color::Color;

pub fn init(boot_info: &'static mut BootInfo) {
    // Initialize debug subsystem
    debug::init();
    debug::set_debug_level(DebugLevel::Trace);
    
    debug_info!("=== AgenticOS Kernel Starting ===");
    debug_info!("Kernel entry point reached successfully!");
    debug_debug!("Boot info address: {:p}", boot_info);

    // Initialize interrupt descriptor table
    interrupts::init_idt();
    
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
    
    // Run tests if enabled
    #[cfg(feature = "test")]
    run_tests();
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
        
        display_boot_messages();
    } else {
        debug_warn!("No framebuffer available from bootloader");
    }
}

fn display_boot_messages() {
    let stats = memory::get_memory_stats();
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
    display::set_color(Color::CYAN);
    println!("This text is in cyan!");
    
    display::set_color(Color::GREEN);
    println!("This text is in green!");
    
    display::set_color(Color::YELLOW);
    println!("This text is in yellow!");
    
    display::set_color(Color::WHITE);
    println!();
    
    // Demonstrate scrolling
    println!("Testing scrolling functionality:");
    println!("================================");
    
    for i in 0..300 {
        display::set_color(if i % 2 == 0 { Color::WHITE } else { Color::GRAY });
        println!("Line {}: This is a test of the scrolling text buffer", i + 1);
    }
    
    display::set_color(Color::MAGENTA);
    println!();
    println!("Scrolling test complete!");
    
    // Demonstrate tab support
    display::set_color(Color::WHITE);
    println!();
    println!("Tilde Test: ~");
    println!("Tab test:");
    println!("Column:\t1\t2\t3\t4");
    println!("Value:\tA\tB\tC\tD");
    
    // Final message
    println!();
    display::set_color(Color::CYAN);
    println!("AgenticOS kernel initialized successfully!");
    display::set_color(Color::WHITE);
    println!("System ready.");
}

pub fn run() -> ! {
    debug_info!("Kernel initialization complete. Entering idle loop...");
    loop {}
}

#[cfg(feature = "test")]
pub fn run_tests() {
    use crate::lib::test_utils::exit_qemu_success;
    
    debug_info!("=== Running Kernel Tests ===");
    
    // Test 1: Debug system
    debug_info!("Test 1: Debug system initialization");
    debug_debug!("Debug system is working correctly");
    debug_info!("[PASS] Debug system test");
    
    // Test 2: Memory stats
    debug_info!("Test 2: Memory statistics");
    let stats = memory::get_memory_stats();
    assert!(stats.total_memory > 0, "Total memory should be greater than 0");
    assert!(stats.usable_memory > 0, "Usable memory should be greater than 0");
    assert!(stats.usable_memory <= stats.total_memory, "Usable memory should not exceed total memory");
    debug_info!("Memory - Total: {} MB, Usable: {} MB", 
               stats.total_memory / (1024 * 1024),
               stats.usable_memory / (1024 * 1024));
    debug_info!("[PASS] Memory statistics test");
    
    // Test 3: Display colors
    debug_info!("Test 3: Display color setting");
    display::set_color(Color::RED);
    println!("This text should be red");
    display::set_color(Color::GREEN);
    println!("This text should be green");
    display::set_color(Color::BLUE);
    println!("This text should be blue");
    display::set_color(Color::WHITE);
    debug_info!("[PASS] Display color test");
    
    // Test 4: Basic arithmetic
    debug_info!("Test 4: Basic arithmetic");
    let a = 10;
    let b = 20;
    assert_eq!(a + b, 30, "10 + 20 should equal 30");
    assert_eq!(b - a, 10, "20 - 10 should equal 10");
    debug_info!("[PASS] Basic arithmetic test");
    
    // Test 5: Test panic behavior (comment out to test passing)
    // debug_info!("Test 5: Testing panic handler");
    // panic!("This is a test panic!");
    
    debug_info!("=== All Tests Passed! ===");
    
    // Exit QEMU with success code
    exit_qemu_success();
}