# `src/process/` — Process Management

**Today, this is a command dispatcher, not real process management.** All "processes" run synchronously in kernel space. Scheduler / context-switch scaffolding is present in this folder but not wired up.

## Key files

- `process.rs` — `Process` and `BaseProcess` traits; sequential PID allocation (no reuse, starts at 1).
- `manager.rs` — command registry; maps command names to factory functions; the shell routes unknown commands here.
- `pcb.rs` — process control block scaffolding.
- `context.rs` — CPU context scaffolding for future context switching.
- `scheduler.rs` — scheduler scaffolding.
- `stack.rs` — kernel stack scaffolding.

## What's wired up today

- Process / `BaseProcess` traits define the interface.
- Sequential PID allocation.
- Command registry: `register_command(name, factory)` populates a name → factory map.
- Shell integration: the shell calls `execute_command(name args…)`, which dispatches via the registry. Commands run to completion synchronously.

## What's NOT wired up (despite files existing)

- **No scheduling** — `scheduler.rs` exists but the kernel never calls into it.
- **No context switching** — `context.rs` and `stack.rs` are scaffolding for future use.
- **No isolation** — all code shares kernel memory (ring 0).
- **No concurrency** — single execution thread.
- **No IPC** — no inter-process communication.

If you're planning real multitasking, the scaffolding is a starting point but not a finished design — expect to revisit it. If you're adding shell commands today, ignore the scheduler files entirely.

## Adding shell commands

Lives in `src/commands/` — see `src/commands/CLAUDE.md` for the recipe.

## Cross-references

- Command implementations and the add-a-command recipe: `src/commands/CLAUDE.md`.
- The shell that drives command dispatch: `src/commands/shell/`.
