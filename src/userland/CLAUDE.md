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
  page-fault hook, FPU/FS_BASE save/restore orchestrators, and orphan
  adoption for the kernel reaper. The table also maps task IDs to thread-group
  IDs, retains clear-child-TID/robust-list state, and defers dead pthread stack
  reclamation until execution has switched to a safe kernel stack.
- `futex.rs` — bounded TGID-keyed futex registry implementing musl's
  wait/wake/requeue profile, relative wait deadlines, signal interruption,
  and timer-backed timeout completion.
- `process_service.rs` — persistent kernel launch/reap worker, bounded
  non-blocking request queue, explicit terminal launch context, cancellation,
  and completion delivery.
- `launcher.rs` — serialized CR3-sensitive ELF preparation. Production uses
  the unstarted prepare/commit split; the blocking wrapper is test compatibility.
- `mod.rs` — process setup/initial-stack construction and
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
  FAT/tmpfs, no +x check in execve). Native-tool compatibility includes
  bounded `readv`, per-process `umask`, writable-fd `F_GETFL`, and
  `utimensat` with `UTIME_NOW`/`UTIME_OMIT` routed through the VFS.
- `network_syscalls.rs` — finite Linux `AF_INET` socket ABI, sockaddr/iovec
  usercopy, blocking/restart behavior, and socket option mapping. Protocol
  state and buffers remain in `src/net/`.
- `signal.rs` — POSIX signal dispositions, blocked/pending masks.
- `fdtable.rs` — per-process file-descriptor table.
- `devfs.rs` — exact synthetic `/dev` and `/dev/urandom` classification;
  device reads use the kernel cryptographic random broker.
- `gui.rs` — per-PID window ownership plus the bounded, coalescing GUI event
  queue and process-death teardown.
- `gui_syscalls.rs` — syscalls 5001-5005 plus 5011: create, copy-present,
  next-event, destroy, update a window title, and open a selectable event fd.
- `gui_gl.rs` — validated GL syscalls 5006-5009, per-PID logical-context
  ownership, bounded packet parsing, mailbox publication, and teardown.
- `etc.rs` — kernel-managed `/etc` namespace: static account/hosts files,
  the shipped zsh configuration, DHCP-published `resolv.conf` paths, and
  `publish_theme`, which writes the resolved theme token (`classic`, `aero`,
  or `futurism`) to `/etc/theme` for ring-3 GUI apps. `userland/libs/gui`
  caches it (unknown tokens degrade to Classic) and updates the cache from
  `GUI_EVENT_THEME_CHANGED` (payload codes 1/2/3) on live Control Center
  changes.
  `GUI_EVENT_SETTINGS_CHANGED` lets multiple Control Center instances refresh
  after non-theme changes such as a new desktop wallpaper.
- `abi.rs` — Linux x86-64 syscall ABI: dispatch table, compatibility
  pointer bounds for synthetic tests, and errno constants. Real processes
  use VMA-aware user-copy validation. Unknown syscall numbers always return
  `-ENOSYS`; trace mode changes logging detail only. AgenticOS-private syscall
  5012 is the bounded UTF-8 host-clipboard operation used by `PBCLIP.ELF`.
- `bin_namespace.rs` — virtual `/bin/<applet>` namespace that dispatches
  to BusyBox or standalone ELFs. BusyBox includes the procfs-backed `free`
  and `top` monitors, VT `reset`, and `vi`; the namespace adds `vim` as an
  alias that rewrites `argv[0]` to BusyBox's real `vi` applet. Standalone
  entries include `/host/CALC.ELF`, `/host/CONTROL.ELF`
  (`control` + `settings` aliases), `/host/FILEMAN.ELF`
  (compat command `explorer`), `/host/GLGAME.ELF`, `/host/NOTEPAD.ELF`,
  `/host/PAINTING.ELF`, `/host/TASKMGR.ELF` (`taskmgr` + legacy
  `tasks` alias), and `/host/TCC.ELF` (TinyCC; both `tcc` and the `cc`
  alias), plus `/host/LINKS.ELF` (Links 2.30; `links` and `links2`, with the
  Rust-backed `agenticos` graphics driver), `/host/CURL.ELF` (curl 8.21.0;
  static IPv4 HTTP/HTTPS transfer tool sharing Links' pinned OpenSSL profile
  and `/etc/ssl/cert.pem` trust store), `/host/GIT.ELF` (git 2.52.0; all
  builtins in one binary, compiled-in exec path `/bin`) with
  `/host/GITRHTTP.ELF` (`git-remote-http` + `git-remote-https`; one
  libcurl/OpenSSL transport helper, scheme from `argv[0]`),
  `/host/PBCLIP.ELF` (text-only `pbcopy` and `pbpaste`, including bounded
  transforms, inspection modes, shell quoting, and explicit
  `pbpaste --exec` through zsh), and GNU
  binutils 2.46.0 (`addr2line`, `ar`, `as`, `c++filt`, `elfedit`, `ld`, `nm`,
  `objcopy`, `objdump`, `ranlib`, `readelf`, `size`, `strings`, `strip`). GNU
  `strings` owns that name; the conflicting BusyBox applet is disabled. Links
  supports text and GUI IPv4 HTTP(S) browsing with pinned static OpenSSL,
  TLS 1.2+, SNI, and strict chain/hostname validation against the
  kernel-managed `/etc/ssl/cert.pem` trust store.
  The `GLAUNCH.ELF` GUI-applet list is empty today.
- `procfs.rs` — synthetic read-only `/proc` namespace, modeled on the
  `/bin` synthesis pattern. Linux-shaped files (`uptime`, `meminfo`,
  `stat`, `loadavg`, `net/dev`, `/proc/<pid>/{stat,status,cmdline,statm}`
  — ring-3 PIDs only) scoped to what BusyBox `ps`/`free`/`uptime` parse,
  plus AgenticOS TSV tables under `/proc/agenticos/{kthreads,gui,sockets}`.
  File content is generated **once at open()** into an fd-owned buffer
  (`FdSlot::VirtualFile`/`VirtualDir`) — no kernel lock is held across
  user reads and each open sees one consistent snapshot. Also home to
  the `sysinfo(2)` snapshot helper. Per-process RSS is a read-only
  page-table walk (`MemoryMapper::count_user_resident_pages`), not a
  maintained counter. `/proc/stat` exposes one `cpuN` row per online logical
  processor plus an exact aggregate from monotonic per-CPU user/system/idle
  timer counters; `/proc/uptime` reports the Linux-style sum of CPU idle time.
- `path.rs` — POSIX-ish path normalization.
- `pipe.rs`, `stdin.rs`, `tty.rs` — fd-backed I/O endpoints.
- `error.rs` — loader-side error enum.
- `user_state.rs` — `UserState`: 16 GPRs + RIP + RFLAGS + RSP. Layout
  is **load-bearing** — `enter_user_mode_with_regs_asm`,
  `iretq_to_user_with_regs`, and `resume_ring3_asm` all read fields by
  hard-coded offset. `test_user_state_offsets_match_asm_contract`
  pins the contract.

## Process and pthread model

Linux-visible process identity is the thread-group leader's TGID. Every
schedulable member has a unique TID and its own saved registers, FS_BASE/FPU
image, signal mask state, and 64 KiB kernel stack. Secondary task entries map
back to the leader, which owns the shared address space, VMAs, file table,
cwd, terminal/GUI resources, executable metadata, and process exit record.
`getpid` returns the TGID; `gettid` and `set_tid_address` use the current TID.

`clone` deliberately accepts the musl 1.2.5 pthread flag profile only:
shared VM/FS/files/sighand/thread/sysvsem plus SETTLS, PARENT_SETTID, and
CHILD_CLEARTID (musl also supplies the ignored historical DETACHED bit).
`SYS_exit` stops one task and performs clear-child-TID + futex wake;
`exit_group` stops every member. A group produces one zombie/completion only
after the final member exits. Multithreaded `fork` and `execve` return
`EAGAIN` until atfork/de-thread behavior exists.

Musl mutexes, condvars, join, detached threads, and ELF TLS are supported.
Futex keys are currently group-private even when userspace omits the PRIVATE
flag; process-shared/PI futexes, robust owner-death walking, WAIT_BITSET, and
full pthread cancellation remain unsupported. Because user-address-space TLB
shootdown is not implemented, all members of a group are pinned to the CPU
where its first pthread was cloned. Other processes and kernel threads remain
free to run on the other CPUs.

One `Process` per ring-3 program (the "PID 0" sentinel is a never-
schedulable slot the table keeps to preserve old singleton semantics).
Each process owns:

- a fresh `AddressSpace` (own L4, kernel half copied from the kernel
  L4 so kernel code keeps executing after `Cr3::write`),
- a 64 KiB `KernelStack` for syscalls / interrupts taken from CPL=3,
- `signal_state` (dispositions + blocked mask, inherited across fork
  per POSIX; pending cleared per POSIX),
- `fd_table` (cloned by value across fork),
- `umask` (initialized to `0o022`, inherited by fork, preserved by exec),
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
- `signal_state.suspend_restore_mask` preserves the caller's original mask
  while blocking `rt_sigsuspend` temporarily replaces it. Actionable signals
  wake `Ring3BlockReason::WaitingForSignal`; delivery transfers the saved mask
  into the user signal frame for `rt_sigreturn` to restore.
- `utime_ticks` (CPU time charged by the timer ISR whenever it observes the
  process at CPL=3 — sampled, so sub-tick syscall time is unattributed) and
  `cmdline` (retained argv, capped at `CMDLINE_MAX_BYTES`), both read by
  `/proc/<pid>/*` generators.
- `sleep_deadline` — restart-stable absolute PIT deadline for a blocking
  `nanosleep`. Set on first entry, checked on every SYSCALL re-fire, cleared
  on completion (`lifecycle::nanosleep_deadline`) or signal interruption. The
  process parks on `Ring3BlockReason::Sleeping { deadline_tick }`;
  `process_expired_sleeps()` wakes it at the deadline. That wake pass runs
  from the PIT ISR every tick — the kernel main loop is the idle task under
  U10 and can starve for seconds while kernel threads hop between each other
  via `sleep_ticks`' direct thread→thread switch — with the compositor loop,
  main loop, and inline dispatch loop as redundant backstops. Prompt dispatch
  of the woken process comes from `sleep_ticks`' `has_ready_ring3()` check: a
  voluntarily-sleeping kernel thread bounces through the kernel main loop
  (whose gate runs `save_kernel_and_resume_ring3`) whenever ring-3 work is
  pending. Self-driven ring-3 animation loops (`PAINTING.ELF`,
  `TASKMGR.ELF`) and zsh's `sleep`/`usleep` depend on this. On EINTR the
  remaining time is not written to `rem` (known POSIX gap).

Signals: `kill(2)` addresses **any** live ring-3 PID (single-user model, no
permission checks) and wakes a blocked target via `wake_ring3_for_signal`.
Unhandled fatal-default signals (SIGKILL, SIGTERM-without-trap, …) terminate
the process at the dispatcher tail (`maybe_deliver_signal`'s fatal check →
`notify_parent_of_signaled_exit` + `cooperative_exit(128+sig)`). Ignored
signals (SIGCHLD & co.) stay pending as a notification record but are never
fatal. CPU-bound processes are covered too: a remote signal sends a reschedule
IPI to the owning CPU, and both that handler and the local timer fallback run
the fatal-default check before returning to ring 3.

Socket slots hold `Arc<net::socket::SocketHandle>`. The handle is a shared
open-file description: dup/fork share `O_NONBLOCK` and protocol state, while
`FD_CLOEXEC` remains per descriptor. Do not take the network lock from an
FD-table mutation or hold process/network locks or user pointers across a
yield. Final handle drop uses the deferred-close queue documented in
`src/net/CLAUDE.md`.

Descriptor endpoint destruction is two-phase whenever an FD-table mutation
holds `PROCESS_TABLE`: remove/take the slot under the lock, then drop it after
unlocking. This is load-bearing for pipe EOF/EPIPE wakes. `execve` uses
`FdTable::take_cloexec`, and `dup2` retains a temporary copy of a replaced
endpoint, so their final `Drop` can acquire the process table. Ordinary
`dup`/`dup2`/`F_DUPFD` clear the new descriptor's `FD_CLOEXEC` bit;
`F_DUPFD_CLOEXEC` sets it.

Blocking pipe reads and writes use the descriptor-readiness sequence as a
check-to-park handshake: sample before inspecting the pipe, publish the block
reason, then reconcile the sampled sequence before dispatch. Preserve this
ordering when adding a pipe syscall path; a producer wake can otherwise land
before the waiter is visible and strand both sides of a full/empty pipeline.
The blocking path abandons its kernel frame instead of unwinding it, so it must
explicitly drop temporary cloned pipe handles and staging buffers before the
divergent yield. Leaking a clone leaves a phantom reader or writer that can
permanently suppress EOF or EPIPE after the real endpoint exits.

`CLOCK_MONOTONIC`, scheduler sleeps, polling deadlines, interval timers, and
watchdogs remain based on the 100 Hz PIT. `CLOCK_REALTIME` and `gettimeofday`
use the boot CMOS RTC snapshot advanced by PIT ticks through `crate::time`;
when RTC validation fails they retain the prior uptime-from-zero fallback.

Processes live in `lifecycle::PROCESS_TABLE`, indexed by PID.
`CpuLocal.current_user_pid` names the process whose CR3 / FS_BASE / FPU /
kernel stack is loaded on each CPU. The scheduler's per-CPU current slot owns
the same tagged entity, so one process cannot be restored on two CPUs.
`PROCESS_TABLE` uses `InterruptMutex`, not a plain `spin::Mutex`. This is
load-bearing under SMP: local IF masking prevents same-CPU preemption while
the spin lock provides exclusion from other CPUs and IF-cleared fault/SYSCALL
paths.

## Ring-3 GUI ABI

The AgenticOS-private range extends syscall 5000 with ten GUI calls:

- 5001 `gui_win_create(width, height, title, title_len, flags)`
- 5002 `gui_win_present(handle, pixels, width, height, stride)`
- 5003 `gui_next_event(event, len, flags)`
- 5004 `gui_win_destroy(handle)`
- 5005 `gui_win_set_title(handle, title, title_len)`
- 5006 `gui_gl_context_create(window, flags)`
- 5007 `gui_gl_submit_frame(context, packet, len, flags)`
- 5008 `gui_gl_get_info(context, info, len)`
- 5009 `gui_gl_context_destroy(context)`
- 5011 `gui_event_open(O_NONBLOCK | O_CLOEXEC)`

Private syscall 5010 is the versioned `system_control` command surface used by
`CONTROL.ELF` to query renderer/display/personalization state and to apply
persistent theme or wallpaper changes. Preferences live at
`/data/agenticos/settings.conf`; missing storage degrades to session-only.

Pixels are little-endian XRGB8888 (`u32` value `0x00RRGGBB`). Presents copy a
full client surface into kernel memory. Events use a fixed 32-byte
`GuiEvent { kind, window, payload[6] }`; an empty blocking read parks on
`WaitingForGuiEvent`, while `GUI_NONBLOCK` returns `-EAGAIN`. A selectable
event descriptor integrates that same queue with poll/select;
reads contain only whole event records, dup shares `O_NONBLOCK`, and fork drops
the process-owned descriptor rather than allowing a child to drain the parent.
Each PID has a 128-entry queue: consecutive motion events coalesce and other
overflow drops the oldest entry. Mouse button events carry a 64-bit PIT tick
in payload slots 4 and 5; mouse modifier bits occupy payload slot 2 above the
button-state byte.
Title updates and destruction are ownership-checked. Removing a process
destroys every frame it owns.

GL packets are versioned, self-contained colored-triangle frames capped at
192 KiB, 1,024 draws, and 4,096 vertices. The kernel validates every offset,
range, float, viewport, and flag before publishing a one-slot mailbox to the
single VirGL owner. A GL-backed `RemoteSurface` rejects XRGB presents. Hardware
depth is advertised only when the VirGL capset supplies a depth format; the
userland GL library uses bounded painter sorting otherwise.

The ring-3 timer hands control back to `KERNEL_CONTEXT` every second tick,
saving and requeueing the current user process first. Direct ring3-to-ring3
switching remains available on intervening ticks and blocking syscalls, but
cannot starve the compositor or network worker when zsh polls for a child.

## Launch flow today

1. Start, Run, or Terminal constructs an owned `LaunchSpec` and calls
   `process_service::submit`, receiving a `LaunchId` before a PID exists.
2. The persistent `process-service` kernel thread drains the bounded queue and
   calls `launcher::prepare_user_binary_unstarted`. ELF bytes are read through
   asynchronous VirtIO block I/O before `BINARY_SETUP_MUTEX` serializes the
   CR3-sensitive `AddressSpace::new → activate → load/map ELF → initialize
   VMAs → build stack/install Process` transaction. A per-CPU preemption guard
   covers the interval where the new CR3 is active, and setup restores the
   permanent kernel CR3 before publishing the process to another CPU.
3. Setup installs a complete but unschedulable Process. The service publishes
   the LaunchId→PID record, checks cancellation, then marks the user entity
   Ready. Explicit `terminal_id`, cwd, argv, and env replace wrapper-thread
   inference.
4. Ring 3 runs on the unified scheduler. On exit it records status, destroys
   GUI ownership, unregisters its entity, marks process-service work pending,
   and dispatches away without dropping its live kernel stack. Every
   ring3→kernel handoff installs the permanent kernel CR3 before a remote CPU
   may reap the old address space.
5. `process-service` later removes the Process from its own kernel stack,
   freeing the address space, kernel stack, fds, and timers before invoking a
   completion handler outside all locks. Fork children remain parent-owned
   zombies until `wait4`; children of an exited parent are adopted by the
   kernel reaper.

`WaitingForRing3Exit` and `launch_user_binary` remain only for synchronous
QEMU fixtures. Production creates no per-application launcher kernel thread.

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
  the compositor in alongside terminal launchers. VirtIO DMA storage
  allows input and rendering to continue throughout binary loading.
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
  This wrapper-per-terminal lifetime model has since been superseded in
  production by `process-service`; the blocking path remains for QEMU fixtures.
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

Across the subsystem, the reviewed tracked order is
`SCHEDULER → PROCESS_TABLE → stack/heap → MAPPER → serial`. In particular,
drop `PROCESS_TABLE` before inspecting the scheduler (signal wake does this in
two revalidated process-table phases), and allocate user-mapping result
buffers before entering `MAPPER`. Rich modes make undeclared edges and cycles
`LOCK-004`.

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
