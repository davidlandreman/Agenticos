//! Saved ring-3 kernel-continuation lifetime and exact-wake shadow.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::process::CpuContext;

use super::{latch, ViolationRecord};

pub const CONT_001: u32 = 0x0500_0001;
pub const CONT_002: u32 = 0x0500_0002;
pub const CONT_003: u32 = 0x0500_0003;
pub const DIAG_CAPACITY_CONTINUATION: u32 = 0x0f00_0004;

const CAPACITY: usize = 128;
const CLAIMED: u64 = u64::MAX;
const FLAG_WAKE_PENDING: u8 = 1 << 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Saving = 1,
    PublishedBlocked = 2,
    Runnable = 3,
    Resuming = 4,
    Consumed = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    Publish = 1,
    Wake = 2,
    Dispatch = 3,
    Consume = 4,
}

pub fn transition_state(state: State, operation: Operation) -> Result<State, u32> {
    match (state, operation) {
        (State::Saving, Operation::Publish) => Ok(State::PublishedBlocked),
        (State::PublishedBlocked, Operation::Wake) => Ok(State::Runnable),
        (State::Runnable, Operation::Dispatch) => Ok(State::Resuming),
        (State::Resuming, Operation::Consume) => Ok(State::Consumed),
        (State::Saving, Operation::Dispatch) => Err(CONT_001),
        (_, Operation::Consume) => Err(CONT_003),
        _ => Err(CONT_001),
    }
}

#[derive(Clone, Copy)]
struct Continuation {
    generation: u64,
    token: u64,
    stack_generation: u64,
    rip: u64,
    rsp: u64,
    rflags: u64,
    stack_bottom: u64,
    stack_top: u64,
    pid: u32,
    state: State,
    flags: u8,
    _reserved: [u8; 2],
}

impl Continuation {
    const fn empty() -> Self {
        Self {
            generation: 0,
            token: 0,
            stack_generation: 0,
            rip: 0,
            rsp: 0,
            rflags: 0,
            stack_bottom: 0,
            stack_top: 0,
            pid: 0,
            state: State::Consumed,
            flags: 0,
            _reserved: [0; 2],
        }
    }
}

struct Slot {
    key: AtomicU64,
    pid_key: AtomicU64,
    sequence: AtomicU64,
    mutating: AtomicBool,
    terminal: AtomicBool,
    continuation: UnsafeCell<Continuation>,
}

unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            pid_key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            mutating: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            continuation: UnsafeCell::new(Continuation::empty()),
        }
    }
}

static SLOTS: [Slot; CAPACITY] = [const { Slot::new() }; CAPACITY];
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn report(id: u32, continuation: Continuation, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 4,
        epoch: continuation.generation,
        subject: u64::from(continuation.pid),
        expected0: expected,
        observed0: observed,
        expected1: continuation.token,
        observed1: continuation.rsp,
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

fn slot_for_pid(pid: u32) -> Option<&'static Slot> {
    SLOTS.iter().find(|slot| {
        let key = slot.key.load(Ordering::Acquire);
        key != 0
            && key != CLAIMED
            && !slot.terminal.load(Ordering::Acquire)
            && slot.pid_key.load(Ordering::Acquire) == u64::from(pid)
    })
}

pub fn allocate(pid: u32, token: u64, stack_generation: u64, stack_bottom: u64, stack_top: u64) {
    if !enabled() {
        return;
    }
    if let Some(slot) = slot_for_pid(pid) {
        report(CONT_003, unsafe { *slot.continuation.get() }, 0, token);
        return;
    }
    let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
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
        slot.pid_key.store(0, Ordering::Release);
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe {
            slot.continuation.get().write(Continuation {
                generation,
                token,
                stack_generation,
                rip: 0,
                rsp: 0,
                rflags: 0,
                stack_bottom,
                stack_top,
                pid,
                state: State::Saving,
                flags: 0,
                _reserved: [0; 2],
            });
        }
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.pid_key.store(u64::from(pid), Ordering::Release);
        slot.key.store(generation, Ordering::Release);
        return;
    }
    report(
        DIAG_CAPACITY_CONTINUATION,
        Continuation {
            generation,
            token,
            stack_generation,
            pid,
            stack_bottom,
            stack_top,
            ..Continuation::empty()
        },
        CAPACITY as u64,
        0,
    );
}

fn mutate(
    pid: u32,
    token: Option<u64>,
    operation: Operation,
    update: impl FnOnce(&mut Continuation),
) {
    if !enabled() {
        return;
    }
    let Some(slot) = slot_for_pid(pid) else {
        report(
            CONT_003,
            Continuation {
                pid,
                token: token.unwrap_or(0),
                ..Continuation::empty()
            },
            u64::from(pid),
            0,
        );
        return;
    };
    while slot
        .mutating
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        core::hint::spin_loop();
    }
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    let continuation = unsafe { &mut *slot.continuation.get() };
    if token.is_some_and(|token| token != continuation.token) {
        report(
            CONT_003,
            *continuation,
            continuation.token,
            token.unwrap_or(0),
        );
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.mutating.store(false, Ordering::Release);
        return;
    }
    let transition = match (continuation.state, operation) {
        // The scheduler intentionally accepts notify-before-publication and
        // withholds the entity from its run queue until publish_context. Keep
        // that causal fact without claiming the continuation is runnable yet.
        (State::Saving, Operation::Wake) if continuation.flags & FLAG_WAKE_PENDING == 0 => {
            continuation.flags |= FLAG_WAKE_PENDING;
            Ok(State::Saving)
        }
        (State::Saving, Operation::Publish) if continuation.flags & FLAG_WAKE_PENDING != 0 => {
            Ok(State::Runnable)
        }
        _ => transition_state(continuation.state, operation),
    };
    match transition {
        Ok(next) => {
            update(continuation);
            continuation.state = next;
            slot.sequence.fetch_add(1, Ordering::Release);
            if next == State::Consumed {
                slot.terminal.store(true, Ordering::Release);
            }
        }
        Err(id) => {
            report(
                id,
                *continuation,
                operation as u64,
                continuation.state as u64,
            );
            slot.sequence.fetch_add(1, Ordering::Release);
        }
    }
    slot.mutating.store(false, Ordering::Release);
}

pub fn published(pid: u32, context: &CpuContext) {
    mutate(pid, None, Operation::Publish, |continuation| {
        continuation.rip = context.rip;
        continuation.rsp = context.rsp;
        continuation.rflags = context.rflags;
    });
}

pub fn wake(pid: u32, token: u64) {
    mutate(pid, Some(token), Operation::Wake, |_| {});
}

pub fn dispatch(pid: u32, context: &CpuContext) {
    let Some(slot) = slot_for_pid(pid) else {
        return;
    };
    let continuation = unsafe { *slot.continuation.get() };
    let valid_rip = context.rip >= 0xffff_8000_0000_0000;
    let valid_rsp = context.rsp >= continuation.stack_bottom
        && context.rsp < continuation.stack_top
        && context.rsp & 7 == 0;
    let live_stack = crate::diagnostics::shadow::stack::validate(
        continuation.stack_generation,
        pid,
        context.rsp,
    );
    if !valid_rip || !valid_rsp || !live_stack {
        report(CONT_002, continuation, continuation.stack_top, context.rsp);
        return;
    }
    mutate(pid, Some(continuation.token), Operation::Dispatch, |_| {});
}

pub fn consumed(pid: u32, token: u64) {
    mutate(pid, Some(token), Operation::Consume, |_| {});
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
        let continuation = unsafe { *slot.continuation.get() };
        let after = slot.sequence.load(Ordering::Acquire);
        if before != after || after & 1 != 0 || continuation.generation != key {
            unstable = 1;
            continue;
        }
        writer.u64(continuation.generation);
        writer.u64(continuation.token);
        writer.u64(continuation.stack_generation);
        writer.u64(continuation.rip);
        writer.u64(continuation.rsp);
        writer.u64(continuation.rflags);
        writer.u64(continuation.stack_bottom);
        writer.u64(continuation.stack_top);
        writer.u32(continuation.pid);
        writer.u8(continuation.state as u8);
        writer.u8(continuation.flags);
        writer.raw(&[0; 2]);
        count += 1;
    }
    writer.patch_u32(count_at, count);
    unstable
}

pub fn snapshot_flags() -> u32 {
    u32::from(SLOTS.iter().any(|slot| {
        slot.key.load(Ordering::Acquire) == CLAIMED
            || slot.mutating.load(Ordering::Acquire)
            || slot.sequence.load(Ordering::Acquire) & 1 != 0
    }))
}
