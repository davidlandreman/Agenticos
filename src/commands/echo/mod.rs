use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::{println, print};
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct EchoProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl EchoProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("echo"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("echo"),
            args,
        }
    }
}

impl HasBaseProcess for EchoProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for EchoProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl EchoProcess {
    pub fn run(&mut self) {
        let mut output = String::new();
        let mut newline = true;
        let mut i = 0;
        
        while i < self.args.len() {
            let arg = &self.args[i];
            
            if arg == "-n" {
                newline = false;
                i += 1;
                continue;
            }
            
            if !output.is_empty() {
                output.push(' ');
            }
            
            let processed_arg = self.process_escape_sequences(arg);
            output.push_str(&processed_arg);
            
            i += 1;
        }
        
        if newline {
            println!("{}", output);
        } else {
            print!("{}", output);
        }
    }
    
    fn process_escape_sequences(&self, input: &str) -> String {
        let mut result = String::new();
        let mut chars = input.chars().peekable();
        
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(&next_ch) = chars.peek() {
                    match next_ch {
                        'n' => {
                            result.push('\n');
                            chars.next();
                        }
                        't' => {
                            result.push('\t');
                            chars.next();
                        }
                        'r' => {
                            result.push('\r');
                            chars.next();
                        }
                        '\\' => {
                            result.push('\\');
                            chars.next();
                        }
                        _ => {
                            result.push(ch);
                        }
                    }
                } else {
                    result.push(ch);
                }
            } else {
                result.push(ch);
            }
        }
        
        result
    }
}

pub fn create_echo_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(EchoProcess::new_with_args(args))
}