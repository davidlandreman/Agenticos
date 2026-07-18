//! Privilege-neutral scheduler entity identifiers and policy metadata.

use super::process::ProcessId;

/// Stable identity of something the single CPU can execute.
///
/// Kernel and user PID allocators are intentionally independent, so the tag is
/// load-bearing: `KernelThread(7)` and `UserProcess(7)` are different entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityId {
    KernelThread(ProcessId),
    UserProcess(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Ready,
    Running,
    Blocked,
    Dead,
}

/// One-shot dispatch ceiling granted when an event-driven worker is woken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatencyContract {
    pub max_dispatch_ticks: u8,
}

impl LatencyContract {
    pub const fn new(max_dispatch_ticks: u8) -> Self {
        Self { max_dispatch_ticks }
    }
}
