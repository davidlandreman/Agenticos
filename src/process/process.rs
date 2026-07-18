pub type ProcessId = u32;

/// Trait for runnable processes that can be spawned by the process manager
pub trait RunnableProcess: Send {
    fn run(&mut self);
    #[expect(dead_code, reason = "intentional kernel API surface")]
    fn get_name(&self) -> &str;
}

static mut NEXT_PID: ProcessId = 1;

pub fn allocate_pid() -> ProcessId {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}
