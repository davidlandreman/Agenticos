use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::{println, print};
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct HexdumpProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl HexdumpProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("hexdump"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("hexdump"),
            args,
        }
    }
}

impl HasBaseProcess for HexdumpProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for HexdumpProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl HexdumpProcess {
    pub fn run(&mut self) {
        let mut canonical = false;
        let mut length: Option<usize> = None;
        let mut file_args = Vec::new();
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            match arg.as_str() {
                "-C" => canonical = true,
                "-n" => {
                    if i + 1 < self.args.len() {
                        if let Ok(n) = self.args[i + 1].parse::<usize>() {
                            length = Some(n);
                            i += 2;
                            continue;
                        } else {
                            display::set_color(Color::RED);
                            println!("hexdump: invalid length: '{}'", self.args[i + 1]);
                            display::set_color(Color::WHITE);
                            return;
                        }
                    } else {
                        display::set_color(Color::RED);
                        println!("hexdump: option requires an argument -- 'n'");
                        display::set_color(Color::WHITE);
                        return;
                    }
                }
                _ if arg.starts_with("-") => {
                    display::set_color(Color::RED);
                    println!("hexdump: invalid option -- '{}'", &arg[1..]);
                    println!("Usage: hexdump [-C] [-n length] <file1> [file2] ...");
                    display::set_color(Color::WHITE);
                    return;
                }
                _ => {
                    file_args.push(arg.clone());
                    i += 1;
                }
            }
        }
        
        if file_args.is_empty() {
            display::set_color(Color::RED);
            println!("hexdump: missing file operand");
            println!("Usage: hexdump [-C] [-n length] <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }
        
        for file_path in &file_args {
            if canonical {
                self.hexdump_canonical(file_path, length);
            } else {
                self.hexdump_standard(file_path, length);
            }
        }
    }
    
    fn hexdump_canonical(&self, file_path: &str, length: Option<usize>) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                let mut buffer = [0u8; 1024];
                let mut offset = 0usize;
                let mut total_read = 0usize;
                
                loop {
                    match file.read(&mut buffer) {
                        Ok(bytes_read) => {
                            if bytes_read == 0 {
                                break;
                            }
                            
                            let data_to_process = if let Some(max_len) = length {
                                let remaining = max_len - total_read;
                                if remaining == 0 {
                                    break;
                                }
                                core::cmp::min(bytes_read, remaining)
                            } else {
                                bytes_read
                            };
                            
                            self.print_canonical_lines(&buffer[..data_to_process], offset);
                            offset += data_to_process;
                            total_read += data_to_process;
                            
                            if let Some(max_len) = length {
                                if total_read >= max_len {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            display::set_color(Color::RED);
                            println!("hexdump: error reading '{}': {}", file_path, e);
                            display::set_color(Color::WHITE);
                            break;
                        }
                    }
                }
                
                if total_read > 0 {
                    println!("{:08x}", offset);
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("hexdump: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
    
    fn hexdump_standard(&self, file_path: &str, length: Option<usize>) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                let mut buffer = [0u8; 1024];
                let mut offset = 0usize;
                let mut total_read = 0usize;
                
                loop {
                    match file.read(&mut buffer) {
                        Ok(bytes_read) => {
                            if bytes_read == 0 {
                                break;
                            }
                            
                            let data_to_process = if let Some(max_len) = length {
                                let remaining = max_len - total_read;
                                if remaining == 0 {
                                    break;
                                }
                                core::cmp::min(bytes_read, remaining)
                            } else {
                                bytes_read
                            };
                            
                            self.print_standard_lines(&buffer[..data_to_process], offset);
                            offset += data_to_process;
                            total_read += data_to_process;
                            
                            if let Some(max_len) = length {
                                if total_read >= max_len {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            display::set_color(Color::RED);
                            println!("hexdump: error reading '{}': {}", file_path, e);
                            display::set_color(Color::WHITE);
                            break;
                        }
                    }
                }
                
                if total_read > 0 {
                    println!("{:08x}", offset);
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("hexdump: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
    
    fn print_canonical_lines(&self, data: &[u8], mut offset: usize) {
        for chunk in data.chunks(16) {
            print!("{:08x}  ", offset);
            
            for (i, &byte) in chunk.iter().enumerate() {
                if i == 8 {
                    print!(" ");
                }
                print!("{:02x} ", byte);
            }
            
            for _ in chunk.len()..16 {
                if chunk.len() <= 8 {
                    print!("   ");
                } else {
                    print!("   ");
                }
            }
            
            if chunk.len() <= 8 {
                print!(" ");
            }
            
            print!(" |");
            for &byte in chunk {
                if byte >= 32 && byte <= 126 {
                    print!("{}", byte as char);
                } else {
                    print!(".");
                }
            }
            println!("|");
            
            offset += chunk.len();
        }
    }
    
    fn print_standard_lines(&self, data: &[u8], mut offset: usize) {
        for chunk in data.chunks(16) {
            print!("{:08x} ", offset);
            
            for pair in chunk.chunks(2) {
                if pair.len() == 2 {
                    print!("{:02x}{:02x} ", pair[1], pair[0]);
                } else {
                    print!("{:02x}   ", pair[0]);
                }
            }
            
            println!();
            offset += chunk.len();
        }
    }
}

pub fn create_hexdump_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(HexdumpProcess::new_with_args(args))
}