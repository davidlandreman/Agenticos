# `src/process/` — Process Management

The kernel runs one **live preemptive scheduler** across all online CPUs.
Kernel threads and ring-3 processes share a tagged run queue; ring-3
implementation details live in `src/userland/`.

## Key files

- `process.rs` — `Process` and `BaseProcess` traits; atomic sequential PID allocation (no reuse, starts at 1).
- `manager.rs` — small singleton holding the active stdin buffer used by kernel-side `read` paths. (The kernel-side shell command registry that used to live here was removed when zsh became the default terminal; see `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.)
- `pcb.rs` — process control block. Carries name, PID, kernel stack, `CpuContext`, watchdog `last_activity_tick`, terminal id, optional entry-fn closure.
- `context.rs` — `CpuContext` (all GPRs + RIP/RSP/RFLAGS + CS/SS). The CS/SS fields default to kernel selectors (0x08/0x10) so existing kernel-process flows don't change.
- `scheduler.rs` — shared privilege-neutral run queue with per-CPU current slots,
  save-before-steal context publication, optional entity CPU affinity, and
  production-only scheduler-shadow transition hooks.
- `stack.rs` — `StackAllocator` over a fixed VA range at `0x_5555_0000_0000`.

## What's wired up today

- Process / `BaseProcess` traits define the interface.
- Sequential PID allocation.
- **Preemptive timer-driven scheduling.** The BSP PIT owns global time; AP LAPIC timers drive local preemption. Every CPU can pick from the shared queue, and a context becomes eligible for migration only after its full save is published.
- **Cooperative voluntary switching.** `switch_context` and `switch_context_full_restore` in `src/arch/x86_64/context_switch.rs` provide the non-interrupt-driven path.
- **Safe kernel-stack retirement.** A terminating kernel thread first switches to its per-CPU main-loop stack; only then does the assembly handoff return the abandoned stack to the shared allocator. Publishing a still-active stack would allow a concurrent spawn to reuse it before termination completed. Cross-CPU watchdog kills set a PCB request that the owning CPU consumes at its next safe timer boundary.
- **Watchdog.** `WATCHDOG_TIMEOUT_TICKS = 1000` (~10 s); processes that don't update `last_activity_tick` are flagged for kill from the kernel main loop.

## Spawning a kernel-side process

`spawn_process(name, terminal_id, entry_fn)` in `src/process/mod.rs` allocates a PCB + kernel stack, sets the entry closure, and enqueues the process for the scheduler. Used by:

- `src/userland/process_service.rs` — one persistent kernel worker that drains non-blocking user launch requests and reaps detached ring-3 exits.
- `src/commands/gui_launch_table.rs::spawn_by_name` — kernel process per GUI app launch (the applet list is empty today — every GUI app has migrated to ring 3; the mechanism remains for a future ring-0-only workload).
- `src/commands/guishell/mod.rs::spawn_guishell_process` — the desktop / taskbar background process.

## Ring-3 awareness

`CpuContext` carries explicit `cs` and `ss` fields (offsets 144 and 152). The naked-asm `iretq` frames in `preemption.rs` and `context_switch.rs` read CS/SS from those fields rather than literal `push 0x08` / `push 0x10`. Kernel processes default to `cs=0x08, ss=0x10`; the kernel-thread scheduler doesn't directly schedule ring-3 (each ring-3 process is in the userland subsystem's `PROCESS_TABLE`, not the kernel-thread scheduler).

**Multi-ring-3 integration:** ring-3 processes and kernel threads share the tagged scheduler queue. Production launchers do not wait: `process-service` installs the process as Ready, then returns to its queue. On exit the user entity is unregistered and the service is woken to drop the `Process` from a safe kernel stack. `WaitingForRing3Exit` remains only for synchronous QEMU-test compatibility.

**Scheduler-shadow rule:** `process::init_scheduler` marks only the singleton
production scheduler as observed. Hook calls belong immediately after their
production state/queue commit. A handoff must save/yield the old entity before
dispatching the next one; temporarily publishing two `Running` entities on one
CPU is `SCHED-002`, even if the next instruction would repair it.

The singleton `SCHEDULER` and `STACK_ALLOCATOR` also carry tracked lock
classes in record/strict diagnostics. Their crash-readable owner and observed
edges are evidence only; they never choose a runnable entity or repair a
stack. Scheduler transitions own `0x01xx_xxxx`, stack lifetime transitions own
`0x07xx_xxxx`, and undeclared lock edges own `0x0900_0004`. New hooks must be
integer-only and adjacent to the production commit they describe.

Ring-3 blocking is published in two structures guarded by different locks:
`PROCESS_TABLE.ring3_blocked` records the targeted wake reason and the unified
scheduler records the entity as Blocked. `mark_ring3_blocked` must reconcile
the scheduler state after both writes: a producer may remove the reason and
mark Ready between them, and without the final reason-presence check the later
Blocked write would consume that wake. Git's multi-process transport exposed
this ordering race.

**Pthread affinity rule:** several ring-3 task entities may share one TGID and
address space. Until user TLB shootdown exists, the group is assigned a home
CPU on its first pthread clone and all members receive scheduler affinity to
that CPU. Dequeue skips ineligible entities without losing their queue order;
targeted reschedule IPIs wake the home CPU.

**Deferred device-wake rule:** PCI handlers only publish bounded lock-free
wake records. Every scheduler-driving context (BSP main loop, AP idle loop,
and the synchronous QEMU launcher) must drain those records before selecting
work and again across its STI+HLT commit. Producers kick CPUs whose published
state is idle. Falling back to the 100 Hz PIT adds up to 10 ms to every RPC;
Git metadata walks over uncached 9p turn that into tens of seconds. Kernel
I/O wake records retain both PID and request token until the PCB publishes the
matching `WaitingForBlockIo`; consuming an early record loses the wake.

Synchronous QEMU launchers are woken by `dispatch_after_user_exit`, which
publishes the exit wake and selects the post-exit entity under one scheduler
lock. Waking from `stop_task` is too early: another CPU can observe unset exit
metadata or reclaim the exiting process's still-active kernel stack.

## What's NOT wired up

- **No isolation between kernel threads.** All kernel code shares one address space (ring 0). Ring-3 processes have their own L4 (USER-bit-protected user half + shared kernel half) — see the userland subsystem at `src/userland/`.
- **Coarse SMP only.** There is one global run queue and scheduler lock;
  per-CPU queues, work stealing, and load balancing are intentionally deferred.
- **No IPC.**

## Cross-references

- Naked-asm context switching and timer ISR: `src/arch/x86_64/CLAUDE.md`.
- Userland (ring-3) processes: `src/userland/CLAUDE.md` (under construction).
- Kernel-side GUI app launchers: `src/commands/CLAUDE.md`.
- The interactive terminal is ring-3 `TERMINAL.ELF`. It creates its GUI
  surface and PTY, then forks/execs zsh; there is no kernel terminal factory.
