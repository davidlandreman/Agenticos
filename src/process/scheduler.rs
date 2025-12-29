//! Round-robin process scheduler
//!
//! Manages process scheduling with timer-based preemption. Each process gets
//! a time slice, and when it expires, the scheduler picks the next ready process.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::pcb::{ProcessControlBlock, ProcessState, BlockReason, WakeEvents, SignalFlags};
use super::process::ProcessId;
use super::context::CpuContext;
use super::stack::{allocate_stack, free_stack};

/// Default time slice in timer ticks
/// With 100 Hz timer (10ms per tick), 2 ticks = 20ms per time slice
/// This provides smooth multitasking where processes appear to run simultaneously
pub const DEFAULT_TIME_SLICE: u64 = 2;

/// Lightweight process info for display purposes (e.g., task manager)
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: ProcessId,
    /// Human-readable process name
    pub name: String,
    /// Current execution state
    pub state: ProcessState,
    /// Total CPU time consumed (in timer ticks)
    pub total_runtime: u64,
    /// Stack size in bytes
    pub stack_size: usize,
    /// Cached CPU percentage (0-100)
    pub cpu_percentage: u8,
}

/// Global scheduler instance
pub static SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());

/// Round-robin scheduler for process management
pub struct Scheduler {
    /// All processes indexed by PID
    processes: BTreeMap<ProcessId, ProcessControlBlock>,
    /// Queue of ready process IDs (round-robin order)
    ready_queue: VecDeque<ProcessId>,
    /// Currently running process (None if idle)
    current: Option<ProcessId>,
    /// The idle process PID (runs when nothing else is ready)
    idle_pid: Option<ProcessId>,
    /// Whether scheduler is initialized
    initialized: bool,
    /// Processes sleeping until a specific tick, ordered by wake time
    sleep_queue: BTreeMap<u64, Vec<ProcessId>>,
    /// Processes waiting for signal events (not time-based)
    signal_waiters: Vec<ProcessId>,
}

impl Scheduler {
    /// Create a new scheduler (const for static initialization)
    pub const fn new() -> Self {
        Scheduler {
            processes: BTreeMap::new(),
            ready_queue: VecDeque::new(),
            current: None,
            idle_pid: None,
            initialized: false,
            sleep_queue: BTreeMap::new(),
            signal_waiters: Vec::new(),
        }
    }

    /// Initialize the scheduler with an idle process
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }

        // Create idle process - it will be scheduled when nothing else is ready
        let idle_pid = super::process::allocate_pid();
        let mut idle_pcb = ProcessControlBlock::new(idle_pid, String::from("idle"));
        idle_pcb.state = ProcessState::Ready;
        // Idle process doesn't need a real stack - it runs in kernel context

        self.idle_pid = Some(idle_pid);
        self.processes.insert(idle_pid, idle_pcb);
        self.initialized = true;

        crate::debug_info!("Scheduler initialized with idle process PID {:?}", idle_pid);
    }

    /// Spawn a new process and add it to the ready queue
    ///
    /// # Arguments
    /// * `pcb` - The process control block for the new process
    ///
    /// # Returns
    /// The PID of the newly spawned process
    pub fn spawn(&mut self, mut pcb: ProcessControlBlock) -> ProcessId {
        let pid = pcb.pid;
        pcb.state = ProcessState::Ready;
        pcb.time_slice_remaining = DEFAULT_TIME_SLICE;

        crate::debug_info!("Scheduler: Spawning process '{}' with PID {:?}", pcb.name, pid);

        self.processes.insert(pid, pcb);
        self.ready_queue.push_back(pid);

        pid
    }

    /// Get the currently running process ID
    pub fn current(&self) -> Option<ProcessId> {
        self.current
    }

    /// Get a reference to a process by PID
    pub fn get_process(&self, pid: ProcessId) -> Option<&ProcessControlBlock> {
        self.processes.get(&pid)
    }

    /// Get a mutable reference to a process by PID
    pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut ProcessControlBlock> {
        self.processes.get_mut(&pid)
    }

    /// Select the next process to run
    ///
    /// # Returns
    /// The PID of the next process to run, or None if only idle is available
    pub fn schedule(&mut self) -> Option<ProcessId> {
        // If there's a process in the ready queue, pick it
        if let Some(next_pid) = self.ready_queue.pop_front() {
            // Mark current as Ready (if not blocked/terminated)
            if let Some(current_pid) = self.current {
                if let Some(current_pcb) = self.processes.get_mut(&current_pid) {
                    if current_pcb.state == ProcessState::Running {
                        current_pcb.state = ProcessState::Ready;
                        // Re-add to ready queue (round-robin)
                        if Some(current_pid) != self.idle_pid {
                            self.ready_queue.push_back(current_pid);
                        }
                    }
                }
            }

            // Set new process as running
            if let Some(next_pcb) = self.processes.get_mut(&next_pid) {
                next_pcb.state = ProcessState::Running;
                next_pcb.time_slice_remaining = DEFAULT_TIME_SLICE;
            }

            self.current = Some(next_pid);
            crate::debug_trace!("Scheduler: Switching to process {:?}", next_pid);
            return Some(next_pid);
        }

        // No processes ready - run idle or stay with current
        if self.current.is_none() || self.current == self.idle_pid {
            self.current = self.idle_pid;
        }

        self.current
    }

    /// Block the current process
    ///
    /// # Arguments
    /// * `reason` - Why the process is blocking
    pub fn block_current(&mut self, reason: BlockReason) {
        if let Some(current_pid) = self.current {
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                pcb.state = ProcessState::Blocked;
                pcb.block_reason = Some(reason);
                crate::debug_info!("Scheduler: Blocked process {:?} for {:?}", current_pid, reason);
            }
            // Clear current - schedule will pick next
            self.current = None;
        }
    }

    /// Wake a blocked process and add it back to the ready queue
    ///
    /// # Arguments
    /// * `pid` - The PID of the process to wake
    pub fn wake(&mut self, pid: ProcessId) {
        if let Some(pcb) = self.processes.get_mut(&pid) {
            if pcb.state == ProcessState::Blocked {
                pcb.state = ProcessState::Ready;
                pcb.block_reason = None;
                self.ready_queue.push_back(pid);
                crate::debug_info!("Scheduler: Woke process {:?}", pid);
            }
        }
    }

    /// Terminate the current process
    pub fn terminate_current(&mut self) {
        if let Some(current_pid) = self.current.take() {
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                pcb.state = ProcessState::Terminated;
                crate::debug_info!("Scheduler: Terminated process {:?}", current_pid);

                // Free the stack
                if pcb.stack_base != 0 {
                    free_stack(pcb.stack_base);
                }
            }
            // Remove from processes map
            self.processes.remove(&current_pid);
        }
    }

    /// Terminate a specific process
    pub fn terminate(&mut self, pid: ProcessId) {
        // Remove from ready queue if present
        self.ready_queue.retain(|&p| p != pid);

        if let Some(pcb) = self.processes.get_mut(&pid) {
            pcb.state = ProcessState::Terminated;
            crate::debug_info!("Scheduler: Terminated process {:?}", pid);

            // Free the stack
            if pcb.stack_base != 0 {
                free_stack(pcb.stack_base);
            }
        }

        // Remove from processes map
        self.processes.remove(&pid);

        // If this was the current process, clear it
        if self.current == Some(pid) {
            self.current = None;
        }
    }

    /// Handle a timer tick - decrement time slice and check for preemption
    ///
    /// # Returns
    /// `true` if the current process's time slice has expired and preemption is needed
    pub fn timer_tick(&mut self) -> bool {
        if let Some(current_pid) = self.current {
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                // Increment total runtime
                pcb.total_runtime += 1;

                // Don't preempt idle process
                if self.idle_pid == Some(current_pid) {
                    return !self.ready_queue.is_empty();
                }

                // Decrement time slice
                if pcb.time_slice_remaining > 0 {
                    pcb.time_slice_remaining -= 1;
                }

                // Check if time slice expired
                if pcb.time_slice_remaining == 0 {
                    crate::debug_trace!("Scheduler: Time slice expired for {:?}", current_pid);
                    return true;
                }
            }
        }
        false
    }

    /// Yield the current process voluntarily
    ///
    /// Moves the current process to the back of the ready queue.
    pub fn yield_current(&mut self) {
        if let Some(current_pid) = self.current {
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                if pcb.state == ProcessState::Running {
                    pcb.state = ProcessState::Ready;
                    if self.idle_pid != Some(current_pid) {
                        self.ready_queue.push_back(current_pid);
                    }
                }
            }
            self.current = None;
        }
    }

    /// Get the number of ready processes
    pub fn ready_count(&self) -> usize {
        self.ready_queue.len()
    }

    /// Get the total number of processes (including blocked/idle)
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    /// Check if the scheduler has been initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the context of the current process
    pub fn current_context(&self) -> Option<&CpuContext> {
        self.current.and_then(|pid| {
            self.processes.get(&pid).map(|pcb| &pcb.context)
        })
    }

    /// Get mutable context of the current process
    pub fn current_context_mut(&mut self) -> Option<&mut CpuContext> {
        if let Some(pid) = self.current {
            self.processes.get_mut(&pid).map(|pcb| &mut pcb.context)
        } else {
            None
        }
    }

    /// Get the context of a specific process
    pub fn get_context(&self, pid: ProcessId) -> Option<&CpuContext> {
        self.processes.get(&pid).map(|pcb| &pcb.context)
    }

    /// Get mutable context of a specific process
    pub fn get_context_mut(&mut self, pid: ProcessId) -> Option<&mut CpuContext> {
        self.processes.get_mut(&pid).map(|pcb| &mut pcb.context)
    }

    /// Find a process associated with a specific terminal
    pub fn find_by_terminal(&self, terminal_id: crate::window::WindowId) -> Option<ProcessId> {
        for (pid, pcb) in &self.processes {
            if pcb.terminal_id == Some(terminal_id) {
                return Some(*pid);
            }
        }
        None
    }

    /// Get a snapshot of all processes for display purposes
    ///
    /// Returns lightweight ProcessInfo structs suitable for a task manager UI.
    pub fn get_process_list(&self) -> Vec<ProcessInfo> {
        self.processes.values()
            .map(|pcb| ProcessInfo {
                pid: pcb.pid,
                name: pcb.name.clone(),
                state: pcb.state,
                total_runtime: pcb.total_runtime,
                stack_size: pcb.stack_size,
                cpu_percentage: pcb.cpu_percentage,
            })
            .collect()
    }

    /// Update CPU percentages for all processes
    ///
    /// Call this periodically (every ~50 ticks / 500ms) to calculate CPU usage.
    /// The percentage represents CPU time used in the elapsed window.
    ///
    /// # Arguments
    /// * `elapsed_ticks` - Number of timer ticks since last update
    pub fn update_cpu_percentages(&mut self, elapsed_ticks: u64) {
        for pcb in self.processes.values_mut() {
            let delta = pcb.total_runtime.saturating_sub(pcb.runtime_last_sample);
            pcb.cpu_percentage = if elapsed_ticks > 0 {
                ((delta * 100) / elapsed_ticks).min(100) as u8
            } else {
                0
            };
            pcb.runtime_last_sample = pcb.total_runtime;
        }
    }

    // =========================================================================
    // Sleep/Wake API
    // =========================================================================

    /// Put the current process to sleep for N timer ticks
    ///
    /// The process will be woken when the specified number of ticks have elapsed.
    /// Does nothing if there is no current process.
    ///
    /// # Arguments
    /// * `ticks` - Number of timer ticks to sleep (1 tick = ~10ms at 100 Hz)
    pub fn sleep_current(&mut self, ticks: u64) {
        let current_tick = crate::arch::x86_64::interrupts::get_timer_ticks();
        let wake_tick = current_tick.saturating_add(ticks);

        if let Some(pid) = self.current.take() {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Blocked;
                pcb.block_reason = Some(BlockReason::SleepingUntilTick(wake_tick));
                pcb.wake_at_tick = Some(wake_tick);
                pcb.wake_events = WakeEvents::TIMER;

                crate::debug_trace!("Scheduler: Process {:?} sleeping until tick {}", pid, wake_tick);
            }

            // Add to sleep queue
            self.sleep_queue
                .entry(wake_tick)
                .or_insert_with(Vec::new)
                .push(pid);
        }
    }

    /// Put the current process to sleep until signaled by specific events
    ///
    /// The process will be woken when any of the specified events occur.
    /// Does nothing if there is no current process.
    ///
    /// # Arguments
    /// * `wake_events` - Events that can wake this process
    pub fn sleep_until_signaled(&mut self, wake_events: WakeEvents) {
        if let Some(pid) = self.current.take() {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Blocked;
                pcb.block_reason = Some(BlockReason::WaitingForSignal);
                pcb.wake_events = wake_events;
                pcb.wake_at_tick = None;

                crate::debug_trace!("Scheduler: Process {:?} waiting for events {:?}", pid, wake_events);
            }

            self.signal_waiters.push(pid);
        }
    }

    /// Check the sleep queue and wake any processes whose time has come
    ///
    /// This should be called from the timer interrupt handler.
    ///
    /// # Arguments
    /// * `current_tick` - The current timer tick count
    pub fn check_sleep_queue(&mut self, current_tick: u64) {
        // Collect expired entries (wake_tick <= current_tick)
        let expired_ticks: Vec<u64> = self.sleep_queue
            .range(..=current_tick)
            .map(|(tick, _)| *tick)
            .collect();

        // Wake all processes in expired entries
        for tick in expired_ticks {
            if let Some(pids) = self.sleep_queue.remove(&tick) {
                for pid in pids {
                    self.wake_from_sleep(pid, WakeEvents::TIMER);
                }
            }
        }
    }

    /// Wake a sleeping process with a specific event
    ///
    /// # Arguments
    /// * `pid` - The process to wake
    /// * `event` - The event that triggered the wake
    fn wake_from_sleep(&mut self, pid: ProcessId, event: WakeEvents) {
        if let Some(pcb) = self.processes.get_mut(&pid) {
            if pcb.state == ProcessState::Blocked {
                // Check if this event can wake the process
                let can_wake = pcb.wake_events.contains(event)
                    || matches!(pcb.block_reason, Some(BlockReason::SleepingUntilTick(_)));

                if can_wake {
                    pcb.state = ProcessState::Ready;
                    pcb.block_reason = None;
                    pcb.wake_at_tick = None;
                    pcb.pending_signals.set(event.bits());
                    self.ready_queue.push_back(pid);

                    crate::debug_trace!("Scheduler: Woke process {:?} with event {:?}", pid, event);
                }
            }
        }
    }

    /// Signal a specific process to wake up
    ///
    /// If the process is waiting for the given signal type, it will be woken.
    ///
    /// # Arguments
    /// * `pid` - The process to signal
    /// * `signal` - The event type to signal
    pub fn signal_process(&mut self, pid: ProcessId, signal: WakeEvents) {
        // Remove from signal_waiters if present
        self.signal_waiters.retain(|&p| p != pid);

        // Also remove from sleep_queue if it's there
        for pids in self.sleep_queue.values_mut() {
            pids.retain(|&p| p != pid);
        }
        // Clean up empty entries
        self.sleep_queue.retain(|_, pids| !pids.is_empty());

        self.wake_from_sleep(pid, signal);
    }

    /// Signal all processes waiting for a specific event type
    ///
    /// All processes in the signal_waiters list that are waiting for this
    /// event type will be woken.
    ///
    /// # Arguments
    /// * `signal` - The event type to broadcast
    pub fn broadcast_signal(&mut self, signal: WakeEvents) {
        let waiters: Vec<ProcessId> = self.signal_waiters.drain(..).collect();
        for pid in waiters {
            self.wake_from_sleep(pid, signal);
        }
    }

    /// Get the number of sleeping processes (timed + signal waiters)
    pub fn sleeping_count(&self) -> usize {
        let timed: usize = self.sleep_queue.values().map(|v| v.len()).sum();
        timed + self.signal_waiters.len()
    }
}
