//! Process Control Block (PCB) for preemptive multitasking
//!
//! The PCB contains all information needed to manage a process,
//! including its CPU context, stack, and I/O associations.

use super::context::CpuContext;
use super::process::ProcessId;
use crate::lib::arc::Arc;
use crate::window::WindowId;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use core::ops::BitOr;
use spin::Mutex;

/// Events that can wake a sleeping process
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WakeEvents(u32);

impl WakeEvents {
    /// No events
    pub const NONE: Self = Self(0);
    /// Timer expired (for timed sleeps)
    #[expect(dead_code, reason = "legacy event bit retained for task diagnostics")]
    pub const TIMER: Self = Self(1 << 0);
    /// Input available (keyboard/stdin)
    pub const INPUT: Self = Self(1 << 1);
    /// Window event occurred (mouse click, focus change, etc.)
    pub const WINDOW_EVENT: Self = Self(1 << 2);
    /// Child process exited
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const CHILD_EXIT: Self = Self(1 << 3);
    /// Explicit signal from another process
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub const SIGNAL: Self = Self(1 << 4);

    /// Check if this contains the specified event type
    #[inline]
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    /// Get raw value
    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }
}

impl BitOr for WakeEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// Signal flags for tracking which events woke a process
#[derive(Debug, Clone, Copy, Default)]
pub struct SignalFlags(u32);

impl SignalFlags {
    /// No signals pending
    pub const NONE: Self = Self(0);

    /// Set a signal flag
    #[inline]
    pub fn set(&mut self, flag: u32) {
        self.0 |= flag;
    }
}

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
    /// Waiting for a child process
    #[expect(dead_code, reason = "intentional kernel API surface")]
    WaitingForChild(ProcessId),
    /// Sleeping until a specific timer tick
    SleepingUntilTick(u64),
    /// U8: kernel-thread is the launcher of a ring-3 process
    /// (`enter_user_mode_with_aspace`) and is parked until that
    /// process exits. Woken by `wake_kernel_threads_waiting_for_ring3_exit`
    /// in the ring-3 exit path (`long_jump_to_run_or_halt`).
    /// The `u32` payload is the ring-3 PID being awaited.
    WaitingForRing3Exit(u32),
    /// Waiting for an asynchronous block request completion token.
    WaitingForBlockIo(u64),
    /// Deferred timer heap has no due work.
    WaitingForTimerWork,
    /// The shared user-process service has no launch or reap work pending.
    WaitingForProcessWork,
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
    #[expect(dead_code, reason = "intentional kernel API surface")]
    pub stdin_buffer: Arc<Mutex<VecDeque<String>>>,

    /// Remaining time slice in timer ticks
    pub time_slice_remaining: u64,

    /// Total CPU time consumed (in timer ticks)
    pub total_runtime: u64,

    /// Why the process is blocked (if state == Blocked)
    pub block_reason: Option<BlockReason>,

    /// Entry point function for new processes
    pub entry_fn: Option<Box<dyn FnOnce() + Send>>,

    /// Timer tick at which this process should wake (for timed sleeps)
    pub wake_at_tick: Option<u64>,

    /// Events that can wake this process when sleeping
    pub wake_events: WakeEvents,

    /// Signals that have been delivered to this process
    pub pending_signals: SignalFlags,

    /// Last tick when process made progress (yielded, slept, or syscall).
    /// Used by watchdog to detect hung processes.
    pub last_activity_tick: u64,
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
            wake_at_tick: None,
            wake_events: WakeEvents::NONE,
            pending_signals: SignalFlags::NONE,
            last_activity_tick: 0, // Will be set when process is spawned
        }
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
