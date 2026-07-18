//! Shared readiness-change sequencing and restartable blocking.
//!
//! A producer increments `READINESS_SEQUENCE` before waking waiters. A waiter
//! samples the sequence before scanning descriptors, publishes its blocked
//! reason, and the switch path rechecks the value. That closes the otherwise
//! unavoidable event-between-scan-and-park race for infinite epoll/poll waits.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::syscall::SyscallArgs;

static READINESS_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

pub fn sequence() -> u64 {
    READINESS_SEQUENCE.load(Ordering::Acquire)
}

pub fn changed_since(observed: u64) -> bool {
    sequence() != observed
}

pub fn notify_changed() {
    READINESS_SEQUENCE.fetch_add(1, Ordering::AcqRel);
    WAKE_PENDING.store(true, Ordering::Release);
    retry_pending_wake();
}

/// Retry a readiness wake that could not acquire the process table.
///
/// Descriptor state can change while an fd-table syscall already holds the
/// process-table lock. The wake path deliberately uses `try_lock` to avoid a
/// re-entrant spin deadlock, so remember a failed attempt and let the timer
/// service retry it on the next tick. Swapping the flag before the scan keeps
/// a concurrent producer's notification from being cleared accidentally.
pub fn retry_pending_wake() {
    if !WAKE_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    if !crate::userland::lifecycle::wake_ring3_blocked_on_readiness() {
        WAKE_PENDING.store(true, Ordering::Release);
    }
}

pub fn wake_pending() -> bool {
    WAKE_PENDING.load(Ordering::Acquire)
}

pub fn block(
    args: &SyscallArgs,
    identity: u64,
    timeout_ticks: Option<u64>,
    observed_sequence: u64,
) -> i64 {
    let deadline =
        match crate::userland::lifecycle::prepare_network_wait(args.rax, identity, timeout_ticks) {
            Ok(deadline) => deadline,
            Err(()) => return 0,
        };
    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForReadiness {
                deadline_tick: deadline,
                observed_sequence,
            },
        )
    }
}
