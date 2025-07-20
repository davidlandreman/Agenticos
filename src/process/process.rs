use crate::stdlib::io::{Stdin, Stdout, IoHandles, StdinBuffer};
use crate::lib::arc::Arc;
use spin::Mutex;

pub type ProcessId = u32;

/// Trait for runnable processes that can be spawned by the process manager
pub trait RunnableProcess: Send {
    fn run(&mut self);
    fn get_name(&self) -> &str;
}

pub trait Process {
    fn get_id(&self) -> ProcessId;
    fn get_name(&self) -> &str;
    fn run(&mut self);
    
    // Standard I/O handles
    fn stdin(&mut self) -> &mut Stdin;
    fn stdout(&mut self) -> &mut Stdout;
    
    // Configure stdin echo behavior (default implementation enables echo)
    fn configure_stdin_echo(&mut self, enabled: bool) {
        self.stdin().set_echo(enabled);
    }
    
    // Check if stdin echo is enabled
    fn stdin_echo_enabled(&self) -> bool {
        // Note: This requires a const version or we need to change the signature
        // For now, let's assume echo is enabled by default
        true
    }
}

// Trait for processes that use BaseProcess composition
pub trait HasBaseProcess {
    fn base(&self) -> &BaseProcess;
    fn base_mut(&mut self) -> &mut BaseProcess;
}

// Macro to implement Process trait using BaseProcess delegation
#[macro_export]
macro_rules! impl_process_for_base {
    ($type:ty) => {
        impl Process for $type {
            fn get_id(&self) -> ProcessId {
                self.base().get_id()
            }
            
            fn get_name(&self) -> &str {
                self.base().get_name()
            }
            
            fn stdin(&mut self) -> &mut Stdin {
                self.base_mut().stdin()
            }
            
            fn stdout(&mut self) -> &mut Stdout {
                self.base_mut().stdout()
            }
            
            fn configure_stdin_echo(&mut self, enabled: bool) {
                self.base_mut().configure_stdin_echo(enabled);
            }
            
            fn run(&mut self);
        }
    };
}

pub struct BaseProcess {
    id: ProcessId,
    name: &'static str,
    io: IoHandles,
    stdin_buffer: Arc<Mutex<StdinBuffer>>,
}

impl BaseProcess {
    pub fn new(name: &'static str) -> Self {
        let stdin_buffer = Arc::new(Mutex::new(StdinBuffer::new()));
        let io = IoHandles::new(stdin_buffer.clone());
        
        Self {
            id: allocate_pid(),
            name,
            io,
            stdin_buffer,
        }
    }
    
    pub fn new_with_echo(name: &'static str, echo_enabled: bool) -> Self {
        let stdin_buffer = Arc::new(Mutex::new(StdinBuffer::new_with_echo(echo_enabled)));
        let io = IoHandles::new(stdin_buffer.clone());
        
        Self {
            id: allocate_pid(),
            name,
            io,
            stdin_buffer,
        }
    }
    
    pub fn get_id(&self) -> ProcessId {
        self.id
    }
    
    pub fn get_name(&self) -> &str {
        self.name
    }
    
    pub fn stdin(&mut self) -> &mut Stdin {
        &mut self.io.stdin
    }
    
    pub fn stdout(&mut self) -> &mut Stdout {
        &mut self.io.stdout
    }
    
    pub fn register_stdin(&self) {
        crate::process::set_active_stdin(self.stdin_buffer.clone());
    }
    
    pub fn unregister_stdin(&self) {
        crate::process::clear_active_stdin();
    }
    
    pub fn with_stdin_registered<F, R>(&self, f: F) -> R 
    where 
        F: FnOnce() -> R,
    {
        self.register_stdin();
        let result = f();
        self.unregister_stdin();
        result
    }
    
    pub fn configure_stdin_echo(&mut self, enabled: bool) {
        self.stdin().set_echo(enabled);
    }
}

impl RunnableProcess for BaseProcess {
    fn run(&mut self) {
        // BaseProcess doesn't have its own run implementation
        // This should be overridden by processes that use BaseProcess
        panic!("BaseProcess::run() called directly - should be implemented by concrete process");
    }
    
    fn get_name(&self) -> &str {
        self.name
    }
}

// Blanket implementation for anything that implements HasBaseProcess + Process
impl<T> RunnableProcess for T 
where 
    T: HasBaseProcess + Process + Send,
{
    fn run(&mut self) {
        Process::run(self);
    }
    
    fn get_name(&self) -> &str {
        self.base().get_name()
    }
}

static mut NEXT_PID: ProcessId = 1;

pub fn allocate_pid() -> ProcessId {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}