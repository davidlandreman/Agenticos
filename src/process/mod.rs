pub mod process;
pub mod manager;
pub mod pcb;
pub mod context;
pub mod stack;
pub mod scheduler;

pub use process::{BaseProcess, HasBaseProcess, RunnableProcess, ProcessId, allocate_pid};
pub use manager::{
    set_active_stdin, clear_active_stdin, push_keyboard_input,
    register_command, execute_command, execute_command_sync, list_commands
};
pub use pcb::{ProcessControlBlock, ProcessState, BlockReason, WakeEvents, SignalFlags};
pub use context::CpuContext;
pub use scheduler::{SCHEDULER, ProcessInfo};

use alloc::boxed::Box;
use alloc::string::String;
use spin::Mutex;
use crate::window::WindowId;

/// Flag indicating whether we're currently running a spawned process
static IN_SPAWNED_PROCESS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Check if we're currently running a spawned process
pub fn is_in_spawned_process() -> bool {
    IN_SPAWNED_PROCESS.load(core::sync::atomic::Ordering::Acquire)
}

/// Initialize the scheduler
pub fn init_scheduler() {
    scheduler::SCHEDULER.lock().init();
}

/// Spawn a new process with the given entry function
///
/// # Arguments
/// * `name` - Human-readable name for the process
/// * `terminal_id` - Optional terminal window for I/O
/// * `entry_fn` - The function to run as the process entry point
///
/// # Returns
/// The PID of the newly spawned process
pub fn spawn_process<F>(name: String, terminal_id: Option<WindowId>, entry_fn: F) -> ProcessId
where
    F: FnOnce() + Send + 'static,
{
    // Allocate a stack for this process
    let (stack_base, stack_top) = stack::allocate_stack()
        .expect("Failed to allocate process stack");

    // Create the PCB
    let pid = allocate_pid();
    let mut pcb = ProcessControlBlock::new(pid, name);
    pcb.stack_base = stack_base;
    pcb.stack_size = stack::STACK_SIZE;
    pcb.terminal_id = terminal_id;
    pcb.entry_fn = Some(Box::new(entry_fn));

    // Initialize the context to start at the trampoline
    pcb.context = CpuContext::init_for_new_process(
        stack_top,
        crate::arch::x86_64::context_switch::process_entry_trampoline as u64,
    );

    // Add to scheduler
    scheduler::SCHEDULER.lock().spawn(pcb)
}

/// Yield the current process voluntarily
///
/// The current process gives up the remainder of its time slice and
/// the scheduler picks the next process to run. The current process
/// will be resumed later by the scheduler.
pub fn yield_current() {
    use crate::arch::x86_64::context_switch::switch_context;

    let (old_ctx_ptr, new_ctx) = {
        let mut sched = scheduler::SCHEDULER.lock();
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return, // No current process
        };

        sched.yield_current();

        // Get the next process
        let next_pid = match sched.schedule() {
            Some(pid) => pid,
            None => return, // No process to switch to
        };

        // If we're switching to ourselves, do nothing
        if next_pid == current_pid {
            return;
        }

        // Get context pointers while holding the lock
        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };
        let new_ctx = match sched.get_context(next_pid) {
            Some(ctx) => *ctx,
            None => return,
        };

        (old_ctx, new_ctx)
    };

    // Perform the context switch - this saves our state and switches
    // When we're scheduled again, we'll resume right after this call
    unsafe {
        switch_context(old_ctx_ptr, &new_ctx);
    }
}

/// Check if preemption is pending and yield if so
///
/// Call this periodically in long-running processes to allow
/// the scheduler to preempt them.
pub fn yield_if_needed() {
    use crate::arch::x86_64::interrupts;

    if interrupts::check_and_clear_preemption() {
        yield_current();
    }
}

/// Terminate the current process
pub fn terminate_current() {
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    crate::debug_trace!("terminate_current: starting");

    let next_ctx = {
        let mut sched = scheduler::SCHEDULER.lock();
        sched.terminate_current();

        // Try to schedule the next REAL process (not idle)
        // Check ready_count first - if 0, don't bother calling schedule
        // because it will just return the idle process
        if sched.ready_count() > 0 {
            if let Some(next_pid) = sched.schedule() {
                crate::debug_trace!("terminate_current: next process is {:?}", next_pid);
                sched.get_context(next_pid).map(|c| *c)
            } else {
                crate::debug_trace!("terminate_current: no more processes");
                None
            }
        } else {
            crate::debug_trace!("terminate_current: ready queue empty, returning to kernel");
            None
        }
    };

    if let Some(ctx) = next_ctx {
        crate::debug_trace!("terminate_current: switching to next process");
        // Switch to next process
        unsafe {
            crate::arch::x86_64::context_switch::switch_to_context(&ctx);
        }
    }

    // No more processes to run - return to kernel context
    if IN_SPAWNED_PROCESS.load(Ordering::Acquire) {
        crate::debug_trace!("terminate_current: returning to kernel context");

        // Get the saved kernel context and switch to it
        let kernel_ctx = unsafe { KERNEL_CONTEXT };
        crate::debug_trace!("terminate_current: kernel RSP={:#x} RIP={:#x}",
            kernel_ctx.rsp, kernel_ctx.rip);

        unsafe {
            crate::arch::x86_64::context_switch::switch_to_context(&kernel_ctx);
        }
        // Should never reach here
    }

    // Fallback: halt if not in spawned process context
    crate::debug_trace!("terminate_current: halting (not in spawned process)");
    loop {
        x86_64::instructions::hlt();
    }
}

/// Block the current process waiting for input
pub fn block_for_input() {
    let mut sched = scheduler::SCHEDULER.lock();
    sched.block_current(BlockReason::WaitingForInput);

    // Schedule the next process
    if let Some(next_pid) = sched.schedule() {
        if let Some(next_ctx) = sched.get_context(next_pid) {
            let next_ctx = *next_ctx;
            drop(sched);

            // Switch to the next process
            unsafe {
                crate::arch::x86_64::context_switch::switch_to_context(&next_ctx);
            }
        }
    }
}

/// Send input to a specific process
///
/// # Arguments
/// * `pid` - The PID of the process to send input to
/// * `input` - The input line to send
pub fn send_input_to_process(pid: ProcessId, input: String) {
    let mut sched = scheduler::SCHEDULER.lock();
    if let Some(pcb) = sched.get_process_mut(pid) {
        pcb.push_input(input);
        if pcb.state == ProcessState::Blocked {
            if let Some(BlockReason::WaitingForInput) = pcb.block_reason {
                sched.wake(pid);
            }
        }
    }
}

/// Handle preemption from the kernel main loop
///
/// This should be called when `check_and_clear_preemption()` returns true.
/// It performs the actual context switch.
pub fn handle_preemption() {
    use crate::arch::x86_64::context_switch::{switch_context, switch_to_context};

    let mut sched = scheduler::SCHEDULER.lock();

    // Get current process if any
    let current_pid = sched.current();

    // Schedule the next process
    let next_pid = match sched.schedule() {
        Some(pid) => pid,
        None => return, // No process to run
    };

    // If same as current, no switch needed
    if current_pid == Some(next_pid) {
        return;
    }

    // If there was a current process, do a full context switch
    if let Some(cur_pid) = current_pid {
        let old_ctx = match sched.get_context_mut(cur_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };
        let new_ctx = match sched.get_context(next_pid) {
            Some(ctx) => ctx as *const CpuContext,
            None => return,
        };

        // Drop the lock before switching
        drop(sched);

        // Perform the context switch
        unsafe {
            switch_context(old_ctx, new_ctx);
        }
    } else {
        // No current process - just switch to the new one
        let new_ctx = match sched.get_context(next_pid) {
            Some(ctx) => *ctx,
            None => return,
        };

        // Drop the lock before switching
        drop(sched);

        // Switch to new process (no save needed)
        unsafe {
            switch_to_context(&new_ctx);
        }
    }
}

/// Try to run any scheduled processes
///
/// Call this from the kernel main loop to check if there are processes
/// waiting to run and switch to them. Saves the kernel context so we can
/// return here when the process is preempted or terminates.
///
/// With timer-based preemption:
/// - Kernel switches to a process
/// - Timer interrupt fires during process execution
/// - Timer handler saves process context and switches back to kernel
/// - Kernel runs its loop (input, render) then calls this again
/// - Kernel switches to the next ready process
pub fn try_run_scheduled_processes() {
    use crate::arch::x86_64::context_switch::switch_context_full_restore;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    // Only try if scheduler is initialized
    let mut sched = match scheduler::SCHEDULER.try_lock() {
        Some(s) if s.is_initialized() => s,
        _ => return,
    };

    // Only run if there are ready processes
    if sched.ready_count() == 0 {
        return;
    }

    if let Some(next_pid) = sched.schedule() {
        if let Some(next_ctx) = sched.get_context(next_pid) {
            let next_ctx = next_ctx as *const CpuContext;

            // Get context values BEFORE dropping the lock
            let ctx_copy = unsafe { *next_ctx };

            drop(sched);

            crate::debug_trace!("Starting process {:?}", next_pid);
            crate::debug_trace!("  Target RIP: {:#x}", ctx_copy.rip);
            crate::debug_trace!("  Target RSP: {:#x}", ctx_copy.rsp);

            // Pre-map the stack pages before switching context
            // This is critical because if we page fault with an unmapped stack,
            // the CPU can't push the exception frame and we get a triple fault
            let stack_top = ctx_copy.rsp;
            unsafe {
                // Touch a few pages at the top of the stack to ensure they're mapped
                let page_size = 4096u64;
                for offset in (0..page_size * 4).step_by(page_size as usize) {
                    let addr = stack_top - offset - 8;
                    // Volatile read to trigger page fault (which will map the page)
                    core::ptr::read_volatile(addr as *const u8);
                }
            }

            // Mark that we're entering a spawned process
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);

            // Save kernel context to global for timer handler to use
            // The switch_context_full_restore will save our current state here
            let kernel_ctx_ptr = unsafe { &mut KERNEL_CONTEXT as *mut CpuContext };

            // Switch to the process using full context restore
            // This saves kernel's callee-saved regs and restores ALL process regs
            // When the timer preempts, it switches back to kernel context
            unsafe {
                switch_context_full_restore(kernel_ctx_ptr, next_ctx);
            }

            // We get here when:
            // 1. Timer preempted the process and switched back to kernel context
            // 2. Process terminated and switched to kernel context
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            crate::debug_trace!("Returned to kernel from process");
        }
    }
}

/// Check if there's a process for the given terminal
pub fn get_process_for_terminal(terminal_id: WindowId) -> Option<ProcessId> {
    scheduler::SCHEDULER.lock().find_by_terminal(terminal_id)
}

/// Get a snapshot of all running processes for display purposes
///
/// Returns lightweight ProcessInfo structs suitable for a task manager UI.
pub fn get_process_list() -> alloc::vec::Vec<ProcessInfo> {
    scheduler::SCHEDULER.lock().get_process_list()
}

/// Terminate a specific process by PID
///
/// This immediately marks the process as terminated and removes it
/// from the scheduler.
pub fn terminate_process(pid: ProcessId) {
    scheduler::SCHEDULER.lock().terminate(pid);
}

/// Update CPU percentages for all processes
///
/// Call this periodically (every ~50 ticks / 500ms) to calculate CPU usage.
/// The percentage represents CPU time used in the elapsed window.
///
/// # Arguments
/// * `elapsed_ticks` - Number of timer ticks since last update
pub fn update_cpu_percentages(elapsed_ticks: u64) {
    scheduler::SCHEDULER.lock().update_cpu_percentages(elapsed_ticks);
}

// =============================================================================
// Sleep API
// =============================================================================

/// Sleep the current process for N timer ticks
///
/// The process will be blocked and woken after the specified number of ticks.
/// 1 tick = ~10ms at 100 Hz timer frequency.
///
/// # Arguments
/// * `ticks` - Number of timer ticks to sleep (minimum 1)
pub fn sleep_ticks(ticks: u64) {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    let ticks = ticks.max(1);

    let result: Option<(*mut CpuContext, SwitchTarget)> = {
        let mut sched = scheduler::SCHEDULER.lock();
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return, // No current process
        };

        // Mark as sleeping
        sched.sleep_current(ticks);

        // Get context pointer for current process
        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };

        // Check if there are any real processes to run (not counting idle)
        let switch_target = if sched.ready_count() > 0 {
            // Get the next process
            if let Some(next_pid) = sched.schedule() {
                if let Some(ctx) = sched.get_context(next_pid) {
                    SwitchTarget::Process(*ctx)
                } else {
                    SwitchTarget::Kernel
                }
            } else {
                SwitchTarget::Kernel
            }
        } else {
            // No ready processes - return to kernel context
            SwitchTarget::Kernel
        };

        Some((old_ctx, switch_target))
    };

    let Some((old_ctx_ptr, switch_target)) = result else {
        return;
    };

    // Perform the context switch
    match switch_target {
        SwitchTarget::Process(new_ctx) => {
            // Switch to another process
            unsafe {
                switch_context(old_ctx_ptr, &new_ctx);
            }
        }
        SwitchTarget::Kernel => {
            // Return to kernel context - save our state and switch to kernel
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            let kernel_ctx = unsafe { KERNEL_CONTEXT };
            unsafe {
                switch_context(old_ctx_ptr, &kernel_ctx);
            }
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);
        }
    }
}

/// Target for context switch
enum SwitchTarget {
    Process(CpuContext),
    Kernel,
}

/// Sleep the current process for approximately N milliseconds
///
/// Since the timer runs at 100 Hz (10ms per tick), the actual sleep time
/// will be rounded up to the nearest 10ms.
///
/// # Arguments
/// * `ms` - Milliseconds to sleep
pub fn sleep_ms(ms: u64) {
    // 100 Hz timer = 10ms per tick
    // Round up to nearest tick
    let ticks = (ms + 9) / 10;
    sleep_ticks(ticks.max(1));
}

/// Sleep the current process until signaled by specific events
///
/// The process will be blocked until any of the specified events occur.
/// When woken, use `check_signals()` to see which event triggered the wake.
///
/// # Arguments
/// * `events` - Bitmask of events that can wake this process
pub fn sleep_until_event(events: WakeEvents) {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    let result: Option<(*mut CpuContext, SwitchTarget)> = {
        let mut sched = scheduler::SCHEDULER.lock();
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return,
        };

        // Mark as waiting for signal
        sched.sleep_until_signaled(events);

        // Get context pointer for current process
        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };

        // Check if there are any real processes to run (not counting idle)
        let switch_target = if sched.ready_count() > 0 {
            if let Some(next_pid) = sched.schedule() {
                if let Some(ctx) = sched.get_context(next_pid) {
                    SwitchTarget::Process(*ctx)
                } else {
                    SwitchTarget::Kernel
                }
            } else {
                SwitchTarget::Kernel
            }
        } else {
            SwitchTarget::Kernel
        };

        Some((old_ctx, switch_target))
    };

    let Some((old_ctx_ptr, switch_target)) = result else {
        return;
    };

    match switch_target {
        SwitchTarget::Process(new_ctx) => {
            unsafe {
                switch_context(old_ctx_ptr, &new_ctx);
            }
        }
        SwitchTarget::Kernel => {
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            let kernel_ctx = unsafe { KERNEL_CONTEXT };
            unsafe {
                switch_context(old_ctx_ptr, &kernel_ctx);
            }
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);
        }
    }
}

/// Signal a specific process to wake up
///
/// If the process is waiting for the given event type, it will be woken
/// and moved to the ready queue.
///
/// # Arguments
/// * `pid` - The process to signal
/// * `signal` - The event type to signal
pub fn signal_process(pid: ProcessId, signal: WakeEvents) {
    scheduler::SCHEDULER.lock().signal_process(pid, signal);
}

/// Signal all processes waiting for a specific event type
///
/// All processes in the signal waiters list will be woken if they're
/// waiting for this event type.
///
/// # Arguments
/// * `signal` - The event type to broadcast
pub fn broadcast_signal(signal: WakeEvents) {
    scheduler::SCHEDULER.lock().broadcast_signal(signal);
}

/// Check and clear pending signals for the current process
///
/// Call this after waking from `sleep_until_event()` to see which
/// event triggered the wake.
///
/// # Returns
/// The pending signal flags, which are then cleared
pub fn check_signals() -> SignalFlags {
    let mut sched = scheduler::SCHEDULER.lock();
    if let Some(pid) = sched.current() {
        if let Some(pcb) = sched.get_process_mut(pid) {
            let signals = pcb.pending_signals;
            pcb.pending_signals.clear_all();
            return signals;
        }
    }
    SignalFlags::NONE
}

/// Get the number of sleeping processes
pub fn sleeping_count() -> usize {
    scheduler::SCHEDULER.lock().sleeping_count()
}