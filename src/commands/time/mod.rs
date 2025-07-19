use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box, format};

pub struct TimeProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl TimeProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("time"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("time"),
            args,
        }
    }
}

impl HasBaseProcess for TimeProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for TimeProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl TimeProcess {
    pub fn run(&mut self) {
        if self.args.is_empty() {
            display::set_color(Color::RED);
            println!("time: missing command operand");
            println!("Usage: time <command> [args...]");
            display::set_color(Color::WHITE);
            return;
        }
        
        let command_name = &self.args[0];
        let command_args = if self.args.len() > 1 {
            self.args[1..].to_vec()
        } else {
            Vec::new()
        };
        
        let start_time = self.get_timestamp();
        
        display::set_color(Color::CYAN);
        println!("Executing: {} {}", command_name, command_args.join(" "));
        display::set_color(Color::WHITE);
        
        match crate::process::execute_command(&format!("{} {}", command_name, command_args.join(" "))) {
            Ok(()) => {
                let end_time = self.get_timestamp();
                let elapsed = end_time - start_time;
                
                display::set_color(Color::GREEN);
                println!();
                println!("real    {}ms", elapsed);
                println!("user    {}ms", elapsed);
                println!("sys     0ms");
                display::set_color(Color::WHITE);
            }
            Err(e) => {
                let end_time = self.get_timestamp();
                let elapsed = end_time - start_time;
                
                display::set_color(Color::RED);
                println!("Command failed: {}", e);
                display::set_color(Color::YELLOW);
                println!();
                println!("real    {}ms", elapsed);
                println!("user    {}ms", elapsed);
                println!("sys     0ms");
                display::set_color(Color::WHITE);
            }
        }
    }
    
    fn get_timestamp(&self) -> u64 {
        0
    }
}

pub fn create_time_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(TimeProcess::new_with_args(args))
}