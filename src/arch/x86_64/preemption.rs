//! Timer-based preemption support
//!
//! This module implements the low-level timer interrupt handler that supports
//! true preemptive multitasking. When a timer interrupt fires during process
//! execution, this handler saves the full CPU state and can switch to another
//! process without any cooperation from the running process.

use crate::process::context::CpuContext;
use core::arch::naked_asm;
use core::sync::atomic::AtomicU64;

/// Watchdog timeout in timer ticks (1000 ticks = 10 seconds at 100Hz).
/// If a process runs this long without yielding, sleeping, or making progress,
/// it will be killed by the watchdog.
pub const WATCHDOG_TIMEOUT_TICKS: u64 = 1000;

/// PID of process to be killed by watchdog (0 = none).
/// Set by timer interrupt, handled by kernel main loop.
/// We can't kill in interrupt context, so we defer to main loop.
pub static WATCHDOG_KILL_PID: AtomicU64 = AtomicU64::new(0);

/// Kernel context to return to (set by try_run_scheduled_processes before switching to a process)
#[no_mangle]
pub static mut KERNEL_CONTEXT: CpuContext = CpuContext::new();

/// The actual timer interrupt handler with preemption support.
///
/// This is a naked function that:
/// 1. Saves all registers to the stack
/// 2. Calls the Rust handler to check for preemption
/// 3. Either restores registers and returns normally, OR
/// 4. Switches to a different process context
///
/// The interrupt frame pushed by CPU is: SS, RSP, RFLAGS, CS, RIP (from high to low addr)
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn timer_interrupt_handler_preemptive() {
    naked_asm!(
        // Save all general purpose registers
        // The CPU already pushed SS, RSP, RFLAGS, CS, RIP
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",

        // Pass RSP as argument to the Rust handler (points to saved registers)
        "mov rdi, rsp",

        // Call the Rust handler
        "call {timer_handler_inner}",

        // If the Rust handler selects another entity it diverges directly.
        // Returning here therefore always means resume this exact frame.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",
        timer_handler_inner = sym timer_handler_inner,
    );
}

/// Stack frame layout after pushing all registers in the interrupt handler.
/// This matches the order we push registers in the assembly above.
///
/// **Load-bearing layout.** Field order corresponds to the naked-asm
/// `push` sequence in `timer_interrupt_handler_preemptive` (high address
/// first — `r15` was pushed last, so it sits at the lowest offset). The
/// U4 ring-3 switch primitive (`crate::userland::switch::save_ring3`)
/// reads from this layout to snapshot user GPRs into the
/// process's `saved_user_state`; reordering or inserting fields without
/// also updating the asm push sequence and `save_ring3` would silently
/// copy the wrong registers on every preempt.
///
/// `pub(crate)` so the userland subsystem can take a `&InterruptStackFrame`
/// without re-declaring the layout.
#[repr(C)]
pub(crate) struct InterruptStackFrame {
    // Registers we pushed (in reverse order, so first pushed = highest address)
    pub(crate) r15: u64,
    pub(crate) r14: u64,
    pub(crate) r13: u64,
    pub(crate) r12: u64,
    pub(crate) r11: u64,
    pub(crate) r10: u64,
    pub(crate) r9: u64,
    pub(crate) r8: u64,
    pub(crate) rbp: u64,
    pub(crate) rdi: u64,
    pub(crate) rsi: u64,
    pub(crate) rdx: u64,
    pub(crate) rcx: u64,
    pub(crate) rbx: u64,
    pub(crate) rax: u64,
    // CPU-pushed interrupt frame
    pub(crate) rip: u64,
    pub(crate) cs: u64,
    pub(crate) rflags: u64,
    pub(crate) rsp: u64,
    pub(crate) ss: u64,
}

// Static layout assertions — guard the asm-side contract. Every field
// is u64, so offsets are 8 × position. If a future refactor inserts a
// field or changes the type, the const evaluation fires at compile time
// before anything boots with the wrong layout.
const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(InterruptStackFrame, r15) == 0);
    assert!(offset_of!(InterruptStackFrame, rax) == 14 * 8);
    assert!(offset_of!(InterruptStackFrame, rip) == 15 * 8);
    assert!(offset_of!(InterruptStackFrame, cs) == 16 * 8);
    assert!(offset_of!(InterruptStackFrame, rflags) == 17 * 8);
    assert!(offset_of!(InterruptStackFrame, rsp) == 18 * 8);
    assert!(offset_of!(InterruptStackFrame, ss) == 19 * 8);
    assert!(core::mem::size_of::<InterruptStackFrame>() == 20 * 8);
};

/// Inner handler called from the naked interrupt handler.
/// Checks if preemption is needed and sets up context switch if so.
#[no_mangle]
extern "C" fn timer_handler_inner(stack_frame: *mut InterruptStackFrame) {
    use crate::arch::x86_64::interrupts::{InterruptIndex, PICS, TIMER_TICKS};
    use core::sync::atomic::Ordering;

    // Increment tick counter
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = crate::process::drain_kernel_io_wakes();
    crate::process::timer::on_tick(ticks);

    // Ring-3 timer trap: save the user state, requeue the tagged entity, and
    // select once from the same queue used for kernel threads. The selected
    // target may be either privilege class. The CPL=0 saver below must not run
    // on this frame because its register image belongs to UserState, not a
    // kernel CpuContext.
    let frame = unsafe { &*stack_frame };
    if (frame.cs & 3) == 3 {
        // Charge this tick to the running ring-3 process's CPU-time
        // accounting. `current_user_pid` names the interrupted process
        // (resume_ring3's atomic swap invariant). try_lock only: with
        // IF cleared here no holder can be preempted mid-hold, but a
        // dropped sample under contention is harmless for accounting.
        if let Some(mut table) = crate::userland::lifecycle::PROCESS_TABLE.try_lock() {
            if let Some(pid) = table.current_user_pid {
                if let Some(p) = table.by_pid.get_mut(&pid) {
                    p.utime_ticks = p.utime_ticks.saturating_add(1);
                }
            }
        }
        if let Some(mut sched) = crate::process::scheduler::SCHEDULER.try_lock() {
            if let Some(current_pid) = sched.current() {
                if let Some(pcb) = sched.get_process_mut(current_pid) {
                    pcb.last_activity_tick = ticks;
                }
            }
        }
        // EOI before any potential iretq so the next ring-3 process
        // (or this same one resuming) sees a clean PIC for the next
        // tick. Interrupts are disabled in this handler, so EOI'ing
        // early can't cause re-entry.
        unsafe {
            PICS.lock()
                .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
        }
        let Some(current_pid) = crate::userland::lifecycle::current_user_pid() else {
            return;
        };
        let saved = crate::userland::lifecycle::with_process(current_pid, |process| {
            crate::userland::switch::save_ring3(process, frame);
        })
        .is_some();
        if !saved {
            return;
        }
        let current = crate::process::entity::EntityId::UserProcess(current_pid);
        let next = crate::process::scheduler::SCHEDULER
            .lock()
            .preempt_and_pick(current);
        match next {
            Some(crate::process::entity::EntityId::UserProcess(pid)) if pid != current_pid => unsafe {
                crate::userland::switch::resume_ring3(pid)
            },
            Some(crate::process::entity::EntityId::KernelThread(pid)) => unsafe {
                crate::arch::x86_64::context_switch::resume_kernel_thread(pid)
            },
            Some(crate::process::entity::EntityId::UserProcess(_)) | None => {}
        }
        return;
    }

    // A protected kernel critical section receives the clock edge but defers
    // the scheduling decision. Timer expiry itself is always deferred to the
    // bounded timer-service worker.
    if !crate::arch::x86_64::preemption_guard::kernel_preemption_allowed() {
        unsafe {
            PICS.lock()
                .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
        }
        return;
    }

    // Check if we're running a spawned process
    let in_process = crate::process::is_in_spawned_process();

    // Try to acquire the scheduler for accounting and preemption.
    let should_preempt = if let Some(mut sched) = crate::process::scheduler::SCHEDULER.try_lock() {
        if sched.is_initialized() {
            // Watchdog check: detect hung processes
            if in_process {
                if let Some(current_pid) = sched.current() {
                    // Skip idle process
                    if sched.idle_pid != Some(current_pid) {
                        if let Some(pcb) = sched.get_process(current_pid) {
                            let elapsed = ticks.saturating_sub(pcb.last_activity_tick);
                            if elapsed > WATCHDOG_TIMEOUT_TICKS {
                                // Process is hung! Request kill (handled in kernel loop)
                                // Only set if not already set (don't override pending kill)
                                if WATCHDOG_KILL_PID.load(Ordering::Relaxed) == 0 {
                                    crate::debug_warn!(
                                        "WATCHDOG: Process {:?} '{}' unresponsive for {} ticks",
                                        current_pid,
                                        pcb.name,
                                        elapsed
                                    );
                                    WATCHDOG_KILL_PID.store(current_pid as u64, Ordering::Release);
                                }
                            }
                        }
                    }
                }
            }

            // Only check for preemption if we're in a spawned process
            if in_process {
                sched.timer_tick()
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    let mut user_target = None;
    let mut kernel_target = None;
    if should_preempt && in_process {
        // Save current process context from the interrupt stack frame
        if let Some(mut sched) = crate::process::scheduler::SCHEDULER.try_lock() {
            if let Some(current_pid) = sched.current() {
                if let Some(ctx) = sched.get_context_mut(current_pid) {
                    // Copy register state from interrupt frame to context
                    let frame = unsafe { &*stack_frame };
                    ctx.rax = frame.rax;
                    ctx.rbx = frame.rbx;
                    ctx.rcx = frame.rcx;
                    ctx.rdx = frame.rdx;
                    ctx.rsi = frame.rsi;
                    ctx.rdi = frame.rdi;
                    ctx.rbp = frame.rbp;
                    ctx.r8 = frame.r8;
                    ctx.r9 = frame.r9;
                    ctx.r10 = frame.r10;
                    ctx.r11 = frame.r11;
                    ctx.r12 = frame.r12;
                    ctx.r13 = frame.r13;
                    ctx.r14 = frame.r14;
                    ctx.r15 = frame.r15;
                    ctx.rsp = frame.rsp;
                    ctx.rip = frame.rip;
                    ctx.rflags = frame.rflags;
                    ctx.cs = frame.cs;
                    ctx.ss = frame.ss;

                    crate::debug_trace!(
                        "Saved context for PID {:?}, RIP={:#x}",
                        current_pid,
                        ctx.rip
                    );
                }

                let current = crate::process::entity::EntityId::KernelThread(current_pid);
                match sched.preempt_and_pick(current) {
                    Some(crate::process::entity::EntityId::KernelThread(next_pid))
                        if next_pid != current_pid =>
                    {
                        kernel_target = Some(next_pid);
                    }
                    Some(crate::process::entity::EntityId::UserProcess(pid)) => {
                        user_target = Some(pid);
                    }
                    Some(crate::process::entity::EntityId::KernelThread(_)) | None => {}
                }
            }
        }
    }

    // Send EOI
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }

    if let Some(pid) = user_target {
        unsafe { crate::userland::switch::resume_ring3(pid) }
    }
    if let Some(pid) = kernel_target {
        unsafe { crate::arch::x86_64::context_switch::resume_kernel_thread(pid) }
    }
}
