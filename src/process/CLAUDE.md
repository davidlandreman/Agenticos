# `src/process/` — Process Management

The kernel runs a **live preemptive scheduler** with timer-driven context switching. Everything still runs in ring 0 today; ring-3 user processes are layered on top by the userland subsystem (see `src/userland/`).

## Key files

- `process.rs` — `Process` and `BaseProcess` traits; sequential PID allocation (no reuse, starts at 1).
- `manager.rs` — small singleton holding the active stdin buffer used by kernel-side `read` paths. (The kernel-side shell command registry that used to live here was removed when zsh became the default terminal; see `docs/plans/2026-05-16-004-feat-zsh-default-terminal-and-gui-launchers-plan.md`.)
- `pcb.rs` — process control block. Carries name, PID, kernel stack, `CpuContext`, watchdog `last_activity_tick`, terminal id, optional entry-fn closure.
- `context.rs` — `CpuContext` (all GPRs + RIP/RSP/RFLAGS + CS/SS). The CS/SS fields default to kernel selectors (0x08/0x10) so existing kernel-process flows don't change.
- `scheduler.rs` — round-robin scheduler with sleep queue. Runs preemptively under the timer ISR.
- `stack.rs` — `StackAllocator` over a fixed VA range at `0x_5555_0000_0000`.

## What's wired up today

- Process / `BaseProcess` traits define the interface.
- Sequential PID allocation.
- **Preemptive timer-driven scheduling.** The PIT fires at 100 Hz; the timer ISR (`src/arch/x86_64/preemption.rs`) saves the running process's full register state, calls into the scheduler, and either `iretq`s into another process or back into the kernel main loop via the `KERNEL_CONTEXT` shadow.
- **Cooperative voluntary switching.** `switch_context` and `switch_context_full_restore` in `src/arch/x86_64/context_switch.rs` provide the non-interrupt-driven path.
- **Watchdog.** `WATCHDOG_TIMEOUT_TICKS = 1000` (~10 s); processes that don't update `last_activity_tick` are flagged for kill from the kernel main loop.

## Spawning a kernel-side process

`spawn_process(name, terminal_id, entry_fn)` in `src/process/mod.rs` allocates a PCB + kernel stack, sets the entry closure, and enqueues the process for the scheduler. Used by:

- `src/window/terminal_factory.rs::spawn_zsh_for_terminal` — kernel process whose entry function blocks in `launch_user_binary("/host/ZSH.ELF")` until zsh exits, then closes the terminal window.
- `src/commands/gui_launch_table.rs::spawn_by_name` — kernel process per GUI app launch (`painting`, `tasks`, `explorer`).
- `src/commands/guishell/mod.rs::spawn_guishell_process` — the desktop / taskbar background process.

## Ring-3 awareness

`CpuContext` carries explicit `cs` and `ss` fields (offsets 144 and 152). The naked-asm `iretq` frames in `preemption.rs` and `context_switch.rs` read CS/SS from those fields rather than literal `push 0x08` / `push 0x10`. Kernel processes default to `cs=0x08, ss=0x10`; the kernel-thread scheduler doesn't directly schedule ring-3 (each ring-3 process is in the userland subsystem's `PROCESS_TABLE`, not the kernel-thread scheduler).

**Multi-ring-3 (U5-U8) integration:** `timer_handler_inner`'s CPL=3 branch calls `userland::lifecycle::try_preempt_ring3` to round-robin between ring-3 processes via `userland::switch::resume_ring3`. Kernel threads that launched ring-3 processes (e.g., terminal factory) block on `BlockReason::WaitingForRing3Exit(pid)` while their ring-3 process runs; the kernel main loop's `save_kernel_and_resume_ring3` dispatches `ring3_ready` processes when no kernel thread is runnable. See `src/userland/CLAUDE.md` for the full lifecycle.

## What's NOT wired up

- **No isolation between kernel threads.** All kernel code shares one address space (ring 0). Ring-3 processes have their own L4 (USER-bit-protected user half + shared kernel half) — see the userland subsystem at `src/userland/`.
- **No SMP.** Single execution thread per CPU; the scheduler multiplexes both kernel threads and ring-3 processes but does not exploit multiple cores.
- **No IPC.**

## Cross-references

- Naked-asm context switching and timer ISR: `src/arch/x86_64/CLAUDE.md`.
- Userland (ring-3) processes: `src/userland/CLAUDE.md` (under construction).
- Kernel-side GUI app launchers: `src/commands/CLAUDE.md`.
- The interactive shell now runs as ring-3 zsh (`/host/ZSH.ELF`) launched from `src/window/terminal_factory.rs::spawn_zsh_for_terminal`.
