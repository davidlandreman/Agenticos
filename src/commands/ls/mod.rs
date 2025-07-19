use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::{println, print};
use alloc::{vec::Vec, string::String, boxed::Box, format};

pub struct LsProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl LsProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("ls"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("ls"),
            args,
        }
    }
}

impl HasBaseProcess for LsProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for LsProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl LsProcess {
    pub fn run(&mut self) {
        let mut long_format = false;
        let mut all_files = false;
        let mut human_readable = false;
        let mut file_args = Vec::new();
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            match arg.as_str() {
                "-l" => long_format = true,
                "-a" => all_files = true,
                "-h" => human_readable = true,
                "-la" | "-al" => {
                    long_format = true;
                    all_files = true;
                }
                _ if arg.starts_with("-") => {
                    display::set_color(Color::RED);
                    println!("ls: invalid option -- '{}'", &arg[1..]);
                    println!("Usage: ls [-l] [-a] [-h] [directory]");
                    display::set_color(Color::WHITE);
                    return;
                }
                _ => file_args.push(arg.clone()),
            }
            i += 1;
        }
        
        let dir_path = if file_args.is_empty() {
            "/"
        } else {
            &file_args[0]
        };
        
        self.list_directory(dir_path, long_format, all_files, human_readable);
    }
    
    fn list_directory(&self, path: &str, long_format: bool, all_files: bool, human_readable: bool) {
        use crate::fs;
        use crate::fs::filesystem::FileType;
        
        match fs::Directory::open(path) {
            Ok(directory) => {
                let entries = directory.entries();
                
                if entries.is_empty() {
                    return;
                }
                
                let mut filtered_entries = Vec::new();
                for entry in &entries {
                    let name = entry.name_str();
                    if all_files || !name.starts_with('.') {
                        filtered_entries.push(entry);
                    }
                }
                
                if long_format {
                    display::set_color(Color::CYAN);
                    println!("total {}", filtered_entries.len());
                    display::set_color(Color::WHITE);
                    
                    for entry in &filtered_entries {
                        self.print_long_format(entry, human_readable);
                    }
                } else {
                    for (i, entry) in filtered_entries.iter().enumerate() {
                        let name = entry.name_str();
                        
                        match entry.file_type {
                            FileType::Directory => display::set_color(Color::BLUE),
                            FileType::File => display::set_color(Color::WHITE),
                            _ => display::set_color(Color::LIGHT_GRAY),
                        }
                        
                        print!("{}", name);
                        
                        if i < filtered_entries.len() - 1 {
                            print!("  ");
                        }
                        
                        if (i + 1) % 8 == 0 {
                            println!();
                        }
                    }
                    
                    if filtered_entries.len() % 8 != 0 {
                        println!();
                    }
                    
                    display::set_color(Color::WHITE);
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("ls: cannot access '{}': {}", path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
    
    fn print_long_format(&self, entry: &crate::fs::filesystem::DirectoryEntry, human_readable: bool) {
        use crate::fs::filesystem::FileType;
        
        let file_type_char = match entry.file_type {
            FileType::Directory => 'd',
            FileType::File => '-',
            _ => '?',
        };
        
        let permissions = "rwxr-xr-x";
        
        let size_str = if human_readable {
            self.format_human_readable_size(entry.size as u64)
        } else {
            format!("{:8}", entry.size)
        };
        
        match entry.file_type {
            FileType::Directory => display::set_color(Color::BLUE),
            FileType::File => display::set_color(Color::WHITE),
            _ => display::set_color(Color::LIGHT_GRAY),
        }
        
        println!("{}{} 1 root root {} Jan  1 00:00 {}", 
                file_type_char, 
                permissions, 
                size_str,
                entry.name_str());
        
        display::set_color(Color::WHITE);
    }
    
    fn format_human_readable_size(&self, size: u64) -> String {
        const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
        let mut size_f = size as f64;
        let mut unit_index = 0;
        
        while size_f >= 1024.0 && unit_index < UNITS.len() - 1 {
            size_f /= 1024.0;
            unit_index += 1;
        }
        
        if unit_index == 0 {
            format!("{:4}B", size)
        } else {
            format!("{:4.1}{}", size_f, UNITS[unit_index])
        }
    }
}

pub fn create_ls_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(LsProcess::new_with_args(args))
}