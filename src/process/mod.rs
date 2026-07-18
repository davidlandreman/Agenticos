pub mod context;
pub mod entity;
pub mod pcb;
pub mod process;
pub mod run_queue;
pub mod scheduler;
pub mod stack;
pub mod timer;

pub use context::CpuContext;
pub use pcb::{BlockReason, ProcessControlBlock, ProcessState, WakeEvents};
pub use process::{allocate_pid, ProcessId};
pub use scheduler::ProcessInfo;

use crate::window::WindowId;
use alloc::boxed::Box;
use alloc::string::String;

/// Flag indicating whether we're currently running a spawned process
static IN_SPAWNED_PROCESS: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
const IO_WAKE_SLOTS: usize = 64;
static PENDING_IO_WAKES: [core::sync::atomic::AtomicU32; IO_WAKE_SLOTS] =
    [const { core::sync::atomic::AtomicU32::new(0) }; IO_WAKE_SLOTS];

/// Check if we're currently running a spawned process
pub fn is_in_spawned_process() -> bool {
    IN_SPAWNED_PROCESS.load(core::sync::atomic::Ordering::Acquire)
}

/// U8: set the "in spawned process" flag. Used by the ring-3 → kernel
/// yield path (`switch::yield_to_kernel_main_loop`) to clear the flag
/// before switching to `KERNEL_CONTEXT` so the kernel main loop
/// observes the correct state on resume.
pub fn set_in_spawned_process(value: bool) {
    IN_SPAWNED_PROCESS.store(value, core::sync::atomic::Ordering::Release);
}

/// Initialize the scheduler
pub fn init_scheduler() {
    scheduler::SCHEDULER.lock().init();
}

/// Return the current kernel thread when storage is called from a spawned
/// kernel process. Boot/main-loop callers deliberately return `None` and use
/// the driver's early-boot halt wait instead.
pub fn current_io_waiter() -> Option<ProcessId> {
    if !is_in_spawned_process() {
        return None;
    }
    scheduler::SCHEDULER.lock().current()
}

/// Queue an IRQ-originated wake without taking the scheduler's non-IRQ-safe
/// spin lock. The timer and main-loop housekeeping drain this bounded array.
pub fn queue_kernel_io_wake(pid: ProcessId) {
    use core::sync::atomic::Ordering;
    for slot in &PENDING_IO_WAKES {
        if slot
            .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
    }
    crate::debug_error!("kernel I/O wake queue full for PID {}", pid);
}

pub fn drain_kernel_io_wakes() -> bool {
    use core::sync::atomic::Ordering;
    let Some(mut scheduler) = scheduler::SCHEDULER.try_lock() else {
        return false;
    };
    let mut woke = false;
    for slot in &PENDING_IO_WAKES {
        let pid = slot.swap(0, Ordering::AcqRel);
        if pid != 0 {
            scheduler.wake(pid);
            woke = true;
        }
    }
    woke
}

/// Preserve and park the current kernel thread on asynchronous block I/O.
/// Completion moves the exact PCB back to the scheduler ready queue.
pub fn block_current_kernel_thread_on_io(token: u64) {
    use crate::arch::x86_64::context_switch::switch_context;

    let old_context = {
        let mut scheduler = scheduler::SCHEDULER.lock();
        let Some(pid) = scheduler.current() else {
            return;
        };
        let Some(context) = scheduler.get_context_mut(pid) else {
            return;
        };
        let pointer = context as *mut CpuContext;
        scheduler.block_current(BlockReason::WaitingForBlockIo(token));
        pointer
    };
    set_in_spawned_process(false);
    unsafe {
        switch_context(
            old_context,
            &raw const crate::arch::x86_64::preemption::KERNEL_CONTEXT,
        );
    }
    set_in_spawned_process(true);
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
    let (stack_base, stack_top) =
        stack::allocate_stack().expect("Failed to allocate process stack");

    // Contexts selected directly from the PIT cannot fault safely while
    // abandoning an interrupt frame. Materialize the top stack pages now.
    unsafe {
        for offset in (0..4096u64 * 4).step_by(4096) {
            core::ptr::read_volatile((stack_top - offset - 8) as *const u8);
        }
    }

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
    crate::debug_trace!("spawn_process: locking scheduler");
    let pid = {
        let mut sched = scheduler::SCHEDULER.lock();
        crate::debug_trace!("spawn_process: scheduler locked, calling spawn()");
        let p = sched.spawn(pcb);
        crate::debug_trace!("spawn_process: spawn() returned PID {:?}", p);
        p
    };
    crate::debug_trace!(
        "spawn_process: scheduler lock released, returning PID {:?}",
        pid
    );
    pid
}

/// Yield the current process voluntarily
///
/// The current process gives up the remainder of its time slice and
/// the scheduler picks the next process to run. The current process
/// will be resumed later by the scheduler.
///
/// Kernel and user entities share the same fair queue, so yielding can
/// dispatch either privilege class directly.
pub fn yield_current() {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::interrupts::get_timer_ticks;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    enum YieldTarget {
        Process(CpuContext),
        User(u32),
        Kernel,
    }

    let (old_ctx_ptr, target) = {
        let mut sched = scheduler::SCHEDULER.lock();
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return, // No current process
        };

        // Update activity tick to show process is making progress (not hung)
        if let Some(pcb) = sched.get_process_mut(current_pid) {
            pcb.last_activity_tick = get_timer_ticks();
        }

        sched.yield_current();

        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };

        match sched.schedule_entity() {
            Some(crate::process::entity::EntityId::KernelThread(next_pid))
                if next_pid == current_pid =>
            {
                return
            }
            Some(crate::process::entity::EntityId::KernelThread(next_pid)) => {
                let new_ctx = match sched.get_context(next_pid) {
                    Some(ctx) => *ctx,
                    None => return,
                };
                (old_ctx, YieldTarget::Process(new_ctx))
            }
            Some(crate::process::entity::EntityId::UserProcess(pid)) => {
                (old_ctx, YieldTarget::User(pid))
            }
            None => (old_ctx, YieldTarget::Kernel),
        }
    };

    match target {
        YieldTarget::Process(new_ctx) => {
            // Pre-map the stack pages before switching context
            // This is critical for new processes that haven't started yet
            let stack_top = new_ctx.rsp;
            unsafe {
                let page_size = 4096u64;
                for offset in (0..page_size * 4).step_by(page_size as usize) {
                    let addr = stack_top - offset - 8;
                    // Volatile read to trigger page fault (which will map the page)
                    core::ptr::read_volatile(addr as *const u8);
                }
            }

            // Perform the context switch - this saves our state and switches
            // When we're scheduled again, we'll resume right after this call
            unsafe {
                switch_context(old_ctx_ptr, &new_ctx);
            }
        }
        YieldTarget::User(pid) => unsafe {
            crate::userland::switch::save_kernel_and_resume_ring3(pid, old_ctx_ptr);
        },
        YieldTarget::Kernel => {
            // Hand off to the kernel main loop. It'll dispatch the
            // pending ring-3 process and eventually re-pick us via
            // try_run_scheduled_processes. Mirrors the pattern
            // sleep_ticks uses for SwitchTarget::Kernel.
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            let kernel_ctx = unsafe { KERNEL_CONTEXT };
            unsafe {
                switch_context(old_ctx_ptr, &kernel_ctx);
            }
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);
        }
    }
}

/// Check if preemption is pending and yield if so
///
/// Call this periodically in long-running processes to allow
/// the scheduler to preempt them. Also updates the activity tick
/// to show the process is making progress (for watchdog).
pub fn yield_if_needed() {
    use crate::arch::x86_64::interrupts::get_timer_ticks;

    // Always update activity tick to show process is responsive
    // This prevents watchdog from killing processes that are polling
    if let Some(mut sched) = scheduler::SCHEDULER.try_lock() {
        if let Some(pid) = sched.current() {
            if let Some(pcb) = sched.get_process_mut(pid) {
                pcb.last_activity_tick = get_timer_ticks();
            }
        }
    }

    yield_current();
}

/// Terminate the current process
pub fn terminate_current() {
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    crate::debug_trace!("terminate_current: starting");

    let (terminated, target) = {
        let mut sched = scheduler::SCHEDULER.lock();
        let terminated = sched.current().map(entity::EntityId::KernelThread);
        sched.terminate_current();
        (terminated, next_switch_target(&mut sched))
    };
    if let Some(entity) = terminated {
        timer::cancel_entity(entity);
    }

    match target {
        SwitchTarget::Process(context) => unsafe {
            crate::arch::x86_64::context_switch::switch_to_context(&context);
        },
        SwitchTarget::User(pid) => unsafe {
            crate::userland::switch::resume_ring3(pid);
        },
        SwitchTarget::Kernel => {
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            let kernel_ctx = unsafe { KERNEL_CONTEXT };
            unsafe {
                crate::arch::x86_64::context_switch::switch_to_context(&kernel_ctx);
            }
        }
    }

    // Fallback: halt if not in spawned process context
    crate::debug_trace!("terminate_current: halting (not in spawned process)");
    loop {
        x86_64::instructions::hlt();
    }
}

/// U8: block the current kernel thread until ring-3 process `pid`
/// exits, then return. Called from `enter_user_mode_with_aspace` after
/// the launcher has installed the Process and marked it Ready.
///
/// The kernel thread is marked Blocked in the scheduler with
/// `BlockReason::WaitingForRing3Exit(pid)`. The scheduler switches
/// directly to the next runnable entity, regardless of privilege level.
///
/// When the ring-3 process exits, `long_jump_to_run_or_halt` calls
/// `wake_threads_waiting_for_ring3_exit(pid)` which moves us back to
/// the ready queue. The kernel-thread scheduler eventually picks us
/// via `switch_context`, restoring our state from the PCB's saved
/// `context`. Execution resumes right after the `switch_context`
/// call below.
pub fn block_kernel_thread_for_ring3_exit(pid: u32) {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    // Real kernel-thread path: block on the scheduler.
    let block_outcome = {
        let mut sched = scheduler::SCHEDULER.lock();
        if let Some(current_pid) = sched.current() {
            let old_ctx = match sched.get_context_mut(current_pid) {
                Some(c) => c as *mut CpuContext,
                None => return,
            };

            sched.block_current(BlockReason::WaitingForRing3Exit(pid));

            Some((old_ctx, next_switch_target(&mut sched)))
        } else {
            None
        }
    };

    if let Some((old_ctx_ptr, target)) = block_outcome {
        match target {
            SwitchTarget::Process(context) => unsafe {
                switch_context(old_ctx_ptr, &context);
            },
            SwitchTarget::User(next_pid) => unsafe {
                crate::userland::switch::save_kernel_and_resume_ring3(next_pid, old_ctx_ptr);
            },
            SwitchTarget::Kernel => {
                IN_SPAWNED_PROCESS.store(false, Ordering::Release);
                let kernel = unsafe { KERNEL_CONTEXT };
                unsafe { switch_context(old_ctx_ptr, &kernel) };
                IN_SPAWNED_PROCESS.store(true, Ordering::Release);
            }
        }
        return;
    }

    // No current kernel thread (called from the kernel main loop or test
    // runner). Drive a compatibility mini-loop until our target
    // process exits. Yield to hlt between dispatches when no ring-3
    // is ready.
    drive_inline_ring3_until_exit(pid);
}

/// Park the current kernel thread until an explicit wake and switch away.
pub fn park_current(reason: BlockReason) {
    let _ = park_current_if(reason, || true);
}

/// Park the current kernel thread only when `should_park` still returns true
/// after the scheduler lock has disabled interrupts.
///
/// Event-driven services use this to close the final-check/park race: a
/// producer publishes an atomic work bit before waking the service, while the
/// service consumes that bit here at the same instant it commits its blocked
/// scheduler state.
pub fn park_current_if(reason: BlockReason, should_park: impl FnOnce() -> bool) -> bool {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    let outcome = {
        let mut sched = scheduler::SCHEDULER.lock();
        if !should_park() {
            return false;
        }
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return false,
        };
        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(context) => context as *mut CpuContext,
            None => return false,
        };
        sched.block_current(reason);
        let target = next_switch_target(&mut sched);
        (old_ctx, target)
    };

    match outcome.1 {
        SwitchTarget::Process(context) => unsafe {
            switch_context(outcome.0, &context);
        },
        SwitchTarget::User(pid) => unsafe {
            crate::userland::switch::save_kernel_and_resume_ring3(pid, outcome.0);
        },
        SwitchTarget::Kernel => {
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
            let kernel = unsafe { KERNEL_CONTEXT };
            unsafe {
                switch_context(outcome.0, &kernel);
            }
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);
        }
    }
    true
}

/// U8: inline ring-3 dispatch loop for non-kernel-thread contexts
/// (kernel main loop, test runner). Polls until `awaited_pid`'s
/// `exit_kind` becomes non-None.
fn drive_inline_ring3_until_exit(awaited_pid: u32) {
    use crate::userland::lifecycle::{pop_next_ring3, with_process, ExitKind};
    #[cfg(feature = "test")]
    let test_deadline = crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(3_000);
    loop {
        let exited =
            with_process(awaited_pid, |p| !matches!(p.exit_kind, ExitKind::None)).unwrap_or(true); // process gone (already reaped) → treat as exited
        if exited {
            return;
        }

        #[cfg(feature = "test")]
        assert!(
            crate::arch::x86_64::interrupts::get_timer_ticks() < test_deadline,
            "ring-3 process {} exceeded the 30-second test deadline",
            awaited_pid,
        );

        if crate::process::timer::take_work_pending() {
            let now = crate::arch::x86_64::interrupts::get_timer_ticks();
            let _ = crate::process::timer::process_due(now);
        }

        // Tests and other non-kernel-thread launchers use this inline loop,
        // so the ordinary `net-rx-tx` kernel worker is not scheduled while
        // the caller waits. Drive the same bounded one-pass poll here; it
        // wakes blocked ring-3 socket syscalls after dropping the net lock.
        crate::net::poll_once();

        if let Some(next) = pop_next_ring3() {
            unsafe {
                crate::userland::switch::save_kernel_and_resume_ring3(
                    next,
                    &raw mut crate::arch::x86_64::preemption::KERNEL_CONTEXT,
                );
            }
            // Resumed via KERNEL_CONTEXT — continue polling.
        } else {
            // Nothing to dispatch; wait for an interrupt (timer, input)
            // that might unblock our awaited process.
            // Keep IF enabled after the wake. The next ring-3 dispatch saves
            // this kernel context; saving it with IF cleared would return the
            // test runner/main-loop caller with interrupts permanently off
            // after the user process exits.
            x86_64::instructions::interrupts::enable_and_hlt();
        }
    }
}

/// Dispatch one runnable entity from the kernel idle/main-loop context.
///
/// Call this from the kernel main loop to check if there are processes
/// waiting to run and switch to them. Saves the kernel context so we can
/// return here when the process is preempted or terminates.
///
pub fn try_run_scheduled_processes() {
    use crate::arch::x86_64::context_switch::switch_context_full_restore;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    // Only try if scheduler is initialized
    let mut sched = match scheduler::SCHEDULER.try_lock() {
        Some(s) if s.is_initialized() => s,
        _ => return,
    };

    if sched.ready_entity_count() == 0 {
        return;
    }

    let target = next_switch_target(&mut sched);
    drop(sched);

    match target {
        SwitchTarget::Process(context) => {
            IN_SPAWNED_PROCESS.store(true, Ordering::Release);
            unsafe {
                switch_context_full_restore(&raw mut KERNEL_CONTEXT, &context);
            }
            IN_SPAWNED_PROCESS.store(false, Ordering::Release);
        }
        SwitchTarget::User(pid) => unsafe {
            crate::userland::switch::save_kernel_and_resume_ring3(pid, &raw mut KERNEL_CONTEXT);
        },
        SwitchTarget::Kernel => {}
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
    timer::cancel_entity(entity::EntityId::KernelThread(pid));
    scheduler::SCHEDULER.lock().terminate(pid);
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
    sleep_ticks_with_contract(ticks, None);
}

pub fn sleep_ticks_with_contract(
    ticks: u64,
    latency: Option<crate::process::entity::LatencyContract>,
) {
    use crate::arch::x86_64::context_switch::switch_context;
    use crate::arch::x86_64::interrupts::get_timer_ticks;
    use crate::arch::x86_64::preemption::KERNEL_CONTEXT;
    use core::sync::atomic::Ordering;

    let ticks = ticks.max(1);

    let result: Option<(*mut CpuContext, SwitchTarget, ProcessId, u64)> = {
        let mut sched = scheduler::SCHEDULER.lock();
        let current_pid = match sched.current() {
            Some(pid) => pid,
            None => return, // No current process
        };

        // Update activity tick before sleeping (shows process is making progress)
        if let Some(pcb) = sched.get_process_mut(current_pid) {
            pcb.last_activity_tick = get_timer_ticks();
        }

        let wake_tick = get_timer_ticks().saturating_add(ticks);
        sched.block_current(BlockReason::SleepingUntilTick(wake_tick));

        // Get context pointer for current process
        let old_ctx = match sched.get_context_mut(current_pid) {
            Some(ctx) => ctx as *mut CpuContext,
            None => return,
        };

        let switch_target = next_switch_target(&mut sched);

        Some((old_ctx, switch_target, current_pid, wake_tick))
    };

    let Some((old_ctx_ptr, switch_target, current_pid, wake_tick)) = result else {
        return;
    };

    crate::process::timer::arm(
        crate::process::timer::TimerKey {
            entity: crate::process::entity::EntityId::KernelThread(current_pid),
            kind: crate::process::timer::TimerKind::KernelSleep,
        },
        wake_tick,
        crate::process::timer::TimerAction::Wake {
            entity: crate::process::entity::EntityId::KernelThread(current_pid),
            latency,
        },
    )
    .expect("timer capacity exceeded while arming kernel sleep");

    // Perform the context switch
    match switch_target {
        SwitchTarget::Process(new_ctx) => {
            // Pre-map the stack pages before switching context
            // This is critical because if we page fault with an unmapped stack,
            // the CPU can't push the exception frame and we get a triple fault
            let stack_top = new_ctx.rsp;
            unsafe {
                let page_size = 4096u64;
                for offset in (0..page_size * 4).step_by(page_size as usize) {
                    let addr = stack_top - offset - 8;
                    // Volatile read to trigger page fault (which will map the page)
                    core::ptr::read_volatile(addr as *const u8);
                }
            }

            // Switch to another process
            unsafe {
                switch_context(old_ctx_ptr, &new_ctx);
            }
        }
        SwitchTarget::User(pid) => unsafe {
            crate::userland::switch::save_kernel_and_resume_ring3(pid, old_ctx_ptr);
        },
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
    User(u32),
    Kernel,
}

fn next_switch_target(sched: &mut scheduler::Scheduler) -> SwitchTarget {
    match sched.schedule_entity() {
        Some(entity::EntityId::KernelThread(pid)) => sched
            .get_context(pid)
            .copied()
            .map(SwitchTarget::Process)
            .unwrap_or(SwitchTarget::Kernel),
        Some(entity::EntityId::UserProcess(pid)) => SwitchTarget::User(pid),
        None => SwitchTarget::Kernel,
    }
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

/// Signal a specific process to wake up
///
/// If the process is waiting for the given event type, it will be woken
/// and moved to the ready queue.
///
/// # Arguments
/// * `pid` - The process to signal
/// * `signal` - The event type to signal
pub fn signal_process(pid: ProcessId, signal: WakeEvents) {
    crate::process::timer::cancel(crate::process::timer::TimerKey {
        entity: crate::process::entity::EntityId::KernelThread(pid),
        kind: crate::process::timer::TimerKind::KernelSleep,
    });
    scheduler::SCHEDULER.lock().signal_process(pid, signal);
}
