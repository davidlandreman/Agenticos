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
pub use pcb::{ProcessControlBlock, ProcessState, BlockReason};
pub use context::CpuContext;
pub use scheduler::SCHEDULER;

use alloc::boxed::Box;
use alloc::string::String;
use crate::window::WindowId;

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
/// the scheduler picks the next process to run.
pub fn yield_current() {
    let mut sched = scheduler::SCHEDULER.lock();
    sched.yield_current();

    // Get the next process context
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

/// Terminate the current process
pub fn terminate_current() {
    let mut sched = scheduler::SCHEDULER.lock();
    sched.terminate_current();

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

    // If no process to switch to, halt
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
/// waiting to run and switch to them.
pub fn try_run_scheduled_processes() {
    use crate::arch::x86_64::context_switch::switch_to_context;

    // Only try if scheduler is initialized
    let mut sched = match scheduler::SCHEDULER.try_lock() {
        Some(s) if s.is_initialized() => s,
        _ => return,
    };

    // If there's no current process and there are ready processes, run one
    if sched.current().is_none() && sched.ready_count() > 0 {
        if let Some(next_pid) = sched.schedule() {
            if let Some(next_ctx) = sched.get_context(next_pid) {
                let next_ctx = *next_ctx;
                drop(sched);

                crate::debug_info!("Starting process {:?}", next_pid);

                // Switch to the process
                unsafe {
                    switch_to_context(&next_ctx);
                }
            }
        }
    }
}

/// Check if there's a process for the given terminal
pub fn get_process_for_terminal(terminal_id: WindowId) -> Option<ProcessId> {
    scheduler::SCHEDULER.lock().find_by_terminal(terminal_id)
}