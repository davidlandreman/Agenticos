//! Privilege-neutral fair scheduler.
//!
//! Kernel threads and ring-3 processes are tagged entities in one queue. Each
//! running entity gets one PIT tick before it is requeued at the tail; overdue
//! one-shot latency contracts may override FIFO order.

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::context::CpuContext;
use super::entity::{EntityId, LatencyContract, RunState};
use super::pcb::{BlockReason, ProcessControlBlock, ProcessState, WakeEvents};
use super::process::ProcessId;
use super::run_queue::RunQueue;
use super::stack::free_stack;

/// Default time slice in timer ticks
/// With 100 Hz timer (10ms per tick), 2 ticks = 20ms per time slice
/// This provides smooth multitasking where processes appear to run simultaneously
pub const DEFAULT_TIME_SLICE: u64 = 1;

/// Lightweight process info for display purposes (e.g., task manager)
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: ProcessId,
    /// Human-readable process name
    pub name: String,
    /// Current execution state
    pub state: ProcessState,
    /// Total CPU time consumed (in timer ticks)
    pub total_runtime: u64,
    /// Stack size in bytes
    pub stack_size: usize,
}

/// Compatibility view of a tagged scheduling decision used by QEMU tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runnable {
    KernelThread(ProcessId),
    RingThree(u32),
}

#[derive(Debug, Clone, Copy)]
pub struct SchedEntity {
    pub state: RunState,
    pub runtime_ticks: u64,
    pub ready_since_tick: u64,
    pub must_run_by_tick: Option<u64>,
    pub latency_contract: Option<LatencyContract>,
    /// Full architecture context is safe for another CPU to restore.
    pub context_published: bool,
    /// Optional logical CPU constraint. User tasks that share an address
    /// space are pinned together until remote user-TLB shootdown exists.
    pub cpu_affinity: Option<usize>,
}

impl SchedEntity {
    const fn new(_id: EntityId) -> Self {
        Self {
            state: RunState::Blocked,
            runtime_ticks: 0,
            ready_since_tick: 0,
            must_run_by_tick: None,
            latency_contract: None,
            context_published: true,
            cpu_affinity: None,
        }
    }
}

/// Global scheduler instance
pub static SCHEDULER: InterruptMutex<Scheduler> = InterruptMutex::new_tracked(
    Scheduler::new(),
    crate::diagnostics::shadow::locks::LockClassId::Scheduler,
);

/// Fair scheduler for every runnable entity on the CPU.
pub struct Scheduler {
    /// All processes indexed by PID
    processes: BTreeMap<ProcessId, ProcessControlBlock>,
    /// Privilege-neutral scheduling state indexed by stable tagged identity.
    entities: BTreeMap<EntityId, SchedEntity>,
    /// The one ready queue shared by kernel threads and user processes.
    run_queue: RunQueue,
    /// Entity whose execution state is loaded on each logical CPU.
    current: [Option<EntityId>; crate::arch::x86_64::acpi::MAX_CPUS],
    /// The idle process PID (runs when nothing else is ready)
    pub idle_pid: Option<ProcessId>,
    /// Whether scheduler is initialized
    initialized: bool,
    /// Processes waiting for signal events (not time-based)
    signal_waiters: Vec<ProcessId>,
    /// Number of contracted dispatches selected after their ceiling.
    latency_misses: u64,
    /// Whether mutations belong to the singleton production scheduler and
    /// therefore participate in the global crash-readable shadow.
    shadow_observed: bool,
}

impl Scheduler {
    fn trace_dispatch(
        &self,
        id: EntityId,
        source: crate::diagnostics::trace::DispatchSource,
        missed_deadline: bool,
    ) {
        if !self.shadow_observed {
            return;
        }
        let arg1 = u64::from(source as u8) | (u64::from(missed_deadline) << 8);
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::SchedulerDispatch,
            crate::diagnostics::shadow::scheduler::entity_key(id),
            crate::arch::x86_64::percpu::cpu_id() as u64,
            arg1,
            crate::diagnostics::shadow::scheduler::committed_epoch(),
        );
    }

    fn trace_context_publish(&self, id: EntityId, existed: bool, enqueued: bool) {
        if !self.shadow_observed {
            return;
        }
        let state = self
            .entities
            .get(&id)
            .map_or(0, |entity| match entity.state {
                RunState::Ready => 1,
                RunState::Running => 2,
                RunState::Blocked => 3,
                RunState::Dead => 4,
            });
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::ContextPublish,
            crate::diagnostics::shadow::scheduler::entity_key(id),
            state,
            u64::from(existed) | (u64::from(enqueued) << 1),
            crate::diagnostics::shadow::scheduler::committed_epoch(),
        );
    }

    /// Create a new scheduler (const for static initialization)
    pub const fn new() -> Self {
        Scheduler {
            processes: BTreeMap::new(),
            entities: BTreeMap::new(),
            run_queue: RunQueue::new(),
            current: [None; crate::arch::x86_64::acpi::MAX_CPUS],
            idle_pid: None,
            initialized: false,
            signal_waiters: Vec::new(),
            latency_misses: 0,
            shadow_observed: false,
        }
    }

    /// Mark this scheduler as the production instance before initialization.
    /// Isolated scheduler values used by unit tests intentionally remain out
    /// of the singleton shadow namespace.
    pub fn observe_with_shadow(&mut self) {
        assert!(!self.initialized, "scheduler shadow enabled after init");
        self.shadow_observed = true;
    }

    /// Initialize the scheduler with an idle process
    pub fn init(&mut self) {
        if self.initialized {
            return;
        }

        self.run_queue
            .reserve()
            .expect("scheduler run-queue reservation failed");

        // Create idle process - it is never inserted in the normal run queue.
        let idle_pid = super::process::allocate_pid();
        let mut idle_pcb = ProcessControlBlock::new(idle_pid, String::from("idle"));
        idle_pcb.state = ProcessState::Ready;
        // Idle process doesn't need a real stack - it runs in kernel context

        self.idle_pid = Some(idle_pid);
        self.processes.insert(idle_pid, idle_pcb);
        self.entities.insert(
            EntityId::KernelThread(idle_pid),
            SchedEntity::new(EntityId::KernelThread(idle_pid)),
        );
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::register(EntityId::KernelThread(idle_pid));
        }
        self.initialized = true;

        crate::debug_info!("Scheduler initialized with idle process PID {:?}", idle_pid);
    }

    /// Spawn a new process and add it to the ready queue
    ///
    /// # Arguments
    /// * `pcb` - The process control block for the new process
    ///
    /// # Returns
    /// The PID of the newly spawned process
    pub fn spawn(&mut self, mut pcb: ProcessControlBlock) -> ProcessId {
        let pid = pcb.pid;
        pcb.state = ProcessState::Ready;
        pcb.time_slice_remaining = DEFAULT_TIME_SLICE;
        // Initialize activity tick so watchdog doesn't immediately kill new processes
        pcb.last_activity_tick = crate::arch::x86_64::interrupts::get_timer_ticks();

        crate::debug_info!(
            "Scheduler: Spawning process '{}' with PID {:?}",
            pcb.name,
            pid
        );

        self.processes.insert(pid, pcb);
        let id = EntityId::KernelThread(pid);
        self.entities.insert(id, SchedEntity::new(id));
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::register(id);
        }
        self.make_ready(id, None)
            .expect("scheduler entity capacity exceeded while spawning");

        pid
    }

    /// Get the currently running process ID
    pub fn current(&self) -> Option<ProcessId> {
        match self.current[crate::arch::x86_64::percpu::cpu_id()] {
            Some(EntityId::KernelThread(pid)) => Some(pid),
            Some(EntityId::UserProcess(_)) | None => None,
        }
    }

    #[cfg_attr(
        not(feature = "test"),
        expect(dead_code, reason = "scheduler diagnostics API")
    )]
    pub fn current_entity(&self) -> Option<EntityId> {
        self.current[crate::arch::x86_64::percpu::cpu_id()]
    }

    pub fn current_entity_on_cpu(&self, cpu: usize) -> Option<EntityId> {
        self.current.get(cpu).copied().flatten()
    }

    #[cfg(feature = "test")]
    pub fn entity_diagnostics_for_test(
        &self,
        id: EntityId,
    ) -> Option<(RunState, bool, Option<ProcessState>, Option<BlockReason>)> {
        let entity = self.entities.get(&id)?;
        let (process_state, block_reason) = match id {
            EntityId::KernelThread(pid) => self
                .processes
                .get(&pid)
                .map(|pcb| (Some(pcb.state), pcb.block_reason))
                .unwrap_or((None, None)),
            EntityId::UserProcess(_) => (None, None),
        };
        Some((
            entity.state,
            entity.context_published,
            process_state,
            block_reason,
        ))
    }

    pub fn register_user(&mut self, pid: u32) -> Result<(), ()> {
        let id = EntityId::UserProcess(pid);
        if let alloc::collections::btree_map::Entry::Vacant(entry) = self.entities.entry(id) {
            entry.insert(SchedEntity::new(id));
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::register(id);
            }
        }
        Ok(())
    }

    pub fn make_ready(
        &mut self,
        id: EntityId,
        contract: Option<LatencyContract>,
    ) -> Result<bool, ()> {
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        let Some(state) = self.entities.get(&id).map(|entity| entity.state) else {
            return Err(());
        };
        if state == RunState::Running && self.current.contains(&Some(id)) {
            return Ok(false);
        }
        if state == RunState::Ready {
            if let Some(contract) = contract {
                let deadline = now.saturating_add(contract.max_dispatch_ticks as u64);
                let entity = self.entities.get_mut(&id).expect("ready entity vanished");
                entity.latency_contract = Some(contract);
                entity.must_run_by_tick = Some(
                    entity
                        .must_run_by_tick
                        .map(|existing| existing.min(deadline))
                        .unwrap_or(deadline),
                );
            }
            if !self
                .entities
                .get(&id)
                .is_some_and(|entity| entity.context_published)
            {
                if self.shadow_observed {
                    crate::diagnostics::shadow::scheduler::make_ready(id, false);
                }
                return Ok(false);
            }
            let result = self.run_queue.enqueue(id);
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::make_ready(id, true);
            }
            if matches!(result, Ok(true)) {
                crate::arch::x86_64::smp::notify_work();
            }
            return result;
        }
        if let EntityId::KernelThread(pid) = id {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Ready;
                pcb.block_reason = None;
            }
        }
        let entity = self
            .entities
            .get_mut(&id)
            .expect("scheduler entity vanished");
        entity.state = RunState::Ready;
        entity.ready_since_tick = now;
        entity.latency_contract = contract;
        entity.must_run_by_tick = contract.map(|c| now.saturating_add(c.max_dispatch_ticks as u64));
        if !entity.context_published {
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::make_ready(id, false);
            }
            return Ok(false);
        }
        let result = self.run_queue.enqueue(id);
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::make_ready(id, true);
        }
        if matches!(result, Ok(true)) {
            crate::arch::x86_64::smp::notify_work();
        }
        result
    }

    pub fn block_entity(&mut self, id: EntityId) {
        self.run_queue.remove(id);
        let existed = if let Some(entity) = self.entities.get_mut(&id) {
            entity.state = RunState::Blocked;
            entity.must_run_by_tick = None;
            true
        } else {
            false
        };
        for current in &mut self.current {
            if *current == Some(id) {
                *current = None;
            }
        }
        if existed && self.shadow_observed {
            crate::diagnostics::shadow::scheduler::block(id);
        }
    }

    pub fn unregister_entity(&mut self, id: EntityId) {
        let existed = self.entities.contains_key(&id);
        let was_current = self.current.contains(&Some(id));
        self.run_queue.remove(id);
        for current in &mut self.current {
            if *current == Some(id) {
                *current = None;
            }
        }
        if let Some(entity) = self.entities.get_mut(&id) {
            entity.state = RunState::Dead;
        }
        self.entities.remove(&id);
        if existed && self.shadow_observed {
            crate::diagnostics::shadow::scheduler::unregister(id, was_current);
        }
    }

    pub fn entity_state(&self, id: EntityId) -> Option<RunState> {
        self.entities.get(&id).map(|entity| entity.state)
    }

    pub fn set_cpu_affinity(&mut self, id: EntityId, cpu: Option<usize>) -> Result<(), ()> {
        if cpu.is_some_and(|cpu| cpu >= crate::arch::x86_64::smp::online_cpu_count()) {
            return Err(());
        }
        let entity = self.entities.get_mut(&id).ok_or(())?;
        entity.cpu_affinity = cpu;
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::set_affinity(id, cpu);
        }
        Ok(())
    }

    pub fn cpu_affinity(&self, id: EntityId) -> Option<usize> {
        self.entities
            .get(&id)
            .and_then(|entity| entity.cpu_affinity)
    }

    pub fn mark_context_saving(&mut self, id: EntityId) {
        if let Some(entity) = self.entities.get_mut(&id) {
            entity.context_published = false;
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::begin_save(id);
            }
        }
    }

    pub fn publish_context(&mut self, id: EntityId) {
        let (existed, ready) = if let Some(entity) = self.entities.get_mut(&id) {
            entity.context_published = true;
            (true, entity.state == RunState::Ready)
        } else {
            (false, false)
        };
        let enqueued = if ready {
            let enqueued = self
                .run_queue
                .enqueue(id)
                .expect("scheduler entity capacity exceeded while publishing context");
            if enqueued {
                crate::arch::x86_64::smp::notify_work();
            }
            enqueued
        } else {
            false
        };
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::publish(id);
        }
        self.trace_context_publish(id, existed, enqueued);
    }

    pub fn publish_kernel_context_ptr(&mut self, context: *mut CpuContext) -> bool {
        let id = self.processes.iter_mut().find_map(|(pid, pcb)| {
            core::ptr::eq(&mut pcb.context, context).then_some(EntityId::KernelThread(*pid))
        });
        if let Some(id) = id {
            self.publish_context(id);
            true
        } else {
            false
        }
    }

    /// Number of user entities currently eligible to execute or running.
    pub fn runnable_user_count(&self) -> usize {
        self.entities
            .iter()
            .filter(|(id, entity)| {
                matches!(id, EntityId::UserProcess(_))
                    && matches!(entity.state, RunState::Ready | RunState::Running)
            })
            .count()
    }

    fn eligible_on_cpu(&self, id: EntityId, cpu: usize) -> bool {
        self.entities
            .get(&id)
            .is_some_and(|entity| entity.cpu_affinity.is_none_or(|bound| bound == cpu))
    }

    fn pick_ready_index(&self, now: u64, cpu: usize) -> Option<usize> {
        let mut due: Option<(usize, u64)> = None;
        for (index, id) in self.run_queue.iter().enumerate() {
            if !self.eligible_on_cpu(*id, cpu) {
                continue;
            }
            let Some(deadline) = self
                .entities
                .get(id)
                .and_then(|entity| entity.must_run_by_tick)
            else {
                continue;
            };
            if deadline <= now && due.map(|(_, earliest)| deadline < earliest).unwrap_or(true) {
                due = Some((index, deadline));
            }
        }
        due.map(|(index, _)| index).or_else(|| {
            self.run_queue
                .iter()
                .position(|id| self.eligible_on_cpu(*id, cpu))
        })
    }

    pub fn schedule_entity(&mut self) -> Option<EntityId> {
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        let index = self.pick_ready_index(now, cpu)?;
        let next = self.run_queue.remove_at(index)?;
        let missed = self
            .entities
            .get(&next)
            .and_then(|entity| entity.must_run_by_tick)
            .is_some_and(|deadline| now > deadline);
        if missed {
            self.latency_misses = self.latency_misses.saturating_add(1);
        }
        if let Some(entity) = self.entities.get_mut(&next) {
            entity.state = RunState::Running;
            entity.must_run_by_tick = None;
            entity.latency_contract = None;
        }
        if let EntityId::KernelThread(pid) = next {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Running;
                pcb.time_slice_remaining = DEFAULT_TIME_SLICE;
                pcb.last_activity_tick = now;
            }
        }
        self.current[crate::arch::x86_64::percpu::cpu_id()] = Some(next);
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::dispatch(next);
        }
        self.trace_dispatch(
            next,
            crate::diagnostics::trace::DispatchSource::FairQueue,
            missed,
        );
        crate::arch::x86_64::percpu::record_dispatch();
        Some(next)
    }

    /// Requeue an interrupted running entity and select once from the shared
    /// fair queue. Context saving is the caller's architecture responsibility.
    pub fn preempt_and_pick(&mut self, current: EntityId) -> Option<EntityId> {
        if let Some(entity) = self.entities.get_mut(&current) {
            entity.runtime_ticks = entity.runtime_ticks.saturating_add(1);
        }
        if let EntityId::KernelThread(pid) = current {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Ready;
            }
        }
        // The interrupt frame and entity stack remain live until the
        // architecture handoff switches stacks. Keep the saved context out of
        // the shared queue until that handoff publishes it.
        self.mark_context_saving(current);
        if let Some(entity) = self.entities.get_mut(&current) {
            entity.state = RunState::Ready;
            entity.must_run_by_tick = None;
        }
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if self.current[cpu] == Some(current) {
            self.current[cpu] = None;
        }

        let Some(next) = self.schedule_entity() else {
            // Nothing else can run, so keep executing the current interrupt
            // frame without publishing it to another CPU.
            if let Some(entity) = self.entities.get_mut(&current) {
                entity.state = RunState::Running;
                entity.context_published = true;
            }
            if let EntityId::KernelThread(pid) = current {
                if let Some(pcb) = self.processes.get_mut(&pid) {
                    pcb.state = ProcessState::Running;
                    pcb.time_slice_remaining = DEFAULT_TIME_SLICE;
                }
            }
            self.current[cpu] = Some(current);
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::resume_same_cpu(current);
            }
            self.trace_dispatch(
                current,
                crate::diagnostics::trace::DispatchSource::ResumeSameCpu,
                false,
            );
            return Some(current);
        };
        if let EntityId::KernelThread(pid) = next {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.state = ProcessState::Running;
                pcb.time_slice_remaining = DEFAULT_TIME_SLICE;
                pcb.last_activity_tick = crate::arch::x86_64::interrupts::get_timer_ticks();
            }
        }
        Some(next)
    }

    pub fn pop_next_user(&mut self) -> Option<u32> {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        let index = self.run_queue.iter().position(|id| {
            matches!(id, EntityId::UserProcess(_)) && self.eligible_on_cpu(*id, cpu)
        })?;
        let EntityId::UserProcess(pid) = self.run_queue.remove_at(index)? else {
            return None;
        };
        let id = EntityId::UserProcess(pid);
        if let Some(entity) = self.entities.get_mut(&id) {
            entity.state = RunState::Running;
            entity.must_run_by_tick = None;
        }
        self.current[crate::arch::x86_64::percpu::cpu_id()] = Some(id);
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::dispatch(id);
        }
        self.trace_dispatch(
            id,
            crate::diagnostics::trace::DispatchSource::UserQueue,
            false,
        );
        Some(pid)
    }

    pub fn peek_next_user(&self) -> Option<u32> {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        self.run_queue.iter().find_map(|id| match id {
            EntityId::UserProcess(pid) if self.eligible_on_cpu(*id, cpu) => Some(*pid),
            EntityId::KernelThread(_) => None,
            EntityId::UserProcess(_) => None,
        })
    }

    pub fn clear_current_user(&mut self, pid: u32) {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        let id = EntityId::UserProcess(pid);
        if self.current[cpu] == Some(id) {
            self.current[cpu] = None;
            if let Some(entity) = self.entities.get_mut(&id) {
                if entity.state == RunState::Running {
                    entity.state = RunState::Blocked;
                    if self.shadow_observed {
                        crate::diagnostics::shadow::scheduler::block(id);
                    }
                }
            }
        }
    }

    pub fn set_running(&mut self, id: EntityId) {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if let Some(previous) = self.current[cpu].filter(|previous| *previous != id) {
            if let Some(entity) = self.entities.get_mut(&previous) {
                if entity.state == RunState::Running {
                    entity.state = RunState::Blocked;
                    if self.shadow_observed {
                        crate::diagnostics::shadow::scheduler::block(previous);
                    }
                }
            }
        }
        let inserted =
            if let alloc::collections::btree_map::Entry::Vacant(entry) = self.entities.entry(id) {
                entry.insert(SchedEntity::new(id));
                true
            } else {
                false
            };
        if inserted && self.shadow_observed {
            crate::diagnostics::shadow::scheduler::register(id);
        }
        self.run_queue.remove(id);
        if let Some(entity) = self.entities.get_mut(&id) {
            entity.state = RunState::Running;
            entity.must_run_by_tick = None;
        }
        self.current[cpu] = Some(id);
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::force_running(id);
        }
        self.trace_dispatch(
            id,
            crate::diagnostics::trace::DispatchSource::ForceRunning,
            false,
        );
    }

    pub fn yield_entity(&mut self, id: EntityId) {
        if let Some(entity) = self.entities.get_mut(&id) {
            entity.state = RunState::Ready;
            entity.must_run_by_tick = None;
            entity.context_published = true;
        }
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if self.current[cpu] == Some(id) {
            self.current[cpu] = None;
        }
        self.run_queue
            .enqueue(id)
            .expect("scheduler entity capacity exceeded while yielding");
        if self.shadow_observed {
            crate::diagnostics::shadow::scheduler::yielded(id);
        }
    }

    #[cfg(feature = "test")]
    pub fn clear_user_entities_for_test(&mut self) {
        let users: Vec<EntityId> = self
            .entities
            .keys()
            .copied()
            .filter(|id| matches!(id, EntityId::UserProcess(_)))
            .collect();
        for id in users {
            self.unregister_entity(id);
        }
    }

    /// Get a reference to a process by PID
    pub fn get_process(&self, pid: ProcessId) -> Option<&ProcessControlBlock> {
        self.processes.get(&pid)
    }

    /// Get a mutable reference to a process by PID
    pub fn get_process_mut(&mut self, pid: ProcessId) -> Option<&mut ProcessControlBlock> {
        self.processes.get_mut(&pid)
    }

    /// Compatibility decision surface while call sites migrate to EntityId.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn next_runnable(&mut self) -> Runnable {
        match self.schedule_entity() {
            Some(EntityId::KernelThread(pid)) => Runnable::KernelThread(pid),
            Some(EntityId::UserProcess(pid)) => Runnable::RingThree(pid),
            None => Runnable::KernelThread(
                self.idle_pid
                    .expect("scheduler not initialized — no idle process"),
            ),
        }
    }

    /// Block the current process
    ///
    /// # Arguments
    /// * `reason` - Why the process is blocking
    pub fn block_current(&mut self, reason: BlockReason) {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if let Some(EntityId::KernelThread(current_pid)) = self.current[cpu] {
            self.mark_context_saving(EntityId::KernelThread(current_pid));
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                pcb.state = ProcessState::Blocked;
                pcb.block_reason = Some(reason);
                crate::debug_trace!(
                    "Scheduler: Blocked process {:?} for {:?}",
                    current_pid,
                    reason
                );
            }
            // Clear current; the unified selector will pick the next entity.
            self.block_entity(EntityId::KernelThread(current_pid));
        }
    }

    /// Wake a blocked process and add it back to the ready queue
    ///
    /// # Arguments
    /// * `pid` - The PID of the process to wake
    pub fn wake(&mut self, pid: ProcessId) {
        self.wake_with_contract(pid, None);
    }

    /// Wake an exact asynchronous-I/O waiter. Returns false while the
    /// completion is early and the caller has not published its Blocked
    /// state yet, so the IRQ-side wake record remains queued for retry.
    pub fn wake_block_io(&mut self, pid: ProcessId, token: u64) -> bool {
        let Some(pcb) = self.processes.get(&pid) else {
            return true;
        };
        if pcb.state == ProcessState::Terminated {
            return true;
        }
        match (pcb.state, pcb.block_reason) {
            (ProcessState::Blocked, Some(BlockReason::WaitingForBlockIo(awaited)))
                if awaited == token =>
            {
                self.wake(pid);
                true
            }
            (ProcessState::Blocked, Some(BlockReason::WaitingForBlockIo(_))) => true,
            _ => false,
        }
    }

    pub fn wake_with_contract(&mut self, pid: ProcessId, contract: Option<LatencyContract>) {
        if let Some(pcb) = self.processes.get_mut(&pid) {
            if pcb.state == ProcessState::Blocked {
                pcb.state = ProcessState::Ready;
                pcb.block_reason = None;
                self.make_ready(EntityId::KernelThread(pid), contract)
                    .expect("scheduler entity capacity exceeded while waking");
                crate::debug_trace!("Scheduler: Woke process {:?}", pid);
            }
        }
    }

    /// U8: wake any kernel thread blocked on `WaitingForRing3Exit(ring3_pid)`.
    /// Called from the ring-3 exit path so the launching kernel thread
    /// (which called `enter_user_mode_with_aspace` and then blocked on
    /// the scheduler) becomes runnable and can run its cleanup +
    /// return.
    ///
    /// The set of kernel threads is small; the linear scan is fine.
    pub fn wake_threads_waiting_for_ring3_exit(&mut self, ring3_pid: u32) {
        let waking: alloc::vec::Vec<ProcessId> = self
            .processes
            .iter()
            .filter_map(|(p, pcb)| {
                if pcb.state == ProcessState::Blocked
                    && matches!(
                        pcb.block_reason,
                        Some(BlockReason::WaitingForRing3Exit(awaited))
                            if awaited == ring3_pid
                    )
                {
                    Some(*p)
                } else {
                    None
                }
            })
            .collect();
        for pid in waking {
            self.wake(pid);
        }
    }

    /// Terminate the current process
    /// Remove the current process and return the stack that must be retired
    /// after the caller has switched to a different stack.
    pub fn terminate_current(&mut self) -> Option<u64> {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if let Some(EntityId::KernelThread(current_pid)) = self.current[cpu].take() {
            let mut retired_stack = None;
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                pcb.state = ProcessState::Terminated;
                crate::debug_info!("Scheduler: Terminated process {:?}", current_pid);
                if pcb.stack_base != 0 {
                    retired_stack = Some(pcb.stack_base);
                }
            }
            // Remove from processes map
            self.processes.remove(&current_pid);
            if self.shadow_observed {
                crate::diagnostics::shadow::scheduler::block(EntityId::KernelThread(current_pid));
            }
            self.unregister_entity(EntityId::KernelThread(current_pid));
            return retired_stack;
        }
        None
    }

    /// Terminate a non-running process immediately, or request that the CPU
    /// currently executing it terminate at its next safe timer boundary.
    /// Returns the logical CPU that owns a deferred termination.
    pub fn terminate(&mut self, pid: ProcessId) -> Option<usize> {
        let id = EntityId::KernelThread(pid);
        if let Some(cpu) = self.current.iter().position(|current| *current == Some(id)) {
            if let Some(pcb) = self.processes.get_mut(&pid) {
                pcb.termination_requested = true;
                return Some(cpu);
            }
        }

        // Remove from ready queue if present
        self.run_queue.remove(id);

        if let Some(pcb) = self.processes.get_mut(&pid) {
            pcb.state = ProcessState::Terminated;
            crate::debug_info!("Scheduler: Terminated process {:?}", pid);

            // Free the stack
            if pcb.stack_base != 0 {
                free_stack(pcb.stack_base);
            }
        }

        // Remove from processes map
        self.processes.remove(&pid);
        self.unregister_entity(id);
        None
    }

    pub fn current_termination_requested(&self) -> bool {
        self.current()
            .and_then(|pid| self.processes.get(&pid))
            .is_some_and(|pcb| pcb.termination_requested)
    }

    /// Handle a timer tick - decrement time slice and check for preemption
    ///
    /// # Returns
    /// `true` if the current process's time slice has expired and preemption is needed
    pub fn timer_tick(&mut self) -> bool {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if let Some(EntityId::KernelThread(current_pid)) = self.current[cpu] {
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                // Increment total runtime
                pcb.total_runtime += 1;

                // Don't preempt idle process
                if self.idle_pid == Some(current_pid) {
                    return !self.run_queue.is_empty();
                }

                // Decrement time slice
                if pcb.time_slice_remaining > 0 {
                    pcb.time_slice_remaining -= 1;
                }

                // Check if time slice expired
                if pcb.time_slice_remaining == 0 {
                    crate::debug_trace!("Scheduler: Time slice expired for {:?}", current_pid);
                    return true;
                }
            }
        }
        false
    }

    /// Yield the current process voluntarily
    ///
    /// Moves the current process to the back of the ready queue.
    pub fn yield_current(&mut self) {
        let cpu = crate::arch::x86_64::percpu::cpu_id();
        if let Some(EntityId::KernelThread(current_pid)) = self.current[cpu] {
            self.mark_context_saving(EntityId::KernelThread(current_pid));
            if let Some(pcb) = self.processes.get_mut(&current_pid) {
                if pcb.state == ProcessState::Running {
                    pcb.state = ProcessState::Ready;
                    if self.idle_pid != Some(current_pid) {
                        let id = EntityId::KernelThread(current_pid);
                        if let Some(entity) = self.entities.get_mut(&id) {
                            entity.state = RunState::Ready;
                        }
                        // `switch_context` publishes the completed save and
                        // enqueues this entity from its assembly-side hook.
                    }
                }
            }
            self.current[cpu] = None;
        }
    }

    /// Get the number of ready processes
    #[expect(dead_code, reason = "legacy diagnostics API")]
    pub fn ready_count(&self) -> usize {
        self.run_queue
            .iter()
            .filter(|id| matches!(id, EntityId::KernelThread(pid) if Some(*pid) != self.idle_pid))
            .count()
    }

    pub fn ready_entity_count(&self) -> usize {
        self.run_queue.len()
    }

    #[cfg(feature = "test")]
    pub fn latency_misses_for_test(&self) -> u64 {
        self.latency_misses
    }

    /// Get the total number of processes (including blocked/idle)

    /// Check if the scheduler has been initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the context of the current process

    /// Get mutable context of the current process

    /// Get the context of a specific process
    pub fn get_context(&self, pid: ProcessId) -> Option<&CpuContext> {
        self.processes.get(&pid).map(|pcb| &pcb.context)
    }

    /// Get mutable context of a specific process
    pub fn get_context_mut(&mut self, pid: ProcessId) -> Option<&mut CpuContext> {
        self.processes.get_mut(&pid).map(|pcb| &mut pcb.context)
    }

    /// Find a process associated with a specific terminal
    pub fn find_by_terminal(&self, terminal_id: crate::window::WindowId) -> Option<ProcessId> {
        for (pid, pcb) in &self.processes {
            if pcb.terminal_id == Some(terminal_id) {
                return Some(*pid);
            }
        }
        None
    }

    /// Get a snapshot of all processes for display purposes
    ///
    /// Returns lightweight ProcessInfo structs suitable for a task manager UI.
    pub fn get_process_list(&self) -> Vec<ProcessInfo> {
        self.processes
            .values()
            .map(|pcb| ProcessInfo {
                pid: pcb.pid,
                name: pcb.name.clone(),
                state: pcb.state,
                total_runtime: pcb.total_runtime,
                stack_size: pcb.stack_size,
            })
            .collect()
    }

    fn wake_with_event(&mut self, pid: ProcessId, event: WakeEvents) {
        if let Some(pcb) = self.processes.get_mut(&pid) {
            if pcb.state == ProcessState::Blocked {
                // Check if this event can wake the process
                let can_wake = pcb.wake_events.contains(event)
                    || matches!(pcb.block_reason, Some(BlockReason::SleepingUntilTick(_)));

                if can_wake {
                    pcb.state = ProcessState::Ready;
                    pcb.block_reason = None;
                    pcb.wake_at_tick = None;
                    pcb.pending_signals.set(event.bits());
                    self.make_ready(EntityId::KernelThread(pid), None)
                        .expect("scheduler entity capacity exceeded while waking sleeper");

                    crate::debug_trace!("Scheduler: Woke process {:?} with event {:?}", pid, event);
                }
            }
        }
    }

    /// Signal a specific process to wake up
    ///
    /// If the process is waiting for the given signal type, it will be woken.
    ///
    /// # Arguments
    /// * `pid` - The process to signal
    /// * `signal` - The event type to signal
    pub fn signal_process(&mut self, pid: ProcessId, signal: WakeEvents) {
        // Remove from signal_waiters if present
        self.signal_waiters.retain(|&p| p != pid);

        self.wake_with_event(pid, signal);
    }
}
