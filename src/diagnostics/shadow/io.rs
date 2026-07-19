//! Exact-token VirtIO request/wake shadow for suspended ring-3 continuations.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{latch, ViolationRecord};

pub const IO_001: u32 = 0x0400_0001;
pub const IO_002: u32 = 0x0400_0002;
pub const IO_003: u32 = 0x0400_0003;
pub const IO_004: u32 = 0x0400_0004;
pub const CONT_004: u32 = 0x0500_0004;
pub const DIAG_CAPACITY_IO: u32 = 0x0f00_0003;

const CAPACITY: usize = 256;
const CLAIMED: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Submitted = 1,
    Completed = 2,
    WakePending = 3,
    WakeAccepted = 4,
    Consumed = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    Complete = 1,
    QueueWake = 2,
    AcceptWake = 3,
    Consume = 4,
}

pub fn transition_state(state: State, operation: Operation) -> Result<State, u32> {
    match (state, operation) {
        (State::Submitted, Operation::Complete) => Ok(State::Completed),
        (State::Completed, Operation::QueueWake) => Ok(State::WakePending),
        (State::WakePending, Operation::AcceptWake) => Ok(State::WakeAccepted),
        (State::WakeAccepted, Operation::Consume) => Ok(State::Consumed),
        (State::Submitted | State::Completed | State::WakePending, Operation::Consume) => {
            Err(IO_002)
        }
        _ => Err(IO_001),
    }
}

#[derive(Clone, Copy)]
struct Request {
    token: u64,
    page_generation: u64,
    pid: u32,
    requested: u32,
    actual: u32,
    device: u16,
    queue_head: u16,
    state: State,
    status: u8,
    _reserved: [u8; 6],
}

impl Request {
    const fn empty() -> Self {
        Self {
            token: 0,
            page_generation: 0,
            pid: 0,
            requested: 0,
            actual: 0,
            device: 0,
            queue_head: 0,
            state: State::Consumed,
            status: 0,
            _reserved: [0; 6],
        }
    }
}

struct Slot {
    key: AtomicU64,
    sequence: AtomicU64,
    terminal: AtomicBool,
    request: UnsafeCell<Request>,
}

unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            terminal: AtomicBool::new(false),
            request: UnsafeCell::new(Request::empty()),
        }
    }
}

static SLOTS: [Slot; CAPACITY] = [const { Slot::new() }; CAPACITY];

fn record(request: Request, phase: crate::diagnostics::trace::IoPhase, arg: u64) {
    // Ordinary filesystem traffic is already represented in the bounded I/O
    // shadow. Reserve the flight-recorder bandwidth for requests causally
    // attached to a pager transaction; otherwise command-heavy workloads can
    // evict the paging history we are trying to preserve and measurably alter
    // scheduler timing.
    if request.page_generation == 0 {
        return;
    }
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::IoToken,
        request.token,
        u64::from(phase as u8),
        arg,
        request.page_generation,
    );
}

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn report(id: u32, request: Request, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 3,
        epoch: request.page_generation,
        subject: request.token,
        expected0: expected,
        observed0: observed,
        expected1: u64::from(request.pid),
        observed1: u64::from(request.queue_head),
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

fn slot_for(token: u64) -> Option<&'static Slot> {
    SLOTS
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == token)
}

pub fn submitted(
    token: u64,
    page_generation: u64,
    pid: u32,
    device: usize,
    queue_head: u16,
    requested: usize,
) {
    if !enabled() {
        return;
    }
    if slot_for(token).is_some() {
        report(
            IO_001,
            Request {
                token,
                ..Request::empty()
            },
            0,
            token,
        );
        return;
    }
    for slot in &SLOTS {
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
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe {
            slot.request.get().write(Request {
                token,
                page_generation,
                pid,
                requested: requested as u32,
                actual: 0,
                device: device as u16,
                queue_head,
                state: State::Submitted,
                status: 0xff,
                _reserved: [0; 6],
            });
        }
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.key.store(token, Ordering::Release);
        record(
            unsafe { *slot.request.get() },
            crate::diagnostics::trace::IoPhase::Submitted,
            u64::from(pid) | ((device as u64) << 32) | (u64::from(queue_head) << 48),
        );
        return;
    }
    report(
        DIAG_CAPACITY_IO,
        Request {
            token,
            page_generation,
            pid,
            ..Request::empty()
        },
        CAPACITY as u64,
        0,
    );
}

fn mutate(token: u64, operation: Operation, pid: Option<u32>, update: impl FnOnce(&mut Request)) {
    if !enabled() {
        return;
    }
    let Some(slot) = slot_for(token) else {
        report(
            IO_003,
            Request {
                token,
                ..Request::empty()
            },
            token,
            0,
        );
        return;
    };
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    let request = unsafe { &mut *slot.request.get() };
    if let Some(pid) = pid {
        if request.pid != pid {
            report(IO_003, *request, u64::from(request.pid), u64::from(pid));
            slot.sequence.fetch_add(1, Ordering::Release);
            return;
        }
    }
    match transition_state(request.state, operation) {
        Ok(next) => {
            update(request);
            request.state = next;
            let snapshot = *request;
            slot.sequence.fetch_add(1, Ordering::Release);
            if next == State::Consumed {
                slot.terminal.store(true, Ordering::Release);
            }
            let (phase, arg) = match next {
                State::Submitted => (
                    crate::diagnostics::trace::IoPhase::Submitted,
                    u64::from(snapshot.pid)
                        | (u64::from(snapshot.device) << 32)
                        | (u64::from(snapshot.queue_head) << 48),
                ),
                State::Completed => (
                    crate::diagnostics::trace::IoPhase::Completed,
                    u64::from(snapshot.status) | (u64::from(snapshot.actual) << 32),
                ),
                State::WakePending => (
                    crate::diagnostics::trace::IoPhase::WakeQueued,
                    u64::from(snapshot.pid),
                ),
                State::WakeAccepted => (
                    crate::diagnostics::trace::IoPhase::WakeAccepted,
                    u64::from(snapshot.pid),
                ),
                State::Consumed => (
                    crate::diagnostics::trace::IoPhase::Consumed,
                    u64::from(snapshot.pid),
                ),
            };
            record(snapshot, phase, arg);
        }
        Err(id) => {
            report(id, *request, operation as u64, request.state as u64);
            slot.sequence.fetch_add(1, Ordering::Release);
        }
    }
}

pub fn completed(token: u64, status: u8, actual: u32) {
    mutate(token, Operation::Complete, None, |request| {
        request.status = status;
        request.actual = actual;
    });
}

pub fn queue_wake(token: u64, pid: u32) {
    mutate(token, Operation::QueueWake, Some(pid), |_| {});
}

pub fn accept_wake(token: u64, pid: u32) {
    mutate(token, Operation::AcceptWake, Some(pid), |_| {});
}

pub fn consumed(token: u64) {
    mutate(token, Operation::Consume, None, |_| {});
}

pub fn wake_lost(token: u64, pid: u32) {
    if !enabled() {
        return;
    }
    let request = slot_for(token)
        .map(|slot| unsafe { *slot.request.get() })
        .unwrap_or(Request {
            token,
            pid,
            ..Request::empty()
        });
    record(
        request,
        crate::diagnostics::trace::IoPhase::WakeLost,
        u64::from(pid),
    );
    report(IO_004, request, 1, 0);
}

pub fn wrong_wake(token: u64, pid: u32, awaited: u64) {
    if !enabled() {
        return;
    }
    let request = slot_for(token)
        .map(|slot| unsafe { *slot.request.get() })
        .unwrap_or(Request {
            token,
            pid,
            ..Request::empty()
        });
    record(
        request,
        crate::diagnostics::trace::IoPhase::WrongWake,
        awaited,
    );
    report(IO_003, request, awaited, token);
}

pub fn reject_generic_io_wake(pid: u32, token: u64) {
    if !enabled() {
        return;
    }
    let request = slot_for(token)
        .map(|slot| unsafe { *slot.request.get() })
        .unwrap_or(Request {
            token,
            pid,
            ..Request::empty()
        });
    record(
        request,
        crate::diagnostics::trace::IoPhase::GenericWakeRejected,
        u64::from(pid),
    );
    report(CONT_004, request, token, 0);
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    let count_at = writer.len();
    writer.u32(0);
    let mut count = 0u32;
    let mut unstable = 0u32;
    for slot in &SLOTS {
        let key = slot.key.load(Ordering::Acquire);
        if key == 0 || key == CLAIMED {
            unstable |= u32::from(key == CLAIMED);
            continue;
        }
        let before = slot.sequence.load(Ordering::Acquire);
        if before & 1 != 0 {
            unstable = 1;
            continue;
        }
        let request = unsafe { *slot.request.get() };
        let after = slot.sequence.load(Ordering::Acquire);
        if before != after || after & 1 != 0 || request.token != key {
            unstable = 1;
            continue;
        }
        writer.u64(request.token);
        writer.u64(request.page_generation);
        writer.u32(request.pid);
        writer.u32(request.requested);
        writer.u32(request.actual);
        writer.u16(request.device);
        writer.u16(request.queue_head);
        writer.u8(request.state as u8);
        writer.u8(request.status);
        writer.raw(&[0; 6]);
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
