//! Captured user-mode register snapshot.
//!
//! Phase 4 PR-C2. `fork()` needs to clone the parent's user CPU state
//! into the child so the child resumes at the same instruction with
//! the same register values, except `rax = 0`.
//!
//! Layout is fixed and known to the asm in
//! `enter_user_mode_with_regs_asm`. Field order matches the offsets the
//! asm reads, so reordering here without updating the asm is a
//! load-bearing bug.

/// Full snapshot of a user thread's registers at a syscall boundary.
///
/// `rcx` and `r11` are intentionally absent — the SYSCALL instruction
/// clobbers them (CPU stashes user RIP in rcx, RFLAGS in r11), so
/// userland's syscall ABI documents them as undefined on return.
/// We don't restore them when re-entering ring 3.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct UserState {
    pub rax: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
}

const _SIZE_CHECK: () = assert!(core::mem::size_of::<UserState>() == 16 * 8);

/// Six callee-saved user registers, captured at the very start of a
/// syscall handler before any Rust code clobbers them. Layout matches
/// the order `capture_callee_saved` writes into the buffer.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct CalleeSavedSnapshot {
    pub rbx: u64,
    pub rbp: u64,
    /// `r12` register at handler entry. The SYSCALL stub stashed user
    /// RSP here before calling the dispatcher, so this slot actually
    /// carries the user RSP, not user R12. The original user R12 is
    /// on the kernel stack at a known offset relative to `SyscallArgs`.
    pub r12_register: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// Naked-asm helper: write rbx/rbp/r12-r15 into the buffer. Must be
/// called as the very first thing in a syscall handler that needs to
/// see user-mode register values, before the Rust compiler has had a
/// chance to spill or reuse those registers.
///
/// SAFETY: `out` must point to a writable, suitably-aligned buffer of
/// at least 6 * 8 bytes. The function follows the System V calling
/// convention — first arg in RDI.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "sysv64" fn capture_callee_saved(_out: *mut CalleeSavedSnapshot) {
    core::arch::naked_asm!(
        "mov [rdi + 0], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        "ret",
    );
}
