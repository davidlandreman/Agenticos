use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct DirProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl DirProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("dir"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("dir"),
            args,
        }
    }
}

impl HasBaseProcess for DirProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for DirProcess {
    fn run(&mut self) {
        self.run(); // Call our inherent run method
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}


impl DirProcess {
    pub fn run(&mut self) {
        // Determine which directory to list
        let dir_path = if self.args.is_empty() {
            "/"  // Default to root directory
        } else {
            &self.args[0]
        };
        
        println!("Directory listing for: {}", dir_path);
        println!("{}", "=".repeat(40));
        
        // List the directory contents
        self.list_directory(dir_path);
    }
    
    fn list_directory(&self, path: &str) {
        use crate::fs;
        use crate::fs::filesystem::FileType;
        
        match fs::Directory::open(path) {
            Ok(directory) => {
                let entries = directory.entries();
                
                if entries.is_empty() {
                    println!("Directory is empty or not accessible");
                    return;
                }
                
                // Count files and directories
                let mut file_count = 0;
                let mut dir_count = 0;
                let mut total_size = 0u64;
                
                // Display header
                display::set_color(Color::CYAN);
                println!("{:<4} {:<20} {:<12} {}", "Type", "Name", "Size", "");
                println!("{}", "-".repeat(40));
                display::set_color(Color::WHITE);
                
                // Display each entry
                for entry in &entries {
                    let name = entry.name_str();
                    let file_type_str = match entry.file_type {
                        FileType::File => {
                            file_count += 1;
                            total_size += entry.size as u64;
                            "FILE"
                        },
                        FileType::Directory => {
                            dir_count += 1;
                            "DIR "
                        },
                        _ => "?   ",
                    };
                    
                    // Set color based on file type
                    match entry.file_type {
                        FileType::File => display::set_color(Color::WHITE),
                        FileType::Directory => display::set_color(Color::YELLOW),
                        _ => display::set_color(Color::LIGHT_GRAY),
                    }
                    
                    if entry.file_type == FileType::Directory {
                        println!("{:<4} {:<20} {:<12} {}", file_type_str, name, "<DIR>", "");
                    } else {
                        println!("{:<4} {:<20} {:<12} bytes", file_type_str, name, entry.size);
                    }
                }
                
                display::set_color(Color::WHITE);
                println!("{}", "-".repeat(40));
                
                // Summary
                display::set_color(Color::GREEN);
                println!("{} file(s), {} directory(ies)", file_count, dir_count);
                if file_count > 0 {
                    println!("Total file size: {} bytes", total_size);
                }
                display::set_color(Color::WHITE);
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("Error: Failed to open directory '{}': {}", path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
}

/// Factory function to create a dir process
pub fn create_dir_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(DirProcess::new_with_args(args))
}