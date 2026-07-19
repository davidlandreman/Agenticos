//! Per-CPU CR3, kernel-stack, current-PID, and context-publication handoff shadow.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::process::entity::EntityId;

use super::{latch, ViolationRecord};

pub const CPU_001: u32 = 0x0200_0001;
pub const CPU_002: u32 = 0x0200_0002;
pub const CPU_003: u32 = 0x0200_0003;
pub const CPU_004: u32 = 0x0200_0004;
pub const CPU_005: u32 = 0x0200_0005;

const STEP_CR3: u8 = 1 << 0;
const STEP_RSP0: u8 = 1 << 1;
const STEP_GS: u8 = 1 << 2;
const STEP_EXTENDED: u8 = 1 << 3;
const STEP_PID: u8 = 1 << 4;
const USER_COMPLETE: u8 = STEP_CR3 | STEP_RSP0 | STEP_GS | STEP_EXTENDED | STEP_PID;
const STEP_PID_CLEAR: u8 = 1 << 5;
const STEP_KERNEL_CR3: u8 = 1 << 6;
const KERNEL_COMPLETE: u8 = STEP_PID_CLEAR | STEP_KERNEL_CR3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Boot = 0,
    KernelStable = 1,
    LoadingUser = 2,
    UserStable = 3,
    LoadingKernel = 4,
    AddressSpaceSetup = 5,
    Crashed = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Operation {
    InitializeKernel = 1,
    BeginUser = 2,
    InstallUserCr3 = 3,
    InstallRsp0 = 4,
    InstallGsStack = 5,
    RestoreExtended = 6,
    SetCurrentPid = 7,
    CommitUser = 8,
    BeginKernel = 9,
    ClearCurrentPid = 10,
    InstallKernelCr3 = 11,
    CommitKernel = 12,
    SetPendingPublish = 13,
    TakePendingPublish = 14,
    BeginAddressSpaceSetup = 15,
    InstallSetupCr3 = 16,
    RestoreSetupKernelCr3 = 17,
    CommitAddressSpaceSetup = 18,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionState {
    pub phase: Phase,
    pub completed: u8,
}

pub fn transition_state(
    mut state: TransitionState,
    operation: Operation,
) -> Result<TransitionState, u32> {
    match operation {
        Operation::InitializeKernel if state.phase == Phase::Boot => {
            state.phase = Phase::KernelStable;
            state.completed = KERNEL_COMPLETE;
        }
        Operation::BeginUser
            if matches!(
                state.phase,
                Phase::KernelStable | Phase::UserStable | Phase::LoadingKernel
            ) =>
        {
            state.phase = Phase::LoadingUser;
            state.completed = 0;
        }
        Operation::InstallUserCr3 if state.phase == Phase::LoadingUser && state.completed == 0 => {
            state.completed |= STEP_CR3;
        }
        Operation::InstallRsp0
            if state.phase == Phase::LoadingUser && state.completed == STEP_CR3 =>
        {
            state.completed |= STEP_RSP0;
        }
        Operation::InstallGsStack
            if state.phase == Phase::LoadingUser && state.completed == STEP_CR3 | STEP_RSP0 =>
        {
            state.completed |= STEP_GS;
        }
        Operation::RestoreExtended
            if state.phase == Phase::LoadingUser
                && state.completed == STEP_CR3 | STEP_RSP0 | STEP_GS =>
        {
            state.completed |= STEP_EXTENDED;
        }
        Operation::SetCurrentPid
            if state.phase == Phase::LoadingUser
                && state.completed == STEP_CR3 | STEP_RSP0 | STEP_GS | STEP_EXTENDED =>
        {
            state.completed |= STEP_PID;
        }
        Operation::CommitUser
            if state.phase == Phase::LoadingUser && state.completed == USER_COMPLETE =>
        {
            state.phase = Phase::UserStable;
        }
        Operation::BeginKernel
            if matches!(state.phase, Phase::KernelStable | Phase::UserStable) =>
        {
            state.phase = Phase::LoadingKernel;
            state.completed = 0;
        }
        Operation::ClearCurrentPid
            if state.phase == Phase::LoadingKernel && state.completed == 0 =>
        {
            state.completed |= STEP_PID_CLEAR;
        }
        Operation::InstallKernelCr3
            if state.phase == Phase::LoadingKernel && state.completed == STEP_PID_CLEAR =>
        {
            state.completed |= STEP_KERNEL_CR3;
        }
        Operation::CommitKernel
            if state.phase == Phase::LoadingKernel && state.completed == KERNEL_COMPLETE =>
        {
            state.phase = Phase::KernelStable;
        }
        Operation::BeginAddressSpaceSetup if state.phase == Phase::KernelStable => {
            state.phase = Phase::AddressSpaceSetup;
            state.completed = 0;
        }
        Operation::InstallSetupCr3
            if state.phase == Phase::AddressSpaceSetup && state.completed == 0 =>
        {
            state.completed = STEP_CR3;
        }
        Operation::RestoreSetupKernelCr3
            if state.phase == Phase::AddressSpaceSetup && state.completed == STEP_CR3 =>
        {
            state.completed |= STEP_KERNEL_CR3;
        }
        Operation::CommitAddressSpaceSetup
            if state.phase == Phase::AddressSpaceSetup
                && state.completed == STEP_CR3 | STEP_KERNEL_CR3 =>
        {
            state.phase = Phase::KernelStable;
        }
        Operation::SetPendingPublish | Operation::TakePendingPublish => return Ok(state),
        _ => return Err(CPU_005),
    }
    Ok(state)
}

struct CpuShadow {
    epoch: AtomicU64,
    phase: AtomicU8,
    completed: AtomicU8,
    last_operation: AtomicU8,
    flags: AtomicU8,
    target_entity: AtomicU64,
    expected_l4: AtomicU64,
    address_space_generation: AtomicU64,
    expected_stack_top: AtomicU64,
    stack_generation: AtomicU64,
    observed_cr3: AtomicU64,
    observed_rsp0: AtomicU64,
    observed_gs_top: AtomicU64,
    observed_pid: AtomicU32,
    pending_entity: AtomicU64,
}

impl CpuShadow {
    const fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            phase: AtomicU8::new(Phase::Boot as u8),
            completed: AtomicU8::new(0),
            last_operation: AtomicU8::new(0),
            flags: AtomicU8::new(0),
            target_entity: AtomicU64::new(0),
            expected_l4: AtomicU64::new(0),
            address_space_generation: AtomicU64::new(0),
            expected_stack_top: AtomicU64::new(0),
            stack_generation: AtomicU64::new(0),
            observed_cr3: AtomicU64::new(0),
            observed_rsp0: AtomicU64::new(0),
            observed_gs_top: AtomicU64::new(0),
            observed_pid: AtomicU32::new(0),
            pending_entity: AtomicU64::new(0),
        }
    }
}

static CPUS: [CpuShadow; crate::arch::x86_64::acpi::MAX_CPUS] =
    [const { CpuShadow::new() }; crate::arch::x86_64::acpi::MAX_CPUS];

fn enabled() -> bool {
    crate::diagnostics::personality() != crate::diagnostics::Personality::Minimal
}

fn phase(value: u8) -> Phase {
    match value {
        1 => Phase::KernelStable,
        2 => Phase::LoadingUser,
        3 => Phase::UserStable,
        4 => Phase::LoadingKernel,
        5 => Phase::AddressSpaceSetup,
        6 => Phase::Crashed,
        _ => Phase::Boot,
    }
}

fn local() -> &'static CpuShadow {
    &CPUS[crate::arch::x86_64::percpu::cpu_id()]
}

fn begin(cpu: &CpuShadow, operation: Operation) -> u64 {
    let previous = cpu.epoch.fetch_add(1, Ordering::AcqRel);
    if previous & 1 != 0 {
        report(CPU_005, previous, operation as u64, 0, previous);
    }
    cpu.last_operation.store(operation as u8, Ordering::Relaxed);
    previous + 1
}

fn finish(cpu: &CpuShadow, odd: u64) {
    cpu.epoch.store(odd + 1, Ordering::Release);
}

fn report(id: u32, epoch: u64, subject: u64, expected: u64, observed: u64) {
    let first = latch(ViolationRecord {
        invariant_id: id,
        severity: 2,
        cpu: 0,
        mode: 0,
        domain: 2,
        epoch,
        subject,
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

fn apply_operation(cpu: &CpuShadow, operation: Operation, odd: u64) -> bool {
    let state = TransitionState {
        phase: phase(cpu.phase.load(Ordering::Relaxed)),
        completed: cpu.completed.load(Ordering::Relaxed),
    };
    match transition_state(state, operation) {
        Ok(next) => {
            cpu.phase.store(next.phase as u8, Ordering::Relaxed);
            cpu.completed.store(next.completed, Ordering::Relaxed);
            true
        }
        Err(id) => {
            report(
                id,
                odd,
                operation as u64,
                state.phase as u64,
                state.completed as u64,
            );
            false
        }
    }
}

fn trace(operation: Operation, subject: u64, arg0: u64, epoch: u64) {
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::CpuHandoff,
        subject,
        operation as u64,
        arg0,
        epoch + 1,
    );
}

pub fn initialize_kernel(kernel_l4: u64, stack_top: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::InitializeKernel);
    if apply_operation(cpu, Operation::InitializeKernel, odd) {
        cpu.expected_l4.store(kernel_l4, Ordering::Relaxed);
        cpu.expected_stack_top.store(stack_top, Ordering::Relaxed);
        cpu.observed_cr3.store(kernel_l4, Ordering::Relaxed);
        cpu.observed_rsp0.store(stack_top, Ordering::Relaxed);
        cpu.observed_gs_top.store(stack_top, Ordering::Relaxed);
        cpu.observed_pid.store(0, Ordering::Relaxed);
    }
    trace(Operation::InitializeKernel, kernel_l4, stack_top, odd);
    finish(cpu, odd);
}

pub fn begin_user(
    pid: u32,
    expected_l4: u64,
    address_space_generation: u64,
    expected_stack_top: u64,
    stack_generation: u64,
) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::BeginUser);
    if apply_operation(cpu, Operation::BeginUser, odd) {
        cpu.target_entity.store(
            super::scheduler::entity_key(EntityId::UserProcess(pid)),
            Ordering::Relaxed,
        );
        cpu.expected_l4.store(expected_l4, Ordering::Relaxed);
        cpu.address_space_generation
            .store(address_space_generation, Ordering::Relaxed);
        cpu.expected_stack_top
            .store(expected_stack_top, Ordering::Relaxed);
        cpu.stack_generation
            .store(stack_generation, Ordering::Relaxed);
    }
    trace(Operation::BeginUser, u64::from(pid), expected_l4, odd);
    finish(cpu, odd);
}

pub fn install_user_cr3(l4: u64, generation: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::InstallUserCr3);
    let expected_l4 = cpu.expected_l4.load(Ordering::Relaxed);
    let expected_generation = cpu.address_space_generation.load(Ordering::Relaxed);
    if l4 != expected_l4 || generation != expected_generation {
        report(CPU_002, odd, l4, expected_l4, l4);
    } else if apply_operation(cpu, Operation::InstallUserCr3, odd) {
        cpu.observed_cr3.store(l4, Ordering::Relaxed);
    }
    trace(Operation::InstallUserCr3, l4, generation, odd);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::Cr3Write,
        l4,
        generation,
        1,
        odd + 1,
    );
    finish(cpu, odd);
}

pub fn install_rsp0(top: u64) {
    install_stack_step(Operation::InstallRsp0, top);
}

pub fn install_gs_stack(top: u64) {
    install_stack_step(Operation::InstallGsStack, top);
}

fn install_stack_step(operation: Operation, top: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, operation);
    let expected = cpu.expected_stack_top.load(Ordering::Relaxed);
    if top != expected {
        report(CPU_005, odd, top, expected, top);
    } else if apply_operation(cpu, operation, odd) {
        match operation {
            Operation::InstallRsp0 => cpu.observed_rsp0.store(top, Ordering::Relaxed),
            Operation::InstallGsStack => cpu.observed_gs_top.store(top, Ordering::Relaxed),
            _ => {}
        }
    }
    trace(
        operation,
        top,
        cpu.stack_generation.load(Ordering::Relaxed),
        odd,
    );
    finish(cpu, odd);
}

pub fn restore_extended(pid: u32) {
    apply_pid_operation(Operation::RestoreExtended, pid);
}

pub fn set_current_pid(pid: u32) {
    apply_pid_operation(Operation::SetCurrentPid, pid);
}

fn apply_pid_operation(operation: Operation, pid: u32) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, operation);
    let expected = cpu.target_entity.load(Ordering::Relaxed) as u32;
    if pid != expected {
        report(
            CPU_001,
            odd,
            u64::from(pid),
            u64::from(expected),
            u64::from(pid),
        );
    } else if apply_operation(cpu, operation, odd) && operation == Operation::SetCurrentPid {
        cpu.observed_pid.store(pid, Ordering::Relaxed);
    }
    trace(operation, u64::from(pid), 0, odd);
    if operation == Operation::SetCurrentPid {
        crate::diagnostics::trace::record(
            crate::diagnostics::trace::EventKind::CurrentPid,
            u64::from(pid),
            1,
            0,
            odd + 1,
        );
    }
    finish(cpu, odd);
}

pub fn commit_user(pid: u32) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::CommitUser);
    let target = super::scheduler::entity_key(EntityId::UserProcess(pid));
    let current = crate::arch::x86_64::percpu::current_user_pid().unwrap_or(0);
    let scheduler_current =
        super::scheduler::running_entity_on_cpu(crate::arch::x86_64::percpu::cpu_id());
    if cpu.target_entity.load(Ordering::Relaxed) != target
        || current != pid
        || scheduler_current.is_some_and(|entity| entity != target)
    {
        report(
            CPU_001,
            odd,
            target,
            target,
            scheduler_current.unwrap_or(u64::from(current)),
        );
    }
    let (live_cr3, _) = x86_64::registers::control::Cr3::read();
    let live_cr3 = live_cr3.start_address().as_u64();
    let expected_l4 = cpu.expected_l4.load(Ordering::Relaxed);
    if live_cr3 != expected_l4 {
        report(CPU_002, odd, target, expected_l4, live_cr3);
    }
    let expected_stack = cpu.expected_stack_top.load(Ordering::Relaxed);
    let live_rsp0 = crate::arch::x86_64::gdt::current_kernel_rsp0().as_u64();
    let live_gs = crate::arch::x86_64::percpu::kernel_rsp_top();
    if live_rsp0 != expected_stack || live_gs != expected_stack {
        report(CPU_005, odd, target, expected_stack, live_rsp0);
    }
    if cpu.pending_entity.load(Ordering::Acquire) != 0 {
        report(
            CPU_004,
            odd,
            target,
            0,
            cpu.pending_entity.load(Ordering::Relaxed),
        );
    }
    cpu.observed_cr3.store(live_cr3, Ordering::Relaxed);
    cpu.observed_rsp0.store(live_rsp0, Ordering::Relaxed);
    cpu.observed_gs_top.store(live_gs, Ordering::Relaxed);
    cpu.observed_pid.store(current, Ordering::Relaxed);
    apply_operation(cpu, Operation::CommitUser, odd);
    trace(Operation::CommitUser, target, live_cr3, odd);
    finish(cpu, odd);
}

pub fn begin_kernel(target: Option<EntityId>) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::BeginKernel);
    let current_phase = phase(cpu.phase.load(Ordering::Relaxed));
    if current_phase == Phase::LoadingKernel {
        cpu.completed.store(0, Ordering::Relaxed);
    } else {
        apply_operation(cpu, Operation::BeginKernel, odd);
    }
    cpu.target_entity.store(
        target.map(super::scheduler::entity_key).unwrap_or(0),
        Ordering::Relaxed,
    );
    trace(
        Operation::BeginKernel,
        cpu.target_entity.load(Ordering::Relaxed),
        0,
        odd,
    );
    finish(cpu, odd);
}

pub fn clear_current_pid() {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::ClearCurrentPid);
    if crate::arch::x86_64::percpu::current_user_pid().is_some() {
        report(CPU_003, odd, 0, 0, 1);
    } else if apply_operation(cpu, Operation::ClearCurrentPid, odd) {
        cpu.observed_pid.store(0, Ordering::Relaxed);
    }
    trace(Operation::ClearCurrentPid, 0, 0, odd);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::CurrentPid,
        0,
        0,
        0,
        odd + 1,
    );
    finish(cpu, odd);
}

pub fn install_kernel_cr3(l4: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::InstallKernelCr3);
    let expected = crate::mm::paging::kernel_l4_frame()
        .map(|frame| frame.start_address().as_u64())
        .unwrap_or(0);
    if l4 != expected {
        report(CPU_003, odd, l4, expected, l4);
    } else if apply_operation(cpu, Operation::InstallKernelCr3, odd) {
        cpu.expected_l4.store(l4, Ordering::Relaxed);
        cpu.observed_cr3.store(l4, Ordering::Relaxed);
        cpu.address_space_generation.store(0, Ordering::Relaxed);
    }
    trace(Operation::InstallKernelCr3, l4, 0, odd);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::Cr3Write,
        l4,
        0,
        0,
        odd + 1,
    );
    finish(cpu, odd);
}

pub fn begin_address_space_setup(l4: u64, generation: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::BeginAddressSpaceSetup);
    if apply_operation(cpu, Operation::BeginAddressSpaceSetup, odd) {
        cpu.target_entity.store(0, Ordering::Relaxed);
        cpu.expected_l4.store(l4, Ordering::Relaxed);
        cpu.address_space_generation
            .store(generation, Ordering::Relaxed);
    }
    trace(Operation::BeginAddressSpaceSetup, l4, generation, odd);
    finish(cpu, odd);
}

pub fn install_setup_cr3(l4: u64, generation: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::InstallSetupCr3);
    let expected_l4 = cpu.expected_l4.load(Ordering::Relaxed);
    let expected_generation = cpu.address_space_generation.load(Ordering::Relaxed);
    if l4 != expected_l4 || generation != expected_generation {
        report(CPU_002, odd, l4, expected_l4, l4);
    } else if apply_operation(cpu, Operation::InstallSetupCr3, odd) {
        cpu.observed_cr3.store(l4, Ordering::Relaxed);
    }
    trace(Operation::InstallSetupCr3, l4, generation, odd);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::Cr3Write,
        l4,
        generation,
        2,
        odd + 1,
    );
    finish(cpu, odd);
}

pub fn restore_setup_kernel_cr3(l4: u64) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::RestoreSetupKernelCr3);
    let expected = crate::mm::paging::kernel_l4_frame()
        .map(|frame| frame.start_address().as_u64())
        .unwrap_or(0);
    if l4 != expected {
        report(CPU_003, odd, l4, expected, l4);
    } else if apply_operation(cpu, Operation::RestoreSetupKernelCr3, odd) {
        cpu.expected_l4.store(l4, Ordering::Relaxed);
        cpu.observed_cr3.store(l4, Ordering::Relaxed);
    }
    trace(Operation::RestoreSetupKernelCr3, l4, 0, odd);
    crate::diagnostics::trace::record(
        crate::diagnostics::trace::EventKind::Cr3Write,
        l4,
        0,
        3,
        odd + 1,
    );
    finish(cpu, odd);
}

pub fn commit_address_space_setup() {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::CommitAddressSpaceSetup);
    let expected = crate::mm::paging::kernel_l4_frame()
        .map(|frame| frame.start_address().as_u64())
        .unwrap_or(0);
    let (live, _) = x86_64::registers::control::Cr3::read();
    let live = live.start_address().as_u64();
    if live != expected || crate::arch::x86_64::percpu::current_user_pid().is_some() {
        report(CPU_003, odd, 0, expected, live);
    }
    apply_operation(cpu, Operation::CommitAddressSpaceSetup, odd);
    cpu.address_space_generation.store(0, Ordering::Relaxed);
    cpu.observed_pid.store(0, Ordering::Relaxed);
    trace(Operation::CommitAddressSpaceSetup, expected, live, odd);
    finish(cpu, odd);
}

pub fn commit_kernel() {
    if !enabled() {
        return;
    }
    let cpu = local();
    if phase(cpu.phase.load(Ordering::Acquire)) != Phase::LoadingKernel
        || cpu.completed.load(Ordering::Acquire) != KERNEL_COMPLETE
    {
        return;
    }
    let odd = begin(cpu, Operation::CommitKernel);
    let expected = crate::mm::paging::kernel_l4_frame()
        .map(|frame| frame.start_address().as_u64())
        .unwrap_or(0);
    let (live, _) = x86_64::registers::control::Cr3::read();
    let live = live.start_address().as_u64();
    let current = crate::arch::x86_64::percpu::current_user_pid().unwrap_or(0);
    if live != expected || current != 0 {
        report(
            CPU_003,
            odd,
            cpu.target_entity.load(Ordering::Relaxed),
            expected,
            live,
        );
    }
    if cpu.pending_entity.load(Ordering::Acquire) != 0 {
        report(
            CPU_004,
            odd,
            cpu.target_entity.load(Ordering::Relaxed),
            0,
            cpu.pending_entity.load(Ordering::Relaxed),
        );
    }
    cpu.observed_cr3.store(live, Ordering::Relaxed);
    cpu.observed_pid.store(current, Ordering::Relaxed);
    apply_operation(cpu, Operation::CommitKernel, odd);
    trace(
        Operation::CommitKernel,
        cpu.target_entity.load(Ordering::Relaxed),
        live,
        odd,
    );
    finish(cpu, odd);
}

pub fn set_pending_publish(entity: EntityId) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::SetPendingPublish);
    let encoded = super::scheduler::entity_key(entity);
    let previous = cpu.pending_entity.swap(encoded, Ordering::AcqRel);
    if previous != 0 {
        report(CPU_004, odd, encoded, 0, previous);
    }
    if let EntityId::UserProcess(pid) = entity {
        if phase(cpu.phase.load(Ordering::Relaxed)) == Phase::UserStable
            && crate::arch::x86_64::percpu::current_user_pid() != Some(pid)
        {
            report(
                CPU_004,
                odd,
                encoded,
                u64::from(pid),
                u64::from(crate::arch::x86_64::percpu::current_user_pid().unwrap_or(0)),
            );
        }
    }
    trace(Operation::SetPendingPublish, encoded, previous, odd);
    finish(cpu, odd);
}

pub fn take_pending_publish(entity: EntityId) {
    if !enabled() {
        return;
    }
    let cpu = local();
    let odd = begin(cpu, Operation::TakePendingPublish);
    let encoded = super::scheduler::entity_key(entity);
    let observed = cpu.pending_entity.swap(0, Ordering::AcqRel);
    if observed != encoded {
        report(CPU_004, odd, encoded, encoded, observed);
    }
    trace(Operation::TakePendingPublish, encoded, observed, odd);
    finish(cpu, odd);
}

pub fn write_snapshot(writer: &mut crate::diagnostics::wire::Writer<'_>) -> u32 {
    let count = crate::arch::x86_64::percpu::initialized_cpu_count()
        .min(crate::arch::x86_64::acpi::MAX_CPUS);
    writer.u16(count as u16);
    writer.u16(96);
    let mut unstable = 0u32;
    for (index, cpu) in CPUS.iter().take(count).enumerate() {
        let before = cpu.epoch.load(Ordering::Acquire);
        writer.u8(index as u8);
        writer.u8(cpu.phase.load(Ordering::Relaxed));
        writer.u8(cpu.completed.load(Ordering::Relaxed));
        writer.u8(cpu.last_operation.load(Ordering::Relaxed));
        writer.u32(u32::from(cpu.flags.load(Ordering::Relaxed)));
        writer.u64(before);
        for value in [
            cpu.target_entity.load(Ordering::Relaxed),
            cpu.expected_l4.load(Ordering::Relaxed),
            cpu.address_space_generation.load(Ordering::Relaxed),
            cpu.expected_stack_top.load(Ordering::Relaxed),
            cpu.stack_generation.load(Ordering::Relaxed),
            cpu.observed_cr3.load(Ordering::Relaxed),
            cpu.observed_rsp0.load(Ordering::Relaxed),
            cpu.observed_gs_top.load(Ordering::Relaxed),
        ] {
            writer.u64(value);
        }
        writer.u32(cpu.observed_pid.load(Ordering::Relaxed));
        writer.u32(0);
        writer.u64(cpu.pending_entity.load(Ordering::Acquire));
        let after = cpu.epoch.load(Ordering::Acquire);
        unstable |= u32::from(before != after || after & 1 != 0);
    }
    unstable
}

pub fn snapshot_flags() -> u32 {
    u32::from(
        CPUS.iter()
            .take(crate::arch::x86_64::percpu::initialized_cpu_count())
            .any(|cpu| cpu.epoch.load(Ordering::Acquire) & 1 != 0),
    )
}

#[cfg(feature = "test")]
pub fn inject_wrong_cr3() {
    let pid = 0x7fff_ff10;
    begin_user(pid, 0x1234_5000, 1, 0x8000, 1);
    install_user_cr3(0x9999_9000, 1);
}

#[cfg(feature = "test")]
pub fn inject_wrong_pid() {
    let pid = 0x7fff_ff12;
    begin_user(pid, 0x1234_5000, 1, 0x8000, 1);
    install_user_cr3(0x1234_5000, 1);
    install_rsp0(0x8000);
    install_gs_stack(0x8000);
    restore_extended(pid);
    set_current_pid(pid + 1);
}

#[cfg(feature = "test")]
pub fn inject_wrong_kernel_cr3() {
    begin_kernel(None);
    clear_current_pid();
    install_kernel_cr3(0x9999_9000);
}

#[cfg(feature = "test")]
pub fn inject_wrong_publication() {
    set_pending_publish(EntityId::UserProcess(0x7fff_ff13));
    set_pending_publish(EntityId::UserProcess(0x7fff_ff14));
}

#[cfg(feature = "test")]
pub fn inject_wrong_order() {
    let pid = 0x7fff_ff11;
    begin_user(pid, 0x1234_5000, 1, 0x8000, 1);
    install_rsp0(0x8000);
}
