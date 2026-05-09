// Userland process lifecycle (U7).
//
// Bridges the ring-3 entry path (`crate::userland::enter_user_mode`) and the
// teardown path (`cleanup_user_process`). Both ends of the lifecycle long-jump
// to the same kernel continuation that `enter_user_mode` saves before
// `iretq`-ing to user space:
//
// - **Cooperative exit** (U5's `exit` syscall handler): the syscall dispatcher
//   notices the exit syscall, records the code, and calls
//   `restore_continuation` from `cleanup_user_process` — never returning to
//   the dispatcher's `iretq`.
// - **Abnormal exit** (ring-3 fault routed by `interrupts.rs`): same target.
//
// The continuation is captured as a setjmp-style snapshot: callee-saved GPRs
// + RSP + a return RIP. Restoring it makes `enter_user_mode` "return" as if
// the user app had completed normally; control flows back to the run command,
// which drops the `UserImage`, clears terminal routing, and notifies the shell.
//
// Single-app-synchronous (D5) means there is exactly one continuation slot
// at a time. The slot is taken at `enter_user_mode` time and consumed by the
// long-jump. A second `run` while one is active is rejected by the run
// command before `enter_user_mode` is reached.

use spin::Mutex;
use x86_64::VirtAddr;

use crate::userland::image::UserImage;

/// Reason a user process is being torn down. Populated by exception handlers
/// (fault) or the `exit` syscall (cooperative).
#[derive(Debug, Clone, Copy)]
pub struct AbnormalExit {
    /// Exception vector number (e.g., 13 for #GP, 14 for #PF, 6 for #UD).
    /// For cooperative exits via the `exit` syscall, vector is 0xFF (sentinel).
    pub vector: u8,
    /// CPU-pushed error code, when the vector pushes one.
    pub error_code: Option<u64>,
    /// Faulting linear address (#PF only — read from CR2 by the handler).
    pub fault_addr: Option<VirtAddr>,
    /// Saved RIP at the moment of the fault.
    pub fault_rip: VirtAddr,
}

/// Sentinel vector for cooperative `exit` syscall teardown — distinguishes a
/// clean app exit from a fault in the diagnostic path. Chosen to be outside
/// the architectural exception range (0..32).
pub const COOPERATIVE_EXIT_VECTOR: u8 = 0xFF;

/// Saved kernel state at the moment we entered ring 3. Restored on long-jump.
/// Layout matches the order in which the naked-asm helpers push and pop the
/// callee-saved registers; do not reorder fields without auditing the asm.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContinuation {
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rsp: u64,
    /// Address to resume at — the instruction immediately after the
    /// `enter_user_mode_asm` call site in the run command.
    pub rip: u64,
}

/// The single active per-CPU user-process slot.
///
/// Holds:
/// - The saved kernel continuation (set by `enter_user_mode`, consumed by
///   `restore_continuation`).
/// - The active `UserImage` (transferred from the loader on commit; dropped
///   when teardown returns from the long-jump).
/// - The recorded exit information (cooperative code or fault reason) so the
///   run command can log a diagnostic after returning.
///
/// The mutex is `try_lock`-able from interrupt context because `Spin::Mutex`
/// is fair-acquired but never blocks the kernel — every taker checks for the
/// expected state and gives up if not present. Long-jump readers always
/// observe a consistent snapshot because the writer (`enter_user_mode`)
/// completes the write before the `iretq`.
pub struct ActiveUser {
    pub continuation: Option<KernelContinuation>,
    pub image: Option<UserImage>,
    pub exit_kind: ExitKind,
    pub exit_code: i64,
    /// Current `brk` high-water mark. Initialized to `USER_BRK_BASE` on
    /// `enter_user_mode`; grown by the `brk(addr)` syscall and never shrunk.
    /// `brk(0)` returns this value.
    pub brk_current: u64,
    /// Next free address in the per-process mmap arena. Starts at
    /// `USER_MMAP_BASE` and bumps upward by the page-rounded length of each
    /// successful anonymous `mmap`. No coalescing or reuse for this milestone.
    pub mmap_next: u64,
}

/// What ended the user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// No exit yet (still running).
    None,
    /// Cooperative `exit(code)` syscall.
    Cooperative,
    /// Ring-3 fault — see `AbnormalExit` for vector / fault address.
    Abnormal { vector: u8, fault_rip: u64 },
    /// User issued a syscall the kernel does not implement. The number is
    /// recorded so diagnostic logging can name it; the process is torn
    /// down via the same long-jump path as a fault. Distinct from
    /// `Abnormal` because no CPU exception fired — the failure mode is a
    /// kernel-policy refusal, not a hardware fault.
    UnimplementedSyscall { nr: u64 },
}

static ACTIVE_USER: Mutex<ActiveUser> = Mutex::new(ActiveUser {
    continuation: None,
    image: None,
    exit_kind: ExitKind::None,
    exit_code: 0,
    brk_current: 0,
    mmap_next: 0,
});

/// Acquire the active-user slot for read/write. Used by the run command to
/// install / drop the image and to inspect the recorded exit info.
pub fn with_active_user<R>(f: impl FnOnce(&mut ActiveUser) -> R) -> R {
    let mut g = ACTIVE_USER.lock();
    f(&mut g)
}

/// Returns true while a user process owns the active slot. The run command
/// uses this to enforce the single-user invariant (D5).
pub fn user_active() -> bool {
    ACTIVE_USER.lock().image.is_some()
}

/// Save the kernel continuation. Called by `enter_user_mode` immediately
/// before issuing `iretq` to user space.
pub fn install_continuation(c: KernelContinuation) {
    ACTIVE_USER.lock().continuation = Some(c);
}

/// Take ownership of the active continuation, if any. Used by the long-jump
/// path before restoring registers — the slot is cleared so a second teardown
/// is a no-op.
pub fn take_continuation() -> Option<KernelContinuation> {
    ACTIVE_USER.lock().continuation.take()
}

/// Helper used by exception handlers: returns true when the saved CS in the
/// interrupt frame indicates the fault occurred at ring 3 (CPL=3 / RPL=3).
#[inline]
pub fn frame_is_user(code_segment: u64) -> bool {
    (code_segment & 3) == 3
}

/// Tear down the active user process and long-jump to the saved kernel
/// continuation. **Diverges**: control never returns to the caller (the
/// faulting interrupt handler or the `exit` syscall dispatcher).
///
/// Order of operations:
/// 1. Record the exit reason on the active-user slot (the run command logs it
///    after the long-jump).
/// 2. Clear the active syscall pointer-validation bounds (no user pointers
///    are valid after this).
/// 3. Take the continuation. If somehow not present (no `enter_user_mode`
///    ever ran), fall back to halting.
/// 4. `restore_continuation(cont)` — naked asm jump back to the run command.
///
/// We do NOT drop the `UserImage` here. The image is dropped by the run
/// command after the long-jump returns control — that drop sequence runs in
/// a normal Rust frame, not in interrupt context, which is the right place
/// to walk the mappings list and call back into the memory mapper.
pub fn cleanup_user_process(reason: AbnormalExit) -> ! {
    use crate::debug_error;

    debug_error!(
        "USERLAND: ring-3 fault — vector={}, error_code={:?}, fault_addr={:?}, rip={:?}",
        reason.vector,
        reason.error_code,
        reason.fault_addr,
        reason.fault_rip
    );

    record_exit(ExitKind::Abnormal {
        vector: reason.vector,
        fault_rip: reason.fault_rip.as_u64(),
    }, 0);

    long_jump_to_run_or_halt();
}

/// Cooperative-exit path — invoked from the `exit` syscall handler.
/// Same teardown as `cleanup_user_process`, with `ExitKind::Cooperative`.
pub fn cooperative_exit(code: i64) -> ! {
    record_exit(ExitKind::Cooperative, code);
    long_jump_to_run_or_halt();
}

/// Unimplemented-syscall path — invoked from the dispatcher's default arm
/// when a binary issues a syscall number the kernel does not handle.
///
/// Records `ExitKind::UnimplementedSyscall { nr }` and long-jumps to the
/// run command's continuation. The kernel does not panic, hang, or
/// silently return `-ENOSYS` — the binary is terminated cleanly with a
/// diagnostic on serial.
pub fn unimplemented_syscall_exit(nr: u64) -> ! {
    crate::debug_warn!("USERLAND: unimplemented syscall nr={} — terminating user process", nr);
    let mut g = ACTIVE_USER.lock();
    if matches!(g.exit_kind, ExitKind::None) {
        g.exit_kind = ExitKind::UnimplementedSyscall { nr };
        g.exit_code = -38; // ENOSYS sentinel for the run command's log
    }
    drop(g);
    long_jump_to_run_or_halt();
}

fn record_exit(kind: ExitKind, code: i64) {
    let mut g = ACTIVE_USER.lock();
    // Only record if not already terminated (defensive: a second fault from
    // an already-failing app would otherwise overwrite the original reason).
    if matches!(g.exit_kind, ExitKind::None) {
        g.exit_kind = kind;
        g.exit_code = code;
    }
}

fn long_jump_to_run_or_halt() -> ! {
    // Clear pointer-validation bounds so any straggling syscall after a fault
    // would refuse user pointers (defense in depth — there should be no such
    // syscall because we are about to long-jump out).
    crate::userland::abi::clear_user_va_bounds();

    if let Some(cont) = take_continuation() {
        unsafe {
            restore_continuation(&cont);
        }
    }
    // No continuation — `enter_user_mode` was never invoked. Falling back to
    // halting matches the U2 behavior (the safest possible answer when state
    // is suspect).
    crate::debug_error!("cleanup_user_process: no continuation saved; halting");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Restore the saved kernel continuation: load callee-saved regs, switch to
/// the saved RSP, and `jmp` to the saved RIP.
///
/// SAFETY: `cont` must point to a `KernelContinuation` previously written by
/// the matching `enter_user_mode_asm` setjmp prologue. The saved RSP must
/// reference a still-valid kernel stack frame (the run command's stack is
/// owned by the spawned process and remains live until that process exits;
/// the only way the stack would be invalid is if the run command had already
/// returned, which by construction has not happened — control is only here
/// because we're long-jumping *into* the run command's frame).
///
/// This function is `-> !` — control flow continues at `cont.rip` with the
/// run command's saved registers restored. The caller must not expect to
/// retain any live values across the jump.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn restore_continuation(cont: *const KernelContinuation) -> ! {
    core::arch::naked_asm!(
        // RDI = &KernelContinuation. Field offsets:
        //   0  rbx
        //   8  rbp
        //  16  r12
        //  24  r13
        //  32  r14
        //  40  r15
        //  48  rsp
        //  56  rip
        "mov rbx, [rdi + 0]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",
        "mov rsp, [rdi + 48]",
        // Push the saved RIP and RET. Equivalent to `jmp [rdi+56]`, but using
        // the call/ret protocol leaves the stack pre-aligned for the C ABI
        // expectation that `ret` lands at a 16-byte-aligned-after-call
        // boundary, which is exactly the state the saved frame represents.
        "mov rax, [rdi + 56]",
        "push rax",
        // Re-enable interrupts. We may have arrived here from an interrupt
        // gate (int 0x80 with IF auto-cleared) or from an exception handler
        // — the run command expects to resume with IF=1 because it is part
        // of the normal preemptive scheduler.
        "sti",
        "ret",
    );
}
