//! Process Control Block (PCB) for preemptive multitasking
//!
//! The PCB contains all information needed to manage a process,
//! including its CPU context, stack, and I/O associations.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::boxed::Box;
use spin::Mutex;
use crate::lib::arc::Arc;
use crate::window::WindowId;
use super::process::ProcessId;
use super::context::CpuContext;

/// Process execution state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is ready to run and in the ready queue
    Ready,
    /// Process is currently executing on the CPU
    Running,
    /// Process is waiting for I/O or an event
    Blocked,
    /// Process has finished execution
    Terminated,
}

/// Reason why a process is blocked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Waiting for input from stdin
    WaitingForInput,
    /// Waiting for a child process
    WaitingForChild(ProcessId),
    /// Sleeping for a duration
    Sleeping,
}

/// Process Control Block - complete state for a process
pub struct ProcessControlBlock {
    /// Unique process identifier
    pub pid: ProcessId,

    /// Human-readable process name
    pub name: String,

    /// Current execution state
    pub state: ProcessState,

    /// Saved CPU context (registers) for context switching
    pub context: CpuContext,

    /// Base address of the process stack
    pub stack_base: u64,

    /// Size of the stack in bytes
    pub stack_size: usize,

    /// Associated terminal window (for I/O routing)
    pub terminal_id: Option<WindowId>,

    /// Input buffer for stdin (lines from terminal)
    pub stdin_buffer: Arc<Mutex<VecDeque<String>>>,

    /// Remaining time slice in timer ticks
    pub time_slice_remaining: u64,

    /// Total CPU time consumed (in timer ticks)
    pub total_runtime: u64,

    /// Why the process is blocked (if state == Blocked)
    pub block_reason: Option<BlockReason>,

    /// Entry point function for new processes
    pub entry_fn: Option<Box<dyn FnOnce() + Send>>,
}

impl ProcessControlBlock {
    /// Create a new PCB with default values
    pub fn new(pid: ProcessId, name: String) -> Self {
        Self {
            pid,
            name,
            state: ProcessState::Ready,
            context: CpuContext::default(),
            stack_base: 0,
            stack_size: 0,
            terminal_id: None,
            stdin_buffer: Arc::new(Mutex::new(VecDeque::new())),
            time_slice_remaining: 0,
            total_runtime: 0,
            block_reason: None,
            entry_fn: None,
        }
    }

    /// Check if the process has pending input
    pub fn has_input(&self) -> bool {
        !self.stdin_buffer.lock().is_empty()
    }

    /// Push a line of input to this process
    pub fn push_input(&self, line: String) {
        self.stdin_buffer.lock().push_back(line);
    }

    /// Pop a line of input from this process (if available)
    pub fn pop_input(&self) -> Option<String> {
        self.stdin_buffer.lock().pop_front()
    }
}

impl core::fmt::Debug for ProcessControlBlock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProcessControlBlock")
            .field("pid", &self.pid)
            .field("name", &self.name)
            .field("state", &self.state)
            .field("terminal_id", &self.terminal_id)
            .field("time_slice_remaining", &self.time_slice_remaining)
            .finish()
    }
}
