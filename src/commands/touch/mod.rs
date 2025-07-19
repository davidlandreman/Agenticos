use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct TouchProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl TouchProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("touch"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("touch"),
            args,
        }
    }
}

impl HasBaseProcess for TouchProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for TouchProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl TouchProcess {
    pub fn run(&mut self) {
        if self.args.is_empty() {
            display::set_color(Color::RED);
            println!("touch: missing file operand");
            println!("Usage: touch <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }

        for file_path in &self.args {
            self.touch_file(file_path);
        }
    }
    
    fn touch_file(&self, file_path: &str) {
        use crate::fs;
        
        match fs::exists(file_path) {
            true => {
                display::set_color(Color::YELLOW);
                println!("touch: '{}' already exists (timestamp update not implemented)", file_path);
                display::set_color(Color::WHITE);
            }
            false => {
                match fs::create_file(file_path) {
                    Ok(_) => {
                        display::set_color(Color::GREEN);
                        println!("touch: created '{}'", file_path);
                        display::set_color(Color::WHITE);
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("touch: cannot create '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                    }
                }
            }
        }
    }
}

pub fn create_touch_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(TouchProcess::new_with_args(args))
}