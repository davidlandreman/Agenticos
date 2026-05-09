// Userland process lifecycle.
//
// In U2 this module exposes a placeholder that exception handlers route to
// when they detect a ring-3 fault. U7 promotes the placeholder into the real
// teardown (unmap user pages, free frames, clear current_output_terminal,
// notify the shell, long-jump to the saved kernel continuation).

use x86_64::VirtAddr;

/// Reason a user process is being torn down. Populated by exception handlers
/// (fault) or the `exit` syscall (cooperative).
#[derive(Debug, Clone, Copy)]
pub struct AbnormalExit {
    /// Exception vector number (e.g., 13 for #GP, 14 for #PF, 6 for #UD).
    pub vector: u8,
    /// CPU-pushed error code, when the vector pushes one.
    pub error_code: Option<u64>,
    /// Faulting linear address (#PF only — read from CR2 by the handler).
    pub fault_addr: Option<VirtAddr>,
    /// Saved RIP at the moment of the fault.
    pub fault_rip: VirtAddr,
}

/// Placeholder. Logs the abnormal-exit reason and halts.
///
/// U7 will replace the body with: drop the active `UserImage`, clear the
/// current output terminal, notify the shell command finished, and long-jump
/// to the saved kernel continuation. For now, halting is the safest possible
/// behavior — it prevents the kernel from continuing to run with corrupted
/// state and keeps the failing test loud.
pub fn cleanup_user_process(reason: AbnormalExit) -> ! {
    use crate::debug_error;

    debug_error!(
        "USERLAND: ring-3 fault — vector={}, error_code={:?}, fault_addr={:?}, rip={:?}",
        reason.vector,
        reason.error_code,
        reason.fault_addr,
        reason.fault_rip
    );

    loop {
        x86_64::instructions::hlt();
    }
}

/// Helper used by exception handlers: returns true when the saved CS in the
/// interrupt frame indicates the fault occurred at ring 3 (CPL=3 / RPL=3).
#[inline]
pub fn frame_is_user(code_segment: u64) -> bool {
    (code_segment & 3) == 3
}
