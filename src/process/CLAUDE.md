# `src/process/` — Process Management

The kernel runs one **live preemptive scheduler** across all online CPUs.
Kernel threads and ring-3 processes share a tagged run queue; ring-3
implementation details live in `src/userland/`.

## Key files

- `process.rs` — `Process` and `BaseProcess` traits; atomic sequential PID allocation (no reuse, starts at 1).
- `manager.rs` — small singleton holding the active stdin buffer used by kernel-side `read` paths. (The kernel-side shell command registry that used to live here was removed when zsh became the default terminal; see `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.)
- `pcb.rs` — process control block. Carries name, PID, kernel stack, `CpuContext`, watchdog `last_activity_tick`, terminal id, optional entry-fn closure.
- `context.rs` — `CpuContext` (all GPRs + RIP/RSP/RFLAGS + CS/SS). The CS/SS fields default to kernel selectors (0x08/0x10) so existing kernel-process flows don't change.
- `scheduler.rs` — shared privilege-neutral run queue with per-CPU current slots and save-before-steal context publication.
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

## What's NOT wired up

- **No isolation between kernel threads.** All kernel code shares one address space (ring 0). Ring-3 processes have their own L4 (USER-bit-protected user half + shared kernel half) — see the userland subsystem at `src/userland/`.
- **Coarse SMP only.** There is one global run queue and scheduler lock; per-CPU queues, affinity, work stealing, and load balancing are intentionally deferred.
- **No IPC.**

## Cross-references

- Naked-asm context switching and timer ISR: `src/arch/x86_64/CLAUDE.md`.
- Userland (ring-3) processes: `src/userland/CLAUDE.md` (under construction).
- Kernel-side GUI app launchers: `src/commands/CLAUDE.md`.
- The interactive shell runs as ring-3 zsh (`/host/ZSH.ELF`); terminal factory creates the window/PTY and submits an explicit-terminal launch request to `process-service`.
