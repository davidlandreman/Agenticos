use crate::process::{BaseProcess, HasBaseProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::graphics::images::{BmpImage, Image};
use crate::{print, println};
use alloc;

pub mod async_shell;
pub mod shell_process;

pub struct ShellProcess {
    pub base: BaseProcess,
}

impl ShellProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("shell"),
        }
    }
}

impl HasBaseProcess for ShellProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl ShellProcess {
    pub fn run(&mut self) {
        // Register this process's stdin buffer to receive keyboard input
        self.base.register_stdin();
        // Load and display the BMP image from filesystem
        let image_path = "/banner.bmp";
        
        // Try to load and parse the BMP image using the file API
        match crate::fs::File::open_read(image_path) {
            Ok(file) => {
                // Read the entire file into a vector
                let mut image_data = alloc::vec![0u8; file.size() as usize];
                match file.read(&mut image_data) {
                    Ok(_) => {
                        // Parse the BMP image from the loaded data
                        match BmpImage::from_bytes(&image_data) {
                            Ok(banner_image) => {
                                // Use the double buffer directly to draw the image
                                display::with_double_buffer(|buffer| {
                                    // Clear the screen to black first
                                    for y in 0..720 {
                                        for x in 0..1280 {
                                            buffer.draw_pixel(x, y, Color::BLACK);
                                        }
                                    }
                                    
                                    // Draw the image at position (100, 100)
                                    buffer.draw_image(100, 100, &banner_image);
                                    
                                    // Swap buffers to show the image
                                    buffer.swap_buffers();
                                });
                                
                                // Calculate cursor position below the bitmap
                                // Bitmap is at Y=100, add its height, then convert to text rows
                                let bitmap_bottom_y = 100 + banner_image.height();
                                let text_row = bitmap_bottom_y / 8; // 8 is the font height
                                display::set_cursor_y(text_row + 1); // Add 1 for some spacing
                            }
                            Err(_e) => {
                                println!("Failed to parse BMP from {}", image_path);
                            }
                        }
                    }
                    Err(e) => {
                        println!("Failed to read image file {}: {}", image_path, e);
                    }
                }
            }
            Err(e) => {
                println!("Failed to open image file {}: {}", image_path, e);
            }
        }
        
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
        println!("- Type on keyboard to test keyboard input (with echo)");
        println!("- Use 'echo off' to disable input echoing, 'echo on' to re-enable");
        println!("- Move mouse to test mouse input (check debug logs)");
        println!("- Mouse coordinates and button presses will be logged");
        println!();
        
        // Demonstrate keyboard input with stdin/stdout
        display::set_color(Color::GREEN);
        println!("AgenticOS Shell");
        display::set_color(Color::WHITE);
        
        // Simple shell loop demonstrating stdin reading
        loop {
            print!(">> ");
            
            // Read a line from stdin
            match self.base.stdin().read_line() {
                Ok(line) => {
                    // No need to manually echo - handled automatically by stdin buffer
                    println!(); // New line after the command
                    
                    // Simple command processing
                    let trimmed = line.trim();
                    match trimmed {
                        "help" => {
                            println!("Available commands:");
                            println!("Built-in commands:");
                            println!("  help     - Show this help message");
                            println!("  exit     - Exit the shell");
                            println!("  clear    - Clear the screen");
                            println!("  echo on  - Enable input echoing");
                            println!("  echo off - Disable input echoing");
                            println!("  echo     - Show current echo status");
                            println!("Font commands:");
                            println!("  font arial     - Switch to Arial font");
                            println!("  font embedded  - Switch to embedded font");
                            println!("  font status    - Show font information");
                            
                            // Show registered commands
                            let commands = crate::process::list_commands();
                            if !commands.is_empty() {
                                println!("Available programs:");
                                for cmd in commands {
                                    println!("  {}       - Execute {} command", cmd, cmd);
                                }
                            }
                        }
                        "exit" => {
                            println!("Exiting shell...");
                            break;
                        }
                        "clear" => {
                            display::clear_screen();
                        }
                        "echo on" => {
                            self.base.configure_stdin_echo(true);
                            println!("Echo enabled");
                        }
                        "echo off" => {
                            self.base.configure_stdin_echo(false);
                            println!("Echo disabled");
                        }
                        "echo" => {
                            let enabled = self.base.stdin().echo_enabled();
                            println!("Echo is currently {}", if enabled { "enabled" } else { "disabled" });
                        }
                        "font arial" => {
                            println!("Attempting to load Arial font from filesystem...");
                            if crate::graphics::fonts::core_font::try_load_arial_font() {
                                println!("✓ Arial font loaded successfully!");
                                println!("  New text will use Arial font from /arial.ttf");
                            } else {
                                println!("✗ Failed to load Arial font from /arial.ttf");
                                println!("  Continuing with embedded font");
                            }
                        }
                        "font embedded" => {
                            println!("Switching to embedded font...");
                            crate::graphics::fonts::core_font::reset_to_embedded_font();
                            println!("✓ Switched to embedded 8x8 font");
                        }
                        "font status" => {
                            // We can't easily query which font is active, but we can show what's available
                            println!("Font Status:");
                            println!("  Available fonts:");
                            println!("    - embedded (built-in 8x8 font)");
                            if crate::fs::exists("/arial.ttf") {
                                println!("    - arial (from /arial.ttf)");
                            } else {
                                println!("    - arial (not available - /arial.ttf not found)");
                            }
                            println!("  Commands:");
                            println!("    font arial     - switch to Arial font");
                            println!("    font embedded  - switch to embedded font");
                            println!("    font status    - show this info");
                        }
                        "" => {
                            // Empty command, just show prompt again
                        }
                        _ => {
                            // Try to execute as a registered command through process manager
                            // Use synchronous execution since this is the old blocking shell
                            match crate::process::execute_command_sync(trimmed) {
                                Ok(()) => {
                                    // Command executed successfully
                                }
                                Err(e) => {
                                    println!("{}", e);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("Error reading input: {:?}", e);
                    break;
                }
            }
        }
        
        // Clean up - unregister stdin buffer when shell exits
        self.base.unregister_stdin();
    }
}

impl ShellProcess {
    fn explore_filesystem(&self) {
        
        
        
        println!("Exploring mounted filesystem...");
        
        // Start recursive exploration from root
        self.explore_directory("/", 0);
        
        // Demonstrate Arc-based file operations on discovered files
        self.demonstrate_file_operations();
    }
    
    fn explore_directory(&self, path: &str, depth: usize) {
        use crate::fs;
        use crate::fs::filesystem::FileType;
        use alloc::format;
        
        // Limit recursion depth to prevent infinite loops
        if depth > 3 {
            println!("{}  (max depth reached)", "  ".repeat(depth));
            return;
        }
        
        // Display current directory
        if depth == 0 {
            println!("\nDirectory listing:");
        }
        println!("{}📁 {}", "  ".repeat(depth), if path == "/" { "/ (root)" } else { path });
        
        // Try to open directory using our Arc-based Directory handle
        match fs::Directory::open(path) {
            Ok(directory) => {
                let entries = directory.entries();
                if entries.is_empty() {
                    if depth == 0 {
                        println!("{}  (Directory appears empty or listing not fully supported)", "  ".repeat(depth + 1));
                    }
                } else {
                    // Display each entry
                    for entry in &entries {
                        let name = entry.name_str();
                        let file_type_icon = match entry.file_type {
                            FileType::File => "📄",
                            FileType::Directory => "📁",
                            _ => "❓",
                        };
                        
                        println!("{}  {} {} ({} bytes)", 
                            "  ".repeat(depth + 1), 
                            file_type_icon, 
                            name, 
                            entry.size
                        );
                        
                        // Recursively explore subdirectories
                        if entry.file_type == FileType::Directory {
                            let full_path = if path == "/" {
                                format!("/{}", name)
                            } else {
                                format!("{}/{}", path.trim_end_matches('/'), name)
                            };
                            self.explore_directory(&full_path, depth + 1);
                        }
                    }
                }
            }
            Err(e) => {
                if depth == 0 {
                    println!("{}  (Failed to open directory: {})", "  ".repeat(depth + 1), e);
                }
            }
        }
    }
    
    fn demonstrate_file_operations(&self) {
        use crate::fs;
        
        println!("\n--- File Operations Demo ---");
        
        // Look for the first text file we can find
        let test_files = ["/TEST.TXT", "/assets/TEST.TXT", "/assets/test.txt"];
        let mut demo_file = None;
        
        for &path in &test_files {
            if fs::exists(path) {
                demo_file = Some(path);
                break;
            }
        }
        
        if let Some(file_path) = demo_file {
            println!("Demonstrating file operations with: {}", file_path);
            
            // 1. Basic file opening and reading
            match fs::File::open_read(file_path) {
                Ok(file) => {
                    println!("✓ File opened successfully");
                    println!("  Path: {}", file.path());
                    println!("  Size: {} bytes", file.size());
                    println!("  Position: {}", file.position());
                    println!("  Is open: {}", file.is_open());
                    
                    // 2. Read content
                    match file.read_to_string() {
                        Ok(content) => {
                            println!("✓ Content read successfully ({} bytes)", content.len());
                            if content.len() > 100 {
                                println!("  Preview: {}...", &content[..97]);
                            } else if !content.trim().is_empty() {
                                println!("  Content: {}", content.trim());
                            } else {
                                println!("  (File is empty)");
                            }
                        }
                        Err(e) => {
                            println!("✗ Failed to read content: {}", e);
                        }
                    }
                    
                    // 3. Demonstrate shared ownership
                    let file_clone = file.clone();
                    println!("✓ Created shared file handle");
                    println!("  Original handle open: {}", file.is_open());
                    println!("  Clone handle open: {}", file_clone.is_open());
                    
                    // 4. Demonstrate seeking
                    if file.size() > 10 {
                        match file.seek(5) {
                            Ok(pos) => {
                                println!("✓ Seeked to position: {}", pos);
                                
                                // Read a small portion from the new position
                                let mut buffer = [0u8; 10];
                                match file.read(&mut buffer) {
                                    Ok(bytes_read) => {
                                        println!("✓ Read {} bytes from position {}", bytes_read, pos);
                                    }
                                    Err(e) => {
                                        println!("✗ Failed to read from position {}: {}", pos, e);
                                    }
                                }
                            }
                            Err(e) => {
                                println!("✗ Seek failed: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    println!("✗ Failed to open {}: {}", file_path, e);
                }
            }
        } else {
            println!("No text files found for demonstration");
            println!("(Filesystem may not be mounted or no readable files available)");
        }
        
        println!("--- End Demo ---\n");
    }
}