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
    current_user_pid, mark_ring3_blocked, restore_user_cpu_state, save_user_cpu_state,
    set_current_user_pid, Process, Ring3BlockReason, PROCESS_TABLE,
};
use crate::userland::user_state::UserState;

/// U8: save the calling kernel context into `kernel_save`, then
/// dispatch into ring-3 `pid`. Diverges — does not return to the
/// caller. When the ring-3 process eventually yields back via
/// [`yield_to_kernel_main_loop`] (which `switch_to_context`s to
/// `kernel_save`), control resumes at the caller of this function as
/// if it returned normally.
///
/// Used by a kernel entity or the idle/main context when the unified selector
/// chooses a user entity. The caller's stack frame remains live until that
/// kernel entity is scheduled again or the system returns to idle.
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
        // Leave the entity stack before publishing it. The per-CPU idle/main
        // stack is inactive while an entity is running and provides a safe
        // bridge into the ring-3 dispatcher.
        "mov r12, rdi",
        "mov r13, rsi",
        "cli",
        "mov rsp, gs:[{kernel_context_rsp_offset}]",
        "and rsp, -16",
        "mov rdi, r13",
        "call {publish}",
        "mov edi, r12d",
        "call {dispatch}",
        "ud2",
        publish = sym crate::arch::x86_64::context_switch::publish_handoff_context,
        dispatch = sym dispatch_resume_ring3_diverging,
        kernel_context_rsp_offset = const crate::arch::x86_64::percpu::KERNEL_CONTEXT_RSP_OFFSET,
    );
}

/// C-callable shim that diverges into `resume_ring3`. Used by
/// `save_kernel_and_resume_ring3`'s naked tail call.
#[no_mangle]
extern "C" fn dispatch_resume_ring3_diverging(pid: u32) -> ! {
    unsafe { resume_ring3(pid) }
}

/// Diverge from a ring-3 handler to the saved idle/main context when the
/// unified queue has no runnable entity.
///
/// Restores the kernel main loop's saved `CpuContext` (the one
/// captured by the initial/idle dispatcher), so control resumes immediately
/// after that dispatch call and can halt until an event makes work runnable.
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
/// Clears `current_user_pid` before diverging because no user address space is
/// loaded after the switch.
pub unsafe fn yield_to_kernel_main_loop() -> ! {
    use core::sync::atomic::Ordering;
    // No ring-3 process is loaded on this CPU after the switch.
    if let Some(pid) = crate::userland::lifecycle::current_user_pid() {
        crate::diagnostics::shadow::stack::begin_abandon(pid);
    }
    crate::userland::lifecycle::set_current_user_pid(None);
    crate::process::set_in_spawned_process(false);
    crate::mm::paging::activate_kernel_l4();
    crate::arch::x86_64::context_switch::switch_to_context(
        crate::arch::x86_64::percpu::kernel_context_ptr(),
    );
    // switch_to_context diverges — unreachable.
    let _ = Ordering::Release;
    loop {
        x86_64::instructions::hlt();
    }
}

/// Dispatch once from the unified run queue after the current user process
/// blocked or exited and its state has already been saved.
pub unsafe fn dispatch_after_user_stop() -> ! {
    // Keep the guard out of the match scrutinee: Rust extends a scrutinee
    // temporary through the selected arm, and both resume paths synchronize
    // scheduler state before diverging.
    let next = {
        crate::process::scheduler::SCHEDULER
            .lock()
            .schedule_entity()
    };
    match next {
        Some(crate::process::entity::EntityId::UserProcess(pid)) => resume_ring3(pid),
        Some(crate::process::entity::EntityId::KernelThread(pid)) => {
            crate::arch::x86_64::context_switch::resume_kernel_thread(pid)
        }
        None => yield_to_kernel_main_loop(),
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
    // Timer preemption saves an entity while still on its kernel stack. Move
    // to this CPU's idle/main stack before publishing that entity to the
    // shared run queue, otherwise another CPU can reuse the live stack.
    if crate::arch::x86_64::percpu::has_pending_context_publish()
        || crate::diagnostics::shadow::stack::has_pending_abandon()
    {
        resume_ring3_after_stack_handoff(pid)
    }
    resume_ring3_inner(pid)
}

#[unsafe(naked)]
unsafe extern "C" fn resume_ring3_after_stack_handoff(_pid: u32) -> ! {
    core::arch::naked_asm!(
        "cli",
        "mov rsp, gs:[{kernel_context_rsp_offset}]",
        "and rsp, -16",
        "call {dispatch}",
        "ud2",
        dispatch = sym publish_and_resume_ring3,
        kernel_context_rsp_offset = const crate::arch::x86_64::percpu::KERNEL_CONTEXT_RSP_OFFSET,
    );
}

extern "C" fn publish_and_resume_ring3(pid: u32) -> ! {
    crate::arch::x86_64::context_switch::publish_pending_context();
    unsafe { resume_ring3_inner(pid) }
}

unsafe fn resume_ring3_inner(pid: u32) -> ! {
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
    let (state_copy, kernel_continuation, l4, kstack) = {
        let mut g = PROCESS_TABLE.lock();
        let tgid = g.thread_groups.get(&pid).copied().unwrap_or(pid);
        let l4 = g.by_pid.get(&tgid).and_then(|group| {
            group
                .address_space
                .as_ref()
                .map(|space| (space.l4_frame(), space.shadow_generation()))
        });
        let p = g.by_pid.get_mut(&pid).expect("resume_ring3: unknown pid");
        let state = p.saved_user_state;
        let continuation = p.kernel_continuation.take().map(|saved| *saved);
        let stack = p
            .kernel_stack
            .as_ref()
            .map(|stack| (stack.top(), stack.shadow_generation()));
        (state, continuation, l4, stack)
    };

    // CR3 swap — only if the process has its own address space.
    if let Some((frame, generation)) = l4 {
        use x86_64::registers::control::{Cr3, Cr3Flags};
        Cr3::write(frame, Cr3Flags::empty());
        crate::diagnostics::shadow::address_space::activate(
            generation,
            frame.start_address().as_u64(),
        );
    }

    // TSS.rsp0 + per-CPU SYSCALL rsp top — per-process kernel stack
    // when present, otherwise leave at the global default.
    if let Some((top, generation)) = kstack {
        set_kernel_rsp0(top);
        set_percpu_kernel_rsp_top(top.as_u64());
        crate::diagnostics::shadow::stack::activate(generation, pid, top.as_u64());
    }

    // FS_BASE + FPU. Re-borrow under the lock briefly to get a
    // `&Process` for `restore_user_cpu_state`; both reads are cheap.
    crate::userland::lifecycle::with_process(pid, |p| restore_user_cpu_state(p))
        .expect("resume_ring3: process disappeared between snapshot and restore");

    set_current_user_pid(Some(pid));
    crate::process::set_in_spawned_process(true);

    if let Some(context) = kernel_continuation {
        crate::diagnostics::shadow::continuation::dispatch(pid, &context);
        crate::arch::x86_64::context_switch::switch_to_context(&context);
    }

    let (user_cs, user_ss) = user_selectors();

    // Diverge.
    resume_ring3_asm(&state_copy as *const UserState, user_cs, user_ss)
}

/// Park an in-progress ring-3 kernel operation until an exact block request
/// completes. The current ring-0 stack is retained and execution resumes at
/// the instruction after `switch_context`; the syscall or page fault is not
/// replayed.
pub fn block_current_ring3_on_io(token: u64) {
    use crate::arch::x86_64::context_switch::switch_context;
    use alloc::boxed::Box;

    let Some(pid) = current_user_pid() else {
        return;
    };
    let (old_context, stack_generation, stack_bottom, stack_top) = {
        let mut table = PROCESS_TABLE.lock();
        let process = table
            .by_pid
            .get_mut(&pid)
            .expect("block I/O for unknown ring-3 process");
        // The continuation resumes through `resume_ring3`, which restores the
        // process CPU image before jumping back into this kernel stack. Save
        // the live FS_BASE/FPU state first; otherwise a demand-page sleep
        // replaces the interrupted program's SSE registers with its stale
        // snapshot (often the all-zero fresh-process image).
        save_user_cpu_state(process);
        let saved = process
            .kernel_continuation
            .get_or_insert_with(|| Box::new(crate::process::CpuContext::default()));
        let pointer = (&mut **saved) as *mut crate::process::CpuContext;
        let stack_top = process
            .kernel_stack
            .as_ref()
            .expect("ring-3 I/O block without a kernel stack")
            .top()
            .as_u64();
        let stack_generation = process
            .kernel_stack
            .as_ref()
            .expect("ring-3 I/O block without a kernel stack")
            .shadow_generation();
        let stack_bottom = stack_top - crate::userland::kernel_stack::KERNEL_STACK_BYTES as u64;
        table
            .ring3_blocked
            .insert(pid, Ring3BlockReason::WaitingForBlockIo { token });
        (pointer, stack_generation, stack_bottom, stack_top)
    };
    crate::diagnostics::shadow::continuation::allocate(
        pid,
        token,
        stack_generation,
        stack_bottom,
        stack_top,
    );
    crate::process::scheduler::SCHEDULER
        .lock()
        .mark_context_saving(crate::process::entity::EntityId::UserProcess(pid));
    crate::arch::x86_64::percpu::set_pending_user_context_publish(pid);
    crate::arch::x86_64::percpu::set_current_user_pid(None);
    crate::process::scheduler::SCHEDULER
        .lock()
        .block_entity(crate::process::entity::EntityId::UserProcess(pid));

    crate::process::set_in_spawned_process(false);
    unsafe {
        crate::mm::paging::activate_kernel_l4();
        switch_context(
            old_context,
            crate::arch::x86_64::percpu::kernel_context_ptr(),
        );
    }
    crate::diagnostics::shadow::continuation::consumed(pid, token);
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
/// 2. Mark the process blocked with `reason` and remove its entity from the
///    unified run queue.
/// 3. Select the next tagged entity and resume either privilege class. If the
///    queue is empty, return to the saved idle/main context.
///
/// The wake side makes the tagged entity ready (e.g.,
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

    // A readiness producer may have fired after the syscall scanned its fds
    // but before the blocked reason became visible. Producers increment the
    // sequence before waking, so a post-publication recheck closes both sides
    // of that race. The conservative wake removes us from the blocked index
    // and makes the saved continuation runnable again before dispatch.
    if let Ring3BlockReason::WaitingForReadiness {
        observed_sequence, ..
    } = reason
    {
        if crate::userland::readiness::changed_since(observed_sequence) {
            crate::userland::lifecycle::wake_ring3_blocked_on_readiness();
        }
    }

    crate::diagnostics::shadow::stack::begin_abandon(me);
    dispatch_after_user_stop()
}

/// Save a successful post-SYSCALL continuation, put the current ring-3 entity
/// back on the ready queue, and dispatch another entity. Unlike a blocking
/// syscall, RIP is not rewound and RAX is the successful return value zero.
pub unsafe fn yield_current_ring3(args: &SyscallArgs) -> ! {
    let raw = args as *const SyscallArgs as *const u64;
    let user_rip = core::ptr::read(raw.add(7));
    let user_rflags = core::ptr::read(raw.add(8));
    let saved = crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs);
    let user_r12 = crate::userland::user_state::read_user_r12(args as *const SyscallArgs);
    let snapshot = UserState {
        rax: 0,
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: saved.rbx,
        rbp: saved.rbp,
        rsp: saved.r12_register,
        r12: user_r12,
        r13: saved.r13,
        r14: saved.r14,
        r15: saved.r15,
        rip: user_rip,
        rflags: user_rflags,
        rcx: 0,
        r11: 0,
    };
    let me = current_user_pid().expect("yield_current_ring3: no current ring-3 process");
    {
        let mut table = PROCESS_TABLE.lock();
        let process = table
            .by_pid
            .get_mut(&me)
            .expect("yield_current_ring3: current task missing");
        process.saved_user_state = snapshot;
        save_user_cpu_state(process);
    }
    crate::process::scheduler::SCHEDULER
        .lock()
        .yield_entity(crate::process::entity::EntityId::UserProcess(me));
    crate::diagnostics::shadow::stack::begin_abandon(me);
    dispatch_after_user_stop()
}
