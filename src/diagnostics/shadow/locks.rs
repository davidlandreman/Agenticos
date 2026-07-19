//! Crash-readable ownership and dependency graph for critical kernel locks.

use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

use super::{latch, ViolationRecord};

pub const LOCK_001: u32 = 0x0900_0001;
pub const LOCK_002: u32 = 0x0900_0002;
pub const LOCK_003: u32 = 0x0900_0003;
pub const LOCK_004: u32 = 0x0900_0004;

const CLASS_COUNT: usize = 7;
const NO_OWNER: u8 = u8::MAX;

// Reviewed outer-to-inner dependency graph. The masks intentionally encode
// a partial order, not every edge a workload happened to exercise. Serial is
// terminal; heap may demand-page through the mapper, so Mapper -> Heap is
// forbidden even if an allocation site has not faulted yet.
const ALLOWED_EDGES: [u16; CLASS_COUNT] = [
    0,
    (1 << LockClassId::ProcessTable as u8)
        | (1 << LockClassId::MemoryMapper as u8)
        | (1 << LockClassId::StackAllocator as u8)
        | (1 << LockClassId::HeapAllocator as u8)
        | (1 << LockClassId::SerialLogger as u8),
    (1 << LockClassId::MemoryMapper as u8)
        | (1 << LockClassId::StackAllocator as u8)
        | (1 << LockClassId::HeapAllocator as u8)
        | (1 << LockClassId::SerialLogger as u8),
    1 << LockClassId::SerialLogger as u8,
    (1 << LockClassId::MemoryMapper as u8)
        | (1 << LockClassId::HeapAllocator as u8)
        | (1 << LockClassId::SerialLogger as u8),
    (1 << LockClassId::MemoryMapper as u8) | (1 << LockClassId::SerialLogger as u8),
    0,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LockClassId {
    Untracked = 0,
    Scheduler = 1,
    ProcessTable = 2,
    MemoryMapper = 3,
    StackAllocator = 4,
    HeapAllocator = 5,
    SerialLogger = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LockKind {
    Interrupt = 1,
    Preemption = 2,
}

struct ClassState {
    owner_cpu: AtomicU8,
    recursion_depth: AtomicU8,
    owner_entity: AtomicU32,
    acquire_site: AtomicU64,
    acquire_tsc: AtomicU64,
    acquisitions: AtomicU64,
    failed_try: AtomicU64,
    waiters: AtomicU32,
}

impl ClassState {
    const fn new() -> Self {
        Self {
            owner_cpu: AtomicU8::new(NO_OWNER),
            recursion_depth: AtomicU8::new(0),
            owner_entity: AtomicU32::new(0),
            acquire_site: AtomicU64::new(0),
            acquire_tsc: AtomicU64::new(0),
            acquisitions: AtomicU64::new(0),
            failed_try: AtomicU64::new(0),
            waiters: AtomicU32::new(0),
        }
    }
}

static CLASSES: [ClassState; CLASS_COUNT] = [const { ClassState::new() }; CLASS_COUNT];
static HELD_MASK: [AtomicU16; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU16::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];
static EDGES: [AtomicU16; CLASS_COUNT] = [const { AtomicU16::new(0) }; CLASS_COUNT];
static EPOCH: AtomicU64 = AtomicU64::new(1);

fn enabled(class: LockClassId) -> bool {
    class != LockClassId::Untracked
        && crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn trace_transitions(class: LockClassId) -> bool {
    !matches!(
        class,
        LockClassId::Untracked | LockClassId::HeapAllocator | LockClassId::SerialLogger
    )
}

fn cpu() -> usize {
    if crate::diagnostics::percpu_ready() {
        crate::arch::x86_64::percpu::cpu_id()
    } else {
        0
    }
}

fn entity() -> u32 {
    if crate::diagnostics::percpu_ready() {
        crate::arch::x86_64::percpu::current_user_pid().unwrap_or(0)
    } else {
        0
    }
}

fn tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub fn site_id(location: &core::panic::Location<'_>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in location.file().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash ^= u64::from(location.line());
    hash = hash.wrapping_mul(0x100_0000_01b3);
    hash ^ u64::from(location.column())
}

fn report(id: u32, class: LockClassId, expected: u64, observed: u64) {
    let state = &CLASSES[class as usize];
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: cpu() as u8,
        mode: 0,
        domain: 8,
        epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
        subject: class as u64,
        expected0: expected,
        observed0: observed,
        expected1: state.owner_entity.load(Ordering::Relaxed).into(),
        observed1: state.acquire_tsc.load(Ordering::Relaxed),
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

/// Called after the wrapper has disabled its local scheduling source but
/// before it can block on the production spin mutex.
pub fn before_acquire(class: LockClassId, kind: LockKind, site: u64) {
    if !enabled(class) {
        return;
    }
    let cpu = cpu();
    if trace_transitions(class) {
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::LockAttempt,
            class as u64,
            kind as u64,
            site,
            0,
        );
    }
    let bit = 1u16 << class as u8;
    if HELD_MASK[cpu].load(Ordering::Acquire) & bit != 0 {
        report(LOCK_002, class, 0, cpu as u64);
        return;
    }
    let context_ok = match kind {
        LockKind::Interrupt => !x86_64::instructions::interrupts::are_enabled(),
        LockKind::Preemption => crate::arch::x86_64::preemption_guard::preemption_disabled(),
    };
    if !context_ok {
        report(LOCK_003, class, kind as u64, 0);
    }
    CLASSES[class as usize]
        .waiters
        .fetch_add(1, Ordering::Relaxed);
}

pub fn failed_try(class: LockClassId) {
    if !enabled(class) {
        return;
    }
    let state = &CLASSES[class as usize];
    state.waiters.fetch_sub(1, Ordering::Relaxed);
    state.failed_try.fetch_add(1, Ordering::Relaxed);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::LockTryFailed,
        class as u64,
        0,
        0,
        0,
    );
}

fn path_exists(from: usize, target: usize) -> bool {
    let mut pending = 1u16 << from;
    let mut visited = 0u16;
    while pending != 0 {
        let node = pending.trailing_zeros() as usize;
        let bit = 1u16 << node;
        pending &= !bit;
        if node == target {
            return true;
        }
        if visited & bit != 0 {
            continue;
        }
        visited |= bit;
        pending |= EDGES[node].load(Ordering::Acquire) & !visited;
    }
    false
}

pub fn acquired(class: LockClassId, kind: LockKind, site: u64) {
    if !enabled(class) {
        return;
    }
    let cpu = cpu();
    let state = &CLASSES[class as usize];
    state.waiters.fetch_sub(1, Ordering::Relaxed);
    if state
        .owner_cpu
        .compare_exchange(NO_OWNER, cpu as u8, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        report(
            LOCK_001,
            class,
            NO_OWNER.into(),
            state.owner_cpu.load(Ordering::Acquire).into(),
        );
        return;
    }
    let held = HELD_MASK[cpu].load(Ordering::Acquire);
    for prior in 1..CLASS_COUNT {
        if held & (1u16 << prior) == 0 {
            continue;
        }
        let edge = 1u16 << class as u8;
        if ALLOWED_EDGES[prior] & edge == 0 || path_exists(class as usize, prior) {
            report(LOCK_004, class, prior as u64, class as u64);
            return;
        }
        let previous = EDGES[prior].fetch_or(edge, Ordering::AcqRel);
        if previous & edge == 0 {
            crate::diagnostics::trace::record(
                crate::diagnostics::trace::EventKind::LockOrderEdge,
                prior as u64,
                class as u64,
                0,
                0,
            );
        }
    }
    state.owner_entity.store(entity(), Ordering::Relaxed);
    state.acquire_site.store(site, Ordering::Relaxed);
    state.acquire_tsc.store(tsc(), Ordering::Relaxed);
    state.acquisitions.fetch_add(1, Ordering::Relaxed);
    state.recursion_depth.store(1, Ordering::Relaxed);
    HELD_MASK[cpu].fetch_or(1u16 << class as u8, Ordering::Release);
    if trace_transitions(class) {
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::LockAcquired,
            class as u64,
            site,
            kind as u64,
            0,
        );
    }
    // Recheck after successful acquisition; this also catches a wrapper that
    // accidentally moved publication before its scheduling guard.
    let context_ok = match kind {
        LockKind::Interrupt => !x86_64::instructions::interrupts::are_enabled(),
        LockKind::Preemption => crate::arch::x86_64::preemption_guard::preemption_disabled(),
    };
    if !context_ok {
        report(LOCK_003, class, kind as u64, 0);
    }
}

pub fn released(class: LockClassId) {
    if !enabled(class) {
        return;
    }
    let cpu = cpu();
    let state = &CLASSES[class as usize];
    let owner = state.owner_cpu.load(Ordering::Acquire);
    if owner != cpu as u8 {
        report(LOCK_001, class, cpu as u64, owner.into());
        return;
    }
    HELD_MASK[cpu].fetch_and(!(1u16 << class as u8), Ordering::AcqRel);
    state.recursion_depth.store(0, Ordering::Relaxed);
    state.owner_entity.store(0, Ordering::Relaxed);
    state.acquire_site.store(0, Ordering::Relaxed);
    state.acquire_tsc.store(0, Ordering::Relaxed);
    state.owner_cpu.store(NO_OWNER, Ordering::Release);
    if trace_transitions(class) {
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::LockReleased,
            class as u64,
            cpu as u64,
            0,
            0,
        );
    }
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    writer.u16((CLASS_COUNT - 1) as u16);
    writer.u16(0);
    for class in 1..CLASS_COUNT {
        let state = &CLASSES[class];
        writer.u8(class as u8);
        writer.u8(state.owner_cpu.load(Ordering::Acquire));
        writer.u8(state.recursion_depth.load(Ordering::Relaxed));
        writer.u8(0);
        writer.u16(state.waiters.load(Ordering::Relaxed).min(u16::MAX.into()) as u16);
        writer.u16(0);
        writer.u32(state.owner_entity.load(Ordering::Relaxed));
        writer.u64(state.acquire_site.load(Ordering::Relaxed));
        writer.u64(state.acquire_tsc.load(Ordering::Relaxed));
        writer.u64(state.acquisitions.load(Ordering::Relaxed));
        writer.u64(state.failed_try.load(Ordering::Relaxed));
        writer.u16(EDGES[class].load(Ordering::Acquire));
        writer.u16(0);
    }
    0
}

pub const fn snapshot_flags() -> u32 {
    0
}

#[cfg(feature = "test")]
pub fn observed_graph_is_allowed() -> bool {
    (1..CLASS_COUNT).all(|class| {
        let observed = EDGES[class].load(Ordering::Acquire);
        observed & !ALLOWED_EDGES[class] == 0
    })
}

#[cfg(feature = "test")]
pub fn observed_edges(class: LockClassId) -> u16 {
    EDGES[class as usize].load(Ordering::Acquire)
}

#[cfg(feature = "test")]
pub fn inject_recursion() {
    HELD_MASK[cpu()].fetch_or(1u16 << LockClassId::Scheduler as u8, Ordering::Release);
    before_acquire(LockClassId::Scheduler, LockKind::Interrupt, 0);
}

#[cfg(feature = "test")]
pub fn inject_wrong_owner() {
    CLASSES[LockClassId::ProcessTable as usize]
        .owner_cpu
        .store(1, Ordering::Release);
    released(LockClassId::ProcessTable);
}

#[cfg(feature = "test")]
pub fn inject_wrong_context() {
    before_acquire(LockClassId::MemoryMapper, LockKind::Interrupt, 0);
}

#[cfg(feature = "test")]
pub fn inject_cycle() {
    EDGES[LockClassId::ProcessTable as usize]
        .fetch_or(1u16 << LockClassId::Scheduler as u8, Ordering::Release);
    HELD_MASK[cpu()].fetch_or(1u16 << LockClassId::Scheduler as u8, Ordering::Release);
    CLASSES[LockClassId::ProcessTable as usize]
        .waiters
        .store(1, Ordering::Relaxed);
    acquired(LockClassId::ProcessTable, LockKind::Interrupt, 0);
}
