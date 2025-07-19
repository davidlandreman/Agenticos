pub type ProcessId = u32;

pub trait Process {
    fn get_id(&self) -> ProcessId;
    fn get_name(&self) -> &str;
    fn run(&mut self);
}

static mut NEXT_PID: ProcessId = 1;

pub fn allocate_pid() -> ProcessId {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}