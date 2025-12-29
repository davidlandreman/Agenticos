//! Round-robin process scheduler
//!
//! Manages process scheduling with timer-based preemption. Each process gets
//! a time slice, and when it expires, the scheduler picks the next ready process.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::pcb::{ProcessControlBlock, ProcessState, BlockReason};
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
}
