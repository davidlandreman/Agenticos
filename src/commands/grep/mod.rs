use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::{println, print};
use alloc::{vec::Vec, string::String, boxed::Box, format};
use alloc::string::ToString;

pub struct GrepProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl GrepProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("grep"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("grep"),
            args,
        }
    }
}

impl HasBaseProcess for GrepProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for GrepProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl GrepProcess {
    pub fn run(&mut self) {
        let mut case_insensitive = false;
        let mut show_line_numbers = false;
        let mut invert_match = false;
        let mut count_only = false;
        let mut pattern: Option<String> = None;
        let mut file_args = Vec::new();
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            match arg.as_str() {
                "-i" => case_insensitive = true,
                "-n" => show_line_numbers = true,
                "-v" => invert_match = true,
                "-c" => count_only = true,
                _ if arg.starts_with("-") => {
                    display::set_color(Color::RED);
                    println!("grep: invalid option -- '{}'", &arg[1..]);
                    println!("Usage: grep [-i] [-n] [-v] [-c] <pattern> <file1> [file2] ...");
                    display::set_color(Color::WHITE);
                    return;
                }
                _ => {
                    if pattern.is_none() {
                        pattern = Some(arg.clone());
                    } else {
                        file_args.push(arg.clone());
                    }
                }
            }
            i += 1;
        }
        
        let pattern = match pattern {
            Some(p) => p,
            None => {
                display::set_color(Color::RED);
                println!("grep: missing pattern");
                println!("Usage: grep [-i] [-n] [-v] [-c] <pattern> <file1> [file2] ...");
                display::set_color(Color::WHITE);
                return;
            }
        };
        
        if file_args.is_empty() {
            display::set_color(Color::RED);
            println!("grep: missing file operand");
            println!("Usage: grep [-i] [-n] [-v] [-c] <pattern> <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }
        
        let search_pattern = if case_insensitive {
            pattern.to_lowercase()
        } else {
            pattern
        };
        
        let multiple_files = file_args.len() > 1;
        
        for file_path in &file_args {
            self.grep_file(file_path, &search_pattern, case_insensitive, show_line_numbers, invert_match, count_only, multiple_files);
        }
    }
    
    fn grep_file(&self, file_path: &str, pattern: &str, case_insensitive: bool, show_line_numbers: bool, invert_match: bool, count_only: bool, multiple_files: bool) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        let mut match_count = 0;
                        
                        for (line_num, line) in content.lines().enumerate() {
                            let search_line = if case_insensitive {
                                line.to_lowercase()
                            } else {
                                line.to_string()
                            };
                            
                            let matches = search_line.contains(pattern);
                            let should_print = if invert_match { !matches } else { matches };
                            
                            if should_print {
                                match_count += 1;
                                
                                if !count_only {
                                    let mut output = String::new();
                                    
                                    if multiple_files {
                                        display::set_color(Color::MAGENTA);
                                        output.push_str(file_path);
                                        output.push(':');
                                        display::set_color(Color::WHITE);
                                    }
                                    
                                    if show_line_numbers {
                                        display::set_color(Color::GREEN);
                                        output.push_str(&format!("{}:", line_num + 1));
                                        display::set_color(Color::WHITE);
                                    }
                                    
                                    if case_insensitive {
                                        let highlighted = self.highlight_pattern(line, pattern);
                                        output.push_str(&highlighted);
                                    } else {
                                        let highlighted = self.highlight_pattern(line, pattern);
                                        output.push_str(&highlighted);
                                    }
                                    
                                    println!("{}", output);
                                }
                            }
                        }
                        
                        if count_only {
                            if multiple_files {
                                display::set_color(Color::MAGENTA);
                                print!("{}:", file_path);
                                display::set_color(Color::WHITE);
                            }
                            println!("{}", match_count);
                        }
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("grep: error reading '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                    }
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("grep: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
    
    fn highlight_pattern(&self, line: &str, pattern: &str) -> String {
        let mut result = String::new();
        let mut last_end = 0;
        
        let search_line = line.to_lowercase();
        let search_pattern = pattern.to_lowercase();
        
        let mut pos = 0;
        while let Some(found) = search_line[pos..].find(&search_pattern) {
            let start = pos + found;
            let end = start + pattern.len();
            
            result.push_str(&line[last_end..start]);
            
            display::set_color(Color::RED);
            result.push_str(&line[start..end]);
            display::set_color(Color::WHITE);
            
            last_end = end;
            pos = end;
        }
        
        result.push_str(&line[last_end..]);
        result
    }
}

pub fn create_grep_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(GrepProcess::new_with_args(args))
}