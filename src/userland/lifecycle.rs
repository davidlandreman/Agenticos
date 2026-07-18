// Userland process lifecycle (U7).
//
// Bridges the ring-3 entry path (`crate::userland::enter_user_mode`) and the
// teardown path (`cleanup_user_process`). Both ends of the lifecycle long-jump
// to the same kernel continuation that `enter_user_mode` saves before
// `iretq`-ing to user space:
//
// - **Cooperative exit** (U5's `exit` syscall handler): the syscall dispatcher
//   notices the exit syscall, records the code, and calls
//   `restore_continuation` from `cleanup_user_process` — never returning to
//   the dispatcher's `iretq`.
// - **Abnormal exit** (ring-3 fault routed by `interrupts.rs`): same target.
//
// The continuation is captured as a setjmp-style snapshot: callee-saved GPRs
// + RSP + a return RIP. Restoring it makes `enter_user_mode` "return" as if
// the user app had completed normally; control flows back to the run command,
// which drops the `UserImage`, clears terminal routing, and notifies the shell.
//
// Single-app-synchronous (D5) means there is exactly one continuation slot
// at a time. The slot is taken at `enter_user_mode` time and consumed by the
// long-jump. A second `run` while one is active is rejected by the run
// command before `enter_user_mode` is reached.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;
use x86_64::VirtAddr;

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::userland::address_space::AddressSpace;
use crate::userland::fdtable::FdTable;
use crate::userland::image::UserImage;
use crate::userland::kernel_stack::KernelStack;
use crate::userland::signal::SignalState;

/// Reason a user process is being torn down. Populated by exception handlers
/// (fault) or the `exit` syscall (cooperative).
#[derive(Debug, Clone, Copy)]
pub struct AbnormalExit {
    /// Exception vector number (e.g., 13 for #GP, 14 for #PF, 6 for #UD).
    /// For cooperative exits via the `exit` syscall, vector is 0xFF (sentinel).
    pub vector: u8,
    /// CPU-pushed error code, when the vector pushes one.
    pub error_code: Option<u64>,
    /// Faulting linear address (#PF only — read from CR2 by the handler).
    pub fault_addr: Option<VirtAddr>,
    /// Saved RIP at the moment of the fault.
    pub fault_rip: VirtAddr,
}

/// The single active per-CPU user-process slot.
///
/// Holds:
/// - The saved kernel continuation (set by `enter_user_mode`, consumed by
///   `restore_continuation`).
/// - The active `UserImage` (transferred from the loader on commit; dropped
///   when teardown returns from the long-jump).
/// - The recorded exit information (cooperative code or fault reason) so the
///   run command can log a diagnostic after returning.
///
/// The mutex is `try_lock`-able from interrupt context because `Spin::Mutex`
/// is fair-acquired but never blocks the kernel — every taker checks for the
/// expected state and gives up if not present. Long-jump readers always
/// observe a consistent snapshot because the writer (`enter_user_mode`)
/// completes the write before the `iretq`.
/// A ring-3 process. Each user binary launched by `enter_user_mode_with`
/// gets a fresh `Process` slot; the slot is removed when the long-jump
/// returns and the run command calls `release_active_image`.
///
/// Phase 4 PR-A: this replaces the prior single-process `ActiveUser`
/// global. Today only one slot is ever populated at a time (single-user-
/// app D5), but the table-of-Processes shape unblocks fork (PR-C) and
/// execve (PR-D) which need to address parent and child by PID.
pub struct Process {
    /// Stable per-process identifier. Allocated monotonically from
    /// `NEXT_PID` on `enter_user_mode_with`. Visible to ring 3 via
    /// `getpid`. PID 0 is reserved (kernel proper).
    pub pid: u32,
    /// PID of the parent that created us. For binaries launched by the
    /// `run` command, this is `0` (kernel as parent). For fork-spawned
    /// children (PR-C), this becomes the parent's PID.
    pub parent_pid: u32,
    pub image: Option<UserImage>,
    pub exit_kind: ExitKind,
    pub exit_code: i64,
    /// Byte-granular current program break. Growth is VMA-only until touch;
    /// shrink releases complete pages. `brk(0)` returns this value.
    pub brk_current: u64,
    /// Immutable lower bound for `brk`, derived from this image's PT_LOADs.
    pub brk_base: u64,
    /// Legacy mmap cursor retained for ABI/test compatibility. Allocation is
    /// authoritative in `AddressSpace::vmas()` and uses reusable gap search.
    pub mmap_next: u64,
    /// Phase 2: file-descriptor table. Slots 0/1/2 are pinned to the
    /// standard streams; slots 3..N hold `Arc<File>` opened via `openat`.
    pub fd_table: FdTable,
    /// Restart-stable deadline state for a blocking network syscall.
    pub network_wait: Option<NetworkWaitState>,
    /// Linux ITIMER_REAL state, represented against the monotonic 100 Hz PIT.
    pub real_timer: RealTimerState,
    /// Restart-stable absolute PIT deadline for a blocking `nanosleep`. Set
    /// on the first entry, checked on every SYSCALL re-fire, and cleared when
    /// the sleep completes. See [`Ring3BlockReason::Sleeping`].
    pub sleep_deadline: Option<u64>,
    /// Set when an asynchronous signal wakes a re-fire-based blocking syscall.
    /// The next dispatcher entry delivers the signal with `-EINTR` instead of
    /// running the syscall handler and immediately blocking again.
    pub pending_syscall_interrupt: bool,
    /// Phase 2: per-process current working directory. Anchors relative
    /// paths in `openat(AT_FDCWD, …)`, `stat`, `access`, etc. Always
    /// stored as a normalized absolute path.
    pub cwd: String,
    /// Phase 4 PR-B: per-process L4 page table. Owns the L4 frame. The
    /// option is `None` for the kernel-sentinel slot (PID 0); every
    /// real user process has a populated `AddressSpace`.
    pub address_space: Option<AddressSpace>,
    /// Phase 5 PR-B: signal actions, blocked mask, pending mask.
    /// Reset on every `enter_user_mode_with` and on `execve` (signals
    /// are not preserved across exec, per POSIX). Inherited by fork
    /// children (shallow copy is fine — there are no shared
    /// references inside SignalState).
    pub signal_state: SignalState,
    /// Phase 5 PR-C1: per-process kernel stack. The SYSCALL stub
    /// reads the rsp top from `gs:[0]`, which we update to point at
    /// this stack's `top()` whenever the process is the active one.
    /// Interrupt gates use TSS.rsp0 (also kept in sync). Each process
    /// gets its own buffer so parent + child syscall handlers don't
    /// share a single rsp0 area.
    pub kernel_stack: Option<KernelStack>,
    /// U3: launch path of the binary running in this process. Set by
    /// `enter_user_mode_with_aspace` from `argv[0]` when the run
    /// command (or `execve`) provides one; left `None` for synthetic
    /// in-kernel test launches that bypass argv. `readlink` reads
    /// this for `/proc/self/exe` (see `readlink_handler`).
    pub exe_path: Option<String>,
    /// Demand-grown stack — top of the user stack (exclusive). Constant
    /// per-process; mirrors `image.stack_top` for fast access from the
    /// ring-3 page-fault handler without dereferencing `image`. Zero for
    /// the kernel-sentinel slot (PID 0).
    pub stack_top: u64,
    /// Demand-grown stack — current lowest committed page. Lowered on
    /// each successful `try_grow_user_stack`. Compared against the
    /// faulting address to classify ring-3 page faults.
    pub stack_bottom: u64,
    /// Demand-grown stack — lowest currently-mapped page. Equals
    /// `stack_bottom` after every successful growth; kept separate so a
    /// future shrink path can release frames without touching the
    /// classification baseline. Teardown (`unmap_user_stack`) walks
    /// `[stack_mapped_bottom, stack_top)` as a single range.
    pub stack_mapped_bottom: u64,
    /// Demand-grown stack — lowest page the stack may ever grow into.
    /// `max(USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES*0x1000,
    /// highest_pt_load_end + 16-page guard)`. Faults below this are
    /// true overflows.
    pub stack_max_growth_floor: u64,
    /// Demand-grown stack — remaining per-process growth budget. Starts
    /// at `USER_STACK_MAX_GROWTH_PAGES`; decremented on each successful
    /// growth. Guards against a fault-storm attack pattern consuming
    /// the process's stack reservation and reusable physical frames.
    pub growth_faults_remaining: u64,
    /// U2: per-process FS_BASE MSR value. The SYSCALL fast path does
    /// NOT save/restore this on every syscall (would cost two MSR ops
    /// per call); instead, U4's ring-3 switch primitive saves/restores
    /// it when actually swapping processes. `arch_prctl(ARCH_SET_FS)`
    /// updates this field via `with_current_process`.
    pub fs_base: u64,
    /// U2: per-process x87/SSE state buffer (FXSAVE area). Saved on
    /// switch-out, restored on switch-in by U4. Fresh processes start
    /// with the architectural reset state via
    /// [`crate::arch::x86_64::fpu::FpuState::fresh`].
    pub fpu_state: crate::arch::x86_64::fpu::FpuState,
    /// U4: snapshot of this process's user-mode GPRs + RIP/RFLAGS/RSP
    /// at the moment it was last switched out. Written by
    /// [`crate::userland::switch::save_ring3`] on preempt-out and read
    /// by [`crate::userland::switch::resume_ring3`] on preempt-in.
    /// Zero-initialized for fresh processes; the first ring-3 entry
    /// still goes through [`enter_user_mode_asm`] (which doesn't read
    /// this field), so the first time U5's timer-driven save fires the
    /// snapshot becomes meaningful. Layout matches `UserState` exactly
    /// — the same offsets are baked into both `iretq_to_user_with_regs`
    /// and `resume_ring3_asm`.
    pub saved_user_state: crate::userland::user_state::UserState,
    /// U8/bugfix: terminal window this process's stdout + stderr
    /// should route to. Inherited from the launching kernel thread's
    /// PCB.terminal_id at install time, and from the parent across
    /// fork. `None` for synthetic test processes (output falls back
    /// to the global CURRENT_OUTPUT_TERMINAL). Pre-fix, all ring-3
    /// processes routed via a single global `CURRENT_OUTPUT_TERMINAL`
    /// which the last launcher won — under multi-terminal that caused
    /// zsh1's writes to land in terminal 2's window.
    pub terminal_id: Option<crate::window::WindowId>,
}

/// What ended the user process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// No exit yet (still running).
    None,
    /// Cooperative `exit(code)` syscall.
    Cooperative,
    /// Ring-3 fault — see `AbnormalExit` for vector / fault address.
    Abnormal {
        vector: u8,
        fault_rip: u64,
        fault_addr: Option<u64>,
    },
}

/// Reserved PID for "no current process." Real PIDs start at 1.
pub const KERNEL_PID: u32 = 0;

/// Monotonic PID allocator. Wrapping is unrealistic for our scope; we
/// stop the kernel before exhausting u32.
static NEXT_PID: AtomicU32 = AtomicU32::new(1);

/// PID-indexed table of ring-3 user processes plus the "which one is
/// loaded right now" pointer.
///
/// **Invariant:** there is always an entry at [`KERNEL_PID`] (0). When no
/// ring-3 process is loaded, `with_current_process` falls back to that
/// entry — preserving the old singleton's "the sentinel slot is always
/// there to read/write" semantics. Real ring-3 processes live at PIDs
/// allocated from [`alloc_pid`] (starting at 1).
///
/// Today the table holds at most one real entry plus the sentinel (D5
/// still enforced by `enter_user_mode_with_aspace`), but the shape is
/// what U3..U8 build on: fork inserts a second real entry without
/// removing the parent; the timer ISR (U5) flips `current_user_pid`
/// between entries to time-slice between ring-3 processes.
pub struct ProcessTable {
    pub by_pid: BTreeMap<u32, Process>,
    /// PID of the process whose registers are currently loaded into the
    /// CPU (its CR3, kernel stack, FS_BASE, FPU). `None` when the kernel
    /// is running and no ring-3 process is current.
    pub current_user_pid: Option<u32>,
    /// U3: round-robin queue of ring-3 PIDs ready to be scheduled.
    /// `Runnable::RingThree(pid)` decisions in `schedule_ring3_aware`
    /// pop from the front; readying a process pushes to the back.
    /// Excludes [`KERNEL_PID`] (the sentinel is never schedulable).
    pub ring3_ready: VecDeque<u32>,
    /// U3: PIDs currently blocked, with the reason they're waiting.
    /// Wake paths (e.g., `wake_ring3_blocked_on_child`) look up by
    /// reason and move matches into `ring3_ready`.
    pub ring3_blocked: BTreeMap<u32, Ring3BlockReason>,
}

impl ProcessTable {
    const fn empty() -> Self {
        Self {
            by_pid: BTreeMap::new(),
            current_user_pid: None,
            ring3_ready: VecDeque::new(),
            ring3_blocked: BTreeMap::new(),
        }
    }
}

/// U3: why a ring-3 process is parked. Distinct from `pcb::BlockReason`
/// (which is for kernel-thread blocking) because the keys differ —
/// `WaitingForChild::target` carries the POSIX wait4 selector (`i32`,
/// possibly `-1` for "any child"), not a kernel-thread PID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring3BlockReason {
    /// `wait4(target, ...)` with no matching zombie yet. Wakes when
    /// any child of this process becomes a zombie (U6's child-exit
    /// path calls `wake_ring3_blocked_on_child(parent_pid)`).
    WaitingForChild {
        target: i32,
    },
    /// `read(0, ...)` from stdin with an empty input queue. Wakes
    /// when input lands on the blocked process's terminal via
    /// [`wake_ring3_blocked_on_input`] (called from the stdin push
    /// path with the producing terminal's id). On wake, the parent's
    /// SYSCALL re-fires and `read_stdin_blocking` re-checks the
    /// queue.
    WaitingForInput,
    /// `gui_next_event` with an empty per-process GUI event queue.
    WaitingForGuiEvent,
    /// `read(fd, ...)` on a pipe whose ring buffer is empty but at
    /// least one writer still exists. Wakes when any pipe write
    /// appends bytes, or when the last writer drops (allowing the
    /// reader to observe EOF) — see [`wake_ring3_blocked_on_pipe_readable`].
    /// Wake is conservative (all `WaitingForPipeRead` blockers wake on
    /// any pipe event); each woken reader re-fires its SYSCALL and
    /// either succeeds or blocks again.
    WaitingForPipeRead,
    /// `write(fd, ...)` on a pipe whose ring buffer is full but at
    /// least one reader still exists. Wakes when any pipe read drains
    /// bytes, or when the last reader drops (so the writer re-fires
    /// and returns `EPIPE`) — see [`wake_ring3_blocked_on_pipe_writable`].
    WaitingForPipeWrite,
    WaitingForNetwork {
        deadline_tick: Option<u64>,
    },
    /// `nanosleep` with a not-yet-elapsed absolute PIT deadline. Wakes when
    /// [`process_expired_sleeps`] observes `now >= deadline_tick` from kernel
    /// housekeeping; the re-fired SYSCALL then sees its `sleep_deadline`
    /// elapsed and returns 0.
    Sleeping {
        deadline_tick: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkWaitState {
    pub syscall_nr: u64,
    pub identity: u64,
    pub deadline_tick: Option<u64>,
    pub expired: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RealTimerState {
    pub deadline_tick: Option<u64>,
    pub interval_ticks: u64,
}

impl RealTimerState {
    pub const fn disarmed() -> Self {
        Self {
            deadline_tick: None,
            interval_ticks: 0,
        }
    }
}

// This table is shared by timer-preemptible kernel threads, ring-3 exception
// handlers, and SYSCALL handlers (which enter with IF cleared). A plain spin
// mutex allows a kernel thread to be preempted while holding the table and the
// ring-3 handler to spin forever on the same single CPU. Make the invariant
// structural so new call sites cannot accidentally reintroduce that deadlock.
pub(crate) static PROCESS_TABLE: InterruptMutex<ProcessTable> =
    InterruptMutex::new(ProcessTable::empty());

/// Lazy initializer: ensures the sentinel entry at PID 0 exists. Called
/// from every `with_current_process`/`with_process` path before lookup
/// so test paths that touch the table before `init_userland` runs still
/// observe the invariant. `BTreeMap::insert` is idempotent: subsequent
/// calls are no-ops once the slot is populated.
fn ensure_sentinel(g: &mut ProcessTable) {
    if !g.by_pid.contains_key(&KERNEL_PID) {
        g.by_pid.insert(KERNEL_PID, Process::sentinel());
    }
}

impl Process {
    /// Default "no current process" sentinel. Allocates nothing (every
    /// heap-backed field is empty). The table keeps one of these
    /// permanently installed at PID 0 — read/write through
    /// `with_current_process` when `current_user_pid` is `None` sees
    /// this entry, matching the pre-PR-C singleton behavior.
    fn sentinel() -> Self {
        Process {
            pid: KERNEL_PID,
            parent_pid: KERNEL_PID,
            image: None,
            exit_kind: ExitKind::None,
            exit_code: 0,
            brk_current: 0,
            brk_base: 0,
            mmap_next: 0,
            fd_table: FdTable::new(),
            network_wait: None,
            real_timer: RealTimerState::disarmed(),
            sleep_deadline: None,
            pending_syscall_interrupt: false,
            cwd: String::new(),
            address_space: None,
            signal_state: SignalState::new(),
            kernel_stack: None,
            exe_path: None,
            stack_top: 0,
            stack_bottom: 0,
            stack_mapped_bottom: 0,
            stack_max_growth_floor: 0,
            growth_faults_remaining: 0,
            fs_base: 0,
            fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
            saved_user_state: crate::userland::user_state::UserState::default(),
            terminal_id: None,
        }
    }
}

/// Acquire the currently-loaded ring-3 process slot for read/write.
///
/// When `current_user_pid` is `Some(pid)`, `f` runs against that entry.
/// Otherwise it runs against the persistent sentinel entry at PID 0 —
/// mutations persist there, matching the pre-PR-C singleton behavior
/// that some test helpers (e.g., `stage_stack_window`) and the test
/// hooks rely on.
///
/// Used by syscall handlers, the run command, and the page-fault
/// handler — i.e., all paths that operate on "the process executing
/// right now." Cross-process operations should use [`with_process`].
pub fn with_current_process<R>(f: impl FnOnce(&mut Process) -> R) -> R {
    let mut g = PROCESS_TABLE.lock();
    ensure_sentinel(&mut g);
    let pid = g.current_user_pid.unwrap_or(KERNEL_PID);
    let p = g.by_pid.get_mut(&pid).expect("sentinel invariant violated");
    f(p)
}

/// Operate on a specific process by PID. Returns `None` if `pid` is not
/// in the table. Used by U3..U8 callers that need to inspect or mutate
/// a non-current process (e.g., the scheduler waking a blocked parent,
/// or `notify_parent_of_exit` raising SIGCHLD on the parent without
/// requiring it to be the loaded process).
pub fn with_process<R>(pid: u32, f: impl FnOnce(&mut Process) -> R) -> Option<R> {
    let mut g = PROCESS_TABLE.lock();
    ensure_sentinel(&mut g);
    g.by_pid.get_mut(&pid).map(f)
}

/// Compatibility alias for the (small) tail of callsites still using
/// the pre-PR-C name. New code should use `with_current_process`.
pub fn with_active_user<R>(f: impl FnOnce(&mut Process) -> R) -> R {
    with_current_process(f)
}

/// PID of the currently-loaded ring-3 process, or `None` if none.
/// Distinct from [`current_pid`] which folds `None` into `KERNEL_PID`
/// (0) for the long tail of callers that want a `u32` directly.
pub fn current_user_pid() -> Option<u32> {
    PROCESS_TABLE.lock().current_user_pid
}

/// Set which process is "currently loaded" — i.e., whose CR3 / kernel
/// stack / FS_BASE / FPU are active on the CPU. Used by the install
/// path today; U4/U5 will also call this from the ring-3 switch
/// primitive when time-slicing between processes.
pub fn set_current_user_pid(pid: Option<u32>) {
    PROCESS_TABLE.lock().current_user_pid = pid;
}

/// Insert a freshly-built `Process` into the table. Caller is
/// responsible for setting `current_user_pid` separately when the
/// process should be the loaded one (today, only the install path
/// does both atomically via [`install_new_process_opt`]).
pub fn insert_process(p: Process) -> u32 {
    let pid = p.pid;
    PROCESS_TABLE.lock().by_pid.insert(pid, p);
    pid
}

/// Remove a process from the table by PID. Returns the removed entry
/// or `None` if not present. If the removed PID was the current one,
/// `current_user_pid` is cleared as a side effect.
///
/// Refuses to remove the sentinel entry at [`KERNEL_PID`] — the
/// sentinel is invariant. Real ring-3 processes always carry a PID
/// allocated from [`alloc_pid`] (≥ 1), so this only ever rejects buggy
/// callers.
pub fn remove_process(pid: u32) -> Option<Process> {
    if pid == KERNEL_PID {
        debug_assert!(false, "attempted to remove sentinel process at PID 0");
        return None;
    }
    let mut g = PROCESS_TABLE.lock();
    if g.current_user_pid == Some(pid) {
        g.current_user_pid = None;
    }
    // U3: a removed process must not linger in the scheduler queues.
    g.ring3_ready.retain(|p| *p != pid);
    g.ring3_blocked.remove(&pid);
    let removed = g.by_pid.remove(&pid)?;
    drop(g);
    crate::userland::gui::cleanup_process(pid);
    Some(removed)
}

/// Reset the sentinel entry at PID 0 to its default state. Used by the
/// teardown paths (`release_active_image`, `force_clear_active_for_test`)
/// so that mutations made via `with_current_process` while no real
/// process was loaded — for example, test helpers that synthetically
/// install `image = Some(...)` on the sentinel — don't bleed into the
/// next launch's `user_active()` / `with_active_user` checks.
pub fn reset_sentinel() {
    let mut g = PROCESS_TABLE.lock();
    g.by_pid.insert(KERNEL_PID, Process::sentinel());
}

// ---------- U3: ring-3 scheduling state ----------

/// Mark `pid` ready to run. Removes it from the blocked map (if
/// present) and pushes it onto the back of `ring3_ready` so the next
/// `pop_next_ring3` decision picks it (round-robin order). No-op if
/// `pid` is already in `ring3_ready`. Rejects [`KERNEL_PID`] — the
/// sentinel is never schedulable.
pub fn mark_ring3_ready(pid: u32) {
    if pid == KERNEL_PID {
        debug_assert!(false, "attempted to mark sentinel ring-3 ready");
        return;
    }
    let mut g = PROCESS_TABLE.lock();
    g.ring3_blocked.remove(&pid);
    if !g.ring3_ready.iter().any(|p| *p == pid) {
        g.ring3_ready.push_back(pid);
    }
}

/// Mark `pid` blocked with `reason`. Removes it from `ring3_ready` if
/// present so the scheduler decision skips it; records the reason so
/// wake paths know whether to unblock it.
pub fn mark_ring3_blocked(pid: u32, reason: Ring3BlockReason) {
    if pid == KERNEL_PID {
        debug_assert!(false, "attempted to mark sentinel ring-3 blocked");
        return;
    }
    let mut g = PROCESS_TABLE.lock();
    g.ring3_ready.retain(|p| *p != pid);
    g.ring3_blocked.insert(pid, reason);
}

/// Pop the front of the ring-3 ready queue, if any. Used by U5's
/// timer-ISR-driven ring-3-aware scheduler when deciding what to
/// resume after a ring-3 preemption.
pub fn pop_next_ring3() -> Option<u32> {
    PROCESS_TABLE.lock().ring3_ready.pop_front()
}

/// Peek at the front of the ring-3 ready queue without popping. Used
/// by the U5 decision path to check whether any ring-3 process is
/// runnable before falling back to the kernel-thread scheduler.
pub fn peek_next_ring3() -> Option<u32> {
    PROCESS_TABLE.lock().ring3_ready.front().copied()
}

/// Wake any ring-3 process blocked-on-wait4 whose target matches
/// `child_pid` (positive `target == child_pid as i32`) or `target == -1`
/// (any child) AND whose own PID equals `parent_pid`.
///
/// Called by `notify_parent_of_exit` and `notify_parent_of_signaled_exit`
/// after filing the zombie. Today's pre-U5 callers run this as a no-op
/// (no ring-3 process is ever blocked because the synchronous-fork
/// pattern never reaches a "child running, parent blocked" state);
/// once U5/U7 land, this is the actual wake.
pub fn wake_ring3_blocked_on_child(parent_pid: u32, child_pid: u32) {
    if parent_pid == KERNEL_PID {
        return;
    }
    let mut g = PROCESS_TABLE.lock();
    let should_wake = match g.ring3_blocked.get(&parent_pid) {
        Some(Ring3BlockReason::WaitingForChild { target }) => {
            *target == -1 || *target == child_pid as i32
        }
        Some(Ring3BlockReason::WaitingForInput)
        | Some(Ring3BlockReason::WaitingForGuiEvent)
        | Some(Ring3BlockReason::WaitingForPipeRead)
        | Some(Ring3BlockReason::WaitingForPipeWrite)
        | Some(Ring3BlockReason::WaitingForNetwork { .. })
        | Some(Ring3BlockReason::Sleeping { .. })
        | None => false,
    };
    if should_wake {
        g.ring3_blocked.remove(&parent_pid);
        if !g.ring3_ready.iter().any(|p| *p == parent_pid) {
            g.ring3_ready.push_back(parent_pid);
        }
    }
}

/// Wake any ring-3 process blocked on stdin input whose `terminal_id`
/// matches `terminal_id`. Called by the stdin push path after
/// enqueueing bytes; the caller already knows which terminal the bytes
/// came from, so only readers on that terminal should wake.
///
/// Pre-fix this woke every `WaitingForInput` blocker — which under
/// multi-terminal meant keystrokes typed in terminal 2 woke zsh1, and
/// whichever zsh got dispatched first drained the shared global queue.
/// Pairing each wake with a `terminal_id` makes input routing
/// deterministic.
///
/// `terminal_id == None` matches processes whose own `terminal_id` is
/// `None` (test / legacy paths that don't model a terminal window).
///
/// Walks the blocked map once per call. The set is small (one entry
/// per terminal-bound ring-3 process); the walk cost is negligible.
pub fn wake_ring3_blocked_on_input(terminal_id: Option<crate::window::WindowId>) {
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    let blocked: alloc::vec::Vec<u32> = g
        .ring3_blocked
        .iter()
        .filter_map(|(pid, reason)| {
            if matches!(reason, Ring3BlockReason::WaitingForInput) {
                Some(*pid)
            } else {
                None
            }
        })
        .collect();
    let waking: alloc::vec::Vec<u32> = blocked
        .into_iter()
        .filter(|pid| {
            g.by_pid
                .get(pid)
                .map(|p| p.terminal_id == terminal_id)
                .unwrap_or(false)
        })
        .collect();
    for pid in waking {
        g.ring3_blocked.remove(&pid);
        if !g.ring3_ready.iter().any(|p| *p == pid) {
            g.ring3_ready.push_back(pid);
        }
    }
}

/// Raise `sig` on every ring-3 process whose `terminal_id` matches.
/// Used by the SIGWINCH path on grid resize, and available for any
/// future tty-scoped signal delivery (`SIGINT` on Ctrl-C, etc.).
///
/// Walks the process table once and calls `signal_state.raise(sig)`
/// on each match. Does not deliver the signal — deferred to the
/// per-process `maybe_deliver_signal` path that runs at SYSCALL
/// boundary.
pub fn raise_signal_on_terminal(terminal_id: crate::window::WindowId, sig: i32) {
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    for (_pid, p) in g.by_pid.iter_mut() {
        if p.terminal_id == Some(terminal_id) {
            p.signal_state.raise(sig);
        }
    }
}

/// Force-terminate every ring-3 process bound to `terminal_id` — a shell
/// and everything it forked, since children inherit the parent's
/// `terminal_id` across `fork` (`fork_handler` in `syscalls.rs`). Called
/// by the window-close path so closing a terminal window tears down its
/// process tree (`zsh` + `ping` + …) instead of orphaning it.
///
/// This deliberately bypasses the signal machinery: default-disposition
/// signals (SIGKILL/SIGHUP) are left pending and never acted on today
/// (see `SignalState::consume_deliverable`), and the cooperative-exit
/// path must run on the dying process's own kernel stack. Instead we
/// remove each `Process` from the table and drop it, which frees its
/// `AddressSpace`, `KernelStack`, and fd table.
///
/// Safety: the caller runs on a kernel thread (the compositor / window
/// manager), so none of the target ring-3 processes are executing on
/// their kernel stacks right now — a blocked or ready ring-3 process has
/// its live state in `saved_user_state`, and its kernel stack holds only
/// an abandoned frame that the next SYSCALL entry would overwrite. See
/// the U8 notes in `src/userland/CLAUDE.md`.
///
/// After removing the processes, wakes any kernel launcher thread that
/// was blocked in `wait_for_ring3_exit` on one of them (mirroring the
/// normal exit path) so it unblocks and returns from
/// `launch_user_binary`. Its `release_active_image` then finds the
/// process already gone and reclaims nothing — that path tolerates a
/// missing entry precisely for this case.
pub fn kill_ring3_processes_on_terminal(terminal_id: crate::window::WindowId) {
    // Snapshot the victim PIDs under the table lock, then release it
    // before removing/dropping any `Process`: `remove_process` takes the
    // lock itself, and `AddressSpace::drop` touches the global memory
    // mapper — neither must run while we hold `PROCESS_TABLE`.
    let victims: alloc::vec::Vec<u32> = {
        let g = PROCESS_TABLE.lock();
        g.by_pid
            .iter()
            .filter(|(pid, p)| **pid != KERNEL_PID && p.terminal_id == Some(terminal_id))
            .map(|(pid, _)| *pid)
            .collect()
    };

    if victims.is_empty() {
        return;
    }

    for pid in &victims {
        // `remove_process` yanks the PID out of `by_pid`,
        // `ring3_ready`, and `ring3_blocked`, and clears
        // `current_user_pid` if it matched. Dropping the returned
        // `Process` frees its address space, kernel stack, and fds.
        if let Some(process) = remove_process(*pid) {
            drop(process);
        }
    }

    // Unblock the launcher kernel thread(s) parked on these ring-3 pids'
    // exit so they return from `launch_user_binary` instead of hanging.
    let mut sched = crate::process::scheduler::SCHEDULER.lock();
    for pid in &victims {
        sched.wake_threads_waiting_for_ring3_exit(*pid);
    }
    drop(sched);

    crate::debug_info!(
        "kill_ring3_processes_on_terminal: terminated {} ring-3 process(es) for {:?}",
        victims.len(),
        terminal_id
    );
}

/// Wake every ring-3 process blocked on a pipe read. Called when a
/// pipe write appends bytes or when the last writer drops (so blocked
/// readers can observe EOF). Conservative — any pipe event wakes every
/// `WaitingForPipeRead` blocker; each re-fires its read syscall and
/// either succeeds or blocks again on its own pipe. The blocked set is
/// at most one entry per ring-3 process, so the walk is negligible.
///
/// Uses `try_lock`: the wake may be invoked from inside
/// `PipeWriteHandle::Drop`, which can fire while another path
/// (`close_handler`, `dup2_handler`) holds `PROCESS_TABLE` via
/// `with_fd_table_mut`. On contention the wake is skipped — the
/// holding syscall handler is responsible for issuing an explicit
/// follow-up wake after the lock is released (see
/// `close_handler` / `dup2_handler`). Spin-deadlock-safe.
pub fn wake_ring3_blocked_on_pipe_readable() {
    wake_ring3_blocked_by(|r| matches!(r, Ring3BlockReason::WaitingForPipeRead));
}

/// Wake every ring-3 process blocked on a pipe write. Called when a
/// pipe read drains bytes (freeing buffer space) or when the last
/// reader drops (so blocked writers re-fire and return `EPIPE`).
/// `try_lock` discipline matches
/// [`wake_ring3_blocked_on_pipe_readable`].
pub fn wake_ring3_blocked_on_pipe_writable() {
    wake_ring3_blocked_by(|r| matches!(r, Ring3BlockReason::WaitingForPipeWrite));
}

/// Wake network waiters after a socket state change, and expire any waiter
/// whose absolute PIT deadline has elapsed. The per-process `network_wait`
/// record is deliberately retained across ordinary event wakes so a restarted
/// syscall cannot extend a finite timeout.
pub fn wake_ring3_blocked_on_network(state_changed: bool) {
    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    let waking: alloc::vec::Vec<(u32, bool)> = g
        .ring3_blocked
        .iter()
        .filter_map(|(pid, reason)| match reason {
            Ring3BlockReason::WaitingForNetwork { deadline_tick } => {
                let expired = deadline_tick.is_some_and(|deadline| now >= deadline);
                (state_changed || expired).then_some((*pid, expired))
            }
            _ => None,
        })
        .collect();
    for (pid, expired) in waking {
        if expired {
            if let Some(process) = g.by_pid.get_mut(&pid) {
                if let Some(wait) = process.network_wait.as_mut() {
                    wait.expired = true;
                }
            }
        }
        g.ring3_blocked.remove(&pid);
        if !g.ring3_ready.iter().any(|ready| *ready == pid) {
            g.ring3_ready.push_back(pid);
        }
    }
}

/// Queue expired ITIMER_REAL alarms and make signal-interruptible blocked
/// processes runnable. This runs from kernel housekeeping rather than the PIT
/// ISR: scanning the process table and growing the ready queue do not belong in
/// interrupt context, and timers must not depend on the network worker.
pub fn process_due_real_timers() {
    use crate::userland::signal::SIGALRM;

    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    let mut interruptible = alloc::vec::Vec::new();

    for (&pid, process) in g.by_pid.iter_mut() {
        let Some(deadline) = process.real_timer.deadline_tick else {
            continue;
        };
        if now < deadline {
            continue;
        }

        process.signal_state.raise(SIGALRM);
        if process.real_timer.interval_ticks == 0 {
            process.real_timer.deadline_tick = None;
        } else {
            let interval = process.real_timer.interval_ticks;
            let periods = now.saturating_sub(deadline) / interval + 1;
            let advanced = deadline.saturating_add(periods.saturating_mul(interval));
            process.real_timer.deadline_tick = Some(if advanced <= now {
                now.saturating_add(interval)
            } else {
                advanced
            });
        }

        if process.signal_state.has_deliverable_handler(SIGALRM) {
            interruptible.push(pid);
        }
    }

    for pid in interruptible {
        let Some(reason) = g.ring3_blocked.remove(&pid) else {
            continue;
        };
        if let Some(process) = g.by_pid.get_mut(&pid) {
            if matches!(reason, Ring3BlockReason::WaitingForNetwork { .. }) {
                process.network_wait = None;
            }
            // A signal-interrupted nanosleep returns via the dispatcher's
            // -EINTR path without re-entering the handler, so clear its
            // restart-stable deadline here (mirroring `network_wait`) — else a
            // later nanosleep would see the stale elapsed deadline and return
            // 0 immediately instead of sleeping.
            if matches!(reason, Ring3BlockReason::Sleeping { .. }) {
                process.sleep_deadline = None;
            }
            process.pending_syscall_interrupt = true;
        }
        if !g.ring3_ready.iter().any(|ready| *ready == pid) {
            g.ring3_ready.push_back(pid);
        }
    }
}

/// Wake every ring-3 process whose `nanosleep` deadline has elapsed. Called
/// primarily from the compositor kernel thread's loop (`window::compositor`),
/// which is scheduled every round-robin revolution — the kernel main loop is
/// the idle task under U10 and barely runs once other kernel threads are ready,
/// so relying on it alone would wake self-timed animation loops only every few
/// seconds. Also called from the main loop and the inline ring-3 dispatch loop
/// for the test/launcher paths. Scanning the blocked set and growing the ready
/// queue must not happen in interrupt context, so this is never called from the
/// PIT ISR. The woken process's re-fired SYSCALL observes its `sleep_deadline`
/// as elapsed (via [`nanosleep_deadline`]) and returns 0.
pub fn process_expired_sleeps() {
    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    let waking: alloc::vec::Vec<u32> = g
        .ring3_blocked
        .iter()
        .filter_map(|(pid, reason)| match reason {
            Ring3BlockReason::Sleeping { deadline_tick } if now >= *deadline_tick => Some(*pid),
            _ => None,
        })
        .collect();
    for pid in waking {
        g.ring3_blocked.remove(&pid);
        if !g.ring3_ready.iter().any(|ready| *ready == pid) {
            g.ring3_ready.push_back(pid);
        }
    }
}

/// Restart-stable `nanosleep` state machine for the current ring-3 process.
///
/// `requested_ticks` is the sleep length rounded up to whole PIT ticks (0 ⇒
/// return immediately). Returns `Some(deadline)` if the caller should block
/// with [`Ring3BlockReason::Sleeping`], or `None` if the sleep is already
/// satisfied (elapsed, or zero-length) and the handler should return 0.
///
/// The absolute deadline is recorded on the first entry and preserved across
/// SYSCALL re-fires so a woken-and-re-blocked sleeper cannot extend its own
/// timeout, mirroring [`prepare_network_wait`].
pub fn nanosleep_deadline(requested_ticks: u64) -> Option<u64> {
    with_current_process(|process| {
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        if let Some(deadline) = process.sleep_deadline {
            if now >= deadline {
                process.sleep_deadline = None;
                return None;
            }
            return Some(deadline);
        }
        if requested_ticks == 0 {
            return None;
        }
        let deadline = now.saturating_add(requested_ticks);
        process.sleep_deadline = Some(deadline);
        Some(deadline)
    })
}

/// Return a restart-stable absolute deadline for a blocking network syscall.
/// `None` timeout means infinite. An expired matching state is consumed and
/// reported as `Err(())`.
pub fn prepare_network_wait(
    syscall_nr: u64,
    identity: u64,
    timeout_ticks: Option<u64>,
) -> Result<Option<u64>, ()> {
    with_current_process(|process| {
        if let Some(wait) = process.network_wait {
            if wait.syscall_nr == syscall_nr && wait.identity == identity {
                if wait.expired {
                    process.network_wait = None;
                    return Err(());
                }
                return Ok(wait.deadline_tick);
            }
        }
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        let deadline_tick = timeout_ticks.map(|ticks| now.saturating_add(ticks.max(1)));
        process.network_wait = Some(NetworkWaitState {
            syscall_nr,
            identity,
            deadline_tick,
            expired: false,
        });
        Ok(deadline_tick)
    })
}

pub fn clear_network_wait() {
    with_current_process(|process| process.network_wait = None);
}

/// Consume the marker installed when a signal woke a blocked syscall.
pub fn take_pending_syscall_interrupt() -> bool {
    with_current_process(|process| core::mem::take(&mut process.pending_syscall_interrupt))
}

pub fn clear_stale_network_wait(syscall_nr: u64) {
    with_current_process(|process| {
        if process
            .network_wait
            .is_some_and(|wait| wait.syscall_nr != syscall_nr)
        {
            process.network_wait = None;
        }
    });
}

fn wake_ring3_blocked_by<F>(matcher: F)
where
    F: Fn(&Ring3BlockReason) -> bool,
{
    // `try_lock` to avoid re-entrant spin: pipe-handle `Drop` can fire
    // from inside `with_fd_table_mut`, which already holds this lock.
    // Callers that mutate the FD table under the lock issue explicit
    // wake calls after release so a skipped wake here is recovered.
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return;
    };
    let waking: alloc::vec::Vec<u32> = g
        .ring3_blocked
        .iter()
        .filter_map(|(pid, reason)| if matcher(reason) { Some(*pid) } else { None })
        .collect();
    for pid in waking {
        g.ring3_blocked.remove(&pid);
        if !g.ring3_ready.iter().any(|p| *p == pid) {
            g.ring3_ready.push_back(pid);
        }
    }
}

/// Returns true if `pid` has any child currently tracked in
/// `by_pid` (parent_pid match) OR any unreaped zombie. Used by `wait4`
/// (U6) to distinguish "no children at all → ECHILD" from "has
/// children but none zombie yet → block or EAGAIN".
pub fn has_children(parent_pid: u32) -> bool {
    let g = PROCESS_TABLE.lock();
    let live = g
        .by_pid
        .values()
        .any(|p| p.parent_pid == parent_pid && p.pid != KERNEL_PID);
    drop(g);
    let zombie = ZOMBIES.lock().values().any(|z| z.parent_pid == parent_pid);
    live || zombie
}

// ---------- U2: per-process CPU state save/restore orchestrators ----------

/// Capture the live CPU's per-process state (FS_BASE + FPU/SSE) into
/// `p`. Called by U4's ring-3 switch primitive on switch-out, after the
/// trap-frame GPRs have already been copied into `p.saved_user_state`.
///
/// Must be called from CPL=0 with the CPU still running on `p`'s state
/// — i.e., before any kernel code below this has clobbered FS_BASE or
/// touched XMM. Today the kernel never touches XMM (the target spec
/// carries `+soft-float`), so the timing window is generous; if a
/// future kernel routine emits SSE, the save must move earlier in the
/// switch path.
pub fn save_user_cpu_state(p: &mut Process) {
    p.fs_base = crate::arch::x86_64::msr::read_fs_base();
    crate::arch::x86_64::fpu::save_fpu(&mut p.fpu_state);
}

/// Reload the per-process CPU state (FS_BASE + FPU/SSE) from `p`.
/// Called by U4's ring-3 switch primitive on switch-in, before
/// constructing the iretq frame.
///
/// Must be called from CPL=0. The CR3 swap to `p`'s address space
/// happens separately (via [`crate::userland::address_space::AddressSpace::activate`])
/// — this function only touches MSRs and the FPU register file.
pub fn restore_user_cpu_state(p: &Process) {
    // U8/diagnostic: log when restoring to a non-user FS_BASE — that
    // would indicate Process.fs_base was set wrong somewhere.
    if p.fs_base != 0
        && !(crate::mm::paging::USER_VA_RANGE_START..crate::mm::paging::USER_VA_RANGE_END)
            .contains(&p.fs_base)
    {
        crate::debug_error!(
            "restore_user_cpu_state(pid={}): FS_BASE {:#x} is NOT in user VA range — likely corruption",
            p.pid, p.fs_base
        );
    }
    crate::arch::x86_64::msr::set_fs_base(p.fs_base);
    crate::arch::x86_64::fpu::restore_fpu(&p.fpu_state);
}

/// Test-only: drain `ring3_ready` and `ring3_blocked`. Used by
/// `PreemptTestGuard::drop` in `src/tests/userland_switch.rs` so a
/// test that pushes synthetic PIDs into the queues and then panics
/// (or simply forgets to clean up) doesn't poison subsequent tests.
#[cfg(feature = "test")]
pub fn clear_ring3_queues_for_test() {
    let mut g = PROCESS_TABLE.lock();
    g.ring3_ready.clear();
    g.ring3_blocked.clear();
}

/// U5: timer-ISR ring-3 preempt decision.
///
/// Called from `timer_handler_inner` when the timer fires from CPL=3.
/// Under a single `PROCESS_TABLE.try_lock()`:
///
/// 1. Read `current_user_pid`. If `None` or `KERNEL_PID`, no-op.
/// 2. Pop the front of `ring3_ready`. If empty, no-op.
/// 3. If popped pid equals `cur`, re-queue and no-op (nothing else
///    to switch to).
/// 4. Otherwise: snapshot `cur`'s GPRs + RIP + RFLAGS + RSP from
///    `frame` into `cur`.saved_user_state, capture FS_BASE + FPU
///    via `save_user_cpu_state`, push `cur` to the back of
///    ring3_ready (round-robin), and return `Some(next_pid)`.
///
/// Returns `Some(next_pid)` to signal "caller should
/// [`crate::userland::switch::resume_ring3`] this pid"; returns
/// `None` to signal "fall through and iretq back to the current
/// ring-3 process."
///
/// `try_lock` discipline matches the existing scheduler pattern: if
/// the lock is contended (which shouldn't happen on single-CPU
/// because syscall handlers run with IF=0 per the FMASK MSR, but
/// matters for SMP-forward-compat), skip this preempt opportunity
/// and let ring 3 continue. The next tick retries.
pub fn try_preempt_ring3(
    frame: &crate::arch::x86_64::preemption::InterruptStackFrame,
) -> Option<u32> {
    let mut g = PROCESS_TABLE.try_lock()?;
    let cur = g.current_user_pid?;
    if cur == KERNEL_PID {
        return None;
    }

    let next = g.ring3_ready.pop_front()?;
    if next == cur {
        // Front of queue was us; nothing to switch to. Re-queue to
        // preserve FIFO order.
        g.ring3_ready.push_back(next);
        return None;
    }

    // Snapshot `cur`'s state. The frame's CPU-pushed RIP/CS/RFLAGS/RSP
    // describe ring 3's interrupted instruction; the GPRs were pushed
    // by the naked timer-ISR prologue. After this returns,
    // `cur.saved_user_state` is a complete iretq-ready snapshot.
    let cur_p = g.by_pid.get_mut(&cur)?;
    crate::userland::switch::save_ring3(cur_p, frame);

    // Round-robin: cur goes to the back of the queue so the next
    // preempt of `next` picks cur. `mark_ring3_ready` would re-take
    // the lock; inline the push since we already hold it.
    if !g.ring3_ready.iter().any(|p| *p == cur) {
        g.ring3_ready.push_back(cur);
    }

    // current_user_pid is not flipped here — `resume_ring3` does
    // that atomically with the CR3 / TSS.rsp0 / GSBASE side-effects.
    drop(g);
    Some(next)
}

/// Save the currently running ring-3 process and queue it for a later
/// dispatch from the kernel main loop.
///
/// The ring-3 timer path uses this periodically even when user processes are
/// runnable. Without that class-level handoff, direct ring3-to-ring3 switches
/// can monopolize the single CPU forever: a shell polling `wait3(WNOHANG)`
/// while its child sleeps for network input starves the network worker and
/// compositor that are needed to make either process progress.
pub fn preempt_ring3_to_kernel(
    frame: &crate::arch::x86_64::preemption::InterruptStackFrame,
) -> bool {
    let Some(mut g) = PROCESS_TABLE.try_lock() else {
        return false;
    };
    let Some(cur) = g.current_user_pid else {
        return false;
    };
    if cur == KERNEL_PID {
        return false;
    }
    let Some(process) = g.by_pid.get_mut(&cur) else {
        return false;
    };
    crate::userland::switch::save_ring3(process, frame);
    if !g.ring3_ready.iter().any(|pid| *pid == cur) {
        g.ring3_ready.push_back(cur);
    }
    // No ring-3 address space is considered loaded after the caller switches
    // to KERNEL_CONTEXT. Clearing this under the same lock keeps the ready
    // queue and current marker coherent for the main-loop dispatcher.
    g.current_user_pid = None;
    true
}

/// Allocate a fresh PID. Used by `enter_user_mode_with` and (future)
/// `fork`.
pub fn alloc_pid() -> u32 {
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

/// Variant that accepts an optional address space. The kernel-test
/// path bypasses the run command and runs binaries on the kernel L4
/// directly, passing `None`; production launches always pass `Some`.
pub fn install_new_process_opt(
    image: UserImage,
    brk_base: u64,
    mmap_base: u64,
    address_space: Option<AddressSpace>,
) -> u32 {
    let pid = alloc_pid();
    let stack_top = image.stack_top.as_u64();
    let stack_initial_bottom = image.stack_initial_bottom;
    let stack_max_growth_floor = image.stack_max_growth_floor;
    let mut fd_table = FdTable::new();
    fd_table.install_default_streams();
    let mut p = Process {
        pid,
        parent_pid: KERNEL_PID,
        image: Some(image),
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: brk_base,
        brk_base,
        mmap_next: mmap_base,
        fd_table,
        network_wait: None,
        real_timer: RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: String::from("/host"),
        address_space,
        signal_state: SignalState::new(),
        kernel_stack: Some(KernelStack::new()),
        exe_path: None,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::fresh(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        // Filled in by `setup_user_process` from the launching kernel
        // thread's PCB.
        terminal_id: None,
    };
    // Demand-grown stack (U3): install the loader-computed window.
    // `stack_bottom == stack_mapped_bottom == initial_bottom` at
    // install time — the loader has mapped exactly those pages.
    // The fault handler (U4) lowers both fields together on every
    // successful growth. Full growth budget at install.
    p.set_stack_window(
        stack_top,
        stack_initial_bottom,
        stack_initial_bottom,
        stack_max_growth_floor,
        crate::mm::paging::USER_STACK_MAX_GROWTH_PAGES,
    );
    let mut g = PROCESS_TABLE.lock();
    g.by_pid.insert(pid, p);
    g.current_user_pid = Some(pid);
    pid
}

impl Process {
    /// Install the loader-computed stack window. Called from the
    /// install path (U3) once `install_new_process_opt` has populated
    /// the rest of the slot.
    ///
    /// `mapped_bottom` always equals `bottom` at install time (the
    /// loader maps exactly the initial commit). Later, the ring-3
    /// page-fault handler (U4) updates both fields on each successful
    /// growth.
    pub fn set_stack_window(
        &mut self,
        top: u64,
        bottom: u64,
        mapped_bottom: u64,
        max_growth_floor: u64,
        growth_faults_remaining: u64,
    ) {
        self.stack_top = top;
        self.stack_bottom = bottom;
        self.stack_mapped_bottom = mapped_bottom;
        self.stack_max_growth_floor = max_growth_floor;
        self.growth_faults_remaining = growth_faults_remaining;
    }
}

/// Outcome of a `try_grow_user_stack` call. The page-fault handler
/// (`src/arch/x86_64/interrupts.rs::page_fault_handler`) inspects this
/// to decide whether to return immediately (Grew) or fall through to
/// `cleanup_user_process` with vector 14 / SIGSEGV.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowOutcome {
    /// Faulting address mapped one fresh page; CPU retries the
    /// instruction.
    Grew,
    /// Faulting address is not in the stack-grow window (above the
    /// current bottom, or above stack_top). The fault is something else
    /// — let the standard fault path handle it.
    NotStackGrow,
    /// Faulting address is below the per-process growth floor — true
    /// stack overflow.
    Overflow,
    /// Per-process growth budget exhausted. Defends against a malicious
    /// binary fault-storming the window to chew through the bump
    /// allocator.
    BudgetExhausted,
    /// `PROCESS_TABLE.try_lock()` returned `None`. Some other path
    /// already holds the mutex (shouldn't happen in single-app-
    /// synchronous mode, but defensive: a blocking `lock()` from
    /// interrupt context would deadlock). Treated as overflow — the
    /// process is in an unrecoverable state anyway.
    LockContended,
    /// `map_user_region` failed (OOM or invalid range). Treated as
    /// overflow.
    MapFailed,
}

/// Test-visible cell holding the most recent `GrowOutcome` from the
/// fault handler. Tests assert on this rather than parsing serial.
#[cfg(feature = "test")]
pub static LAST_GROW_OUTCOME: spin::Mutex<Option<GrowOutcome>> = spin::Mutex::new(None);

/// Ring-3 page-fault hook: if `fault_addr` falls in the active
/// process's stack-grow window and the per-process budget allows,
/// map a single fresh page and update the bookkeeping. Otherwise
/// classify the fault for the caller.
///
/// The caller is `page_fault_handler` in `src/arch/x86_64/interrupts.rs`
/// — it invokes this for every ring-3 fault before routing to
/// `cleanup_user_process`. On `Grew` the handler returns immediately;
/// on every other outcome it falls through to cleanup.
pub fn try_grow_user_stack(fault_addr: x86_64::VirtAddr) -> GrowOutcome {
    use crate::mm::paging::UserPerms;

    // U9 invariant: a ring-3 page fault implies CR3 is the faulting
    // process's L4, which (by U5's `resume_ring3` atomic swap)
    // implies `current_user_pid` points at the faulting process.
    // `try_grow_user_stack` reads the stack window from
    // `current_user_pid`'s Process — that's correctly the faulting
    // process under all U5+ flows. The kernel-mode fall-through
    // (sentinel) handles synthetic test paths and pre-launch
    // kernel-page-faults.

    // Classification + mutation happens under the Process lock. We
    // drop the lock before calling map_user_region — the mapper is a
    // separate Mutex and nesting interrupt-context locks invites
    // deadlock if any future code path takes them in the opposite
    // order. Re-acquire after map to update bookkeeping.
    let (new_page, _stack_top, _stack_max_growth_floor) = {
        let mut guard = match PROCESS_TABLE.try_lock() {
            Some(g) => g,
            None => {
                #[cfg(feature = "test")]
                {
                    *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::LockContended);
                }
                return GrowOutcome::LockContended;
            }
        };
        ensure_sentinel(&mut guard);
        // Fall back to the sentinel slot (PID 0) when no ring-3 process
        // is loaded — its zero-valued stack fields hit the existing
        // `stack_top == 0` early return below and the caller routes to
        // the normal fault path. The sentinel is also where test
        // helpers (stage_stack_window) stage synthetic stack windows.
        let cur_pid = guard.current_user_pid.unwrap_or(KERNEL_PID);
        let p = guard
            .by_pid
            .get_mut(&cur_pid)
            .expect("sentinel invariant violated");

        let addr = fault_addr.as_u64();
        // No active process (sentinel slot) — stack fields are zero
        // and any compare against them is meaningless. Treat as
        // not-a-stack-grow so the caller routes to its normal path.
        if p.stack_top == 0 || p.stack_max_growth_floor == 0 {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::NotStackGrow);
            }
            return GrowOutcome::NotStackGrow;
        }

        // Out of the stack VA range — fault is in heap/mmap/code,
        // someone else's problem.
        if addr >= p.stack_top {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::NotStackGrow);
            }
            return GrowOutcome::NotStackGrow;
        }
        // Below the growth floor — true overflow.
        if addr < p.stack_max_growth_floor {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::Overflow);
            }
            return GrowOutcome::Overflow;
        }
        // Budget exhausted — fault-storm defense.
        if p.growth_faults_remaining == 0 {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::BudgetExhausted);
            }
            return GrowOutcome::BudgetExhausted;
        }

        // Note: we don't reject `addr >= stack_bottom` — the
        // initially-mapped pages plus prior grows may have left gaps
        // (e.g., a fork()-child inherits the parent's mapping but
        // libc then writes to a page above its grown floor but
        // below the initial mapping). Inside [stack_max_growth_floor,
        // stack_top), any unmapped page is fair game. If the page is
        // ALREADY mapped, map_user_region will return PageAlreadyMapped
        // and we'll fall through to NotStackGrow below (treating the
        // fault as a permissions violation, not absence).
        let new_page = addr & !0xFFF;
        (new_page, p.stack_top, p.stack_max_growth_floor)
        // guard drops here — release Process lock before taking mapper lock.
    };

    // Map one page R+W under the active address space.
    let map_result = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(x86_64::VirtAddr::new(new_page), 1, UserPerms::ReadWrite)
    });
    match map_result {
        Some(Ok(_)) => {}
        // The page was already mapped — the fault must have been a
        // permissions violation, not absence. Fall through to the
        // standard fault path which will surface SIGSEGV.
        Some(Err(crate::mm::paging::UserMapError::PageAlreadyMapped)) => {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::NotStackGrow);
            }
            return GrowOutcome::NotStackGrow;
        }
        _ => {
            #[cfg(feature = "test")]
            {
                *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::MapFailed);
            }
            return GrowOutcome::MapFailed;
        }
    }

    // Re-acquire Process lock to update bookkeeping. The new page MAY
    // be above the current stack_bottom (gap-filling within the
    // initial-mapped region's footprint, see the long comment above).
    // Only lower stack_bottom / stack_mapped_bottom when the new page
    // actually extends the contiguous low-water mark — otherwise
    // unmap_user_stack would walk a range that includes already-mapped
    // pages it doesn't own.
    if let Some(mut guard) = PROCESS_TABLE.try_lock() {
        ensure_sentinel(&mut guard);
        let cur_pid = guard.current_user_pid.unwrap_or(KERNEL_PID);
        if let Some(p) = guard.by_pid.get_mut(&cur_pid) {
            if new_page < p.stack_bottom {
                p.stack_bottom = new_page;
                p.stack_mapped_bottom = new_page;
            }
            p.growth_faults_remaining = p.growth_faults_remaining.saturating_sub(1);
        }
    } else {
        crate::debug_warn!(
            "try_grow_user_stack: re-acquire failed, single-frame leak at {:#x}",
            new_page
        );
    }

    // Widen syscall validated bounds so the freshly mapped page is
    // accepted by validate_user_slice. We narrow bounds.start to the
    // new_page; bounds.end stays. If user_va_bounds is None (test
    // context), this is a no-op.
    if let Some(mut b) = crate::userland::abi::user_va_bounds() {
        if new_page < b.start {
            b.start = new_page;
            crate::userland::abi::set_user_va_bounds(b);
        }
    }

    crate::debug_trace!("stack grew to {:#x}", new_page);

    #[cfg(feature = "test")]
    {
        *LAST_GROW_OUTCOME.lock() = Some(GrowOutcome::Grew);
    }

    GrowOutcome::Grew
}

/// Unmap the user stack range a process owns and zero its stack-window
/// fields. Called from `cleanup_user_process` and `cooperative_exit`
/// (U4) so grown stack pages are released regardless of which exit path
/// runs.
///
/// No-op when the slot is the kernel sentinel (stack fields all `0`)
/// or when teardown has already run.
pub fn unmap_user_stack(p: &mut Process) {
    if p.stack_top == 0 || p.stack_mapped_bottom == 0 {
        return;
    }
    if p.stack_mapped_bottom >= p.stack_top {
        // Defensive: nothing mapped, or fields inconsistent. Zero and
        // return rather than asking the mapper for a zero-page unmap.
        p.stack_top = 0;
        p.stack_bottom = 0;
        p.stack_mapped_bottom = 0;
        p.stack_max_growth_floor = 0;
        p.growth_faults_remaining = 0;
        if let Some(img) = p.image.as_mut() {
            img.stack_initial_bottom = 0;
        }
        return;
    }
    let page_count = (p.stack_top - p.stack_mapped_bottom) / 0x1000;
    let _ = crate::mm::memory::with_memory_mapper(|m| {
        m.unmap_user_region(x86_64::VirtAddr::new(p.stack_mapped_bottom), page_count)
    });
    // Tell `UserImage::Drop` we already handled the stack so it doesn't
    // try to unmap the (now unmapped) initial commit again.
    if let Some(img) = p.image.as_mut() {
        img.stack_initial_bottom = 0;
    }
    p.stack_top = 0;
    p.stack_bottom = 0;
    p.stack_mapped_bottom = 0;
    p.stack_max_growth_floor = 0;
    p.growth_faults_remaining = 0;
}

/// Returns the PID of the currently-loaded ring-3 process, or
/// `KERNEL_PID` (0) when none is loaded. Convenience wrapper around
/// [`current_user_pid`] for the long tail of callers that want a `u32`
/// directly (matches pre-PR-C return type).
pub fn current_pid() -> u32 {
    current_user_pid().unwrap_or(KERNEL_PID)
}

// ---------- zombie filing + SIGCHLD on parent ----------

/// Notify the parent that a forked child has exited cooperatively (via
/// `exit_group`): file the zombie so `wait4` finds it, raise SIGCHLD
/// on the parent's entry in PROCESS_TABLE, and wake the parent if it's
/// blocked in `wait4`. No-op when `parent_pid == 0` (top-level binary
/// launched by the run command — no userland parent).
pub fn notify_parent_of_exit(pid: u32, parent_pid: u32, exit_code: i64) {
    if parent_pid == 0 {
        return;
    }
    record_zombie(pid, parent_pid, exit_code);
    // U7: raise SIGCHLD on the parent in PROCESS_TABLE (formerly the
    // PARENT_STASH slot). No-op if the parent has already exited.
    let _ = with_process(parent_pid, |parent| {
        parent.signal_state.raise(crate::userland::signal::SIGCHLD);
    });
    // U3: wake the parent if it's blocked in `wait4`. Load-bearing
    // under U7: parent may now `wait4` before the child exits.
    wake_ring3_blocked_on_child(parent_pid, pid);
}

/// Same as [`notify_parent_of_exit`] for children killed by a signal
/// (currently ring-3 faults). The zombie is filed with the
/// signal number so `wait4_handler` can emit a POSIX-correct
/// `WIFSIGNALED` status word — without this, the parent's
/// `wait4` would see `WIFEXITED` with `WEXITSTATUS = 128 + signum`,
/// which is the shell-convention exit code, NOT the POSIX wait status
/// (so `zsh` never prints "Segmentation fault", etc.).
pub fn notify_parent_of_signaled_exit(pid: u32, parent_pid: u32, signum: i32, exit_code: i64) {
    if parent_pid == 0 {
        return;
    }
    record_zombie_signaled(pid, parent_pid, signum, exit_code);
    // U7: SIGCHLD on the parent in PROCESS_TABLE — see
    // `notify_parent_of_exit`.
    let _ = with_process(parent_pid, |parent| {
        parent.signal_state.raise(crate::userland::signal::SIGCHLD);
    });
    // U3/U7: wake the parent if it's blocked in `wait4`.
    wake_ring3_blocked_on_child(parent_pid, pid);
}

/// Record of a child that exited but hasn't been reaped yet.
///
/// `signal_termination = Some(sig)` means the child died by signal `sig`
/// (currently a fault path); `exit_code` is the
/// shell-style `128 + sig` mirror but `wait4` MUST emit the
/// `WIFSIGNALED` encoding, not the `WIFEXITED` encoding. `None` means
/// the child called `exit_group(code)` cooperatively.
#[derive(Debug, Clone, Copy)]
pub struct ZombieRecord {
    pub exit_code: i64,
    pub parent_pid: u32,
    pub signal_termination: Option<i32>,
}

static ZOMBIES: Mutex<BTreeMap<u32, ZombieRecord>> = Mutex::new(BTreeMap::new());

/// Mark `pid` as a cooperatively-exited zombie awaiting reap. Used by
/// `_exit` / `exit_group` when the dying process has a real parent.
pub fn record_zombie(pid: u32, parent_pid: u32, exit_code: i64) {
    ZOMBIES.lock().insert(
        pid,
        ZombieRecord {
            exit_code,
            parent_pid,
            signal_termination: None,
        },
    );
}

/// Mark `pid` as a signal-killed zombie awaiting reap. Used by the
/// abnormal-exit and unimplemented-syscall paths; `exit_code` retains
/// the `128 + signum` shell convention for the rare consumer that
/// reads it directly, but `wait4_handler` keys off `signal_termination`
/// for the POSIX status word.
pub fn record_zombie_signaled(pid: u32, parent_pid: u32, signum: i32, exit_code: i64) {
    ZOMBIES.lock().insert(
        pid,
        ZombieRecord {
            exit_code,
            parent_pid,
            signal_termination: Some(signum),
        },
    );
}

/// Reap a zombie child. If `target_pid` is positive, only that PID
/// matches; if `target_pid == -1` (any-child semantics), the first
/// zombie with `parent_pid == reaper` is returned. Returns `(pid,
/// exit_code, signal_termination)` on success, `None` if no matching
/// zombie exists.
pub fn reap_zombie(target_pid: i32, reaper: u32) -> Option<(u32, i64, Option<i32>)> {
    let mut zombies = ZOMBIES.lock();
    let pid = if target_pid == -1 {
        zombies
            .iter()
            .find(|(_, z)| z.parent_pid == reaper)
            .map(|(&k, _)| k)?
    } else if target_pid > 0 {
        let key = target_pid as u32;
        let z = zombies.get(&key)?;
        if z.parent_pid != reaper {
            return None;
        }
        key
    } else {
        return None;
    };
    let z = zombies.remove(&pid)?;
    drop(zombies);

    // U7: a forked child's Process slot stays in PROCESS_TABLE after
    // exit (its kernel_stack was the one we were executing on at exit
    // time, so the exit path couldn't drop it from there). Reaping
    // is the safe drop point — the reaper is the parent, running on
    // the parent's kernel stack; dropping the child's Process here
    // releases its AddressSpace, KernelStack, fd_table, etc.
    //
    // No-op for top-level kernel-launched binaries (their Process was
    // removed at long-jump time via the legacy release_active_image
    // path), and no-op for the sentinel.
    let _ = remove_process(pid);

    Some((pid, z.exit_code, z.signal_termination))
}

/// Counts the number of active user-binary load/setup transactions. The
/// kernel main loop reads this to skip GUI/render work while the run command
/// is reading the ELF, mapping pages, initializing VMAs, and installing the
/// process — the phases that pre-date the active-user slot being filled.
///
/// Use `BinaryLoadGuard` to bracket the run path; it increments the counter
/// on construction and decrements on drop so early-return / panic paths still
/// release the guard.
static BINARY_LOAD_DEPTH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Returns true while a binary is being loaded and installed. The guard drops
/// before the new process runs so compositor rendering continues normally for
/// interactive programs.
pub fn binary_load_in_progress() -> bool {
    BINARY_LOAD_DEPTH.load(core::sync::atomic::Ordering::Acquire) > 0
}

/// RAII guard that marks the kernel as actively loading-or-running a user
/// binary. Construct on entry to the run path; drop releases the marker even
/// if an intermediate step bails out.
pub struct BinaryLoadGuard;

impl BinaryLoadGuard {
    pub fn enter() -> Self {
        BINARY_LOAD_DEPTH.fetch_add(1, core::sync::atomic::Ordering::Release);
        Self
    }
}

impl Drop for BinaryLoadGuard {
    fn drop(&mut self) {
        BINARY_LOAD_DEPTH.fetch_sub(1, core::sync::atomic::Ordering::Release);
    }
}

/// Helper used by exception handlers: returns true when the saved CS in the
/// interrupt frame indicates the fault occurred at ring 3 (CPL=3 / RPL=3).
#[inline]
pub fn frame_is_user(code_segment: u64) -> bool {
    (code_segment & 3) == 3
}

/// Tear down the active user process and long-jump to the saved kernel
/// continuation. **Diverges**: control never returns to the caller (the
/// faulting interrupt handler or the `exit` syscall dispatcher).
///
/// Order of operations:
/// 1. Record the exit reason on the active-user slot (the run command logs it
///    after the long-jump).
/// 2. Clear the active syscall pointer-validation bounds (no user pointers
///    are valid after this).
/// 3. Take the continuation. If somehow not present (no `enter_user_mode`
///    ever ran), fall back to halting.
/// 4. `restore_continuation(cont)` — naked asm jump back to the run command.
///
/// We do NOT drop the `UserImage` here. The image is dropped by the run
/// command after the long-jump returns control — that drop sequence runs in
/// a normal Rust frame, not in interrupt context, which is the right place
/// to walk the mappings list and call back into the memory mapper.
pub fn cleanup_user_process(reason: AbnormalExit) -> ! {
    use crate::debug_error;

    let live_fs_base = crate::arch::x86_64::msr::read_fs_base();
    let (live_cr3, _) = x86_64::registers::control::Cr3::read();
    let cur_pid = current_user_pid();
    let (proc_fs_base, parent_pid) = with_current_process(|p| (p.fs_base, p.parent_pid));

    debug_error!(
        "USERLAND: ring-3 fault — vector={}, error_code={:?}, fault_addr={:?}, rip={:?}",
        reason.vector,
        reason.error_code,
        reason.fault_addr,
        reason.fault_rip
    );
    debug_error!(
        "USERLAND: fault context — current_user_pid={:?}, parent_pid={}, live_fs_base={:#x}, proc_fs_base={:#x}, cr3_frame={:#x}",
        cur_pid,
        parent_pid,
        live_fs_base,
        proc_fs_base,
        live_cr3.start_address().as_u64(),
    );

    // Diagnostic: cross-check the active CR3 against what PROCESS_TABLE
    // says this process's L4 should be. A mismatch points at a
    // resume_ring3 / current_user_pid bookkeeping race.
    if let Some(pid) = cur_pid {
        let expected_l4 = with_process(pid, |p| {
            p.address_space
                .as_ref()
                .map(|a| a.l4_frame().start_address().as_u64())
        })
        .flatten();
        match expected_l4 {
            Some(exp) if exp != live_cr3.start_address().as_u64() => {
                debug_error!(
                    "USERLAND: CR3 MISMATCH — Process[{}].address_space.l4 = {:#x}, but live CR3 = {:#x}",
                    pid,
                    exp,
                    live_cr3.start_address().as_u64(),
                );
            }
            Some(exp) => {
                debug_error!(
                    "USERLAND: CR3 OK — Process[{}].address_space.l4 = {:#x} matches live CR3",
                    pid,
                    exp,
                );
            }
            None => {
                debug_error!("USERLAND: Process[{}] has no AddressSpace", pid);
            }
        }
    }

    // Diagnostic: walk the live CR3's page tables for the faulting
    // address (or for the RIP if no fault address was reported, e.g.,
    // a #GP). Logs each level's raw u64 entry so we can tell PRESENT
    // vs not-present vs permission issues from the boot log.
    let walk_target = reason.fault_addr.unwrap_or(reason.fault_rip);
    log_page_table_walk(live_cr3.start_address().as_u64(), walk_target);

    // Demand-grown stack (U4): release the [stack_mapped_bottom,
    // stack_top) range before UserImage::Drop runs. unmap_user_stack
    // also clears the image's stack_initial_bottom so Drop skips a
    // duplicate stack unmap.
    with_current_process(unmap_user_stack);

    // Shell convention: keep `exit_code = 128 + signum` for any
    // consumer that reads `exit_code` directly (the run command's log,
    // tests). The parent's wait4 path now uses the POSIX
    // WIFSIGNALED encoding via `notify_parent_of_signaled_exit` so
    // shells (zsh) print "Segmentation fault" instead of treating 139
    // as a normal exit status.
    let signum = signum_for_vector(reason.vector);
    let exit_code = 128 + signum as i64;

    record_exit(
        ExitKind::Abnormal {
            vector: reason.vector,
            fault_rip: reason.fault_rip.as_u64(),
            fault_addr: reason.fault_addr.map(|a| a.as_u64()),
        },
        exit_code,
    );

    let (pid, parent_pid) = with_current_process(|p| (p.pid, p.parent_pid));
    notify_parent_of_signaled_exit(pid, parent_pid, signum, exit_code);

    long_jump_to_run_or_halt();
}

/// Walk the live page tables (rooted at `cr3_pa`) for `va` and log each
/// level's entry. Reads through the bootloader's physical-memory offset
/// alias. Pure diagnostic — no mutation, no faulting.
pub fn log_page_table_walk(cr3_pa: u64, va: VirtAddr) {
    use crate::debug_error;

    let va_u = va.as_u64();
    let pml4_idx = ((va_u >> 39) & 0x1FF) as usize;
    let pdpt_idx = ((va_u >> 30) & 0x1FF) as usize;
    let pd_idx = ((va_u >> 21) & 0x1FF) as usize;
    let pt_idx = ((va_u >> 12) & 0x1FF) as usize;

    let phys_offset = match crate::mm::memory::get_physical_memory_offset() {
        Some(o) => o,
        None => {
            debug_error!("USERLAND: page walk skipped — no physical memory offset");
            return;
        }
    };

    debug_error!(
        "USERLAND: page walk for VA={:#x} (pml4={}, pdpt={}, pd={}, pt={}), CR3={:#x}",
        va_u,
        pml4_idx,
        pdpt_idx,
        pd_idx,
        pt_idx,
        cr3_pa,
    );

    // SAFETY: cr3_pa is the live CR3 frame; the bootloader maps all
    // physical memory at `phys_offset`. We read 8 bytes per level. No
    // writes, no dereferences past the entries we walk.
    unsafe {
        let pml4 = (phys_offset + cr3_pa) as *const u64;
        let pml4e = core::ptr::read(pml4.add(pml4_idx));
        debug_error!(
            "  PML4[{}] = {:#018x} (present={}, us={}, rw={}, nx={})",
            pml4_idx,
            pml4e,
            pml4e & 1 != 0,
            pml4e & (1 << 2) != 0,
            pml4e & (1 << 1) != 0,
            pml4e & (1 << 63) != 0,
        );
        if pml4e & 1 == 0 {
            return;
        }
        let pdpt_pa = pml4e & 0x000F_FFFF_FFFF_F000;

        let pdpt = (phys_offset + pdpt_pa) as *const u64;
        let pdpte = core::ptr::read(pdpt.add(pdpt_idx));
        debug_error!(
            "  PDPT[{}] = {:#018x} (present={}, us={}, rw={}, nx={}, huge={})",
            pdpt_idx,
            pdpte,
            pdpte & 1 != 0,
            pdpte & (1 << 2) != 0,
            pdpte & (1 << 1) != 0,
            pdpte & (1 << 63) != 0,
            pdpte & (1 << 7) != 0,
        );
        if pdpte & 1 == 0 {
            return;
        }
        if pdpte & (1 << 7) != 0 {
            return;
        } // 1 GiB page
        let pd_pa = pdpte & 0x000F_FFFF_FFFF_F000;

        let pd = (phys_offset + pd_pa) as *const u64;
        let pde = core::ptr::read(pd.add(pd_idx));
        debug_error!(
            "  PD[{}]   = {:#018x} (present={}, us={}, rw={}, nx={}, huge={})",
            pd_idx,
            pde,
            pde & 1 != 0,
            pde & (1 << 2) != 0,
            pde & (1 << 1) != 0,
            pde & (1 << 63) != 0,
            pde & (1 << 7) != 0,
        );
        if pde & 1 == 0 {
            return;
        }
        if pde & (1 << 7) != 0 {
            return;
        } // 2 MiB page
        let pt_pa = pde & 0x000F_FFFF_FFFF_F000;

        let pt = (phys_offset + pt_pa) as *const u64;
        let pte = core::ptr::read(pt.add(pt_idx));
        debug_error!(
            "  PT[{}]   = {:#018x} (present={}, us={}, rw={}, nx={})",
            pt_idx,
            pte,
            pte & 1 != 0,
            pte & (1 << 2) != 0,
            pte & (1 << 1) != 0,
            pte & (1 << 63) != 0,
        );
    }
}

/// Map an x86 exception vector to the POSIX signal a Linux kernel would
/// deliver for that fault. Used by the abnormal-exit path to surface a
/// distinguishable exit code via the `128 + signum` shell convention.
fn signum_for_vector(vector: u8) -> i32 {
    use crate::userland::signal::{SIGBUS, SIGFPE, SIGILL, SIGSEGV};
    match vector {
        0 | 16 | 19 => SIGFPE, // #DE divide-by-zero, #MF x87, #XM SIMD
        6 => SIGILL,           // #UD invalid opcode
        17 => SIGBUS,          // #AC alignment check
        13 | 14 => SIGSEGV,    // #GP general protection, #PF page fault
        _ => SIGSEGV,          // conservative default
    }
}

/// Cooperative-exit path — invoked from the `exit` syscall handler.
/// Same teardown as `cleanup_user_process`, with `ExitKind::Cooperative`.
pub fn cooperative_exit(code: i64) -> ! {
    record_exit(ExitKind::Cooperative, code);
    // Demand-grown stack (U4): release [stack_mapped_bottom, stack_top).
    with_current_process(unmap_user_stack);
    long_jump_to_run_or_halt();
}

fn record_exit(kind: ExitKind, code: i64) {
    with_current_process(|p| {
        // Only record if not already terminated (defensive: a second fault
        // from an already-failing app would otherwise overwrite the original
        // reason).
        if matches!(p.exit_kind, ExitKind::None) {
            p.exit_kind = kind;
            p.exit_code = code;
        }
    });
}

fn long_jump_to_run_or_halt() -> ! {
    // U8: ring-3 process exit unified through the scheduler-block
    // model. Both top-level kernel-launched binaries and forked
    // children leave their `Process` slot in PROCESS_TABLE (its
    // kernel_stack is what we're executing on; we can't drop it from
    // here). Cleanup happens later:
    //   - Top-level binary: `enter_user_mode_with_aspace` is woken
    //     via `wake_threads_waiting_for_ring3_exit`, returns,
    //     `remove_process` drops the slot.
    //   - Forked child: parent's `wait4` reap path calls
    //     `remove_process`.
    //
    // We do NOT clear `user_va_bounds`: bounds describe the VA layout
    // shared by all ring-3 processes, and any other ring-3 process
    // still alive expects them in place.

    // Find the current ring-3 PID so we can wake the launching kernel
    // thread blocked on its exit. If no current_user_pid is set, this
    // is a synthetic test path; no kernel thread to wake.
    let exiting_pid = current_user_pid();
    if let Some(pid) = exiting_pid {
        // GUI resources are process-owned and must disappear at death, not
        // only when a parent or launcher eventually reaps the zombie slot.
        // `remove_process` repeats this cleanup as an idempotent backstop.
        crate::userland::gui::cleanup_process(pid);
        // Wake any kernel thread blocked on this process's exit. The
        // thread's `block_kernel_thread_for_ring3_exit` returns when
        // scheduler picks it; it reads exit_kind/exit_code from
        // PROCESS_TABLE and removes the Process.
        let mut sched = crate::process::scheduler::SCHEDULER.lock();
        sched.wake_threads_waiting_for_ring3_exit(pid);
        drop(sched);
        // Clear current_user_pid — we're no longer running this
        // process. The next ring-3 dispatch (via resume_ring3) will
        // set it to the next pid.
        set_current_user_pid(None);
    }

    // Yield to the next runnable ring-3 process if any (e.g., a
    // freshly-woken wait4-parent). Otherwise yield to the kernel
    // main loop, which gives the kernel-thread scheduler a chance to
    // run the woken launcher thread (or any other Ready kernel
    // thread).
    if let Some(next) = pop_next_ring3() {
        unsafe {
            crate::userland::switch::resume_ring3(next);
        }
        // resume_ring3 diverges; unreachable.
    }

    unsafe {
        crate::userland::switch::yield_to_kernel_main_loop();
    }
}
