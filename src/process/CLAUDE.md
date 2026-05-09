# `src/process/` — Process Management

The kernel runs a **live preemptive scheduler** with timer-driven context switching. Everything still runs in ring 0 today; ring-3 user processes are layered on top by the userland subsystem (see `src/userland/`).

## Key files

- `process.rs` — `Process` and `BaseProcess` traits; sequential PID allocation (no reuse, starts at 1).
- `manager.rs` — command registry; maps command names to factory functions; the shell routes unknown commands here.
- `pcb.rs` — process control block. Carries name, PID, kernel stack, `CpuContext`, watchdog `last_activity_tick`, terminal id, optional entry-fn closure.
- `context.rs` — `CpuContext` (all GPRs + RIP/RSP/RFLAGS + CS/SS). The CS/SS fields default to kernel selectors (0x08/0x10) so existing kernel-process flows don't change.
- `scheduler.rs` — round-robin scheduler with sleep queue. Runs preemptively under the timer ISR.
- `stack.rs` — `StackAllocator` over a fixed VA range at `0x_5555_0000_0000`.

## What's wired up today

- Process / `BaseProcess` traits define the interface.
- Sequential PID allocation.
- Command registry: `register_command(name, factory)` populates a name → factory map.
- Shell integration: the shell calls `execute_command(name args…)`, which dispatches via the registry.
- **Preemptive timer-driven scheduling.** The PIT fires at 100 Hz; the timer ISR (`src/arch/x86_64/preemption.rs`) saves the running process's full register state, calls into the scheduler, and either `iretq`s into another process or back into the kernel main loop via the `KERNEL_CONTEXT` shadow.
- **Cooperative voluntary switching.** `switch_context` and `switch_context_full_restore` in `src/arch/x86_64/context_switch.rs` provide the non-interrupt-driven path.
- **Watchdog.** `WATCHDOG_TIMEOUT_TICKS = 1000` (~10 s); processes that don't update `last_activity_tick` are flagged for kill from the kernel main loop.

## Ring-3 awareness

`CpuContext` carries explicit `cs` and `ss` fields (offsets 144 and 152). The naked-asm `iretq` frames in `preemption.rs` and `context_switch.rs` read CS/SS from those fields rather than literal `push 0x08` / `push 0x10`. Kernel processes default to `cs=0x08, ss=0x10` so behavior is preserved; ring-3 processes (when they arrive via the userland subsystem) carry `cs=0x23, ss=0x1B` and resume in ring 3 without further asm changes.

`timer_handler_inner` short-circuits on `(frame.cs & 3) == 3`: it refreshes `last_activity_tick` for the active PCB, sends EOI, and returns immediately. The naked wrapper then `iretq`s straight back to ring-3. This is the single-app-synchronous policy — user apps are not descheduled during execution.

## What's NOT wired up

- **No isolation.** All kernel code shares one address space (ring 0). Ring-3 processes have a USER-bit-protected slice of the same address space (added by the userland subsystem at `src/userland/`).
- **No real concurrency.** Single execution thread per CPU; the scheduler multiplexes processes but does not exploit multiple cores.
- **No IPC.**

## Adding shell commands

Lives in `src/commands/` — see `src/commands/CLAUDE.md` for the recipe.

## Cross-references

- Naked-asm context switching and timer ISR: `src/arch/x86_64/CLAUDE.md`.
- Userland (ring-3) processes: `src/userland/CLAUDE.md` (under construction).
- Command implementations and the add-a-command recipe: `src/commands/CLAUDE.md`.
- The shell that drives command dispatch: `src/commands/shell/`.
