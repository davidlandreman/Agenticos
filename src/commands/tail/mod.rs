use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box, collections::VecDeque};

pub struct TailProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl TailProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("tail"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("tail"),
            args,
        }
    }
}

impl HasBaseProcess for TailProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for TailProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl TailProcess {
    pub fn run(&mut self) {
        let mut lines = 10usize;
        let mut file_args = Vec::new();
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            if arg == "-n" {
                if i + 1 < self.args.len() {
                    if let Ok(n) = self.args[i + 1].parse::<usize>() {
                        lines = n;
                        i += 2;
                        continue;
                    } else {
                        display::set_color(Color::RED);
                        println!("tail: invalid number of lines: '{}'", self.args[i + 1]);
                        display::set_color(Color::WHITE);
                        return;
                    }
                } else {
                    display::set_color(Color::RED);
                    println!("tail: option requires an argument -- 'n'");
                    display::set_color(Color::WHITE);
                    return;
                }
            } else if arg.starts_with("-") && arg.len() > 1 {
                if let Ok(n) = arg[1..].parse::<usize>() {
                    lines = n;
                    i += 1;
                    continue;
                } else {
                    display::set_color(Color::RED);
                    println!("tail: invalid option -- '{}'", &arg[1..]);
                    display::set_color(Color::WHITE);
                    return;
                }
            } else {
                file_args.push(arg.clone());
                i += 1;
            }
        }
        
        if file_args.is_empty() {
            display::set_color(Color::RED);
            println!("tail: missing file operand");
            println!("Usage: tail [-n NUM] <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }
        
        let multiple_files = file_args.len() > 1;
        
        for (index, file_path) in file_args.iter().enumerate() {
            if multiple_files {
                if index > 0 {
                    println!();
                }
                display::set_color(Color::CYAN);
                println!("==> {} <==", file_path);
                display::set_color(Color::WHITE);
            }
            
            self.tail_file(file_path, lines);
        }
    }
    
    fn tail_file(&self, file_path: &str, lines: usize) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        let all_lines: Vec<&str> = content.lines().collect();
                        let start_index = if all_lines.len() > lines {
                            all_lines.len() - lines
                        } else {
                            0
                        };
                        
                        for line in &all_lines[start_index..] {
                            println!("{}", line);
                        }
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("tail: error reading '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                    }
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("tail: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
}

pub fn create_tail_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(TailProcess::new_with_args(args))
}