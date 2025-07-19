use crate::process::{Process, ProcessId, allocate_pid};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::graphics::images::{BmpImage, Image};
use crate::mm::memory;
use crate::{print, println};

pub struct ShellProcess {
    id: ProcessId,
    name: &'static str,
}

impl ShellProcess {
    pub fn new() -> Self {
        Self {
            id: allocate_pid(),
            name: "shell",
        }
    }
}

impl Process for ShellProcess {
    fn get_id(&self) -> ProcessId {
        self.id
    }
    
    fn get_name(&self) -> &str {
        self.name
    }
    
    fn run(&mut self) {
        // Load and display the BMP image
        static LAND_IMAGE_DATA: &[u8] = include_bytes!("../../assets/LAND3.BMP");

        // Try to parse and display the BMP image
        match BmpImage::from_bytes(LAND_IMAGE_DATA) {
            Ok(land_image) => {
                // Use the double buffer directly to draw the image
                display::with_double_buffer(|buffer| {
                    // Clear the screen to black first
                    for y in 0..720 {
                        for x in 0..1280 {
                            buffer.draw_pixel(x, y, Color::BLACK);
                        }
                    }
                    
                    // Draw the image at position (100, 100)
                    buffer.draw_image(100, 100, &land_image);
                    
                    // Swap buffers to show the image
                    buffer.swap_buffers();
                });
                
                // Calculate cursor position below the bitmap
                // Bitmap is at Y=100, add its height, then convert to text rows
                let bitmap_bottom_y = 100 + land_image.height();
                let text_row = bitmap_bottom_y / 8; // 8 is the font height
                display::set_cursor_y(text_row + 1); // Add 1 for some spacing
                
            }
            Err(_e) => {
                println!("Failed to parse BMP");
            }
        }

        // Print memory information
        let stats = memory::get_memory_stats();
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
        
        // Test IDE disk detection
        display::set_color(Color::MAGENTA);
        println!();
        println!("IDE Disk Detection:");
        println!("==================");
        display::set_color(Color::WHITE);
        
        // Test reading from IDE disk if one is present
        self.test_ide_disk();
        
        // Final message
        println!();
        display::set_color(Color::CYAN);
        println!("AgenticOS kernel initialized successfully!");
        display::set_color(Color::WHITE);
        println!("System ready.");
        println!();
        
        // Input testing instructions
        display::set_color(Color::YELLOW);
        println!("Input Device Testing:");
        println!("====================");
        display::set_color(Color::WHITE);
        println!("- Type on keyboard to test keyboard input");
        println!("- Move mouse to test mouse input (check debug logs)");
        println!("- Mouse coordinates and button presses will be logged");
        println!();
        
        // Demonstrate keyboard input
        display::set_color(Color::GREEN);
        println!("AgenticOS Shell");
        display::set_color(Color::WHITE);
        print!(">> ");
        
        // The keyboard interrupt handler will automatically print characters as they are typed
        // In a real OS, we would have a more sophisticated input handling system
        // For now, keyboard input is automatically displayed via the interrupt handler
    }
}

impl ShellProcess {
    fn test_ide_disk(&self) {
        use crate::drivers::ide::{IDE_CONTROLLER, IdeChannel, IdeDrive, IdeBlockDevice};
        use crate::drivers::block::BlockDevice;
        
        // Check primary master disk
        let primary_master = IdeBlockDevice::new(IdeChannel::Primary, IdeDrive::Master);
        
        // Check if disk is present
        if let Some((model_bytes, sectors)) = IDE_CONTROLLER.get_disk_info(IdeChannel::Primary, IdeDrive::Master) {
            let size_mb = (sectors * 512) / (1024 * 1024);
            
            // Convert model bytes to string
            let model_len = model_bytes.iter().position(|&c| c == 0).unwrap_or(40);
            let model = core::str::from_utf8(&model_bytes[..model_len]).unwrap_or("Unknown").trim();
            
            println!("Found IDE disk: {}", model);
            println!("  Size: {} MB ({} sectors)", size_mb, sectors);
            
            // Try to read the first sector (boot sector)
            let mut buffer = [0u8; 512];
            match primary_master.read_blocks(0, 1, &mut buffer) {
                Ok(_) => {
                    println!("  Successfully read boot sector!");
                    
                    // Display first 16 bytes of boot sector in hex
                    print!("  Boot sector (first 16 bytes): ");
                    for i in 0..16 {
                        print!("{:02X} ", buffer[i]);
                    }
                    println!();
                    
                    // Check for common boot sector signatures
                    if buffer[510] == 0x55 && buffer[511] == 0xAA {
                        println!("  Valid boot sector signature found (0x55AA)");
                    } else {
                        println!("  No standard boot sector signature");
                    }
                }
                Err(e) => {
                    println!("  Failed to read boot sector: {}", e);
                }
            }
        } else {
            println!("No IDE disk found on primary master");
        }
    }
}