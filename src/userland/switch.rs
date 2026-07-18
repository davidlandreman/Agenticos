//! U4: ring-3 ↔ ring-3 switch primitive.
//!
//! This module owns the asm + Rust glue that swaps the CPU between two
//! ring-3 processes. Two entry points:
//!
//! - [`save_ring3`] — snapshots the *currently-loaded* ring-3 process's
//!   user GPRs (read from a trap-frame struct) and CPU state (FS_BASE +
//!   FPU) into its `Process` slot. Non-diverging Rust helper; called
//!   from the timer ISR's Rust-side handler (U5) just before deciding
//!   what to run next.
//! - [`resume_ring3`] — diverging primitive that loads another
//!   process's state (CR3, kernel stack, FS_BASE, FPU, all 16 user
//!   GPRs + RIP + RFLAGS + RSP) and `iretq`s into ring 3.
//!
//! ## What this unit does NOT do (deferred)
//!
//! - **Wire either primitive into the timer ISR.** U5 owns that
//!   integration. Today the timer ISR still short-circuits on CPL=3
//!   (`preemption.rs:258-270`); this module's functions are exercised
//!   only by the characterization tests in `src/tests/userland_switch.rs`.
//! - **Replace `enter_user_mode_asm` for first-launch.** First entry to
//!   ring 3 still goes through the setjmp/iretq path in
//!   `src/userland/mod.rs`. `resume_ring3` is only invoked once a
//!   process has been preempted-out at least once and has a populated
//!   `Process.saved_user_state` (the field U4 also added). U7/U8
//!   revisit unifying the entry paths.
//! - **Save/restore on syscall entry.** Locked in by U2 — the SYSCALL
//!   fast path leaves FS_BASE/FPU alone. The kernel is `+soft-float`,
//!   so XMM survives kernel transitions for free.
//!
//! ## Lock ordering
//!
//! `resume_ring3` acquires `PROCESS_TABLE.lock()`, copies the snapshot
//! out, and **releases the lock before** the CR3 swap and the asm
//! transition. The snapshot copy is on the wrapper's kernel stack —
//! once the asm reads from it (during the iretq frame build), no
//! further mutations can land because we're inside the `iretq`
//! itself.
//!
//! The brief re-acquisition of `PROCESS_TABLE` inside
//! `restore_user_cpu_state` (via `with_process`) is safe: the table
//! never blocks the holder, and no other path mutates the process's
//! FS_BASE / FPU fields between the snapshot and the resume in the
//! current design (U5 audits this when wiring the timer ISR).
//!
//! ## ABI contract with the asm
//!
//! `resume_ring3_asm` reads `UserState` by offset. Layout is locked
//! by the static `_SIZE_CHECK` in `user_state.rs` plus the
//! load-bearing comment there. Any field reordering in `UserState`
//! requires updating the asm offsets in lock-step.

use crate::arch::x86_64::preemption::InterruptStackFrame;
use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::lifecycle::{
    current_user_pid, mark_ring3_blocked, pop_next_ring3, restore_user_cpu_state,
    save_user_cpu_state, set_current_user_pid, Process, Ring3BlockReason, PROCESS_TABLE,
};
use crate::userland::user_state::UserState;

/// U8: save the calling kernel context into `kernel_save`, then
/// dispatch into ring-3 `pid`. Diverges — does not return to the
/// caller. When the ring-3 process eventually yields back via
/// [`yield_to_kernel_main_loop`] (which `switch_to_context`s to
/// `kernel_save`), control resumes at the caller of this function as
/// if it returned normally.
///
/// Used by the kernel main loop / idle path to dispatch a
/// `ring3_ready` process while keeping the kernel main loop's stack
/// frame live for the yield-back. Without this, calling
/// `resume_ring3` directly from the main loop would diverge and
/// `yield_to_kernel_main_loop`'s `switch_to_context(KERNEL_CONTEXT)`
/// would jump to a stale RIP/RSP.
///
/// `kernel_save` must outlive the entire ring-3 execution chain.
/// Practical caller: pass `&raw mut KERNEL_CONTEXT` (the static
/// shared with the timer ISR's kernel-return path).
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn save_kernel_and_resume_ring3(
    _pid: u32,                                     // RDI
    _kernel_save: *mut crate::process::CpuContext, // RSI
) {
    core::arch::naked_asm!(
        // Save kernel callee-saved registers into [rsi].
        "mov [rsi + 0], rbx",
        "mov [rsi + 8], rbp",
        "mov [rsi + 16], r12",
        "mov [rsi + 24], r13",
        "mov [rsi + 32], r14",
        "mov [rsi + 40], r15",
        // RSP at resume: just above the return address pushed by our
        // caller (i.e., RSP value the caller had immediately before
        // calling us). switch_to_context loads RSP and jmps to RIP,
        // so this RSP must point at a usable kernel stack frame.
        "lea rax, [rsp + 8]",
        "mov [rsi + 48], rax",
        // RIP at resume = the return address sitting at [rsp].
        "mov rax, [rsp]",
        "mov [rsi + 56], rax",
        // Save RFLAGS for cleanliness.
        "pushfq",
        "pop rax",
        "mov [rsi + 64], rax",
        // Kernel CS=0x08, SS=0x10 — the same selectors the kernel
        // was running on when save_kernel_and_resume_ring3 was called.
        "mov qword ptr [rsi + 144], 0x08",
        "mov qword ptr [rsi + 152], 0x10",
        // Tail-call resume_ring3(pid). RDI is already pid; jmp (not
        // call) so the stack stays in the shape we just saved.
        "jmp {dispatch}",
        dispatch = sym dispatch_resume_ring3_diverging,
    );
}

/// C-callable shim that diverges into `resume_ring3`. Used by
/// `save_kernel_and_resume_ring3`'s naked tail call.
#[no_mangle]
extern "C" fn dispatch_resume_ring3_diverging(pid: u32) -> ! {
    unsafe { resume_ring3(pid) }
}

/// U8: diverge from a ring-3 syscall handler back to the kernel main
/// loop. Used when a ring-3 process must yield (block, exit) and
/// there is no other ring-3 process runnable to switch into.
///
/// Restores the kernel main loop's saved `CpuContext` (the one
/// captured by `try_run_scheduled_processes`' invocation of
/// `switch_context_full_restore`), so control resumes in the kernel
/// main loop right after that call. The kernel main loop then loops
/// back and re-invokes the scheduler, which picks up any newly-Ready
/// kernel threads or runs `idle` (which itself checks ring3_ready
/// and `resume_ring3`s if any).
///
/// The dying syscall handler's kernel-stack frame is abandoned. The
/// process's `Process.kernel_stack` is the storage; nothing dangles
/// because `Process.saved_user_state` carries the ring-3 resume
/// info, and any next ring 3→0 transition lands at
/// `kernel_stack.top()`, overwriting the abandoned frame.
///
/// SAFETY: must be called from a context where abandoning the kernel
/// stack is acceptable (i.e., the calling syscall handler diverges)
/// AND `KERNEL_CONTEXT` was previously captured by the kernel main
/// loop's `switch_context_full_restore` call (or by
/// `save_kernel_and_resume_ring3`). Both hold for the standard U8
/// ring-3 block paths.
///
/// Clears `current_user_pid` before diverging — the kernel main loop
/// reads it to decide whether to dispatch the next ring3_ready
/// process. If we leave it set to the blocked process, the main loop
/// thinks ring 3 is still running and never wakes the queued ones.
pub unsafe fn yield_to_kernel_main_loop() -> ! {
    use core::sync::atomic::Ordering;
    // U10/bugfix: no ring-3 process is loaded on this CPU anymore —
    // we're about to switch back to the kernel main loop. The next
    // ring-3 dispatch comes from `save_kernel_and_resume_ring3`,
    // which gates on `current_user_pid().is_none()`.
    crate::userland::lifecycle::set_current_user_pid(None);
    crate::process::set_in_spawned_process(false);
    crate::arch::x86_64::context_switch::switch_to_context(
        &raw const crate::arch::x86_64::preemption::KERNEL_CONTEXT,
    );
    // switch_to_context diverges — unreachable.
    let _ = Ordering::Release;
    loop {
        x86_64::instructions::hlt();
    }
}

/// Snapshot the trap-frame GPRs + CPU state into `p.saved_user_state`,
/// `p.fs_base`, and `p.fpu_state`.
///
/// Must run on the live CPU before any kernel code between the trap
/// and now has clobbered FS_BASE or touched XMM. The kernel target
/// is `+soft-float`, so XMM is safe by construction; FS_BASE is safe
/// as long as no `arch_prctl(ARCH_SET_FS)` or `wrmsr` fires between
/// the trap and this call. The timer-ISR naked prologue + Rust shim
/// satisfy both: neither emits SSE nor touches FS_BASE before
/// reaching this function.
pub fn save_ring3(p: &mut Process, frame: &InterruptStackFrame) {
    p.saved_user_state = UserState {
        rax: frame.rax,
        rdi: frame.rdi,
        rsi: frame.rsi,
        rdx: frame.rdx,
        r10: frame.r10,
        r8: frame.r8,
        r9: frame.r9,
        rbx: frame.rbx,
        rbp: frame.rbp,
        rsp: frame.rsp,
        r12: frame.r12,
        r13: frame.r13,
        r14: frame.r14,
        r15: frame.r15,
        rip: frame.rip,
        rflags: frame.rflags,
        rcx: frame.rcx,
        r11: frame.r11,
    };
    save_user_cpu_state(p);
}

/// Resume ring 3 in process `pid`. Diverges: returns control to ring 3
/// via `iretq` and never falls through to the caller.
///
/// Wiring sequence:
///
/// 1. Snapshot `saved_user_state` + L4 frame ref + kernel stack top
///    under `PROCESS_TABLE.lock()`.
/// 2. Release the lock.
/// 3. Activate `pid`'s address space (CR3 write).
/// 4. Update TSS.rsp0 + the SYSCALL stub's GSBASE-stored kernel rsp top
///    so the next ring 3 → ring 0 transition lands on `pid`'s
///    kernel stack.
/// 5. Restore FS_BASE + FPU register file from `pid`'s buffers.
/// 6. Mark `pid` as the currently-loaded ring-3 process.
/// 7. Jump into [`resume_ring3_asm`] which builds an iretq frame and
///    transfers to ring 3.
///
/// SAFETY: `pid` must exist in `PROCESS_TABLE` and must have a
/// populated `address_space` and `kernel_stack` (i.e., be a real
/// ring-3 process, not the kernel sentinel at PID 0). The
/// process's `saved_user_state` must describe a valid ring-3 entry
/// point: `rip` inside a USER-mapped executable page in `pid`'s L4,
/// `rsp` inside a USER-mapped writable page in the same L4, `rflags`
/// with IF set so the resumed process can be preempted. Callers
/// passing a process whose `saved_user_state` is still the
/// zero-initialized default will jump to RIP=0 and immediately
/// page-fault.
///
/// Caller is responsible for ensuring interrupts are in a sane state
/// — most call sites (U5+) invoke this from inside an interrupt
/// handler with interrupts disabled; the iretq itself re-enables
/// them via the saved RFLAGS.
pub unsafe fn resume_ring3(pid: u32) -> ! {
    use crate::arch::x86_64::gdt::{set_kernel_rsp0, user_selectors};
    use crate::arch::x86_64::syscall::set_percpu_kernel_rsp_top;

    // Snapshot what the asm needs, plus the per-CPU side-effect inputs.
    // Keep the lock window short: copy out, then release. The address
    // space activation and MSR writes happen with no lock held.
    //
    // address_space and kernel_stack are Option for synthetic test
    // paths (kernel-only L4, no per-process kstack). Real ring-3
    // launches always populate both; tests skip them and rely on
    // staying on the kernel L4 + global rsp0 stack.
    let (state_copy, l4_frame, kstack_top) = {
        let mut g = PROCESS_TABLE.lock();
        let p = g.by_pid.get_mut(&pid).expect("resume_ring3: unknown pid");
        let state = p.saved_user_state;
        let l4 = p.address_space.as_ref().map(|a| a.l4_frame());
        let top = p.kernel_stack.as_ref().map(|k| k.top());
        (state, l4, top)
    };

    // CR3 swap — only if the process has its own address space.
    if let Some(frame) = l4_frame {
        use x86_64::registers::control::{Cr3, Cr3Flags};
        Cr3::write(frame, Cr3Flags::empty());
    }

    // TSS.rsp0 + per-CPU SYSCALL rsp top — per-process kernel stack
    // when present, otherwise leave at the global default.
    if let Some(top) = kstack_top {
        set_kernel_rsp0(top);
        set_percpu_kernel_rsp_top(top.as_u64());
    }

    // FS_BASE + FPU. Re-borrow under the lock briefly to get a
    // `&Process` for `restore_user_cpu_state`; both reads are cheap.
    crate::userland::lifecycle::with_process(pid, |p| restore_user_cpu_state(p))
        .expect("resume_ring3: process disappeared between snapshot and restore");

    set_current_user_pid(Some(pid));

    let (user_cs, user_ss) = user_selectors();

    // Diverge.
    resume_ring3_asm(&state_copy as *const UserState, user_cs, user_ss)
}

/// Naked asm that builds an iretq frame from `state` and transfers to
/// ring 3. Diverges. Identical in shape to the back half of
/// `iretq_to_user_with_regs` (which is the path `execve` uses today),
/// minus the setjmp/`KernelContinuation` install that wraps the older
/// `enter_user_mode_with_regs_asm` — `resume_ring3` has no kernel
/// frame to longjmp back to.
///
/// `UserState` field offsets (must match `src/userland/user_state.rs`):
///
/// ```text
///   rax = 0    rdi = 8    rsi = 16   rdx = 24
///   r10 = 32   r8  = 40   r9  = 48
///   rbx = 56   rbp = 64   rsp = 72
///   r12 = 80   r13 = 88   r14 = 96   r15 = 104
///   rip = 112  rflags = 120  rcx = 128  r11 = 136
/// ```
///
/// SAFETY: see [`resume_ring3`]. Calling this with invalid offsets,
/// a malformed `UserState`, or while the CPU is in an inconsistent
/// state (CR3 not pointing at the target process's L4, kernel stack
/// not pointing at the target process's kernel stack, FS_BASE / FPU
/// not restored) will fault inside ring 3 or worse.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "sysv64" fn resume_ring3_asm(
    _state: *const UserState, // RDI
    _user_cs: u64,            // RSI
    _user_ss: u64,            // RDX
) -> ! {
    core::arch::naked_asm!(
        // Build iretq frame (CPU pops in order RIP, CS, RFLAGS, RSP, SS;
        // we push reverse: SS, RSP, RFLAGS, CS, RIP).
        "push rdx", // SS
        "mov rax, [rdi + 72]",
        "push rax", // user RSP
        "mov rax, [rdi + 120]",
        "push rax", // RFLAGS
        "push rsi", // CS
        "mov rax, [rdi + 112]",
        "push rax", // RIP
        // Load user GPRs. Keep RDI live as the state pointer until
        // the very last move so the other loads can index off it.
        "mov r11, rdi",
        "mov rax, [r11 + 0]",
        "mov rbx, [r11 + 56]",
        "mov rbp, [r11 + 64]",
        "mov r12, [r11 + 80]",
        "mov r13, [r11 + 88]",
        "mov r14, [r11 + 96]",
        "mov r15, [r11 + 104]",
        "mov r10, [r11 + 32]",
        "mov r8,  [r11 + 40]",
        "mov r9,  [r11 + 48]",
        "mov rdx, [r11 + 24]",
        "mov rsi, [r11 + 16]",
        "mov rdi, [r11 + 8]", // RDI last — was the state ptr.
        // Unlike a SYSCALL return, a timer interrupt may land while
        // RCX/R11 contain live values. Restore both, replacing the state
        // pointer in R11 only after every other indexed load is complete.
        "mov rcx, [r11 + 128]",
        "mov r11, [r11 + 136]",
        "iretq",
    );
}

// ---------- U6: block-and-yield from a SYSCALL handler ----------

/// Block the currently-loaded ring-3 process on `reason` and yield to
/// the next runnable ring-3 process (or back to the kernel main loop
/// if none). Diverges — does not return to the caller's syscall
/// handler.
///
/// Called from a SYSCALL handler that has decided to park (e.g.,
/// [`crate::userland::syscalls::wait4_handler`] when no matching
/// zombie is yet present, or `read_stdin_blocking` when the input
/// queue is empty). The flow:
///
/// 1. Capture the user-mode CPU state (all 16 GPRs + RIP + RFLAGS +
///    RSP) into the calling process's [`Process::saved_user_state`].
///    RIP is rewound **2 bytes** so the resumed process re-executes
///    the `SYSCALL` instruction (`0F 05`, 2 bytes) — the kernel then
///    re-enters the same syscall handler, which can re-check its
///    condition and either return normally or block again.
/// 2. Mark the process blocked with `reason` (removes from
///    `ring3_ready`, inserts into `ring3_blocked`).
/// 3. Pop the next runnable ring-3 process from `ring3_ready` and
///    [`resume_ring3`] it. If empty, [`yield_to_kernel_main_loop`].
///
/// The wake side moves us from `ring3_blocked` to `ring3_ready` (e.g.,
/// [`crate::userland::lifecycle::wake_ring3_blocked_on_child`] or
/// [`crate::userland::lifecycle::wake_ring3_blocked_on_input`]).
/// The kernel's idle process (or another ring-3 yielding) eventually
/// picks us up via `resume_ring3`, our SYSCALL re-fires, and the
/// handler returns normally.
///
/// ## Re-firing SYSCALL on resume
///
/// The SYSCALL instruction reads its number from RAX. To make the
/// re-fire dispatch to the same handler, `saved_user_state.rax` must
/// equal the original syscall number. The caller passes the
/// `SyscallArgs` it received — `args.rax` holds the original number
/// (the SYSCALL stub put it there and the dispatcher never mutates
/// that field — it only returns through the stack-allocated return
/// slot).
///
/// User callee-saved registers are recovered via
/// [`crate::userland::user_state::read_user_callee_saved`], which reads
/// the slots the SYSCALL stub explicitly pushed at known offsets above
/// `SyscallArgs`. Callers only need to pass `args` — no separate
/// snapshot needs to be captured at handler entry. (Earlier versions
/// of this API took a `CalleeSavedSnapshot` captured via a naked-asm
/// helper from inside the dispatcher; that helper read live registers
/// after Rust's prologue had already clobbered them with scratch
/// values, corrupting user rbx/rbp/r13-r15 across blocking syscalls.
/// See `docs/solutions/learnings/` for the post-mortem.)
pub unsafe fn block_current_ring3_and_yield(args: &SyscallArgs, reason: Ring3BlockReason) -> ! {
    // Read user state from the SYSCALL stub's saved slots above
    // SyscallArgs. Layout post-stub-fix:
    //   args +  56 = saved RCX (user RIP, post-SYSCALL)
    //   args +  64 = saved R11 (user RFLAGS)
    //   args +  72..+104 = user rbx/rbp/r13/r14/r15
    //   args + 112 = saved user R12
    //   gs:[8]      = saved user RSP
    let (user_rip_post, user_rflags) = {
        let p = args as *const SyscallArgs as *const u64;
        (core::ptr::read(p.add(7)), core::ptr::read(p.add(8)))
    };
    let saved = crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs);
    let user_r12 = crate::userland::user_state::read_user_r12(args as *const SyscallArgs);

    // SYSCALL is 2 bytes (0F 05). Rewinding RIP makes the resumed
    // process re-execute it — the kernel re-enters this syscall
    // handler with the same args, re-checks its blocking condition,
    // and either returns normally or blocks again.
    let resumed_rip = user_rip_post.wrapping_sub(2);

    let snapshot = UserState {
        rax: args.rax, // original syscall number — needed for re-fire dispatch
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: saved.rbx,
        rbp: saved.rbp,
        rsp: saved.r12_register, // = user RSP (gs:[8] via read_user_rsp)
        r12: user_r12,
        r13: saved.r13,
        r14: saved.r14,
        r15: saved.r15,
        rip: resumed_rip,
        rflags: user_rflags,
        // SYSCALL defines RCX/R11 as clobbered on return.
        rcx: 0,
        r11: 0,
    };

    let me = current_user_pid().expect("block_current_ring3_and_yield: no current ring-3 process");

    // Stamp snapshot + CPU state (FS_BASE + FPU) into the slot, then
    // mark blocked. Order matters: snapshot first so the wake path
    // (which moves us from blocked → ready) doesn't race with a
    // partially-written saved_user_state.
    {
        let mut g = PROCESS_TABLE.lock();
        let p = g
            .by_pid
            .get_mut(&me)
            .expect("block_current_ring3_or_panic: own pid not in table");
        p.saved_user_state = snapshot;
        save_user_cpu_state(p);
    }
    mark_ring3_blocked(me, reason);

    // U8: prefer to switch into another runnable ring-3 process; if
    // none, yield back to the kernel main loop. The idle process /
    // kernel-thread scheduler will eventually pick up state changes
    // (e.g., a forked sibling becoming Ready) and resume_ring3 us
    // later via the idle-loop's ring3_ready check.
    if let Some(next) = pop_next_ring3() {
        resume_ring3(next)
    } else {
        yield_to_kernel_main_loop()
    }
}
