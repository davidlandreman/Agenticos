//! Fixed-size per-CPU state and the GS-relative ABI shared with assembly.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::process::context::CpuContext;

use super::acpi::MAX_CPUS;

/// `None` encoding for PID slots. Real ring-3 PIDs start above zero.
const NO_PID: u32 = 0;

#[repr(C, align(64))]
pub struct CpuLocal {
    // Offsets 0/8/16 are a stable assembly ABI.
    kernel_rsp_top: u64,
    user_rsp_scratch: u64,
    logical_id: u32,
    lapic_id: u32,
    preemption_disable_depth: AtomicUsize,
    kernel_context: UnsafeCell<CpuContext>,
    handoff_context: UnsafeCell<CpuContext>,
    current_user_pid: AtomicU32,
    in_spawned_process: AtomicBool,
    idle_interruptible: AtomicBool,
    dispatches: AtomicU64,
    reschedule_ipis: AtomicU64,
    user_ticks: AtomicU64,
    system_ticks: AtomicU64,
    idle_ticks: AtomicU64,
    in_mapper: AtomicBool,
    pending_context_publish: AtomicU64,
}

unsafe impl Sync for CpuLocal {}

impl CpuLocal {
    const fn new() -> Self {
        Self {
            kernel_rsp_top: 0,
            user_rsp_scratch: 0,
            logical_id: 0,
            lapic_id: 0,
            preemption_disable_depth: AtomicUsize::new(0),
            kernel_context: UnsafeCell::new(CpuContext::new()),
            handoff_context: UnsafeCell::new(CpuContext::new()),
            current_user_pid: AtomicU32::new(NO_PID),
            in_spawned_process: AtomicBool::new(false),
            idle_interruptible: AtomicBool::new(false),
            dispatches: AtomicU64::new(0),
            reschedule_ipis: AtomicU64::new(0),
            user_ticks: AtomicU64::new(0),
            system_ticks: AtomicU64::new(0),
            idle_ticks: AtomicU64::new(0),
            in_mapper: AtomicBool::new(false),
            pending_context_publish: AtomicU64::new(0),
        }
    }
}

// Each slot is initialized before its corresponding CPU checks in. Thereafter
// the slot is exclusively mutated by that CPU, with cross-CPU telemetry fields
// using atomics.
static mut CPU_LOCALS: [CpuLocal; MAX_CPUS] = [const { CpuLocal::new() }; MAX_CPUS];
static INITIALIZED_CPUS: AtomicUsize = AtomicUsize::new(0);

const _: () = {
    use core::mem::offset_of;
    assert!(offset_of!(CpuLocal, kernel_rsp_top) == 0);
    assert!(offset_of!(CpuLocal, user_rsp_scratch) == 8);
    assert!(offset_of!(CpuLocal, logical_id) == 16);
    assert!(offset_of!(CpuLocal, kernel_context) + offset_of!(CpuContext, rsp) == 80);
};

/// GS-relative offset of the saved idle/main-loop RSP. Context-switch assembly
/// uses this stack as a per-CPU handoff stack after abandoning an entity stack.
pub const KERNEL_CONTEXT_RSP_OFFSET: usize =
    core::mem::offset_of!(CpuLocal, kernel_context) + core::mem::offset_of!(CpuContext, rsp);

#[cfg(feature = "test")]
pub fn abi_offsets_for_test() -> (usize, usize, usize) {
    (
        core::mem::offset_of!(CpuLocal, kernel_rsp_top),
        core::mem::offset_of!(CpuLocal, user_rsp_scratch),
        core::mem::offset_of!(CpuLocal, logical_id),
    )
}

/// Initialize one slot and install it as both kernel GS bases on this CPU.
///
/// # Safety
/// Each logical CPU slot must be initialized exactly once by that CPU's
/// bring-up owner before interrupts or user transitions are enabled there.
pub unsafe fn init_cpu(logical_id: usize, lapic_id: u8, kernel_rsp_top: u64) -> *mut CpuLocal {
    assert!(logical_id < MAX_CPUS);
    let local = core::ptr::addr_of_mut!(CPU_LOCALS[logical_id]);
    (*local).kernel_rsp_top = kernel_rsp_top;
    (*local).user_rsp_scratch = 0;
    (*local).logical_id = logical_id as u32;
    (*local).lapic_id = u32::from(lapic_id);
    (*local)
        .preemption_disable_depth
        .store(0, Ordering::Release);
    (*local).current_user_pid.store(NO_PID, Ordering::Release);
    (*local).in_spawned_process.store(false, Ordering::Release);
    (*local).idle_interruptible.store(false, Ordering::Release);
    (*local).user_ticks.store(0, Ordering::Release);
    (*local).system_ticks.store(0, Ordering::Release);
    (*local).idle_ticks.store(0, Ordering::Release);
    super::msr::init_gs_base(local as u64);
    INITIALIZED_CPUS.fetch_add(1, Ordering::AcqRel);
    local
}

#[inline]
pub fn cpu_id() -> usize {
    let id: u32;
    unsafe {
        core::arch::asm!(
            "mov {id:e}, gs:[16]",
            id = out(reg) id,
            options(nostack, preserves_flags, readonly)
        );
    }
    id as usize
}

#[inline]
pub fn lapic_id() -> u8 {
    local().lapic_id as u8
}

pub fn initialized_cpu_count() -> usize {
    INITIALIZED_CPUS.load(Ordering::Acquire)
}

#[inline]
fn local() -> &'static CpuLocal {
    let id = cpu_id();
    assert!(id < MAX_CPUS);
    unsafe { &*core::ptr::addr_of!(CPU_LOCALS[id]) }
}

pub unsafe fn set_kernel_rsp_top(top: u64) {
    let id = cpu_id();
    (*core::ptr::addr_of_mut!(CPU_LOCALS[id])).kernel_rsp_top = top;
}

pub fn preemption_depth() -> &'static AtomicUsize {
    &local().preemption_disable_depth
}

pub fn kernel_context_ptr() -> *mut CpuContext {
    local().kernel_context.get()
}

/// Copy a destination register image into storage owned exclusively by this
/// CPU. The source may live on the outgoing entity's stack, which becomes
/// reusable immediately after the handoff is published.
///
/// # Safety
/// The caller must keep local interrupts/preemption disabled until the staged
/// image has been fully restored or otherwise abandoned.
pub unsafe fn stage_handoff_context(context: *const CpuContext) -> *const CpuContext {
    let staged = local().handoff_context.get();
    staged.write(context.read());
    staged
}

pub fn current_user_pid() -> Option<u32> {
    match local().current_user_pid.load(Ordering::Acquire) {
        NO_PID => None,
        pid => Some(pid),
    }
}

pub fn set_current_user_pid(pid: Option<u32>) {
    local()
        .current_user_pid
        .store(pid.unwrap_or(NO_PID), Ordering::Release);
}

pub fn in_spawned_process() -> bool {
    local().in_spawned_process.load(Ordering::Acquire)
}

pub fn set_in_spawned_process(value: bool) {
    local().in_spawned_process.store(value, Ordering::Release);
}

pub fn set_idle_interruptible(value: bool) {
    local().idle_interruptible.store(value, Ordering::Release);
}

pub fn idle_interruptible(cpu: usize) -> bool {
    cpu < MAX_CPUS
        && unsafe { &*core::ptr::addr_of!(CPU_LOCALS[cpu]) }
            .idle_interruptible
            .load(Ordering::Acquire)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuTimeKind {
    User,
    System,
    Idle,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimeSnapshot {
    pub user: u64,
    pub system: u64,
    pub idle: u64,
}

/// Charge one local scheduling-timer sample without taking a shared lock.
pub fn record_cpu_time(kind: CpuTimeKind) {
    let counter = match kind {
        CpuTimeKind::User => &local().user_ticks,
        CpuTimeKind::System => &local().system_ticks,
        CpuTimeKind::Idle => &local().idle_ticks,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Snapshot the monotonic scheduling-timer counters for one logical CPU.
pub fn cpu_time_snapshot(cpu: usize) -> Option<CpuTimeSnapshot> {
    if cpu >= initialized_cpu_count() || cpu >= MAX_CPUS {
        return None;
    }
    let local = unsafe { &*core::ptr::addr_of!(CPU_LOCALS[cpu]) };
    Some(CpuTimeSnapshot {
        user: local.user_ticks.load(Ordering::Relaxed),
        system: local.system_ticks.load(Ordering::Relaxed),
        idle: local.idle_ticks.load(Ordering::Relaxed),
    })
}

pub fn record_dispatch() {
    local().dispatches.fetch_add(1, Ordering::Relaxed);
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "SMP telemetry API"))]
pub fn dispatches(cpu: usize) -> u64 {
    if cpu >= MAX_CPUS {
        return 0;
    }
    unsafe { &*core::ptr::addr_of!(CPU_LOCALS[cpu]) }
        .dispatches
        .load(Ordering::Relaxed)
}

pub fn record_reschedule_ipi(cpu: usize) {
    if cpu < MAX_CPUS {
        unsafe { &*core::ptr::addr_of!(CPU_LOCALS[cpu]) }
            .reschedule_ipis
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub fn mapper_enter() {
    let was_in_mapper = local().in_mapper.swap(true, Ordering::AcqRel);
    debug_assert!(!was_in_mapper, "recursive memory-mapper acquisition");
}

pub fn mapper_exit() {
    let was_in_mapper = local().in_mapper.swap(false, Ordering::AcqRel);
    debug_assert!(was_in_mapper, "unbalanced memory-mapper release");
}

pub fn set_pending_user_context_publish(pid: u32) {
    local()
        .pending_context_publish
        .store((1u64 << 63) | u64::from(pid), Ordering::Release);
}

pub fn set_pending_kernel_context_publish(pid: crate::process::ProcessId) {
    local()
        .pending_context_publish
        .store(u64::from(pid), Ordering::Release);
}

pub fn has_pending_context_publish() -> bool {
    local().pending_context_publish.load(Ordering::Acquire) != 0
}

pub fn take_pending_context_publish() -> Option<crate::process::entity::EntityId> {
    let encoded = local().pending_context_publish.swap(0, Ordering::AcqRel);
    if encoded == 0 {
        None
    } else if encoded >> 63 != 0 {
        Some(crate::process::entity::EntityId::UserProcess(
            encoded as u32,
        ))
    } else {
        Some(crate::process::entity::EntityId::KernelThread(
            encoded as u32,
        ))
    }
}
