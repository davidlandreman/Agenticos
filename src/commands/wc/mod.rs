use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box, format};

pub struct WcProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl WcProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("wc"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("wc"),
            args,
        }
    }
}

impl HasBaseProcess for WcProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for WcProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

struct WcCounts {
    lines: usize,
    words: usize,
    chars: usize,
    bytes: usize,
}

impl WcProcess {
    pub fn run(&mut self) {
        let mut show_lines = false;
        let mut show_words = false;
        let mut show_chars = false;
        let mut show_bytes = false;
        let mut file_args = Vec::new();
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            match arg.as_str() {
                "-l" => show_lines = true,
                "-w" => show_words = true,
                "-c" => show_bytes = true,
                "-m" => show_chars = true,
                _ if arg.starts_with("-") => {
                    display::set_color(Color::RED);
                    println!("wc: invalid option -- '{}'", &arg[1..]);
                    println!("Usage: wc [-l] [-w] [-c] [-m] <file1> [file2] ...");
                    display::set_color(Color::WHITE);
                    return;
                }
                _ => file_args.push(arg.clone()),
            }
            i += 1;
        }
        
        if !show_lines && !show_words && !show_chars && !show_bytes {
            show_lines = true;
            show_words = true;
            show_bytes = true;
        }
        
        if file_args.is_empty() {
            display::set_color(Color::RED);
            println!("wc: missing file operand");
            println!("Usage: wc [-l] [-w] [-c] [-m] <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }
        
        let mut total = WcCounts {
            lines: 0,
            words: 0,
            chars: 0,
            bytes: 0,
        };
        
        let multiple_files = file_args.len() > 1;
        
        for file_path in &file_args {
            if let Some(counts) = self.wc_file(file_path) {
                self.print_counts(&counts, file_path, show_lines, show_words, show_chars, show_bytes);
                total.lines += counts.lines;
                total.words += counts.words;
                total.chars += counts.chars;
                total.bytes += counts.bytes;
            }
        }
        
        if multiple_files {
            self.print_counts(&total, "total", show_lines, show_words, show_chars, show_bytes);
        }
    }
    
    fn wc_file(&self, file_path: &str) -> Option<WcCounts> {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        let lines = content.lines().count();
                        let words = content.split_whitespace().count();
                        let chars = content.chars().count();
                        let bytes = content.len();
                        
                        Some(WcCounts {
                            lines,
                            words,
                            chars,
                            bytes,
                        })
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("wc: error reading '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                        None
                    }
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("wc: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
                None
            }
        }
    }
    
    fn print_counts(&self, counts: &WcCounts, name: &str, show_lines: bool, show_words: bool, show_chars: bool, show_bytes: bool) {
        let mut output = String::new();
        
        if show_lines {
            output.push_str(&format!("{:8}", counts.lines));
        }
        if show_words {
            output.push_str(&format!("{:8}", counts.words));
        }
        if show_chars {
            output.push_str(&format!("{:8}", counts.chars));
        }
        if show_bytes {
            output.push_str(&format!("{:8}", counts.bytes));
        }
        
        println!("{} {}", output, name);
    }
}

pub fn create_wc_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(WcProcess::new_with_args(args))
}