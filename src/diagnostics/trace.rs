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
    PageFault = 0x400,
    PageInTerminal = 0x401,
    IoToken = 0x500,
    SignalWakeAttempt = 0x600,
    SignalWakeDeferredIo = 0x601,
    InvariantLatched = 0x900,
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
