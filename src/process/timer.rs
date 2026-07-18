//! Allocation-free-after-init deadline heap and bounded deferred delivery.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::interrupt_guard::InterruptMutex;

use super::entity::{EntityId, LatencyContract};

pub const MAX_TIMERS: usize = 512;
pub const MAX_TIMER_EXPIRATIONS_PER_PASS: usize = 32;
const INDEX_SLOTS: usize = MAX_TIMERS * 2;
const NO_DEADLINE: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    KernelSleep,
    UserSleep,
    UserNetworkTimeout,
    UserRealTimer,
    UserFutex,
    NetworkPoll,
    CompositorFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerKey {
    pub entity: EntityId,
    pub kind: TimerKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerAction {
    Wake {
        entity: EntityId,
        latency: Option<LatencyContract>,
    },
    UserSleep(u32),
    UserNetworkTimeout(u32),
    UserRealTimer(u32),
    UserFutex(u32),
}

#[derive(Debug, Clone, Copy)]
struct TimerEntry {
    key: TimerKey,
    deadline_tick: u64,
    sequence: u64,
    generation: u64,
    action: TimerAction,
}

#[derive(Debug, Clone, Copy)]
enum IndexSlot {
    Empty,
    Tombstone,
    Occupied { key: TimerKey, heap_index: usize },
}

pub struct TimerQueue {
    heap: Vec<TimerEntry>,
    index: [IndexSlot; INDEX_SLOTS],
    next_sequence: u64,
    initialized: bool,
    high_water: usize,
    capacity_failures: u64,
}

impl TimerQueue {
    pub const fn new() -> Self {
        Self {
            heap: Vec::new(),
            index: [IndexSlot::Empty; INDEX_SLOTS],
            next_sequence: 0,
            initialized: false,
            high_water: 0,
            capacity_failures: 0,
        }
    }

    fn init(&mut self) -> Result<(), ()> {
        if self.initialized {
            return Ok(());
        }
        self.heap.try_reserve(MAX_TIMERS).map_err(|_| ())?;
        self.initialized = true;
        Ok(())
    }

    fn hash(key: TimerKey) -> usize {
        let (tag, value) = match key.entity {
            EntityId::KernelThread(pid) => (0x9e37_79b9_u64, pid as u64),
            EntityId::UserProcess(pid) => (0x85eb_ca6b_u64, pid as u64),
        };
        let kind = match key.kind {
            TimerKind::KernelSleep => 1,
            TimerKind::UserSleep => 2,
            TimerKind::UserNetworkTimeout => 3,
            TimerKind::UserRealTimer => 4,
            TimerKind::NetworkPoll => 5,
            TimerKind::CompositorFrame => 6,
            TimerKind::UserFutex => 7,
        };
        value
            .wrapping_mul(tag)
            .wrapping_add(kind)
            .wrapping_mul(0xc2b2_ae35) as usize
            % INDEX_SLOTS
    }

    fn lookup_slot(&self, key: TimerKey) -> Option<usize> {
        let start = Self::hash(key);
        for offset in 0..INDEX_SLOTS {
            let slot = (start + offset) % INDEX_SLOTS;
            match self.index[slot] {
                IndexSlot::Empty => return None,
                IndexSlot::Occupied { key: candidate, .. } if candidate == key => {
                    return Some(slot);
                }
                IndexSlot::Tombstone | IndexSlot::Occupied { .. } => {}
            }
        }
        None
    }

    fn insertion_slot(&self, key: TimerKey) -> Option<usize> {
        let start = Self::hash(key);
        let mut tombstone = None;
        for offset in 0..INDEX_SLOTS {
            let slot = (start + offset) % INDEX_SLOTS;
            match self.index[slot] {
                IndexSlot::Empty => return Some(tombstone.unwrap_or(slot)),
                IndexSlot::Tombstone => {
                    tombstone.get_or_insert(slot);
                }
                IndexSlot::Occupied { key: candidate, .. } if candidate == key => {
                    return Some(slot);
                }
                IndexSlot::Occupied { .. } => {}
            }
        }
        tombstone
    }

    fn heap_index(&self, key: TimerKey) -> Option<usize> {
        let slot = self.lookup_slot(key)?;
        match self.index[slot] {
            IndexSlot::Occupied { heap_index, .. } => Some(heap_index),
            IndexSlot::Empty | IndexSlot::Tombstone => None,
        }
    }

    fn set_heap_index(&mut self, key: TimerKey, heap_index: usize) {
        let slot = self
            .lookup_slot(key)
            .expect("timer index lost an existing key");
        self.index[slot] = IndexSlot::Occupied { key, heap_index };
    }

    fn less(a: &TimerEntry, b: &TimerEntry) -> bool {
        (a.deadline_tick, a.sequence) < (b.deadline_tick, b.sequence)
    }

    fn swap_heap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        let a_key = self.heap[a].key;
        let b_key = self.heap[b].key;
        self.set_heap_index(a_key, a);
        self.set_heap_index(b_key, b);
    }

    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !Self::less(&self.heap[index], &self.heap[parent]) {
                break;
            }
            self.swap_heap(index, parent);
            index = parent;
        }
    }

    fn sift_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.heap.len() {
                break;
            }
            let right = left + 1;
            let child =
                if right < self.heap.len() && Self::less(&self.heap[right], &self.heap[left]) {
                    right
                } else {
                    left
                };
            if !Self::less(&self.heap[child], &self.heap[index]) {
                break;
            }
            self.swap_heap(index, child);
            index = child;
        }
    }

    fn repair(&mut self, index: usize) {
        if index > 0 && Self::less(&self.heap[index], &self.heap[(index - 1) / 2]) {
            self.sift_up(index);
        } else {
            self.sift_down(index);
        }
    }

    fn arm(&mut self, key: TimerKey, deadline_tick: u64, action: TimerAction) -> Result<u64, ()> {
        self.init()?;
        if let Some(index) = self.heap_index(key) {
            let generation = self.heap[index].generation.wrapping_add(1);
            self.heap[index].deadline_tick = deadline_tick;
            self.heap[index].generation = generation;
            self.heap[index].action = action;
            self.repair(index);
            return Ok(generation);
        }
        if self.heap.len() >= MAX_TIMERS {
            self.capacity_failures = self.capacity_failures.saturating_add(1);
            return Err(());
        }
        let index_slot = self.insertion_slot(key).ok_or(())?;
        let heap_index = self.heap.len();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.heap.push(TimerEntry {
            key,
            deadline_tick,
            sequence,
            generation: 1,
            action,
        });
        self.index[index_slot] = IndexSlot::Occupied { key, heap_index };
        self.sift_up(heap_index);
        self.high_water = self.high_water.max(self.heap.len());
        Ok(1)
    }

    fn remove_at(&mut self, index: usize) -> TimerEntry {
        let removed_key = self.heap[index].key;
        let slot = self
            .lookup_slot(removed_key)
            .expect("timer index missing removed key");
        self.index[slot] = IndexSlot::Tombstone;
        let removed = self.heap.swap_remove(index);
        if index < self.heap.len() {
            let moved_key = self.heap[index].key;
            self.set_heap_index(moved_key, index);
            self.repair(index);
        }
        removed
    }

    fn cancel(&mut self, key: TimerKey) -> bool {
        let Some(index) = self.heap_index(key) else {
            return false;
        };
        self.remove_at(index);
        true
    }

    fn pop_due(&mut self, now: u64) -> Option<TimerEntry> {
        if self.heap.first()?.deadline_tick > now {
            return None;
        }
        Some(self.remove_at(0))
    }

    fn earliest(&self) -> u64 {
        self.heap
            .first()
            .map(|entry| entry.deadline_tick)
            .unwrap_or(NO_DEADLINE)
    }

    #[cfg(feature = "test")]
    pub fn arm_for_test(
        &mut self,
        key: TimerKey,
        deadline_tick: u64,
        action: TimerAction,
    ) -> Result<u64, ()> {
        self.arm(key, deadline_tick, action)
    }

    #[cfg(feature = "test")]
    pub fn cancel_for_test(&mut self, key: TimerKey) -> bool {
        self.cancel(key)
    }

    #[cfg(feature = "test")]
    pub fn pop_due_for_test(&mut self, now: u64) -> Option<(TimerKey, u64)> {
        self.pop_due(now)
            .map(|entry| (entry.key, entry.deadline_tick))
    }

    #[cfg(feature = "test")]
    pub fn pending_for_test(&self) -> usize {
        self.heap.len()
    }
}

static TIMERS: InterruptMutex<TimerQueue> = InterruptMutex::new(TimerQueue::new());
static EARLIEST_DEADLINE: AtomicU64 = AtomicU64::new(NO_DEADLINE);
static WORK_PENDING: AtomicBool = AtomicBool::new(false);
static TIMER_SERVICE_PID: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    TIMERS.lock().init().expect("timer heap reservation failed");
}

pub fn start_service() {
    if TIMER_SERVICE_PID.load(Ordering::Acquire) != 0 {
        return;
    }
    let pid = crate::process::spawn_process(String::from("timer-service"), None, service_main);
    TIMER_SERVICE_PID.store(pid as u64, Ordering::Release);
}

fn service_main() {
    loop {
        if take_work_pending() {
            crate::userland::readiness::retry_pending_wake();
            let now = crate::arch::x86_64::interrupts::get_timer_ticks();
            let _ = process_due(now);
            if deadline_due(now) {
                crate::process::yield_current();
                continue;
            }
        }
        crate::process::park_current(crate::process::BlockReason::WaitingForTimerWork);
    }
}

fn publish_earliest(queue: &TimerQueue) {
    EARLIEST_DEADLINE.store(queue.earliest(), Ordering::Release);
}

pub fn arm(key: TimerKey, deadline_tick: u64, action: TimerAction) -> Result<u64, ()> {
    let mut queue = TIMERS.lock();
    let generation = queue.arm(key, deadline_tick, action)?;
    publish_earliest(&queue);
    Ok(generation)
}

pub fn cancel(key: TimerKey) -> bool {
    let mut queue = TIMERS.lock();
    let cancelled = queue.cancel(key);
    publish_earliest(&queue);
    cancelled
}

pub fn cancel_entity(entity: EntityId) {
    for kind in [
        TimerKind::KernelSleep,
        TimerKind::UserSleep,
        TimerKind::UserNetworkTimeout,
        TimerKind::UserRealTimer,
        TimerKind::NetworkPoll,
        TimerKind::CompositorFrame,
        TimerKind::UserFutex,
    ] {
        let _ = cancel(TimerKey { entity, kind });
    }
}

pub fn deadline_due(now: u64) -> bool {
    now >= EARLIEST_DEADLINE.load(Ordering::Acquire)
}

/// PIT-side notification: no heap lock or allocation. The due flag is atomic;
/// waking the service performs one bounded scheduler-ready operation.
pub fn on_tick(now: u64) {
    if deadline_due(now) || crate::userland::readiness::wake_pending() {
        WORK_PENDING.store(true, Ordering::Release);
        let pid = TIMER_SERVICE_PID.load(Ordering::Acquire);
        if pid != 0 {
            crate::process::scheduler::SCHEDULER
                .lock()
                .wake_with_contract(
                    pid as crate::process::ProcessId,
                    Some(LatencyContract::new(1)),
                );
        }
    }
}

pub fn take_work_pending() -> bool {
    WORK_PENDING.swap(false, Ordering::AcqRel)
}

/// Drain one bounded pass. Actions run after the heap lock is released.
pub fn process_due(now: u64) -> usize {
    WORK_PENDING.store(false, Ordering::Release);
    let mut delivered = 0;
    while delivered < MAX_TIMER_EXPIRATIONS_PER_PASS {
        let entry = {
            let mut queue = TIMERS.lock();
            let entry = queue.pop_due(now);
            publish_earliest(&queue);
            entry
        };
        let Some(entry) = entry else {
            break;
        };
        let _generation = entry.generation;
        deliver(entry.action, now);
        delivered += 1;
    }
    if deadline_due(now) {
        WORK_PENDING.store(true, Ordering::Release);
    }
    delivered
}

fn deliver(action: TimerAction, now: u64) {
    match action {
        TimerAction::Wake { entity, latency } => {
            let _ = crate::process::scheduler::SCHEDULER
                .lock()
                .make_ready(entity, latency);
        }
        TimerAction::UserSleep(pid) => {
            crate::userland::lifecycle::expire_user_sleep(pid, now);
        }
        TimerAction::UserNetworkTimeout(pid) => {
            crate::userland::lifecycle::expire_network_wait(pid, now);
        }
        TimerAction::UserRealTimer(pid) => {
            crate::userland::lifecycle::expire_real_timer(pid, now);
        }
        TimerAction::UserFutex(pid) => {
            let _ = now;
            crate::userland::futex::expire_wait(pid);
        }
    }
}
