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

/// Six callee-saved user registers, returned by
/// [`read_user_callee_saved`]. The `r12_register` field carries the
/// user's RSP (the SYSCALL stub stashes user RSP into `r12` before
/// calling the dispatcher); the original user R12 is read separately
/// via [`read_user_r12`].
#[repr(C)]
#[derive(Default, Clone, Copy, Debug)]
pub struct CalleeSavedSnapshot {
    pub rbx: u64,
    pub rbp: u64,
    pub r12_register: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// Read the user's callee-saved registers from the slots the SYSCALL
/// stub pushed onto the kernel stack. Layout (relative to `args` ptr):
///
/// ```text
///   args +  72: user RBX
///   args +  80: user RBP
///   args +  88: user R13
///   args +  96: user R14
///   args + 104: user R15
///   args + 112: original user R12
/// ```
///
/// User RSP is captured separately — it lives in gs:[8]
/// (`PERCPU.user_rsp_scratch`) from SYSCALL entry until the iretq
/// epilogue, and we expose it via [`read_user_rsp`] below. The
/// `r12_register` field on the returned snapshot holds **user RSP**.
///
/// An earlier implementation used a naked-asm `capture_callee_saved`
/// helper that read live registers. By the time a syscall handler
/// invoked it, the Rust dispatcher's prologue had already clobbered
/// `rbx` (and potentially other callee-saved regs) with locals — so
/// the snapshot recorded kernel scratch values as "user state".
/// Saving those bogus values into `Process.saved_user_state` and
/// restoring them across a blocking syscall corrupted user rbx with
/// a kernel-heap pointer, which then faulted on the next dereference.
/// Reading from explicit stub-pushed slots is the only reliable path.
///
/// SAFETY: `args` must point at the SyscallArgs struct the stub built
/// on the kernel stack; the function dereferences slots above it.
pub unsafe fn read_user_callee_saved(
    args: *const crate::arch::x86_64::syscall::SyscallArgs,
) -> CalleeSavedSnapshot {
    let p = args as *const u64;
    CalleeSavedSnapshot {
        rbx: core::ptr::read(p.add(9)),
        rbp: core::ptr::read(p.add(10)),
        r12_register: read_user_rsp(),
        r13: core::ptr::read(p.add(11)),
        r14: core::ptr::read(p.add(12)),
        r15: core::ptr::read(p.add(13)),
    }
}

/// Read the user's original R12 register from the kernel-stack slot
/// the SYSCALL stub pushed (at `args + 112`).
///
/// SAFETY: as for [`read_user_callee_saved`].
pub unsafe fn read_user_r12(
    args: *const crate::arch::x86_64::syscall::SyscallArgs,
) -> u64 {
    let p = args as *const u64;
    core::ptr::read(p.add(14))
}

/// Read the user's RSP from the per-CPU SYSCALL scratch slot. The
/// SYSCALL stub writes it there with `mov gs:[8], rsp` before
/// switching stacks; the value persists for the syscall body because
/// `FMASK` masks `IF` and no nested SYSCALL can overwrite it.
#[inline(always)]
pub unsafe fn read_user_rsp() -> u64 {
    let rsp: u64;
    core::arch::asm!(
        "mov {}, gs:[8]",
        out(reg) rsp,
        options(nostack, preserves_flags),
    );
    rsp
}
