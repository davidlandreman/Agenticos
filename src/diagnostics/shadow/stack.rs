//! Per-task kernel-stack generation, activation, and retirement shadow.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::{latch, ViolationRecord};

pub const STACK_001: u32 = 0x0700_0001;
pub const STACK_002: u32 = 0x0700_0002;
pub const STACK_003: u32 = 0x0700_0003;
pub const DIAG_CAPACITY_STACK: u32 = 0x0f00_0006;

const CAPACITY: usize = 192;
const CLAIMED: u64 = u64::MAX;
const NO_CPU: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Allocated = 1,
    LiveInactive = 2,
    Active = 3,
    Retiring = 4,
    Dead = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    Publish = 1,
    Activate = 2,
    Deactivate = 3,
    BeginRetire = 4,
    Release = 5,
}

pub fn transition_state(state: State, operation: Operation) -> Result<State, u32> {
    match (state, operation) {
        (State::Allocated, Operation::Publish) => Ok(State::LiveInactive),
        (State::LiveInactive, Operation::Activate) => Ok(State::Active),
        (State::Active, Operation::Deactivate) => Ok(State::LiveInactive),
        (State::LiveInactive | State::Allocated, Operation::BeginRetire) => Ok(State::Retiring),
        (State::Retiring, Operation::Release) => Ok(State::Dead),
        (State::Active, Operation::BeginRetire) => Err(STACK_001),
        (_, Operation::Activate) => Err(STACK_002),
        _ => Err(STACK_003),
    }
}

#[derive(Clone, Copy)]
struct Stack {
    generation: u64,
    bottom: u64,
    top: u64,
    owner_pid: u32,
    state: State,
    active_cpu: u8,
    flags: u16,
    last_rsp: u64,
    last_epoch: u64,
}

impl Stack {
    const fn empty() -> Self {
        Self {
            generation: 0,
            bottom: 0,
            top: 0,
            owner_pid: 0,
            state: State::Dead,
            active_cpu: NO_CPU,
            flags: 0,
            last_rsp: 0,
            last_epoch: 0,
        }
    }
}

struct Slot {
    key: AtomicU64,
    sequence: AtomicU64,
    mutating: AtomicBool,
    terminal: AtomicBool,
    stack: UnsafeCell<Stack>,
}

unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            mutating: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            stack: UnsafeCell::new(Stack::empty()),
        }
    }
}

static SLOTS: [Slot; CAPACITY] = [const { Slot::new() }; CAPACITY];
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static EPOCH: AtomicU64 = AtomicU64::new(1);
static ACTIVE_GENERATION: [AtomicU64; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];
static PENDING_ABANDON_PID: [AtomicU32; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU32::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn report(id: u32, stack: Stack, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 6,
        epoch: stack.generation,
        subject: u64::from(stack.owner_pid),
        expected0: expected,
        observed0: observed,
        expected1: stack.top,
        observed1: stack.last_rsp,
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

fn slot_for(generation: u64) -> Option<&'static Slot> {
    SLOTS
        .iter()
        .find(|slot| slot.key.load(Ordering::Acquire) == generation)
}

pub fn allocate(bottom: u64, top: u64) -> u64 {
    if !enabled() {
        return 0;
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
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe {
            slot.stack.get().write(Stack {
                generation,
                bottom,
                top,
                state: State::Allocated,
                active_cpu: NO_CPU,
                last_epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
                ..Stack::empty()
            });
        }
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.key.store(generation, Ordering::Release);
        return generation;
    }
    report(
        DIAG_CAPACITY_STACK,
        Stack {
            generation,
            bottom,
            top,
            ..Stack::empty()
        },
        CAPACITY as u64,
        0,
    );
    0
}

fn mutate(generation: u64, operation: Operation, update: impl FnOnce(&mut Stack)) {
    if generation == 0 || !enabled() {
        return;
    }
    let Some(slot) = slot_for(generation) else {
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
    let stack = unsafe { &mut *slot.stack.get() };
    match transition_state(stack.state, operation) {
        Ok(next) => {
            update(stack);
            stack.state = next;
            stack.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
            if next == State::Dead {
                slot.terminal.store(true, Ordering::Release);
            }
        }
        Err(id) => report(id, *stack, operation as u64, stack.state as u64),
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn publish_owner(generation: u64, pid: u32) {
    mutate(generation, Operation::Publish, |stack| {
        stack.owner_pid = pid;
    });
}

pub fn activate(generation: u64, pid: u32, rsp: u64) {
    if generation == 0 || !enabled() {
        return;
    }
    let cpu = crate::arch::x86_64::percpu::cpu_id();
    let previous = ACTIVE_GENERATION[cpu].swap(generation, Ordering::AcqRel);
    if previous != 0 && previous != generation {
        if let Some(slot) = slot_for(previous) {
            report(
                STACK_001,
                unsafe { *slot.stack.get() },
                previous,
                generation,
            );
        }
    }
    let Some(slot) = slot_for(generation) else {
        return;
    };
    let snapshot = unsafe { *slot.stack.get() };
    if snapshot.owner_pid != pid || rsp != snapshot.top {
        report(STACK_002, snapshot, snapshot.top, rsp);
        return;
    }
    mutate(generation, Operation::Activate, |stack| {
        stack.active_cpu = cpu as u8;
        stack.last_rsp = rsp;
    });
}

pub fn deactivate_owner(pid: u32) {
    if !enabled() {
        return;
    }
    let cpu = crate::arch::x86_64::percpu::cpu_id();
    let generation = ACTIVE_GENERATION[cpu].swap(0, Ordering::AcqRel);
    if generation == 0 {
        return;
    }
    let Some(slot) = slot_for(generation) else {
        return;
    };
    let snapshot = unsafe { *slot.stack.get() };
    if snapshot.owner_pid != pid {
        report(
            STACK_002,
            snapshot,
            u64::from(snapshot.owner_pid),
            u64::from(pid),
        );
        return;
    }
    mutate(generation, Operation::Deactivate, |stack| {
        stack.active_cpu = NO_CPU;
    });
}

pub fn begin_abandon(pid: u32) {
    if enabled() {
        PENDING_ABANDON_PID[crate::arch::x86_64::percpu::cpu_id()].store(pid, Ordering::Release);
    }
}

pub fn has_pending_abandon() -> bool {
    enabled()
        && PENDING_ABANDON_PID[crate::arch::x86_64::percpu::cpu_id()].load(Ordering::Acquire) != 0
}

pub fn complete_abandon() {
    if !enabled() {
        return;
    }
    let pid = PENDING_ABANDON_PID[crate::arch::x86_64::percpu::cpu_id()].swap(0, Ordering::AcqRel);
    if pid != 0 {
        deactivate_owner(pid);
    }
}

pub fn begin_retire(generation: u64) {
    mutate(generation, Operation::BeginRetire, |_| {});
}

pub fn release(generation: u64) {
    mutate(generation, Operation::Release, |_| {});
}

pub fn validate(generation: u64, pid: u32, rsp: u64) -> bool {
    if generation == 0 || !enabled() {
        return true;
    }
    let Some(slot) = slot_for(generation) else {
        return false;
    };
    let before = slot.sequence.load(Ordering::Acquire);
    if before & 1 != 0 {
        return false;
    }
    let stack = unsafe { *slot.stack.get() };
    let after = slot.sequence.load(Ordering::Acquire);
    before == after
        && after & 1 == 0
        && stack.owner_pid == pid
        && stack.state == State::Active
        && rsp >= stack.bottom
        && rsp < stack.top
        && rsp & 7 == 0
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
        let stack = unsafe { *slot.stack.get() };
        let after = slot.sequence.load(Ordering::Acquire);
        if before != after || after & 1 != 0 || stack.generation != key {
            unstable = 1;
            continue;
        }
        writer.u64(stack.generation);
        writer.u64(stack.bottom);
        writer.u64(stack.top);
        writer.u32(stack.owner_pid);
        writer.u8(stack.state as u8);
        writer.u8(stack.active_cpu);
        writer.u16(stack.flags);
        writer.u64(stack.last_rsp);
        writer.u64(stack.last_epoch);
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
