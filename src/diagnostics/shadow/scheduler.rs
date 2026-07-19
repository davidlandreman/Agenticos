//! Composite scheduler shadow, mutated under the production scheduler lock.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::process::entity::EntityId;

use super::{latch, ViolationRecord};

pub const SCHED_001: u32 = 0x0100_0001;
pub const SCHED_002: u32 = 0x0100_0002;
pub const SCHED_004: u32 = 0x0100_0004;
pub const SCHED_005: u32 = 0x0100_0005;
pub const SCHED_007: u32 = 0x0100_0007;
pub const DIAG_CAPACITY_SCHEDULER: u32 = 0x0f00_0001;

const CAPACITY: usize = crate::process::run_queue::MAX_ENTITIES * 2;
const NO_CPU: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ShadowState {
    Absent = 0,
    Blocked = 1,
    ReadyQueued = 2,
    ReadyUnpublished = 3,
    Running = 4,
    Dead = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum OperationKind {
    Register = 1,
    MakeReady = 2,
    Dispatch = 3,
    BeginSave = 4,
    Publish = 5,
    ResumeSameCpu = 6,
    Block = 7,
    Yield = 8,
    Unregister = 9,
    SetAffinity = 10,
    ForceRunning = 11,
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ShadowEntity {
    pub key: u64,
    pub state: ShadowState,
    pub cpu: u8,
    pub affinity: u8,
    pub generation: u8,
    pub _reserved: u32,
    pub last_epoch: u64,
    pub last_operation: OperationKind,
    pub _tail: [u8; 7],
}

impl ShadowEntity {
    const fn empty() -> Self {
        Self {
            key: 0,
            state: ShadowState::Absent,
            cpu: NO_CPU,
            affinity: NO_CPU,
            generation: 0,
            _reserved: 0,
            last_epoch: 0,
            last_operation: OperationKind::Register,
            _tail: [0; 7],
        }
    }
}

struct Slot(UnsafeCell<ShadowEntity>);
unsafe impl Sync for Slot {}

static SLOTS: [Slot; CAPACITY] = [const { Slot(UnsafeCell::new(ShadowEntity::empty())) }; CAPACITY];
static EPOCH: AtomicU64 = AtomicU64::new(0);
static PENDING_OPERATION: AtomicU64 = AtomicU64::new(0);
static PENDING_SUBJECT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct Transition {
    pub operation: OperationKind,
    pub cpu: u8,
    pub published: bool,
    pub allow_running_exit: bool,
}

pub fn entity_key(id: EntityId) -> u64 {
    match id {
        EntityId::KernelThread(pid) => u64::from(pid),
        EntityId::UserProcess(pid) => (1u64 << 63) | u64::from(pid),
    }
}

/// Return the most recently committed scheduler-shadow epoch.
///
/// Commit-adjacent flight-recorder hooks call this after applying their
/// shadow transition. A normal result is even; an odd value means a crash
/// interrupted a transition and is preserved as evidence rather than retried.
pub fn committed_epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}

pub fn running_entity_on_cpu(cpu: usize) -> Option<u64> {
    let before = EPOCH.load(Ordering::Acquire);
    if before & 1 != 0 {
        return None;
    }
    let found = SLOTS.iter().find_map(|slot| {
        let entity = unsafe { *slot.0.get() };
        (entity.key != 0 && entity.state == ShadowState::Running && usize::from(entity.cpu) == cpu)
            .then_some(entity.key)
    });
    let after = EPOCH.load(Ordering::Acquire);
    (before == after).then_some(found).flatten()
}

fn begin(operation: OperationKind, key: u64) -> u64 {
    PENDING_SUBJECT.store(key, Ordering::Relaxed);
    PENDING_OPERATION.store(operation as u64, Ordering::Relaxed);
    let previous = EPOCH.fetch_add(1, Ordering::AcqRel);
    debug_assert_eq!(previous & 1, 0, "nested scheduler shadow transition");
    previous + 1
}

fn finish(odd_epoch: u64) -> u64 {
    let committed = odd_epoch + 1;
    EPOCH.store(committed, Ordering::Release);
    PENDING_OPERATION.store(0, Ordering::Release);
    PENDING_SUBJECT.store(0, Ordering::Release);
    committed
}

fn slot_for(key: u64, create: bool) -> Option<&'static Slot> {
    let start = (key ^ (key >> 32)) as usize % CAPACITY;
    for distance in 0..CAPACITY {
        let slot = &SLOTS[(start + distance) % CAPACITY];
        let current = unsafe { &*slot.0.get() };
        if current.key == key {
            return Some(slot);
        }
        if current.key == 0 {
            if !create {
                return None;
            }
            unsafe {
                (*slot.0.get()).key = key;
            }
            return Some(slot);
        }
    }
    None
}

fn report(id: u32, epoch: u64, key: u64, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 1,
        epoch,
        subject: key,
        expected0: expected,
        observed0: observed,
        expected1: 0,
        observed1: 0,
        trace_sequence: 0,
    });
    if first && crate::diagnostics::personality() == crate::diagnostics::Personality::Strict {
        crate::diagnostics::crash::begin_invariant(id);
    }
}

pub fn apply(id: EntityId, transition: Transition) {
    let key = entity_key(id);
    let odd = begin(transition.operation, key);
    let Some(slot) = slot_for(key, transition.operation == OperationKind::Register) else {
        report(DIAG_CAPACITY_SCHEDULER, odd, key, CAPACITY as u64, 0);
        finish(odd);
        return;
    };
    let current = unsafe { *slot.0.get() };
    if matches!(
        transition.operation,
        OperationKind::Dispatch | OperationKind::ForceRunning
    ) {
        if current.state == ShadowState::Running && current.cpu != transition.cpu {
            report(
                SCHED_001,
                odd,
                key,
                u64::from(current.cpu),
                u64::from(transition.cpu),
            );
            finish(odd);
            return;
        }
        for candidate in &SLOTS {
            let other = unsafe { *candidate.0.get() };
            if other.key != 0
                && other.key != key
                && other.state == ShadowState::Running
                && other.cpu == transition.cpu
            {
                report(SCHED_002, odd, key, 0, other.key);
                finish(odd);
                return;
            }
        }
    }
    let result = transition_state(current, transition);
    match result {
        Ok(mut next) => {
            let committed = odd + 1;
            next.last_epoch = committed;
            next.last_operation = transition.operation;
            unsafe { slot.0.get().write(next) };
        }
        Err(invariant) => report(
            invariant,
            odd,
            key,
            transition.operation as u64,
            current.state as u64,
        ),
    }
    finish(odd);
}

pub fn transition_state(
    mut entity: ShadowEntity,
    transition: Transition,
) -> Result<ShadowEntity, u32> {
    match transition.operation {
        OperationKind::Register => match entity.state {
            ShadowState::Absent | ShadowState::Dead => {
                entity.state = ShadowState::Blocked;
                entity.cpu = NO_CPU;
                entity.generation = entity.generation.wrapping_add(1).max(1);
            }
            _ => return Err(SCHED_007),
        },
        OperationKind::MakeReady => match entity.state {
            ShadowState::Blocked | ShadowState::ReadyUnpublished => {
                entity.state = if transition.published {
                    ShadowState::ReadyQueued
                } else {
                    ShadowState::ReadyUnpublished
                };
                entity.cpu = if transition.published {
                    NO_CPU
                } else {
                    transition.cpu
                };
            }
            ShadowState::ReadyQueued | ShadowState::Running => {}
            ShadowState::Absent | ShadowState::Dead => return Err(SCHED_004),
        },
        OperationKind::Dispatch | OperationKind::ForceRunning => {
            if transition.operation == OperationKind::Dispatch
                && entity.state != ShadowState::ReadyQueued
            {
                return Err(SCHED_004);
            }
            if matches!(entity.state, ShadowState::Absent | ShadowState::Dead) {
                return Err(SCHED_004);
            }
            if entity.affinity != NO_CPU && entity.affinity != transition.cpu {
                return Err(SCHED_005);
            }
            entity.state = ShadowState::Running;
            entity.cpu = transition.cpu;
        }
        OperationKind::BeginSave => {
            if entity.state != ShadowState::Running || entity.cpu != transition.cpu {
                return Err(SCHED_001);
            }
            entity.state = ShadowState::ReadyUnpublished;
        }
        OperationKind::Publish => match entity.state {
            ShadowState::ReadyUnpublished => {
                entity.state = ShadowState::ReadyQueued;
                entity.cpu = NO_CPU;
            }
            ShadowState::Blocked => {}
            _ => return Err(SCHED_004),
        },
        OperationKind::ResumeSameCpu => {
            if entity.state != ShadowState::ReadyUnpublished || entity.cpu != transition.cpu {
                return Err(SCHED_004);
            }
            entity.state = ShadowState::Running;
        }
        OperationKind::Block => {
            if matches!(entity.state, ShadowState::Absent | ShadowState::Dead) {
                return Err(SCHED_007);
            }
            entity.state = ShadowState::Blocked;
            entity.cpu = NO_CPU;
        }
        OperationKind::Yield => {
            if entity.state != ShadowState::Running {
                return Err(SCHED_004);
            }
            entity.state = ShadowState::ReadyQueued;
            entity.cpu = NO_CPU;
        }
        OperationKind::Unregister => {
            if entity.state == ShadowState::Running && !transition.allow_running_exit {
                return Err(SCHED_007);
            }
            if matches!(entity.state, ShadowState::Absent | ShadowState::Dead) {
                return Err(SCHED_007);
            }
            entity.state = ShadowState::Dead;
            entity.cpu = NO_CPU;
        }
        OperationKind::SetAffinity => {
            entity.affinity = transition.cpu;
        }
    }
    Ok(entity)
}

pub fn register(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::Register,
            cpu: NO_CPU,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn make_ready(id: EntityId, published: bool) {
    apply(
        id,
        Transition {
            operation: OperationKind::MakeReady,
            cpu: crate::arch::x86_64::percpu::cpu_id() as u8,
            published,
            allow_running_exit: false,
        },
    );
}

pub fn dispatch(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::Dispatch,
            cpu: crate::arch::x86_64::percpu::cpu_id() as u8,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn begin_save(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::BeginSave,
            cpu: crate::arch::x86_64::percpu::cpu_id() as u8,
            published: false,
            allow_running_exit: false,
        },
    );
}

pub fn publish(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::Publish,
            cpu: NO_CPU,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn resume_same_cpu(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::ResumeSameCpu,
            cpu: crate::arch::x86_64::percpu::cpu_id() as u8,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn block(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::Block,
            cpu: NO_CPU,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn unregister(id: EntityId, was_current: bool) {
    apply(
        id,
        Transition {
            operation: OperationKind::Unregister,
            cpu: NO_CPU,
            published: true,
            allow_running_exit: was_current,
        },
    );
}

pub fn set_affinity(id: EntityId, cpu: Option<usize>) {
    apply(
        id,
        Transition {
            operation: OperationKind::SetAffinity,
            cpu: cpu.map(|value| value as u8).unwrap_or(NO_CPU),
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn force_running(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::ForceRunning,
            cpu: crate::arch::x86_64::percpu::cpu_id() as u8,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn yielded(id: EntityId) {
    apply(
        id,
        Transition {
            operation: OperationKind::Yield,
            cpu: NO_CPU,
            published: true,
            allow_running_exit: false,
        },
    );
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    let before = EPOCH.load(Ordering::Acquire);
    writer.u64(before);
    writer.u64(PENDING_OPERATION.load(Ordering::Acquire));
    writer.u64(PENDING_SUBJECT.load(Ordering::Acquire));
    let count_at = writer.len();
    writer.u32(0);
    let mut count = 0u32;
    for slot in &SLOTS {
        let entity = unsafe { *slot.0.get() };
        if entity.key == 0 {
            continue;
        }
        writer.u64(entity.key);
        writer.u8(entity.state as u8);
        writer.u8(entity.cpu);
        writer.u8(entity.affinity);
        writer.u8(entity.generation);
        writer.u64(entity.last_epoch);
        writer.u8(entity.last_operation as u8);
        writer.raw(&[0; 7]);
        count += 1;
    }
    writer.patch_u32(count_at, count);
    let after = EPOCH.load(Ordering::Acquire);
    u32::from(before != after || before & 1 != 0)
}

pub fn snapshot_flags() -> u32 {
    u32::from(EPOCH.load(Ordering::Acquire) & 1 != 0)
}

#[cfg(feature = "test")]
pub fn initial_for_test(key: u64) -> ShadowEntity {
    let mut entity = ShadowEntity::empty();
    entity.key = key;
    entity
}
