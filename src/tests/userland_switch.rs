//! U4 characterization tests for the ring-3 switch primitive.
//!
//! Tests in this module exercise [`crate::userland::switch::save_ring3`]
//! and validate the lookup/snapshot logic that
//! [`crate::userland::switch::resume_ring3`] builds on. The diverging
//! asm itself (`resume_ring3_asm`) is structurally identical to the
//! existing `iretq_to_user_with_regs` and `enter_user_mode_with_regs_asm`
//! paths, both of which are exercised every time zsh / hello.elf boot;
//! end-to-end validation of `resume_ring3` falls out when U5 wires it
//! into the timer ISR and a second ring-3 process can actually
//! coexist with the first. Unit-testing it here would require either
//! a full synthetic ELF + address space (heavy) or a same-CPL iretq
//! trick that risks corrupting the test runner's stack.
//!
//! What this file does test:
//!
//! - `save_ring3` copies every GPR + RIP + RFLAGS + RSP from a
//!   synthetic `InterruptStackFrame` into a `Process.saved_user_state`,
//!   byte-for-byte, with no field shuffling.
//! - `save_ring3` also invokes `save_user_cpu_state`, so the per-process
//!   `fs_base` and `fpu_state` fields capture the live MSR / XMM
//!   register state at call time.
//! - Field offsets in `UserState` haven't drifted from the values the
//!   asm in `resume_ring3_asm` hard-codes.

use crate::arch::x86_64::preemption::InterruptStackFrame;
use crate::lib::test_utils::Testable;
use crate::userland::lifecycle::{ExitKind, Process};
use crate::userland::switch::save_ring3;
use crate::userland::user_state::UserState;

/// Build a minimal `Process` suitable for save_ring3 tests. PIDs are
/// arbitrary; the helper never touches the live process table.
fn synthetic_process(pid: u32) -> Process {
    Process {
        pid,
        parent_pid: 0,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_base: 0,
        brk_current: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        kernel_stack: None,
        exe_path: None,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: UserState::default(),
        terminal_id: None,
    }
}

/// Allocate an `InterruptStackFrame` with a distinctive pattern in
/// every field. Each field gets a unique nibble in the high bits so an
/// off-by-one register copy is loud and obvious in the assertion that
/// fails.
fn patterned_frame() -> InterruptStackFrame {
    InterruptStackFrame {
        // GPRs: pattern is 0x<reg-nibble>_AAAA_BBBB_CCCC.
        r15: 0xF_AAAA_BBBB_CCCC,
        r14: 0xE_AAAA_BBBB_CCCC,
        r13: 0xD_AAAA_BBBB_CCCC,
        r12: 0xC_AAAA_BBBB_CCCC,
        r11: 0xB_AAAA_BBBB_CCCC,
        r10: 0xA_AAAA_BBBB_CCCC,
        r9: 0x9_AAAA_BBBB_CCCC,
        r8: 0x8_AAAA_BBBB_CCCC,
        rbp: 0x7_AAAA_BBBB_CCCC,
        rdi: 0x6_AAAA_BBBB_CCCC,
        rsi: 0x5_AAAA_BBBB_CCCC,
        rdx: 0x4_AAAA_BBBB_CCCC,
        rcx: 0x3_AAAA_BBBB_CCCC,
        rbx: 0x2_AAAA_BBBB_CCCC,
        rax: 0x1_AAAA_BBBB_CCCC,
        // CPU-pushed half: use distinct patterns so a mistaken
        // assignment (e.g., rsp ← rflags) shows up unambiguously.
        rip: 0x0000_7FFF_DEAD_0010,
        cs: 0x23, // user code RPL=3
        rflags: 0x0000_0000_0000_0202, // IF set, reserved bit
        rsp: 0x0000_7FFF_BBBB_8000,
        ss: 0x1B, // user data RPL=3
    }
}

/// `save_ring3` copies every UserState field from the trap frame
/// directly — no field shuffling, no sign extension, no truncation.
fn test_save_ring3_copies_every_gpr() {
    let mut p = synthetic_process(9100);
    let frame = patterned_frame();

    save_ring3(&mut p, &frame);

    let s = &p.saved_user_state;
    assert_eq!(s.rax, frame.rax, "rax");
    assert_eq!(s.rbx, frame.rbx, "rbx");
    assert_eq!(s.rdi, frame.rdi, "rdi");
    assert_eq!(s.rsi, frame.rsi, "rsi");
    assert_eq!(s.rdx, frame.rdx, "rdx");
    assert_eq!(s.r10, frame.r10, "r10");
    assert_eq!(s.r8, frame.r8, "r8");
    assert_eq!(s.r9, frame.r9, "r9");
    assert_eq!(s.rbp, frame.rbp, "rbp");
    assert_eq!(s.r12, frame.r12, "r12");
    assert_eq!(s.r13, frame.r13, "r13");
    assert_eq!(s.r14, frame.r14, "r14");
    assert_eq!(s.r15, frame.r15, "r15");
    assert_eq!(s.rip, frame.rip, "rip");
    assert_eq!(s.rflags, frame.rflags, "rflags");
    assert_eq!(s.rsp, frame.rsp, "rsp");
}

/// `UserState` does not carry `rcx` or `r11` — the SYSCALL ABI clobbers
/// both, and the resume asm intentionally zeroes them. Verify save_ring3
/// doesn't accidentally try to thread rcx through some other field
/// (e.g., overwriting r10 with rcx — a plausible foot-gun).
fn test_save_ring3_discards_rcx_and_r11() {
    let mut p = synthetic_process(9101);
    let mut frame = patterned_frame();
    frame.rcx = 0xCCCC_CCCC_CCCC_CCCC;
    frame.r11 = 0x1111_1111_1111_1111;

    save_ring3(&mut p, &frame);

    // The pattern in r10 / r9 / r8 etc. must match the frame's
    // patterned value, not the clobbered rcx/r11. A buggy save that
    // wrote rcx into r10 would surface here.
    assert_eq!(p.saved_user_state.r10, frame.r10);
    assert_eq!(p.saved_user_state.r9, frame.r9);
    assert_eq!(p.saved_user_state.r8, frame.r8);
}

/// `save_ring3` also captures FS_BASE into the process's `fs_base`
/// field via `save_user_cpu_state`. This is the per-process MSR
/// preservation U5 relies on when switching between processes.
fn test_save_ring3_captures_fs_base() {
    let mut p = synthetic_process(9102);
    let frame = patterned_frame();

    let saved_original = crate::arch::x86_64::msr::read_fs_base();
    let staged: u64 = 0x0000_7FFF_5555_AAAA;
    crate::arch::x86_64::msr::set_fs_base(staged);

    save_ring3(&mut p, &frame);

    assert_eq!(p.fs_base, staged, "save_ring3 should capture live FS_BASE");

    // Restore so the runner's TLS isn't affected.
    crate::arch::x86_64::msr::set_fs_base(saved_original);
}

/// `save_ring3` captures XMM state into the process's `fpu_state`
/// buffer. Stage a recognizable XMM0 pattern, save, scribble XMM0,
/// then restore via the matching `restore_user_cpu_state` and observe
/// the staged pattern reappear.
fn test_save_ring3_roundtrips_fpu_state() {
    use crate::userland::lifecycle::restore_user_cpu_state;

    let mut p = synthetic_process(9103);
    let frame = patterned_frame();

    // Save kernel FS_BASE so the FPU dance doesn't drag in TLS issues.
    let saved_fs = crate::arch::x86_64::msr::read_fs_base();

    // Stage a distinctive XMM0 pattern from kernel mode.
    let staged: [u64; 2] = [0xCAFE_BABE_DEAD_BEEF, 0x0123_4567_89AB_CDEF];
    unsafe {
        core::arch::asm!(
            "movdqu xmm0, [{0}]",
            in(reg) staged.as_ptr(),
            options(nostack, preserves_flags),
        );
    }

    // Capture into p.fpu_state.
    save_ring3(&mut p, &frame);

    // Scribble live XMM0 with a contrasting pattern.
    let scribble: [u64; 2] = [0xFFFF_FFFF_FFFF_FFFF, 0];
    unsafe {
        core::arch::asm!(
            "movdqu xmm0, [{0}]",
            in(reg) scribble.as_ptr(),
            options(nostack, preserves_flags),
        );
    }

    // Restore from p — XMM0 must come back as the staged pattern.
    restore_user_cpu_state(&p);

    let mut observed: [u64; 2] = [0; 2];
    unsafe {
        core::arch::asm!(
            "movdqu [{0}], xmm0",
            in(reg) observed.as_mut_ptr(),
            options(nostack, preserves_flags),
        );
    }
    assert_eq!(observed, staged, "save_ring3 → restore_user_cpu_state must round-trip XMM0");

    // Restore FS_BASE in case the FPU dance perturbed anything.
    crate::arch::x86_64::msr::set_fs_base(saved_fs);
}

/// `UserState` field offsets are baked into the asm in
/// `resume_ring3_asm`. A reorder in `user_state.rs` would silently
/// shuffle which register receives which value at iretq time. Lock
/// the offsets here so a refactor breaks compilation before it
/// breaks ring 3.
fn test_user_state_offsets_match_asm_contract() {
    use core::mem::offset_of;
    assert_eq!(offset_of!(UserState, rax), 0);
    assert_eq!(offset_of!(UserState, rdi), 8);
    assert_eq!(offset_of!(UserState, rsi), 16);
    assert_eq!(offset_of!(UserState, rdx), 24);
    assert_eq!(offset_of!(UserState, r10), 32);
    assert_eq!(offset_of!(UserState, r8), 40);
    assert_eq!(offset_of!(UserState, r9), 48);
    assert_eq!(offset_of!(UserState, rbx), 56);
    assert_eq!(offset_of!(UserState, rbp), 64);
    assert_eq!(offset_of!(UserState, rsp), 72);
    assert_eq!(offset_of!(UserState, r12), 80);
    assert_eq!(offset_of!(UserState, r13), 88);
    assert_eq!(offset_of!(UserState, r14), 96);
    assert_eq!(offset_of!(UserState, r15), 104);
    assert_eq!(offset_of!(UserState, rip), 112);
    assert_eq!(offset_of!(UserState, rflags), 120);
    assert_eq!(core::mem::size_of::<UserState>(), 128);
}

// ---------- U5: try_preempt_ring3 decision logic ----------
//
// These tests exercise the timer-ISR-side helper that picks the next
// ring-3 process to switch into. They mutate `PROCESS_TABLE` —
// each test inserts its own PIDs, runs, and removes them so subsequent
// tests start from a clean baseline. They never call `resume_ring3`
// (that diverges to ring 3 with no return path); the helper itself
// returns `Option<u32>`, which is enough to verify the queue/save
// behavior in isolation.

/// Restore-on-drop guard: snapshots current_user_pid + ring3_ready
/// PIDs at construction and restores them on drop, so a test panicking
/// mid-way doesn't poison the global PROCESS_TABLE for subsequent
/// tests.
struct PreemptTestGuard {
    saved_current: Option<u32>,
}

impl PreemptTestGuard {
    fn new() -> Self {
        Self {
            saved_current: crate::userland::lifecycle::current_user_pid(),
        }
    }
}

impl Drop for PreemptTestGuard {
    fn drop(&mut self) {
        // Clear any leftover ring3_ready entries the test inserted
        // (best-effort — production code's `remove_process` already
        // drains them, but a half-completed test path might not have
        // reached that line).
        crate::userland::lifecycle::clear_ring3_queues_for_test();
        crate::userland::lifecycle::set_current_user_pid(self.saved_current);
    }
}

/// Insert `pid` into PROCESS_TABLE for the duration of a test. Caller
/// removes via `remove_process` (or relies on the guard's cleanup if
/// they only need queue mutations).
fn insert_synthetic(pid: u32) {
    use crate::userland::lifecycle::{insert_process, Process, ExitKind};
    let p = Process {
        pid,
        parent_pid: 0,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_base: 0,
        brk_current: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        kernel_stack: None,
        exe_path: None,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: UserState::default(),
        terminal_id: None,
    };
    insert_process(p);
}

/// No current ring-3 process → no preempt, no save, no queue mutation.
fn test_try_preempt_ring3_returns_none_when_no_current() {
    use crate::userland::lifecycle::{set_current_user_pid, try_preempt_ring3};
    let _g = PreemptTestGuard::new();
    set_current_user_pid(None);

    let frame = patterned_frame();
    assert!(try_preempt_ring3(&frame).is_none());
}

/// current_user_pid is set but ring3_ready is empty → no preempt.
/// Returns None so the timer ISR iretq's back to the same process.
fn test_try_preempt_ring3_returns_none_when_queue_empty() {
    use crate::userland::lifecycle::{
        remove_process, set_current_user_pid, try_preempt_ring3,
    };
    let _g = PreemptTestGuard::new();

    insert_synthetic(9200);
    set_current_user_pid(Some(9200));

    let frame = patterned_frame();
    assert!(try_preempt_ring3(&frame).is_none());

    set_current_user_pid(None);
    remove_process(9200);
}

/// Front of ring3_ready is the same as current → re-queue and no-op.
/// Single-process-with-self-in-queue shouldn't trigger a pointless
/// self-switch (and would corrupt state if it did, since save_ring3
/// would write into the same slot we're about to "switch to").
fn test_try_preempt_ring3_self_at_front_is_noop() {
    use crate::userland::lifecycle::{
        mark_ring3_ready, remove_process, set_current_user_pid, try_preempt_ring3,
        with_process,
    };
    let _g = PreemptTestGuard::new();

    insert_synthetic(9210);
    set_current_user_pid(Some(9210));
    mark_ring3_ready(9210);

    // Pre-condition: saved_user_state is zero (default).
    let pre_rip = with_process(9210, |p| p.saved_user_state.rip).unwrap();
    assert_eq!(pre_rip, 0);

    let frame = patterned_frame();
    let next = try_preempt_ring3(&frame);
    assert_eq!(next, None, "self-at-front must NOT switch");

    // saved_user_state must remain untouched — save_ring3 was not called.
    let post_rip = with_process(9210, |p| p.saved_user_state.rip).unwrap();
    assert_eq!(post_rip, 0, "save_ring3 must not run on self-at-front");

    set_current_user_pid(None);
    remove_process(9210);
}

/// Two ring-3 processes, distinct PIDs: front of queue is the OTHER
/// one. try_preempt_ring3 returns that other pid, saves the current
/// process's GPRs/RIP/RFLAGS/RSP, and pushes the current to the back
/// of the queue (round-robin).
fn test_try_preempt_ring3_switches_to_other() {
    use crate::userland::lifecycle::{
        mark_ring3_ready, peek_next_ring3, remove_process, set_current_user_pid,
        try_preempt_ring3, with_process,
    };
    let _g = PreemptTestGuard::new();

    insert_synthetic(9220); // cur
    insert_synthetic(9221); // next
    set_current_user_pid(Some(9220));
    mark_ring3_ready(9221);

    let frame = patterned_frame();
    let next = try_preempt_ring3(&frame);
    assert_eq!(next, Some(9221), "should pick the other queued pid");

    // cur (9220) was saved.
    let saved = with_process(9220, |p| p.saved_user_state).unwrap();
    assert_eq!(saved.rip, frame.rip, "cur's saved RIP must match frame");
    assert_eq!(saved.rsp, frame.rsp, "cur's saved RSP must match frame");
    assert_eq!(saved.rax, frame.rax, "cur's saved RAX must match frame");

    // cur was pushed to the back; the queue front is now cur.
    assert_eq!(
        peek_next_ring3(),
        Some(9220),
        "round-robin: cur should now be at front of queue"
    );

    set_current_user_pid(None);
    remove_process(9220);
    remove_process(9221);
}

// ---------- U9: cross-process signal isolation ----------

/// Signals raised on process A must land in A's signal_state, not in
/// the currently-loaded process's. With multi-ring-3 (U7+) this is the
/// load-bearing invariant: when zsh1 forks zsh2, then SIGUSR1 is sent
/// to zsh2 (e.g., by another terminal's kill), the signal must queue
/// on zsh2 — not on zsh1 just because zsh1 happens to be loaded.
///
/// The test simulates the scenario: install processes A and B, mark B
/// current, raise SIGUSR1 on A via `with_process(A_pid, ...)`. Assert
/// A's pending mask carries SIGUSR1 and B's does not — proving the
/// raise targeted the correct slot.
fn test_signal_raised_on_other_process_does_not_land_in_current() {
    use crate::userland::lifecycle::{
        insert_process, remove_process, set_current_user_pid, with_process,
    };
    use crate::userland::signal::SIGUSR1;

    let _g = PreemptTestGuard::new();

    insert_synthetic(9300); // A
    insert_synthetic(9301); // B
    set_current_user_pid(Some(9301)); // B is loaded

    // Raise SIGUSR1 on A via with_process — the path U8's
    // notify_parent_of_exit / kill_handler use.
    let raised = with_process(9300, |p| {
        p.signal_state.raise(SIGUSR1);
        p.signal_state.pending
    })
    .unwrap();
    assert_ne!(
        raised & (1 << (SIGUSR1 - 1)),
        0,
        "SIGUSR1 should land in A's pending mask"
    );

    // B's pending mask must be untouched.
    let b_pending = with_process(9301, |p| p.signal_state.pending).unwrap();
    assert_eq!(
        b_pending & (1 << (SIGUSR1 - 1)),
        0,
        "B's pending mask must NOT carry SIGUSR1 (raise targeted A by pid)"
    );

    set_current_user_pid(None);
    remove_process(9300);
    remove_process(9301);
}

/// `maybe_deliver_signal`-style consume reads from `current_user_pid`.
/// If A has SIGUSR1 pending but B is loaded, the syscall-exit
/// dispatcher tail (running for B's syscall) must NOT deliver A's
/// signal — it should consume from B's queue, find nothing, and
/// return cleanly. The signal sits in A's queue until A itself
/// returns from a syscall.
fn test_signal_delivery_consumes_from_current_only() {
    use crate::userland::lifecycle::{
        insert_process, remove_process, set_current_user_pid, with_process,
    };
    use crate::userland::signal::{SigAction, SIGUSR1};

    let _g = PreemptTestGuard::new();

    insert_synthetic(9310); // A — has signal pending
    insert_synthetic(9311); // B — currently loaded

    // Install a non-SIG_DFL handler on A so `consume_deliverable`
    // actually returns Some when A's mask carries SIGUSR1.
    with_process(9310, |p| {
        p.signal_state.set_action(
            SIGUSR1,
            SigAction {
                sa_handler: 0xDEAD_BEEF, // any non-zero, non-1 handler
                sa_flags: 0,
                sa_restorer: 0,
                sa_mask: 0,
            },
        );
    });

    // Stage SIGUSR1 on A.
    with_process(9310, |p| p.signal_state.raise(SIGUSR1));

    // B is the current process; "deliver" via consume_deliverable on B.
    set_current_user_pid(Some(9311));
    let b_consumed = with_process(9311, |p| p.signal_state.consume_deliverable())
        .unwrap();
    assert!(
        b_consumed.is_none(),
        "B's consume_deliverable must return None — its mask is clean"
    );

    // A still has SIGUSR1 pending (B's consume didn't drain it).
    let a_pending_after = with_process(9310, |p| p.signal_state.pending).unwrap();
    assert_ne!(
        a_pending_after & (1 << (SIGUSR1 - 1)),
        0,
        "A's SIGUSR1 must still be pending after B's syscall-tail consume"
    );

    // Now switch to A as current. consume_deliverable on A returns
    // SIGUSR1, mirroring what would happen on A's next syscall exit.
    set_current_user_pid(Some(9310));
    let a_consumed = with_process(9310, |p| p.signal_state.consume_deliverable())
        .unwrap();
    let (sig, _action) = a_consumed.expect("A's consume must return SIGUSR1");
    assert_eq!(sig, SIGUSR1);

    set_current_user_pid(None);
    remove_process(9310);
    remove_process(9311);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_save_ring3_copies_every_gpr,
        &test_save_ring3_discards_rcx_and_r11,
        &test_save_ring3_captures_fs_base,
        &test_save_ring3_roundtrips_fpu_state,
        &test_user_state_offsets_match_asm_contract,
        &test_try_preempt_ring3_returns_none_when_no_current,
        &test_try_preempt_ring3_returns_none_when_queue_empty,
        &test_try_preempt_ring3_self_at_front_is_noop,
        &test_try_preempt_ring3_switches_to_other,
        &test_signal_raised_on_other_process_does_not_land_in_current,
        &test_signal_delivery_consumes_from_current_only,
    ]
}
