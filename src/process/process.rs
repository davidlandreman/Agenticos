pub type ProcessId = u32;

static NEXT_PID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

pub fn allocate_pid() -> ProcessId {
    NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}
