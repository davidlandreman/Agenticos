//! Lock-free, crash-readable lazy page-in transaction shadow.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{latch, ViolationRecord};

pub const PAGER_001: u32 = 0x0300_0001;
pub const PAGER_002: u32 = 0x0300_0002;
pub const PAGER_004: u32 = 0x0300_0004;
pub const DIAG_CAPACITY_PAGER: u32 = 0x0f00_0002;

const CAPACITY: usize = 128;
const CLAIMED: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Classified = 1,
    FrameReserved = 2,
    Populated = 3,
    PresentCommitted = 4,
    Aborted = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    ReserveFrame = 1,
    Populate = 2,
    Commit = 3,
    ObservePresent = 4,
    Abort = 5,
}

pub fn transition_state(state: State, operation: Operation) -> Result<State, u32> {
    match (state, operation) {
        (State::Classified, Operation::ReserveFrame) => Ok(State::FrameReserved),
        (State::FrameReserved, Operation::Populate) => Ok(State::Populated),
        (State::Populated, Operation::Commit) => Ok(State::PresentCommitted),
        (State::Classified, Operation::ObservePresent) => Ok(State::PresentCommitted),
        (State::Classified | State::FrameReserved | State::Populated, Operation::Abort) => {
            Ok(State::Aborted)
        }
        _ => Err(PAGER_001),
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Transaction {
    generation: u64,
    l4: u64,
    vma_generation: u64,
    page: u64,
    frame: u64,
    pid: u32,
    state: State,
    _reserved: u8,
    reason: u16,
    requested: u32,
    actual: u32,
    checksum: u64,
}

impl Transaction {
    const fn empty() -> Self {
        Self {
            generation: 0,
            l4: 0,
            vma_generation: 0,
            page: 0,
            frame: 0,
            pid: 0,
            state: State::Aborted,
            _reserved: 0,
            reason: 0,
            requested: 0,
            actual: 0,
            checksum: 0,
        }
    }
}

struct Slot {
    key: AtomicU64,
    l4_key: AtomicU64,
    page_key: AtomicU64,
    sequence: AtomicU64,
    terminal: AtomicBool,
    transaction: UnsafeCell<Transaction>,
}

unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            l4_key: AtomicU64::new(0),
            page_key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            terminal: AtomicBool::new(false),
            transaction: UnsafeCell::new(Transaction::empty()),
        }
    }
}

static SLOTS: [Slot; CAPACITY] = [const { Slot::new() }; CAPACITY];
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static CURRENT_GENERATION: [AtomicU64; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];

#[derive(Clone, Copy)]
pub struct Handle {
    index: u16,
    generation: u64,
}

fn report(id: u32, transaction: Transaction, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 2,
        epoch: transaction.generation,
        subject: transaction.page,
        expected0: expected,
        observed0: observed,
        expected1: transaction.l4,
        observed1: transaction.frame,
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

pub fn begin(pid: u32, l4: u64, vma_generation: u64, page: u64) -> Option<Handle> {
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    for (index, slot) in SLOTS.iter().enumerate() {
        let key = slot.key.load(Ordering::Acquire);
        let reusable = key == 0 || (key != CLAIMED && slot.terminal.load(Ordering::Acquire));
        if !reusable
            || slot
                .key
                .compare_exchange(key, CLAIMED, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            continue;
        }
        slot.terminal.store(false, Ordering::Release);
        slot.l4_key.store(l4, Ordering::Relaxed);
        slot.page_key.store(page, Ordering::Relaxed);
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe {
            slot.transaction.get().write(Transaction {
                generation,
                l4,
                vma_generation,
                page,
                frame: 0,
                pid,
                state: State::Classified,
                _reserved: 0,
                reason: 0,
                requested: 0,
                actual: 0,
                checksum: 0,
            });
        }
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.key.store(generation, Ordering::Release);
        if let Some(other) = SLOTS.iter().find(|other| {
            !core::ptr::eq(*other, slot)
                && other.key.load(Ordering::Acquire) != 0
                && other.key.load(Ordering::Acquire) != CLAIMED
                && !other.terminal.load(Ordering::Acquire)
                && other.l4_key.load(Ordering::Relaxed) == l4
                && other.page_key.load(Ordering::Relaxed) == page
        }) {
            report(
                PAGER_002,
                unsafe { *slot.transaction.get() },
                generation,
                other.key.load(Ordering::Acquire),
            );
        }
        CURRENT_GENERATION[crate::arch::x86_64::percpu::cpu_id()]
            .store(generation, Ordering::Release);
        return Some(Handle {
            index: index as u16,
            generation,
        });
    }
    report(
        DIAG_CAPACITY_PAGER,
        Transaction {
            generation,
            l4,
            vma_generation,
            page,
            pid,
            ..Transaction::empty()
        },
        CAPACITY as u64,
        0,
    );
    None
}

pub fn current_generation() -> u64 {
    CURRENT_GENERATION[crate::arch::x86_64::percpu::cpu_id()].load(Ordering::Acquire)
}

fn mutate(handle: Handle, operation: Operation, update: impl FnOnce(&mut Transaction)) {
    let slot = &SLOTS[handle.index as usize];
    if slot.key.load(Ordering::Acquire) != handle.generation {
        return;
    }
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    let transaction = unsafe { &mut *slot.transaction.get() };
    let terminal = match transition_state(transaction.state, operation) {
        Ok(next) => {
            update(transaction);
            transaction.state = next;
            matches!(next, State::PresentCommitted | State::Aborted)
        }
        Err(id) => {
            report(id, *transaction, operation as u64, transaction.state as u64);
            false
        }
    };
    slot.sequence.fetch_add(1, Ordering::Release);
    if terminal {
        slot.terminal.store(true, Ordering::Release);
        for current in &CURRENT_GENERATION {
            let _ =
                current.compare_exchange(handle.generation, 0, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

pub fn reserve_frame(handle: Handle, frame: u64) {
    mutate(handle, Operation::ReserveFrame, |transaction| {
        transaction.frame = frame;
    });
}

pub fn populated(handle: Handle, requested: usize, actual: usize, checksum: u64) {
    if requested != actual {
        let transaction = unsafe { *SLOTS[handle.index as usize].transaction.get() };
        report(PAGER_004, transaction, requested as u64, actual as u64);
        return;
    }
    mutate(handle, Operation::Populate, |transaction| {
        transaction.requested = requested as u32;
        transaction.actual = actual as u32;
        transaction.checksum = checksum;
    });
}

pub fn commit(handle: Handle) {
    mutate(handle, Operation::Commit, |_| {});
}

pub fn observe_present(handle: Handle) {
    mutate(handle, Operation::ObservePresent, |_| {});
}

pub fn abort(handle: Handle, reason: u16, requested: usize, actual: usize) {
    mutate(handle, Operation::Abort, |transaction| {
        transaction.reason = reason;
        transaction.requested = requested as u32;
        transaction.actual = actual as u32;
    });
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    let count_at = writer.len();
    writer.u32(0);
    let mut count = 0u32;
    let mut unstable = 0u32;
    for slot in &SLOTS {
        let key = slot.key.load(Ordering::Acquire);
        if key == 0 || key == CLAIMED {
            continue;
        }
        let before = slot.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            unstable = 1;
            continue;
        }
        let transaction = unsafe { *slot.transaction.get() };
        let after = slot.sequence.load(Ordering::Acquire);
        if before != after || after & 1 != 0 || transaction.generation != key {
            unstable = 1;
            continue;
        }
        writer.u64(transaction.generation);
        writer.u64(transaction.l4);
        writer.u64(transaction.vma_generation);
        writer.u64(transaction.page);
        writer.u64(transaction.frame);
        writer.u32(transaction.pid);
        writer.u8(transaction.state as u8);
        writer.u8(0);
        writer.u16(transaction.reason);
        writer.u32(transaction.requested);
        writer.u32(transaction.actual);
        writer.u64(transaction.checksum);
        count += 1;
    }
    writer.patch_u32(count_at, count);
    unstable
}

pub fn snapshot_flags() -> u32 {
    u32::from(SLOTS.iter().any(|slot| {
        slot.key.load(Ordering::Acquire) == CLAIMED
            || slot.sequence.load(Ordering::Acquire) & 1 != 0
    }))
}
