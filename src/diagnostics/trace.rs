//! Fixed-size per-CPU semantic flight recorder.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::acpi::MAX_CPUS;

#[cfg(feature = "diagnostics")]
pub const RING_LEN: usize = 1024;
#[cfg(not(feature = "diagnostics"))]
pub const RING_LEN: usize = 128;

const COMMITTED: u64 = 2;
const IN_PROGRESS: u64 = 1;

/// Synthetic boundary ID for the SYSCALL instruction (not an IDT vector).
pub const SYSCALL_BOUNDARY: u64 = 0x100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "stable event IDs are consumed incrementally by subsystem hooks"
)]
#[repr(u16)]
pub enum EventKind {
    DiagnosticsEnabled = 1,
    BootPhase = 2,
    CpuOnline = 3,
    FatalElected = 4,
    NestedFatal = 5,
    CpuRendezvous = 6,
    UnexpectedNmi = 7,
    InterruptEntry = 0x100,
    InterruptExit = 0x101,
    SchedulerDispatch = 0x200,
    ContextPublish = 0x201,
    Cr3Write = 0x300,
    CurrentPid = 0x301,
    CpuHandoff = 0x302,
    PageFault = 0x400,
    PageInTerminal = 0x401,
    IoToken = 0x500,
    SignalWakeAttempt = 0x600,
    SignalWakeDeferredIo = 0x601,
    LockAttempt = 0x800,
    LockAcquired = 0x801,
    LockTryFailed = 0x802,
    LockReleased = 0x803,
    LockOrderEdge = 0x804,
    InvariantLatched = 0x900,
}

// Stable operand schemas for interrupt and scheduler events:
//
// InterruptEntry / InterruptExit:
//   subject = x86 interrupt vector
//   arg0    = interrupted CPL
//   arg1    = EOI-sent flag in bit 0, InterruptOutcome in bits 8..15
//   epoch   = 0 (per-CPU ordering only)
// For subject=SYSCALL_BOUNDARY, arg0 is the Linux syscall number. Entry arg1
// is the current user PID; exit arg1 is the signed return value's raw bits.
//
// SchedulerDispatch:
//   subject = scheduler::entity_key(EntityId)
//   arg0    = logical CPU receiving the entity
//   arg1    = DispatchSource in bits 0..7, deadline-missed flag in bit 8
//   epoch   = committed scheduler-shadow epoch
// ContextPublish:
//   subject = scheduler::entity_key(EntityId)
//   arg0    = resulting production RunState (1 ready, 2 running, 3 blocked,
//             4 dead, 0 missing)
//   arg1    = entity-existed flag in bit 0, newly-enqueued flag in bit 1
//   epoch   = committed scheduler-shadow epoch
// PageFault:
//   subject = page-aligned fault address
//   arg0    = x86 page-fault error-code bits
//   arg1    = faulting instruction pointer
//   epoch   = 0 (the following terminal event carries the pager generation)
// PageInTerminal:
//   subject = page-aligned user virtual address
//   arg0    = PageInTerminalReason in bits 0..15, requested bytes above bit 15
//   arg1    = actual bytes populated
//   epoch   = pager-shadow generation in rich modes, otherwise 0
// IoToken:
//   subject = monotonic block request token
//   arg0    = IoPhase
//   arg1    = submit: PID/device/queue packed low-to-high; complete: status
//             in bits 0..7 and actual bytes in bits 32..63; wake: PID;
//             wrong-wake: awaited token
//   epoch   = associated nonzero page-in generation; ordinary I/O remains in
//             the bounded I/O shadow without consuming recorder bandwidth

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DispatchSource {
    FairQueue = 1,
    UserQueue = 2,
    ForceRunning = 3,
    ResumeSameCpu = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InterruptOutcome {
    Return = 0,
    SwitchUser = 1,
    SwitchKernel = 2,
    Terminate = 3,
    RecoveredCow = 4,
    RecoveredPageIn = 5,
    RecoveredStackGrowth = 6,
    RecoveredKernelDemand = 7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IoPhase {
    Submitted = 1,
    Completed = 2,
    WakeQueued = 3,
    WakeAccepted = 4,
    Consumed = 5,
    WakeLost = 0x81,
    WrongWake = 0x82,
    GenericWakeRejected = 0x83,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct TraceRecord {
    pub sequence: u64,
    pub tsc: u64,
    pub tick: u64,
    pub causal_epoch: u64,
    pub subject: u64,
    pub arg0: u64,
    pub arg1: u64,
    pub meta: u64,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct TraceData {
    tsc: u64,
    tick: u64,
    causal_epoch: u64,
    subject: u64,
    arg0: u64,
    arg1: u64,
    meta: u64,
}

#[repr(C, align(64))]
struct TraceSlot {
    commit: AtomicU64,
    data: UnsafeCell<TraceData>,
}

unsafe impl Sync for TraceSlot {}

impl TraceSlot {
    const fn new() -> Self {
        Self {
            commit: AtomicU64::new(0),
            data: UnsafeCell::new(TraceData {
                tsc: 0,
                tick: 0,
                causal_epoch: 0,
                subject: 0,
                arg0: 0,
                arg1: 0,
                meta: 0,
            }),
        }
    }
}

const _: () = assert!(core::mem::size_of::<TraceSlot>() == 64);

struct TraceRing {
    next: AtomicU64,
    overwrites: AtomicU64,
    drops: AtomicU64,
    slots: [TraceSlot; RING_LEN],
}

impl TraceRing {
    const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            overwrites: AtomicU64::new(0),
            drops: AtomicU64::new(0),
            slots: [const { TraceSlot::new() }; RING_LEN],
        }
    }
}

static RINGS: [TraceRing; MAX_CPUS] = [const { TraceRing::new() }; MAX_CPUS];

#[inline]
fn read_tsc() -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") low, out("edx") high, options(nomem, nostack));
    }
    (u64::from(high) << 32) | u64::from(low)
}

pub fn record_early(kind: EventKind, arg0: u64, arg1: u64) {
    record_on(0, kind, 0, arg0, arg1, 0);
}

pub fn record(kind: EventKind, subject: u64, arg0: u64, arg1: u64, causal_epoch: u64) {
    let cpu = if crate::diagnostics::percpu_ready() {
        crate::arch::x86_64::percpu::cpu_id()
    } else {
        0
    };
    record_on(cpu, kind, subject, arg0, arg1, causal_epoch);
}

pub fn record_interrupt_boundary(
    kind: EventKind,
    vector: u8,
    previous_cpl: u8,
    eoi_sent: bool,
    outcome: InterruptOutcome,
) {
    debug_assert!(matches!(
        kind,
        EventKind::InterruptEntry | EventKind::InterruptExit
    ));
    record(
        kind,
        u64::from(vector),
        u64::from(previous_cpl),
        u64::from(eoi_sent) | (u64::from(outcome as u8) << 8),
        0,
    );
}

pub fn record_on(
    cpu: usize,
    kind: EventKind,
    subject: u64,
    arg0: u64,
    arg1: u64,
    causal_epoch: u64,
) {
    let Some(ring) = RINGS.get(cpu) else {
        return;
    };
    let sequence = ring.next.fetch_add(1, Ordering::Relaxed);
    if sequence > RING_LEN as u64 {
        ring.overwrites.fetch_add(1, Ordering::Relaxed);
    }
    let slot = &ring.slots[sequence as usize % RING_LEN];
    slot.commit
        .store((sequence << 2) | IN_PROGRESS, Ordering::Relaxed);
    let record = TraceData {
        tsc: read_tsc(),
        tick: crate::arch::x86_64::interrupts::get_timer_ticks(),
        causal_epoch,
        subject,
        arg0,
        arg1,
        meta: u64::from(kind as u16) | ((cpu as u64) << 16) | (1u64 << 24),
    };
    unsafe { slot.data.get().write(record) };
    slot.commit
        .store((sequence << 2) | COMMITTED, Ordering::Release);
}

pub fn counters(cpu: usize) -> (u64, u64, u64) {
    let Some(ring) = RINGS.get(cpu) else {
        return (0, 0, 0);
    };
    (
        ring.next.load(Ordering::Acquire),
        ring.overwrites.load(Ordering::Relaxed),
        ring.drops.load(Ordering::Relaxed),
    )
}

pub fn snapshot(cpu: usize, index: usize) -> Option<TraceRecord> {
    let slot = RINGS.get(cpu)?.slots.get(index)?;
    let before = slot.commit.load(Ordering::Acquire);
    if before & 3 != COMMITTED {
        return None;
    }
    let record = unsafe { slot.data.get().read() };
    let after = slot.commit.load(Ordering::Acquire);
    if before == after {
        Some(TraceRecord {
            sequence: before >> 2,
            tsc: record.tsc,
            tick: record.tick,
            causal_epoch: record.causal_epoch,
            subject: record.subject,
            arg0: record.arg0,
            arg1: record.arg1,
            meta: record.meta,
        })
    } else {
        None
    }
}
