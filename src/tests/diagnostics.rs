use crate::diagnostics::{trace, wire};
use crate::lib::test_utils::Testable;

fn test_crc32_golden_vector() {
    assert_eq!(wire::crc32(b"123456789"), 0xcbf4_3926);
}

fn test_wire_layout_contract() {
    assert_eq!(core::mem::size_of::<wire::CapsuleHeader>(), 80);
    assert_eq!(core::mem::size_of::<wire::SectionHeader>(), 16);
    assert_eq!(wire::MAGIC, *b"AGCRASH\0");
}

fn test_memory_shadow_budget_contract() {
    let frames = 65_536usize;
    let expected = if cfg!(any(feature = "diagnostics", feature = "diagnostics-strict")) {
        frames * 64
    } else {
        0
    };
    assert_eq!(
        crate::diagnostics::shadow::memory::storage_bytes(frames),
        expected
    );
    assert!(expected == 0 || expected <= 32 * 1024 * 1024);
}

fn test_trace_commit_and_wrap() {
    // CPU 7 is a valid possible-CPU recorder even when the test VM boots
    // fewer CPUs, which keeps this synthetic writer isolated from live CPUs.
    let cpu = crate::arch::x86_64::acpi::MAX_CPUS - 1;
    let (start, overwritten_before, _) = trace::counters(cpu);
    let writes = trace::RING_LEN as u64 + 5;
    for value in 0..writes {
        trace::record_on(
            cpu,
            trace::EventKind::BootPhase,
            value,
            value ^ 0x55aa,
            0,
            0,
        );
    }
    let (next, overwritten_after, _) = trace::counters(cpu);
    assert_eq!(next, start + writes);
    let expected_new_overwrites = (next.saturating_sub(1 + trace::RING_LEN as u64))
        .saturating_sub(start.saturating_sub(1 + trace::RING_LEN as u64));
    assert_eq!(
        overwritten_after - overwritten_before,
        expected_new_overwrites
    );
    let newest = next - 1;
    let record = trace::snapshot(cpu, newest as usize % trace::RING_LEN)
        .expect("committed newest trace record");
    assert_eq!(record.sequence, newest);
    assert_eq!(record.subject, writes - 1);
    assert_eq!(record.arg0, (writes - 1) ^ 0x55aa);
}

fn test_scheduler_shadow_legal_save_publish_cycle() {
    use crate::diagnostics::shadow::scheduler::{
        initial_for_test, transition_state, OperationKind, ShadowState, Transition,
    };
    let transition = |operation, cpu, published| Transition {
        operation,
        cpu,
        published,
        allow_running_exit: false,
    };
    let mut entity = initial_for_test(42);
    entity = transition_state(entity, transition(OperationKind::Register, u8::MAX, true)).unwrap();
    entity = transition_state(entity, transition(OperationKind::MakeReady, 0, true)).unwrap();
    entity = transition_state(entity, transition(OperationKind::Dispatch, 0, true)).unwrap();
    entity = transition_state(entity, transition(OperationKind::BeginSave, 0, false)).unwrap();
    assert_eq!(entity.state, ShadowState::ReadyUnpublished);
    entity = transition_state(entity, transition(OperationKind::Publish, u8::MAX, true)).unwrap();
    assert_eq!(entity.state, ShadowState::ReadyQueued);
}

fn test_scheduler_shadow_rejects_illegal_edges() {
    use crate::diagnostics::shadow::scheduler::{
        initial_for_test, transition_state, OperationKind, Transition, SCHED_004, SCHED_005,
        SCHED_007,
    };
    let transition = |operation, cpu| Transition {
        operation,
        cpu,
        published: true,
        allow_running_exit: false,
    };
    let blocked = transition_state(
        initial_for_test(77),
        transition(OperationKind::Register, u8::MAX),
    )
    .unwrap();
    assert_eq!(
        transition_state(blocked, transition(OperationKind::Dispatch, 0)).unwrap_err(),
        SCHED_004
    );
    let mut ready = transition_state(blocked, transition(OperationKind::MakeReady, 0)).unwrap();
    ready.affinity = 1;
    assert_eq!(
        transition_state(ready, transition(OperationKind::Dispatch, 0)).unwrap_err(),
        SCHED_005
    );
    let running = transition_state(ready, transition(OperationKind::Dispatch, 1)).unwrap();
    assert_eq!(
        transition_state(running, transition(OperationKind::Unregister, u8::MAX)).unwrap_err(),
        SCHED_007
    );
}

fn test_pager_shadow_transition_table() {
    use crate::diagnostics::shadow::pager::{transition_state, Operation, State, PAGER_001};

    let state = transition_state(State::Classified, Operation::ReserveFrame).unwrap();
    let state = transition_state(state, Operation::Populate).unwrap();
    let state = transition_state(state, Operation::Commit).unwrap();
    assert_eq!(state, State::PresentCommitted);
    assert_eq!(
        transition_state(State::Classified, Operation::Commit).unwrap_err(),
        PAGER_001
    );
    assert_eq!(
        transition_state(State::FrameReserved, Operation::Abort).unwrap(),
        State::Aborted
    );
}

fn test_cpu_shadow_handoff_transition_table() {
    use crate::diagnostics::shadow::cpu::{
        transition_state, Operation, Phase, TransitionState, CPU_005,
    };

    let mut state = TransitionState {
        phase: Phase::Boot,
        completed: 0,
    };
    for operation in [
        Operation::InitializeKernel,
        Operation::BeginUser,
        Operation::InstallUserCr3,
        Operation::InstallRsp0,
        Operation::InstallGsStack,
        Operation::RestoreExtended,
        Operation::SetCurrentPid,
        Operation::CommitUser,
    ] {
        state = transition_state(state, operation).unwrap();
    }
    assert_eq!(state.phase, Phase::UserStable);
    assert_eq!(
        transition_state(
            TransitionState {
                phase: Phase::KernelStable,
                completed: 0,
            },
            Operation::InstallUserCr3,
        )
        .unwrap_err(),
        CPU_005
    );
    for operation in [
        Operation::BeginKernel,
        Operation::ClearCurrentPid,
        Operation::InstallKernelCr3,
        Operation::CommitKernel,
    ] {
        state = transition_state(state, operation).unwrap();
    }
    assert_eq!(state.phase, Phase::KernelStable);
    for operation in [
        Operation::BeginAddressSpaceSetup,
        Operation::InstallSetupCr3,
        Operation::RestoreSetupKernelCr3,
        Operation::CommitAddressSpaceSetup,
    ] {
        state = transition_state(state, operation).unwrap();
    }
    assert_eq!(state.phase, Phase::KernelStable);
}

fn test_io_shadow_transition_table() {
    use crate::diagnostics::shadow::io::{transition_state, Operation, State, IO_002};

    let state = transition_state(State::Submitted, Operation::Complete).unwrap();
    let state = transition_state(state, Operation::QueueWake).unwrap();
    let state = transition_state(state, Operation::AcceptWake).unwrap();
    let state = transition_state(state, Operation::Consume).unwrap();
    assert_eq!(state, State::Consumed);
    assert_eq!(
        transition_state(State::Submitted, Operation::Consume).unwrap_err(),
        IO_002
    );
}

fn test_continuation_shadow_transition_table() {
    use crate::diagnostics::shadow::continuation::{
        transition_state, Operation, State, CONT_001, CONT_003,
    };

    let state = transition_state(State::Saving, Operation::Publish).unwrap();
    let state = transition_state(state, Operation::Wake).unwrap();
    let state = transition_state(state, Operation::Dispatch).unwrap();
    let state = transition_state(state, Operation::Consume).unwrap();
    assert_eq!(state, State::Consumed);
    assert_eq!(
        transition_state(State::Saving, Operation::Dispatch).unwrap_err(),
        CONT_001
    );
    assert_eq!(
        transition_state(State::Runnable, Operation::Consume).unwrap_err(),
        CONT_003
    );
}

fn test_address_space_shadow_transition_table() {
    use crate::diagnostics::shadow::address_space::{
        transition_state, Operation, State, AS_002, AS_003,
    };

    let state = transition_state(State::Building, Operation::Publish).unwrap();
    let state = transition_state(state, Operation::Activate).unwrap();
    let state = transition_state(state, Operation::Deactivate).unwrap();
    let state = transition_state(state, Operation::BeginDestroy).unwrap();
    let state = transition_state(state, Operation::Release).unwrap();
    assert_eq!(state, State::Dead);
    assert_eq!(
        transition_state(State::Dead, Operation::Activate).unwrap_err(),
        AS_002
    );
    assert_eq!(
        transition_state(State::Active, Operation::BeginDestroy).unwrap_err(),
        AS_003
    );
}

fn test_stack_shadow_transition_table() {
    use crate::diagnostics::shadow::stack::{
        transition_state, Operation, State, STACK_001, STACK_002,
    };

    let state = transition_state(State::Allocated, Operation::Publish).unwrap();
    let state = transition_state(state, Operation::Activate).unwrap();
    let state = transition_state(state, Operation::Deactivate).unwrap();
    let state = transition_state(state, Operation::BeginRetire).unwrap();
    let state = transition_state(state, Operation::Release).unwrap();
    assert_eq!(state, State::Dead);
    assert_eq!(
        transition_state(State::Allocated, Operation::Activate).unwrap_err(),
        STACK_002
    );
    assert_eq!(
        transition_state(State::Active, Operation::BeginRetire).unwrap_err(),
        STACK_001
    );
}

fn test_no_production_shadow_violation_latched() {
    if let Some(violation) = crate::diagnostics::shadow::first() {
        crate::debug_error!(
            "scheduler shadow violation invariant={:#010x} cpu={} epoch={} subject={:#x} expected={:#x} observed={:#x}",
            violation.invariant_id,
            violation.cpu,
            violation.epoch,
            violation.subject,
            violation.expected0,
            violation.observed0,
        );
        panic!(
            "clean production transitions latched invariant={:#010x} cpu={} epoch={} subject={:#x} expected={:#x} observed={:#x}",
            violation.invariant_id,
            violation.cpu,
            violation.epoch,
            violation.subject,
            violation.expected0,
            violation.observed0,
        );
    }
}

fn test_observed_lock_graph_matches_reviewed_order() {
    use crate::diagnostics::shadow::locks::{self, LockClassId};

    if !locks::observed_graph_is_allowed() {
        for class in [
            LockClassId::Scheduler,
            LockClassId::ProcessTable,
            LockClassId::MemoryMapper,
            LockClassId::StackAllocator,
            LockClassId::HeapAllocator,
            LockClassId::SerialLogger,
        ] {
            crate::debug_error!(
                "lock class {:?} observed dependency mask={:#06x}",
                class,
                locks::observed_edges(class),
            );
        }
        panic!("observed lock graph departed from the reviewed partial order");
    }
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_crc32_golden_vector,
        &test_wire_layout_contract,
        &test_memory_shadow_budget_contract,
        &test_trace_commit_and_wrap,
        &test_scheduler_shadow_legal_save_publish_cycle,
        &test_scheduler_shadow_rejects_illegal_edges,
        &test_cpu_shadow_handoff_transition_table,
        &test_pager_shadow_transition_table,
        &test_io_shadow_transition_table,
        &test_continuation_shadow_transition_table,
        &test_address_space_shadow_transition_table,
        &test_stack_shadow_transition_table,
        &test_observed_lock_graph_matches_reviewed_order,
        &test_no_production_shadow_violation_latched,
    ]
}
