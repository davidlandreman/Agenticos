pub type ProcessId = u32;

static mut NEXT_PID: ProcessId = 1;

pub fn allocate_pid() -> ProcessId {
    unsafe {
        let pid = NEXT_PID;
        NEXT_PID += 1;
        pid
    }
}
