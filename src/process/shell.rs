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
        
        // Test filesystem access
        display::set_color(Color::MAGENTA);
        println!();
        println!("Filesystem Access:");
        println!("=================");
        display::set_color(Color::WHITE);
        
        // Check what files are available in the mounted filesystem
        self.explore_filesystem();
        
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
    fn explore_filesystem(&self) {
        use crate::fs;
        
        // Check if we have a mounted filesystem
        println!("Checking for files in mounted filesystem...");
        
        // List common files that might exist
        let test_files = &[
            "/TEST.TXT",
            "/README.TXT",
            "/assets/test.txt",
            "/assets/agentic-banner.png",
            "/assets/LAND3.BMP",
            "/CONFIG.SYS",
            "/AUTOEXEC.BAT"
        ];
        
        println!("\nLooking for files:");
        for path in test_files {
            if fs::exists(path) {
                print!("  ✓ Found: {}", path);
                
                // Try to get file metadata
                if let Ok(metadata) = fs::metadata(path) {
                    println!(" ({} bytes)", metadata.size);
                } else {
                    println!();
                }
            }
        }
        
        // Try to read a text file if it exists
        println!("\nTrying to read text files:");
        
        // Try TEST.TXT first
        if fs::exists("/TEST.TXT") {
            match fs::read_to_string::<256>("/TEST.TXT") {
                Ok((content, size)) => {
                    println!("  Content of /TEST.TXT ({} bytes):", size);
                    println!("  {}", &content[..size.min(200)]);
                    if size > 200 {
                        println!("  ... (truncated)");
                    }
                }
                Err(e) => {
                    println!("  Failed to read /TEST.TXT: {:?}", e);
                }
            }
        } else if fs::exists("/assets/test.txt") {
            // Try the assets directory
            match fs::read_to_string::<256>("/assets/test.txt") {
                Ok((content, size)) => {
                    println!("  Content of /assets/test.txt ({} bytes):", size);
                    println!("  {}", &content[..size.min(200)]);
                }
                Err(e) => {
                    println!("  Failed to read /assets/test.txt: {:?}", e);
                }
            }
        } else {
            println!("  No text files found to read.");
            println!("  (Note: Filesystem may not be mounted yet during kernel init)");
        }
        
        // Test the callback-based file API if we have a file
        if fs::exists("/TEST.TXT") || fs::exists("/assets/test.txt") {
            let test_path = if fs::exists("/TEST.TXT") { "/TEST.TXT" } else { "/assets/test.txt" };
            
            println!("\nTesting callback-based file API with {}:", test_path);
            match fs::read_with(test_path, |handle, filesystem| {
                let mut buffer = [0u8; 64];
                let bytes_read = filesystem.read(handle, &mut buffer)
                    .map_err(|_| fs::FsError::IoError)?;
                println!("  Read {} bytes using callback API", bytes_read);
                Ok(bytes_read)
            }) {
                Ok(_) => println!("  ✓ Callback API test successful"),
                Err(e) => println!("  ✗ Callback API test failed: {:?}", e)
            }
        }
    }
}