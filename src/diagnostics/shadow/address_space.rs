//! Generated user-L4 ownership and activation shadow.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{latch, ViolationRecord};

pub const AS_001: u32 = 0x0600_0001;
pub const AS_002: u32 = 0x0600_0002;
pub const AS_003: u32 = 0x0600_0003;
pub const AS_004: u32 = 0x0600_0004;
pub const DIAG_CAPACITY_ADDRESS_SPACE: u32 = 0x0f00_0005;

const CAPACITY: usize = 128;
const CLAIMED: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Building = 1,
    LiveInactive = 2,
    Active = 3,
    Destroying = 4,
    Dead = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[allow(
    dead_code,
    reason = "pure transition-table operations are also exercised directly by diagnostics tests"
)]
pub enum Operation {
    Publish = 1,
    Activate = 2,
    Deactivate = 3,
    BeginDestroy = 4,
    Abort = 5,
    Release = 6,
}

pub fn transition_state(state: State, operation: Operation) -> Result<State, u32> {
    match (state, operation) {
        (State::Building, Operation::Publish) => Ok(State::LiveInactive),
        (State::LiveInactive, Operation::Activate) => Ok(State::Active),
        (State::Active, Operation::Activate) => Ok(State::Active),
        (State::Active, Operation::Deactivate) => Ok(State::LiveInactive),
        (State::Building, Operation::Deactivate) => Ok(State::Building),
        (State::LiveInactive, Operation::BeginDestroy) | (State::Building, Operation::Abort) => {
            Ok(State::Destroying)
        }
        (State::Destroying, Operation::Release) => Ok(State::Dead),
        (_, Operation::Activate) => Err(AS_002),
        (_, Operation::BeginDestroy | Operation::Abort) => Err(AS_003),
        _ => Err(AS_004),
    }
}

#[derive(Clone, Copy)]
struct Root {
    generation: u64,
    l4: u64,
    owner_tgid: u32,
    member_count: u16,
    state: State,
    active_mask: u8,
    vma_generation: u64,
    last_epoch: u64,
    _reserved: u64,
}

impl Root {
    const fn empty() -> Self {
        Self {
            generation: 0,
            l4: 0,
            owner_tgid: 0,
            member_count: 0,
            state: State::Dead,
            active_mask: 0,
            vma_generation: 0,
            last_epoch: 0,
            _reserved: 0,
        }
    }
}

struct Slot {
    key: AtomicU64,
    l4_key: AtomicU64,
    sequence: AtomicU64,
    mutating: AtomicBool,
    terminal: AtomicBool,
    root: UnsafeCell<Root>,
}

unsafe impl Sync for Slot {}

impl Slot {
    const fn new() -> Self {
        Self {
            key: AtomicU64::new(0),
            l4_key: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            mutating: AtomicBool::new(false),
            terminal: AtomicBool::new(false),
            root: UnsafeCell::new(Root::empty()),
        }
    }
}

static SLOTS: [Slot; CAPACITY] = [const { Slot::new() }; CAPACITY];
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static EPOCH: AtomicU64 = AtomicU64::new(1);
static ACTIVE_GENERATION: [AtomicU64; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::acpi::MAX_CPUS];

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn report(id: u32, root: Root, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 5,
        epoch: root.generation,
        subject: root.l4,
        expected0: expected,
        observed0: observed,
        expected1: u64::from(root.owner_tgid),
        observed1: u64::from(root.active_mask),
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

pub fn generation_for_l4(l4: u64) -> u64 {
    if !enabled() {
        return 0;
    }
    SLOTS
        .iter()
        .find(|slot| slot.l4_key.load(Ordering::Acquire) == l4)
        .map(|slot| slot.key.load(Ordering::Acquire))
        .filter(|generation| *generation != 0 && *generation != CLAIMED)
        .unwrap_or(0)
}

pub fn allocate(l4: u64) -> u64 {
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
        slot.l4_key.store(l4, Ordering::Relaxed);
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        unsafe {
            slot.root.get().write(Root {
                generation,
                l4,
                state: State::Building,
                last_epoch: EPOCH.fetch_add(1, Ordering::Relaxed),
                ..Root::empty()
            });
        }
        slot.sequence.fetch_add(1, Ordering::Release);
        slot.key.store(generation, Ordering::Release);
        return generation;
    }
    report(
        DIAG_CAPACITY_ADDRESS_SPACE,
        Root {
            generation,
            l4,
            ..Root::empty()
        },
        CAPACITY as u64,
        0,
    );
    0
}

fn mutate(generation: u64, operation: Operation, update: impl FnOnce(&mut Root)) {
    if generation == 0 || !enabled() {
        return;
    }
    let Some(slot) = slot_for(generation) else {
        report(
            AS_002,
            Root {
                generation,
                ..Root::empty()
            },
            generation,
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
    let root = unsafe { &mut *slot.root.get() };
    let transition = match (root.state, operation) {
        (State::Building, Operation::Publish) if root.active_mask != 0 => Ok(State::Active),
        _ => transition_state(root.state, operation),
    };
    match transition {
        Ok(next) => {
            update(root);
            root.state = next;
            root.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
            if next == State::Dead {
                slot.terminal.store(true, Ordering::Release);
            }
        }
        Err(id) => report(id, *root, operation as u64, root.state as u64),
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn publish_owner(generation: u64, tgid: u32, vma_generation: u64) {
    mutate(generation, Operation::Publish, |root| {
        root.owner_tgid = tgid;
        root.member_count = 1;
        root.vma_generation = vma_generation;
    });
}

pub fn update_vma_generation(generation: u64, vma_generation: u64) {
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
    unsafe {
        (*slot.root.get()).vma_generation = vma_generation;
        (*slot.root.get()).last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn member_join(generation: u64, tgid: u32) {
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
    let root = unsafe { &mut *slot.root.get() };
    if root.owner_tgid != tgid || root.member_count == u16::MAX {
        report(AS_004, *root, u64::from(root.owner_tgid), u64::from(tgid));
    } else {
        root.member_count += 1;
        root.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn member_leave(generation: u64, count: usize) {
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
    let root = unsafe { &mut *slot.root.get() };
    if count > usize::from(root.member_count) {
        report(AS_004, *root, u64::from(root.member_count), count as u64);
    } else {
        root.member_count -= count as u16;
        root.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn activate(generation: u64, l4: u64) {
    if generation == 0 || !enabled() {
        return;
    }
    let cpu = crate::arch::x86_64::percpu::cpu_id();
    let previous = ACTIVE_GENERATION[cpu].swap(generation, Ordering::AcqRel);
    if previous != 0 && previous != generation {
        deactivate_generation(previous, cpu);
    }
    let Some(slot) = slot_for(generation) else {
        report(
            AS_002,
            Root {
                generation,
                l4,
                ..Root::empty()
            },
            generation,
            0,
        );
        return;
    };
    let cpu_bit = 1u8 << cpu;
    while slot
        .mutating
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        core::hint::spin_loop();
    }
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    let root = unsafe { &mut *slot.root.get() };
    if root.l4 != l4 || matches!(root.state, State::Destroying | State::Dead) {
        report(AS_002, *root, root.l4, l4);
    } else if root.active_mask & !cpu_bit != 0 {
        report(
            AS_001,
            *root,
            u64::from(cpu_bit),
            u64::from(root.active_mask),
        );
    } else {
        root.active_mask |= cpu_bit;
        if root.state == State::LiveInactive {
            root.state = State::Active;
        }
        root.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

fn deactivate_generation(generation: u64, cpu: usize) {
    let Some(slot) = slot_for(generation) else {
        return;
    };
    let cpu_bit = 1u8 << cpu;
    while slot
        .mutating
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        core::hint::spin_loop();
    }
    slot.sequence.fetch_add(1, Ordering::AcqRel);
    let root = unsafe { &mut *slot.root.get() };
    root.active_mask &= !cpu_bit;
    if root.active_mask == 0 && root.state == State::Active {
        root.state = State::LiveInactive;
    }
    root.last_epoch = EPOCH.fetch_add(1, Ordering::Relaxed);
    slot.sequence.fetch_add(1, Ordering::Release);
    slot.mutating.store(false, Ordering::Release);
}

pub fn deactivate_cpu() {
    if !enabled() {
        return;
    }
    let cpu = crate::arch::x86_64::percpu::cpu_id();
    let generation = ACTIVE_GENERATION[cpu].swap(0, Ordering::AcqRel);
    if generation != 0 {
        deactivate_generation(generation, cpu);
    }
}

pub fn begin_destroy(generation: u64) {
    if let Some(slot) = slot_for(generation) {
        let root = unsafe { *slot.root.get() };
        if root.active_mask != 0 {
            report(AS_003, root, 0, u64::from(root.active_mask));
        }
    }
    let operation = slot_for(generation)
        .map(|slot| unsafe { (*slot.root.get()).state })
        .filter(|state| *state == State::Building)
        .map_or(Operation::BeginDestroy, |_| Operation::Abort);
    mutate(generation, operation, |_| {});
}

pub fn release(generation: u64) {
    mutate(generation, Operation::Release, |_| {});
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
        let root = unsafe { *slot.root.get() };
        let after = slot.sequence.load(Ordering::Acquire);
        if before != after || after & 1 != 0 || root.generation != key {
            unstable = 1;
            continue;
        }
        writer.u64(root.generation);
        writer.u64(root.l4);
        writer.u32(root.owner_tgid);
        writer.u16(root.member_count);
        writer.u8(root.state as u8);
        writer.u8(root.active_mask);
        writer.u64(root.vma_generation);
        writer.u64(root.last_epoch);
        writer.u64(0);
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
