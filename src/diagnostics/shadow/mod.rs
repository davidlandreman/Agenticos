//! Independently shaped, crash-readable diagnostic state machines.

pub mod address_space;
pub mod continuation;
pub mod io;
pub mod pager;
pub mod scheduler;
pub mod stack;

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct ViolationRecord {
    pub invariant_id: u32,
    pub severity: u8,
    pub cpu: u8,
    pub mode: u8,
    pub domain: u8,
    pub epoch: u64,
    pub subject: u64,
    pub expected0: u64,
    pub observed0: u64,
    pub expected1: u64,
    pub observed1: u64,
    pub trace_sequence: u64,
}

struct Latch(UnsafeCell<ViolationRecord>);
unsafe impl Sync for Latch {}

static ID: AtomicU32 = AtomicU32::new(0);
static RECORD: Latch = Latch(UnsafeCell::new(ViolationRecord {
    invariant_id: 0,
    severity: 0,
    cpu: 0,
    mode: 0,
    domain: 0,
    epoch: 0,
    subject: 0,
    expected0: 0,
    observed0: 0,
    expected1: 0,
    observed1: 0,
    trace_sequence: 0,
}));

pub fn latch(mut record: ViolationRecord) -> bool {
    if ID
        .compare_exchange(0, u32::MAX, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    record.cpu = if crate::diagnostics::percpu_ready() {
        crate::arch::x86_64::percpu::cpu_id() as u8
    } else {
        0
    };
    unsafe { RECORD.0.get().write(record) };
    ID.store(record.invariant_id, Ordering::Release);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::InvariantLatched,
        u64::from(record.invariant_id),
        record.expected0,
        record.observed0,
        record.epoch,
    );
    true
}

pub fn first() -> Option<ViolationRecord> {
    let id = ID.load(Ordering::Acquire);
    if id == 0 || id == u32::MAX {
        return None;
    }
    let record = unsafe { RECORD.0.get().read() };
    (record.invariant_id == id).then_some(record)
}
