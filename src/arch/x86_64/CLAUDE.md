# `src/arch/x86_64/` — x86_64 Architecture

Low-level x86_64 plumbing: GDT/TSS, IDT, naked-asm context switching, preemption, interrupt-disable guard.

## Key files

- `gdt.rs` — GDT layout, TSS, IST stacks. Loaded once at boot via `gdt::init()`.
- `interrupts.rs` — IDT setup, all exception handlers, PIC/PIT configuration, hardware-IRQ entry points.
- `context_switch.rs` — naked-asm `switch_*` functions used by the cooperative scheduler.
- `preemption.rs` — naked-asm `timer_interrupt_handler_preemptive` and Rust-side `timer_handler_inner`. Round-robin preemptive scheduler.
- `interrupt_guard.rs` — RAII guard for `cli`/`sti` regions.

## GDT layout (load-bearing)

```
slot   selector   descriptor               DPL
 0     0x00       null                       —
 1     0x08       kernel code (64-bit)        0
 2     0x10       kernel data                 0
 3     0x18       user data                   3
 4     0x20       user code (64-bit)          3
 5,6   0x28       TSS (16-byte system desc)   —
```

The kernel CS=0x08 / SS=0x10 selectors are hard-coded as literal pushes inside the naked asm in `preemption.rs` (lines around the `iretq` frame construction) and `context_switch.rs`. **Do not reorder slots 1 or 2.** The user_data-before-user_code order is also load-bearing: it keeps the door open for `syscall`/`sysret` later, which derives `user_cs = STAR[63:48] + 16` and `user_ss = STAR[63:48] + 8` by formula. Adding new descriptors should append after the TSS, not insert before it.

## TSS

Single static instance. `privilege_stack_table[0]` (`rsp0`) holds the kernel stack to which the CPU switches on a ring 3 → ring 0 transition (interrupt or exception). For single-app-at-a-time, `rsp0` is set once before each `run` and not updated again until the next user app. `interrupt_stack_table[0]` is the dedicated `#DF` stack — kernel-stack overflow during user-mode work would otherwise triple-fault.

## Boot ordering

`gdt::init()` runs *before* `interrupts::init_idt()` in `src/kernel.rs`. The IDT entry for `#DF` references IST index 0 in the TSS; the CPU consults the TSS only at fault time, so loading the IDT before the TSS is in TR is technically safe — but if the very first interrupt arrives between the two calls, IST lookup would fail. Keep the order: GDT → IDT → … .

## What the userland platform adds

- `gdt.rs` exposes `selectors()` so the userland's iretq frame can be built with the correct user CS (0x23) and user SS (0x1B) selectors with RPL=3.
- The `#DF` IST stack protects the kernel when exception-handler refactors begin to deal with user-mode faults (see `src/userland/CLAUDE.md` once it lands).

## Cross-references

- Process traits and PCB: `src/process/CLAUDE.md`.
- Memory mapper / paging: `src/mm/CLAUDE.md`.
- `no_std` / panic-handler / testing rules: `.claude/rules/`.
