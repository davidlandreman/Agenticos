# `src/userland/` — Ring-3 process platform

Everything that turns an ELF on disk into a running ring-3 process: the
loader, the kernel-side ABI (Linux x86-64 syscall surface), the
fork/exec/wait machinery, the per-process address space and kernel
stack, signal delivery, and the switch primitives the multi-ring-3
scheduler builds on. The kernel-thread side (the scheduler itself,
preemptive timer ISR, kernel `Process` PCB) lives next door in
`src/process/`; this folder owns everything specific to ring 3.

## Key files

- `loader.rs` — minimal ELF64 loader. Reads PT_LOAD segments off the
  filesystem, allocates user pages, populates the page tables.
- `image.rs` — `UserImage`: ELF entry/stack/TLS/program-header metadata;
  production mapping ownership transfers to `AddressSpace` during setup.
- `address_space.rs` — `AddressSpace`: owns a fresh L4, every private user
  page-table subtree, and the authoritative VMA set. Drop performs targeted
  teardown even when another CR3 is active.
- `vm.rs` — sorted non-overlapping VMAs, split/merge/protect/remove, and
  reusable top-down gap search.
- `usercopy.rs` — VMA-aware reads/writes that page in lazy buffers and
  resolve COW before kernel writes.
- `kernel_stack.rs` — per-process 64 KiB kernel stack. TSS.rsp0 +
  GSBASE-stored SYSCALL rsp top are pointed at this when the process
  is current.
- `lifecycle.rs` — `Process` PCB, `ProcessTable`, the
  install/teardown flow, zombie filing, the demand-grown user-stack
  page-fault hook, FPU/FS_BASE save/restore orchestrators, and the
  legacy `KernelContinuation` setjmp/longjmp scaffolding used by
  today's launch + fork paths.
- `mod.rs` — `enter_user_mode_with_aspace` (top-level launcher:
  installs Process, marks Ready, blocks launcher kernel thread) and
  `iretq_to_user_with_regs` (same-process iretq with caller-supplied
  UserState — used by execve, rt_sigreturn, signal-delivery).
- `switch.rs` — U4: `save_ring3` and `resume_ring3` for the
  multi-ring-3 scheduler. **Not wired into the timer ISR yet** —
  consumed by U5 onward.
- `syscalls.rs` — every kernel-side syscall handler. ~5k lines;
  fork/wait4/sigreturn are the most fragile pieces. File/terminal writes
  accept arbitrary lengths via kernel-side ≤4 KiB chunking; pipe/socket
  writes short-write at that bound instead (a blocked pipe/socket restarts
  the whole SYSCALL, so chunking would duplicate consumed bytes).
  `chmod`/`fchmod` are validated success no-ops (no permission bits on
  FAT/tmpfs, no +x check in execve).
- `network_syscalls.rs` — finite Linux `AF_INET` socket ABI, sockaddr/iovec
  usercopy, blocking/restart behavior, and socket option mapping. Protocol
  state and buffers remain in `src/net/`.
- `signal.rs` — POSIX signal dispositions, blocked/pending masks.
- `fdtable.rs` — per-process file-descriptor table.
- `gui.rs` — per-PID window ownership plus the bounded, coalescing GUI event
  queue and process-death teardown.
- `gui_syscalls.rs` — syscalls 5001-5004: create, copy-present, next-event,
  and destroy.
- `abi.rs` — Linux x86-64 syscall ABI: dispatch table, compatibility
  pointer bounds for synthetic tests, and errno constants. Real processes
  use VMA-aware user-copy validation. Unknown syscall numbers always return
  `-ENOSYS`; trace mode changes logging detail only.
- `bin_namespace.rs` — virtual `/bin/<applet>` namespace that dispatches
  to BusyBox, the remaining kernel-side GUI apps through `GLAUNCH.ELF`, or
  standalone ELFs such as `/host/CALC.ELF`, `/host/NOTEPAD.ELF`, and
  `/host/TCC.ELF` (TinyCC; both `tcc` and the `cc` alias).
- `path.rs` — POSIX-ish path normalization.
- `pipe.rs`, `stdin.rs`, `tty.rs` — fd-backed I/O endpoints.
- `error.rs` — loader-side error enum.
- `user_state.rs` — `UserState`: 16 GPRs + RIP + RFLAGS + RSP. Layout
  is **load-bearing** — `enter_user_mode_with_regs_asm`,
  `iretq_to_user_with_regs`, and `resume_ring3_asm` all read fields by
  hard-coded offset. `test_user_state_offsets_match_asm_contract`
  pins the contract.

## Process model

One `Process` per ring-3 program (the "PID 0" sentinel is a never-
schedulable slot the table keeps to preserve old singleton semantics).
Each process owns:

- a fresh `AddressSpace` (own L4, kernel half copied from the kernel
  L4 so kernel code keeps executing after `Cr3::write`),
- a 64 KiB `KernelStack` for syscalls / interrupts taken from CPL=3,
- `signal_state` (dispositions + blocked mask, inherited across fork
  per POSIX; pending cleared per POSIX),
- `fd_table` (cloned by value across fork),
- `cwd`, a byte-granular `brk_current` and derived `brk_base`, plus an
  AddressSpace-owned VMA set (`mmap_next` is compatibility state only),
- demand-grown stack bookkeeping (`stack_top`/`stack_bottom`/
  `stack_mapped_bottom`/`stack_max_growth_floor`/
  `growth_faults_remaining`),
- `fs_base` (per-process FS_BASE MSR value),
- `fpu_state` (512-byte 16-aligned FXSAVE area),
- `saved_user_state` (U4 snapshot used by `resume_ring3`).
- `network_wait` (restart-stable absolute deadline for a blocked socket
  syscall; cleared on success, close, signal, exit, or syscall identity
  change).
- `real_timer` plus `pending_syscall_interrupt` for 100 Hz ITIMER_REAL /
  SIGALRM delivery. Timer expiry is processed by kernel housekeeping and the
  inline test dispatcher; a signal-woken blocking syscall re-enters the
  dispatcher as `-EINTR` so its handler runs before the syscall can re-block.
- `sleep_deadline` — restart-stable absolute PIT deadline for a blocking
  `nanosleep`. Set on first entry, checked on every SYSCALL re-fire, cleared on
  completion (`lifecycle::nanosleep_deadline`). The process parks on
  `Ring3BlockReason::Sleeping { deadline_tick }`; `process_expired_sleeps()`
  wakes it when the deadline elapses. That wake pass runs primarily from the
  compositor kernel thread's loop (`window::compositor::run`) — the kernel main
  loop is the idle task under U10 and is starved, so a self-timed animation
  would otherwise wake only every few seconds; it is also called from the main
  loop and the inline dispatch loop for the launcher/test paths. Dispatch of the
  woken process is fast (`scheduler::next_runnable` pops `ring3_ready` each
  switch). Self-driven ring-3 animation loops (`PAINTING.ELF`) and zsh's
  `sleep`/`usleep` depend on this; `-EINTR`/remaining-time is not modeled.

Socket slots hold `Arc<net::socket::SocketHandle>`. The handle is a shared
open-file description: dup/fork share `O_NONBLOCK` and protocol state, while
`FD_CLOEXEC` remains per descriptor. Do not take the network lock from an
FD-table mutation or hold process/network locks or user pointers across a
yield. Final handle drop uses the deferred-close queue documented in
`src/net/CLAUDE.md`.

Processes live in `lifecycle::PROCESS_TABLE`, indexed by PID. The
single field `current_user_pid: Option<u32>` names which process's
CR3 / FS_BASE / FPU / kernel-stack are loaded right now. Today only
one real ring-3 process is ever current; U5 wires the time-slicing
that makes the "current" field meaningful as a per-instant pointer.
`PROCESS_TABLE` uses `InterruptMutex`, not a plain `spin::Mutex`. This is
load-bearing on the single CPU: timer-preemptible launcher/compositor threads
and IF-cleared page-fault/SYSCALL paths share the table, so every acquisition
must mask timer preemption until its guard releases the spinlock.

## Ring-3 GUI ABI

The AgenticOS-private range extends syscall 5000 with four calls:

- 5001 `gui_win_create(width, height, title, title_len, flags)`
- 5002 `gui_win_present(handle, pixels, width, height, stride)`
- 5003 `gui_next_event(event, len, flags)`
- 5004 `gui_win_destroy(handle)`

Pixels are little-endian XRGB8888 (`u32` value `0x00RRGGBB`). Presents copy a
full client surface into kernel memory. Events use a fixed 32-byte
`GuiEvent { kind, window, payload[6] }`; an empty blocking read parks on
`WaitingForGuiEvent`, while `GUI_NONBLOCK` returns `-EAGAIN`. Each PID has a
128-entry queue: consecutive motion events coalesce and other overflow drops
the oldest entry. Removing a process destroys every frame it owns.

The ring-3 timer hands control back to `KERNEL_CONTEXT` every second tick,
saving and requeueing the current user process first. Direct ring3-to-ring3
switching remains available on intervening ticks and blocking syscalls, but
cannot starve the compositor or network worker when zsh polls for a child.

## Launch flow today (post-U8)

1. Terminal opens → `spawn_zsh_for_terminal` (in `src/window/`)
   spawns a kernel thread that calls
   `launcher::launch_user_binary("/host/ZSH.ELF")`.
2. `launch_user_binary` holds `BINARY_SETUP_MUTEX` and `BinaryLoadGuard`,
   reads the ELF bytes while CR3 is still the kernel L4, then performs
   `AddressSpace::new → activate → load/map ELF → initialize VMAs once →
   build stack/install Process`. The setup mutex also serializes teardown;
   kernel-thread preemption remains enabled so FAT-backed page-in and sparse
   address-space destruction cannot freeze the compositor.
3. `enter_user_mode_with_aspace` builds the initial user stack,
   inserts the Process into `PROCESS_TABLE` with `saved_user_state`
   populated to the binary's entry frame (rip=entry, rsp=user_rsp,
   rflags=0x202, GPRs=0), marks ring3_ready, and calls
   `block_kernel_thread_for_ring3_exit(pid)` which blocks the
   launcher kernel thread on `BlockReason::WaitingForRing3Exit{pid}`.
4. Kernel-thread scheduler picks another runnable thread (typically
   the kernel main loop's idle path; another terminal's launcher; the
   compositor when U10 lands). The kernel main loop's
   `save_kernel_and_resume_ring3` checks `ring3_ready` and dispatches
   the first ready ring-3 process via `resume_ring3` — saving
   `KERNEL_CONTEXT` so a ring-3 yield-back lands here cleanly.
5. Ring 3 runs. Syscalls take the SYSCALL fast path; faults take the
   IDT path. The timer at CPL=3 fires `try_preempt_ring3` (U5) which
   round-robins between ring3_ready processes via `resume_ring3`.
6. On exit (cooperative or fault), `long_jump_to_run_or_halt` wakes
   any kernel thread blocked on this pid's exit (via
   `wake_threads_waiting_for_ring3_exit`), then yields to the next
   ring3_ready (`resume_ring3`) or back to the kernel main loop
   (`yield_to_kernel_main_loop` restores `KERNEL_CONTEXT`).
7. The kernel scheduler eventually picks the woken launcher thread;
   it resumes inside `block_kernel_thread_for_ring3_exit`, returns to
   `enter_user_mode_with_aspace`, reads exit info from the Process,
   returns to the caller. `release_active_image` (or the caller's
   cleanup) drops the Process from PROCESS_TABLE, freeing
   `AddressSpace`/`KernelStack`/fd_table/etc. AddressSpace teardown releases
   resident leaves and private page-table frames to the reusable allocator.

Two terminals coexisting: two `spawn_zsh_for_terminal` kernel threads
are created; each calls `enter_user_mode_with_aspace` for its own
zsh, blocks on `WaitingForRing3Exit{own_pid}`. The kernel scheduler
runs whichever launcher hasn't blocked yet, then idle dispatches
each ring-3 process in turn via `save_kernel_and_resume_ring3`. U5's
timer ISR time-slices between them at CPL=3. When zsh blocks on
stdin (no input), `read_stdin_blocking` yields via
`block_current_ring3_and_yield(WaitingForInput)`, which picks the
next runnable ring-3 (the other zsh) or falls back to the kernel
main loop. Input from the keyboard ISR's stdin push calls
`wake_ring3_blocked_on_input`, moving blocked readers back to
ring3_ready; the next dispatch picks them up.

## Virtual memory

User space is the canonical lower half minus the reserved kernel-heap and
kernel-stack PML4 slots. ELF segments retain ET_EXEC addresses; brk is derived
from the highest PT_LOAD; mmap searches reusable gaps top-down; the stack ends
at `0x0000_7fff_ffff_f000` with a 64 MiB sparse reservation.

Fork clones VMA metadata and page-table structure, retaining resident leaves.
Writable private leaves become read-only `BIT_9` COW mappings and copy only on
first write. Anonymous mmap and heap growth are metadata-only; stack, heap,
anonymous, private-file, and production ELF pages are materialized by the VMA
fault resolver. The production loader records PT_LOAD file offsets and keeps
only the initial stack and TLS resident; direct loader fixtures may opt into
eager segment population for deterministic tests.
`munmap`, shrinking brk, and `mprotect` update both VMAs and resident PTEs.
Writable+executable mappings are rejected and NXE is enabled.

Syscall/path/signal user pointers must go through `usercopy`; raw user virtual
dereferences are reserved for loader/initial-stack construction while the new
address space is active but not yet installed as a Process.

## Multi-ring-3 scheduling (in progress)

Plan: `docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md`.
Unit status:

- U1 — `ProcessTable` with PID indexing + `current_user_pid`. **Done.**
- U2 — Per-process FS_BASE + FPU save/restore primitives. **Done.**
- U3 — `Runnable` decision surface + ring-3 ready/blocked queues. **Done.**
- U4 — `save_ring3` / `resume_ring3` switch primitives. **Done.**
- U5 — Timer ISR ring-3 preempt wiring. **Done.** `try_preempt_ring3`
  in `lifecycle.rs` is the decision point: under one
  `PROCESS_TABLE.try_lock()` it reads `current_user_pid`, pops the
  ring3_ready front, saves cur via `save_ring3`, and returns the
  next pid (or `None`). The timer ISR in `preemption.rs` calls
  `resume_ring3(next)` when `Some`, otherwise iretq's back to the
  same process.
- U6 — Blocking `wait4`. **Done.** `wait4_handler` captures
  callee-saved at entry, then on the no-zombie-has-children-no-WNOHANG
  branch calls `switch::block_current_ring3_or_panic`, which records
  the parent's user state with RIP rewound 2 bytes (the SYSCALL
  instruction's length), marks the parent
  `Ring3BlockReason::WaitingForChild`, and resumes the next
  ring3_ready (typically the child the parent just forked). When the
  child exits, `notify_parent_of_exit` wakes the parent to ring3_ready
  and the child's exit path's `long_jump_to_run_or_halt` resumes the
  parent — its SYSCALL re-fires, this time finding the zombie and
  returning normally.
- U7 — Collapse fork into "register child as Ready, return to parent."
  **Done.** `fork_handler` builds child's `Process` with
  `saved_user_state` populated (rax=0 child marker + parent's
  RIP/RFLAGS/RSP/GPRs at SYSCALL boundary), inserts into PROCESS_TABLE,
  calls `mark_ring3_ready(child)`, and returns `child_pid` to the
  parent immediately. PARENT_STASH / stash_parent / take_stashed_parent
  / raise_signal_on_stashed_parent / swap_current_process all deleted.
  SIGCHLD on parent now raises directly via `with_process(parent_pid,
  ...)`. Child's `Process` slot stays in PROCESS_TABLE after exit (its
  kernel_stack is what the exit path was executing on); the parent's
  `wait4` reap path calls `remove_process` to free it.
- U10 — Compositor as a scheduled kernel thread. **Done.** Input
  processing + terminal output + `render_frame` moved out of the
  kernel main loop into `src/window/compositor.rs::run`, spawned at
  boot via `process::spawn_process("compositor", None, run)`. The
  kernel main loop is now pure scheduler housekeeping + `hlt`. A
  busy ring-3 process no longer freezes the desktop — U5's timer ISR
  splits ring-3 slices and the kernel-thread scheduler round-robins
  the compositor in alongside terminal launchers. The
  `BinaryLoadGuard` IDE-PIO atomicity gate is preserved (the
  compositor checks `binary_load_in_progress()` and skips input +
  render while a binary is loading).
- U9 — Signal/page-fault audit for cross-process correctness. **Done.**
  Audit confirmed: `maybe_deliver_signal` / `deliver_signal` /
  `consume_deliverable` all read via `with_current_process` (=
  `current_user_pid`'s slot), which is the syscall-issuing process by
  the SYSCALL stub's contract. Cross-process signal raises (kill,
  notify_parent_of_exit, etc) target by PID via `with_process(target,
  ...)`, never by "current." `try_grow_user_stack` reads
  `current_user_pid`'s stack window — correct because ring-3 page
  faults imply CR3 = faulting process's L4 = current_user_pid
  (`resume_ring3`'s atomic swap). Two new tests in
  `src/tests/userland_switch.rs` verify raises target by-pid and
  consume reads from current only. Debug assertion in
  `maybe_deliver_signal` documents the invariant (gated out of test
  builds because tests drive syscall_dispatch synthetically).
- U8 — Terminal launch refactor: "register and yield." **Done.**
  `enter_user_mode_with_aspace` no longer iretqs directly into ring 3.
  It installs the Process with `saved_user_state` populated to the
  binary's entry frame (rip=entry, rsp=user_rsp, rflags=0x202,
  GPRs=0), marks it ring3_ready, and blocks the launching kernel
  thread via `scheduler::block_current(WaitingForRing3Exit{pid})`. The
  kernel main loop's `save_kernel_and_resume_ring3` (in `switch.rs`)
  saves KERNEL_CONTEXT and dispatches the process for its first ring-3
  execution. When the ring-3 process exits,
  `long_jump_to_run_or_halt` wakes the launcher thread; the kernel
  scheduler eventually picks it back up, it reads exit info and
  returns to `launch_user_binary`. Multiple terminals can now coexist:
  each terminal's launcher kernel thread blocks on its own pid; U5's
  timer ISR round-robins between the multiple ring-3 processes.
  `read_stdin_blocking` no longer spins — it
  `block_current_ring3_and_yield(WaitingForInput)`s; the input ISR's
  stdin push path wakes blocked readers. Legacy setjmp scaffolding
  (`KernelContinuation`, `install_continuation`, `take_continuation`,
  `restore_continuation`, `enter_user_mode_asm`,
  `enter_user_mode_with_regs_asm`, `Process.continuation` field) all
  deleted; `iretq_to_user_with_regs` kept for execve / sigreturn /
  signal-delivery (same-process iretq with new register state).
- U8 — Terminal launch path: "register and yield" pattern; multiple
  terminals.
- U9 — Signal / page-fault audit for cross-process correctness.
- U10 — Compositor as a scheduled kernel thread.
- U11 — Documentation refresh.

## The switch primitive (U4)

`switch.rs` owns the asm + Rust glue for ring-3 ↔ ring-3 transitions.

**`save_ring3(p, frame)`** — Rust helper. Copies the trap-frame GPRs
(plus RIP / RFLAGS / RSP) from the `InterruptStackFrame` argument into
`p.saved_user_state`, then calls `save_user_cpu_state(p)` to snapshot
FS_BASE + FPU. Must run on the live CPU before any kernel code
clobbers FS_BASE or XMM — the kernel target is `+soft-float` so XMM is
safe by construction; FS_BASE is safe as long as no `wrmsr`/arch_prctl
fires between trap and call. Called from the timer ISR's Rust-side
handler in U5; not called anywhere today.

**`resume_ring3(pid)` (diverging)** — Rust wrapper around the asm. Looks
up the process under `PROCESS_TABLE.lock()`, copies out the snapshot +
L4 frame + kernel-stack top, releases the lock, then in order:
activates the L4 (CR3 write), updates TSS.rsp0 + GSBASE-stored SYSCALL
rsp top, restores FS_BASE + FPU, sets `current_user_pid = Some(pid)`,
calls `resume_ring3_asm`. The asm builds an iretq frame from the
`UserState` snapshot and transfers to ring 3.

**Lock ordering:** `PROCESS_TABLE` only. Never the scheduler or the
memory mapper from inside this primitive. The brief re-acquire inside
`restore_user_cpu_state` (via `with_process`) is safe — `PROCESS_TABLE`
never blocks the holder.

**ABI contract:** `resume_ring3_asm` reads `UserState` by hard-coded
offset. The contract is locked by `_SIZE_CHECK` in `user_state.rs` and
the assertion test `test_user_state_offsets_match_asm_contract` in
`src/tests/userland_switch.rs`. Any reorder in `UserState` breaks both
the asm and the test at compile time.

**Validation:** the diverging asm has the same iretq-frame shape as
`iretq_to_user_with_regs` and the back half of
`enter_user_mode_with_regs_asm`, but additionally restores RCX/R11 because
timer interrupts may land while both contain live values. End-to-end ring-3
switching falls out of U5's integration test (two synthetic ring-3 processes
coexisting); U4 itself is validated by unit tests around `save_ring3` and the
offset contract.

## Cross-references

- Architecture-specific asm (timer ISR, GDT, SSE enable, MSRs):
  `src/arch/x86_64/CLAUDE.md`.
- Kernel-thread scheduler / PCB: `src/process/CLAUDE.md`.
- Memory mapper / paging: `src/mm/CLAUDE.md`.
- Multi-ring-3 plan: `docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md`.
- U4 plan: `docs/plans/2026-05-16-006-feat-u4-ring3-switch-primitive-plan.md`.
- `no_std` / panic-handler / testing rules: `.claude/rules/`.
