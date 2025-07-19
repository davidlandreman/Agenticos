use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::{println, print};
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct CatProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl CatProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("cat"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("cat"),
            args,
        }
    }
}

impl HasBaseProcess for CatProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for CatProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl CatProcess {
    pub fn run(&mut self) {
        if self.args.is_empty() {
            display::set_color(Color::RED);
            println!("cat: missing file operand");
            println!("Usage: cat <file1> [file2] ...");
            display::set_color(Color::WHITE);
            return;
        }

        for file_path in &self.args {
            self.cat_file(file_path);
        }
    }
    
    fn cat_file(&self, file_path: &str) {
        use crate::fs;
        
        match fs::File::open_read(file_path) {
            Ok(file) => {
                match file.read_to_string() {
                    Ok(content) => {
                        print!("{}", content);
                    }
                    Err(e) => {
                        display::set_color(Color::RED);
                        println!("cat: error reading '{}': {}", file_path, e);
                        display::set_color(Color::WHITE);
                    }
                }
            }
            Err(e) => {
                display::set_color(Color::RED);
                println!("cat: cannot open '{}': {}", file_path, e);
                display::set_color(Color::WHITE);
            }
        }
    }
}

pub fn create_cat_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(CatProcess::new_with_args(args))
}