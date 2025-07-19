use crate::process::{BaseProcess, HasBaseProcess, RunnableProcess};
use crate::println;
use alloc::{vec::Vec, string::String, boxed::Box};

pub struct PwdProcess {
    pub base: BaseProcess,
    args: Vec<String>,
}

impl PwdProcess {
    pub fn new() -> Self {
        Self {
            base: BaseProcess::new("pwd"),
            args: Vec::new(),
        }
    }
    
    pub fn new_with_args(args: Vec<String>) -> Self {
        Self {
            base: BaseProcess::new("pwd"),
            args,
        }
    }
}

impl HasBaseProcess for PwdProcess {
    fn base(&self) -> &BaseProcess {
        &self.base
    }
    
    fn base_mut(&mut self) -> &mut BaseProcess {
        &mut self.base
    }
}

impl RunnableProcess for PwdProcess {
    fn run(&mut self) {
        self.run();
    }
    
    fn get_name(&self) -> &str {
        self.base.get_name()
    }
}

impl PwdProcess {
    pub fn run(&mut self) {
        println!("/");
    }
}

pub fn create_pwd_process(args: Vec<String>) -> Box<dyn RunnableProcess> {
    Box::new(PwdProcess::new_with_args(args))
}