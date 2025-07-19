use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct HeadProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl HeadProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("head"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("head"),
            args,
        }
    }
}

impl HasBaseProcess for HeadProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for HeadProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl HeadProcess {
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
                        println!("head: invalid number of lines: '{}'", self.args[i + 1]);
                        display::set_color(Color::WHITE);
                        return;
                    }
                } else {
                    display::set_color(Color::RED);
                    println!("head: option requires an argument -- 'n'");
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
                    println!("head: invalid option -- '{}'", &arg[1..]);
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
            println!("head: missing file operand");
            println!("Usage: head [-n NUM] <file1> [file2] ...");
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
            
            self.head_file(file_path, lines);
        }
    }
    
    fn head_file(&self, file_path: &str, lines: usize) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        let mut line_count = 0;
                        for line in content.lines() {
                            if line_count >= lines {
                                break;
                            }
                            println!("{}", line);
                            line_count += 1;
                        }
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("head: error reading '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                    }
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("head: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
}

pub fn create_head_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(HeadProcess::new_with_args(args))
}