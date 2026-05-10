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

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;
use x86_64::VirtAddr;

use crate::userland::address_space::AddressSpace;
use crate::userland::fdtable::FdTable;
use crate::userland::image::UserImage;
use crate::userland::kernel_stack::KernelStack;
use crate::userland::signal::SignalState;

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
/// A ring-3 process. Each user binary launched by `enter_user_mode_with`
/// gets a fresh `Process` slot; the slot is removed when the long-jump
/// returns and the run command calls `release_active_image`.
///
/// Phase 4 PR-A: this replaces the prior single-process `ActiveUser`
/// global. Today only one slot is ever populated at a time (single-user-
/// app D5), but the table-of-Processes shape unblocks fork (PR-C) and
/// execve (PR-D) which need to address parent and child by PID.
pub struct Process {
    /// Stable per-process identifier. Allocated monotonically from
    /// `NEXT_PID` on `enter_user_mode_with`. Visible to ring 3 via
    /// `getpid`. PID 0 is reserved (kernel proper).
    pub pid: u32,
    /// PID of the parent that created us. For binaries launched by the
    /// `run` command, this is `0` (kernel as parent). For fork-spawned
    /// children (PR-C), this becomes the parent's PID.
    pub parent_pid: u32,
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
    /// Phase 2: file-descriptor table. Slots 0/1/2 are pinned to the
    /// standard streams; slots 3..N hold `Arc<File>` opened via `openat`.
    pub fd_table: FdTable,
    /// Phase 2: per-process current working directory. Anchors relative
    /// paths in `openat(AT_FDCWD, …)`, `stat`, `access`, etc. Always
    /// stored as a normalized absolute path.
    pub cwd: String,
    /// Phase 4 PR-B: per-process L4 page table. Owns the L4 frame. The
    /// option is `None` for the kernel-sentinel slot (PID 0); every
    /// real user process has a populated `AddressSpace`.
    pub address_space: Option<AddressSpace>,
    /// Phase 5 PR-B: signal actions, blocked mask, pending mask.
    /// Reset on every `enter_user_mode_with` and on `execve` (signals
    /// are not preserved across exec, per POSIX). Inherited by fork
    /// children (shallow copy is fine — there are no shared
    /// references inside SignalState).
    pub signal_state: SignalState,
    /// Phase 5 PR-C1: per-process kernel stack. The SYSCALL stub
    /// reads the rsp top from `gs:[0]`, which we update to point at
    /// this stack's `top()` whenever the process is the active one.
    /// Interrupt gates use TSS.rsp0 (also kept in sync). Each process
    /// gets its own buffer so parent + child syscall handlers don't
    /// share a single rsp0 area.
    pub kernel_stack: Option<KernelStack>,
    /// U3: launch path of the binary running in this process. Set by
    /// `enter_user_mode_with_aspace` from `argv[0]` when the run
    /// command (or `execve`) provides one; left `None` for synthetic
    /// in-kernel test launches that bypass argv. `readlink` reads
    /// this for `/proc/self/exe` (see `readlink_handler`).
    pub exe_path: Option<String>,
}

/// Compatibility alias retained for the long tail of callsites using
/// the old name. New code should refer to `Process` directly.
pub type ActiveUser = Process;

/// State of a user process. Phase 4 PR-C lays the type in place; the
/// transitions to `Zombie` and back to `Reaped` are wired up by `_exit`
/// and `waitpid` in a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ProcessState {
    /// Currently runnable / running.
    Running,
    /// Exited but not yet reaped by a `waitpid`. Holds the exit code.
    Zombie { exit_code: i64 },
    /// Reaped — slot is free.
    Reaped,
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

/// Reserved PID for "no current process." Real PIDs start at 1.
const KERNEL_PID: u32 = 0;

/// Monotonic PID allocator. Wrapping is unrealistic for our scope; we
/// stop the kernel before exhausting u32.
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// The single live `Process` slot. PR-A still enforces D5 (single user
/// app at a time). PR-C will replace this with a real `BTreeMap<pid,
/// Process>`-shaped table.
static CURRENT_PROCESS: Mutex<Process> = Mutex::new(Process {
    pid: KERNEL_PID,
    parent_pid: KERNEL_PID,
    continuation: None,
    image: None,
    exit_kind: ExitKind::None,
    exit_code: 0,
    brk_current: 0,
    mmap_next: 0,
    fd_table: FdTable::new(),
    cwd: String::new(),
    address_space: None,
    signal_state: SignalState::new(),
    kernel_stack: None,
    exe_path: None,
});

/// Acquire the current process slot for read/write. Used by syscall
/// handlers and the run command to install / drop the image and to
/// inspect the recorded exit info.
pub fn with_current_process<R>(f: impl FnOnce(&mut Process) -> R) -> R {
    let mut g = CURRENT_PROCESS.lock();
    f(&mut g)
}

/// Compatibility alias for old callsites — slated for removal as PR-C
/// lands. New code should use `with_current_process`.
pub fn with_active_user<R>(f: impl FnOnce(&mut Process) -> R) -> R {
    with_current_process(f)
}

/// Returns true while a user process owns the slot. The run command
/// uses this to enforce the single-user invariant (D5).
pub fn user_active() -> bool {
    CURRENT_PROCESS.lock().image.is_some()
}

/// Allocate a fresh PID. Used by `enter_user_mode_with` and (future)
/// `fork`.
pub fn alloc_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

/// Initialize the process slot for a new ring-3 binary launched by the
/// kernel (`run /HOST/...`). Assigns a fresh PID with `parent_pid =
/// KERNEL_PID`. Called by `enter_user_mode_with` before iretq.
pub fn install_new_process(
    image: UserImage,
    brk_base: u64,
    mmap_base: u64,
    address_space: AddressSpace,
) -> u32 {
    install_new_process_opt(image, brk_base, mmap_base, Some(address_space))
}

/// Variant that accepts an optional address space. The kernel-test
/// path bypasses the run command and runs binaries on the kernel L4
/// directly, passing `None`; production launches always pass `Some`.
pub fn install_new_process_opt(
    image: UserImage,
    brk_base: u64,
    mmap_base: u64,
    address_space: Option<AddressSpace>,
) -> u32 {
    let pid = alloc_pid();
    with_current_process(|p| {
        p.pid = pid;
        p.parent_pid = KERNEL_PID;
        p.image = Some(image);
        p.exit_kind = ExitKind::None;
        p.exit_code = 0;
        p.brk_current = brk_base;
        p.mmap_next = mmap_base;
        p.fd_table.clear();
        p.fd_table.install_default_streams();
        p.cwd = String::from("/host");
        p.address_space = address_space;
        p.signal_state = SignalState::new();
        p.kernel_stack = Some(KernelStack::new());
        p.exe_path = None;
    });
    pid
}

/// Returns the PID of the running process, or `KERNEL_PID` (0) if none.
pub fn current_pid() -> u32 {
    CURRENT_PROCESS.lock().pid
}

// ---------- Phase 4 PR-C2: parent stash + zombie table ----------

/// While `fork()` runs the child synchronously, the parent's `Process`
/// is moved out of `CURRENT_PROCESS` into here. When the child exits
/// (long-jumps back to `fork_handler`), the parent is moved back.
///
/// One slot — i.e., fork nesting depth = 1 — is sufficient for zsh's
/// pattern of "fork from main, child runs, parent waits." Deeply
/// nested fork (fork from inside a forked child) would need a stack;
/// trivial extension when needed.
static PARENT_STASH: Mutex<Option<Process>> = Mutex::new(None);

pub fn stash_parent(process: Process) {
    let mut g = PARENT_STASH.lock();
    debug_assert!(g.is_none(), "PARENT_STASH already occupied — nested fork not yet supported");
    *g = Some(process);
}

pub fn take_stashed_parent() -> Option<Process> {
    PARENT_STASH.lock().take()
}

pub fn parent_stashed() -> bool {
    PARENT_STASH.lock().is_some()
}

/// Raise a signal on the parent stashed by `fork()`. Used by the
/// child-exit path to set SIGCHLD pending on the parent before the
/// long-jump back. No-op if the stash is empty (the dying process is
/// the top-level kernel-launched binary, which has no userland
/// parent).
pub fn raise_signal_on_stashed_parent(sig: i32) {
    if let Some(parent) = PARENT_STASH.lock().as_mut() {
        parent.signal_state.raise(sig);
    }
}

/// Replace the current process with a fresh one, returning the previous.
pub fn swap_current_process(new: Process) -> Process {
    let mut g = CURRENT_PROCESS.lock();
    core::mem::replace(&mut *g, new)
}

/// Record of a child that exited but hasn't been reaped yet.
#[derive(Debug, Clone, Copy)]
pub struct ZombieRecord {
    pub exit_code: i64,
    pub parent_pid: u32,
}

static ZOMBIES: Mutex<BTreeMap<u32, ZombieRecord>> = Mutex::new(BTreeMap::new());

/// Mark `pid` as a zombie awaiting reap. Called by `_exit` / `exit_group`
/// when the dying process has a real parent (i.e. was forked).
pub fn record_zombie(pid: u32, parent_pid: u32, exit_code: i64) {
    ZOMBIES.lock().insert(pid, ZombieRecord { exit_code, parent_pid });
}

/// Reap a zombie child. If `target_pid` is positive, only that PID
/// matches; if `target_pid == -1` (any-child semantics), the first
/// zombie with `parent_pid == reaper` is returned. Returns `(pid,
/// exit_code)` on success, `None` if no matching zombie exists.
pub fn reap_zombie(target_pid: i32, reaper: u32) -> Option<(u32, i64)> {
    let mut zombies = ZOMBIES.lock();
    let pid = if target_pid == -1 {
        zombies
            .iter()
            .find(|(_, z)| z.parent_pid == reaper)
            .map(|(&k, _)| k)?
    } else if target_pid > 0 {
        let key = target_pid as u32;
        let z = zombies.get(&key)?;
        if z.parent_pid != reaper {
            return None;
        }
        key
    } else {
        return None;
    };
    let z = zombies.remove(&pid)?;
    Some((pid, z.exit_code))
}

/// Counts the number of active "loading-or-running a user binary" calls. The
/// kernel main loop reads this to skip GUI/render work while the run command
/// is reading the ELF, mapping pages, or executing the binary in ring 3 —
/// the long-running phases that pre-date the active-user slot being filled.
///
/// Use `BinaryLoadGuard` to bracket the run path; it increments the counter
/// on construction and decrements on drop so early-return / panic paths still
/// release the guard.
static BINARY_LOAD_DEPTH: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Returns true while a binary is being loaded or executed by the run command.
/// Covers the entire `read_to_vec → load_elf → enter_user_mode → ring-3 →
/// exit_group → release_active_image` window — `user_active()` only spans the
/// inner ring-3 portion.
pub fn binary_load_in_progress() -> bool {
    BINARY_LOAD_DEPTH.load(core::sync::atomic::Ordering::Acquire) > 0
}

/// RAII guard that marks the kernel as actively loading-or-running a user
/// binary. Construct on entry to the run path; drop releases the marker even
/// if an intermediate step bails out.
pub struct BinaryLoadGuard;

impl BinaryLoadGuard {
    pub fn enter() -> Self {
        BINARY_LOAD_DEPTH.fetch_add(1, core::sync::atomic::Ordering::Release);
        Self
    }
}

impl Drop for BinaryLoadGuard {
    fn drop(&mut self) {
        BINARY_LOAD_DEPTH.fetch_sub(1, core::sync::atomic::Ordering::Release);
    }
}

/// Save the kernel continuation. Called by `enter_user_mode` immediately
/// before issuing `iretq` to user space.
pub fn install_continuation(c: KernelContinuation) {
    CURRENT_PROCESS.lock().continuation = Some(c);
}

/// Take ownership of the active continuation, if any. Used by the long-jump
/// path before restoring registers — the slot is cleared so a second teardown
/// is a no-op.
pub fn take_continuation() -> Option<KernelContinuation> {
    CURRENT_PROCESS.lock().continuation.take()
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
    let mut g = CURRENT_PROCESS.lock();
    if matches!(g.exit_kind, ExitKind::None) {
        g.exit_kind = ExitKind::UnimplementedSyscall { nr };
        g.exit_code = -38; // ENOSYS sentinel for the run command's log
    }
    drop(g);
    long_jump_to_run_or_halt();
}

fn record_exit(kind: ExitKind, code: i64) {
    let mut g = CURRENT_PROCESS.lock();
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
