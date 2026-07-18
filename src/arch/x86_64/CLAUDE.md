# `src/arch/x86_64/` — x86_64 Architecture

Low-level x86_64 plumbing: SMP/AP bring-up, ACPI/APIC interrupt routing,
per-CPU GDT/TSS state, naked-asm context switching, and preemption.

## Key files

- `acpi.rs` — allocation-free RSDP/RSDT/XSDT/MADT discovery, capped at `MAX_CPUS = 8` with a BSP-only fallback.
- `smp.rs` — INIT-SIPI AP startup through the `0x8000` trampoline, LAPIC-timer calibration, AP idle/dispatch loop, and work/panic IPIs.
- `percpu.rs` — fixed per-CPU state and the GS ABI (`gs:[0]` kernel stack, `gs:[8]` user RSP scratch, `gs:[16]` logical CPU id), plus lock-free monotonic user/system/idle timer accounting consumed by `/proc/stat`.
- `lapic.rs` / `ioapic.rs` — local APIC timers/EOI/IPIs and BSP-affine external IRQ routing.
- `gdt.rs` — identical-selector per-CPU GDTs, TSSes, rsp0 stacks, and double-fault IST stacks.
- `fpu.rs` — `enable_sse()` configures CR0/CR4 for SSE/SSE2 execution. Required before any ring-3 transition; musl + libstdc++ binaries emit SSE2 in `__init_tls` before reaching `main` and would `#UD` without it. The kernel target spec uses `+soft-float`, so the kernel itself never needs SSE — but ring 3 does.
- `interrupts.rs` — shared IDT setup, exceptions, PIT, APIC/IOAPIC cutover with legacy-PIC fallback, and hardware-IRQ entry points.
- `rtc.rs` — bounded, stable PC CMOS RTC snapshots with BCD/binary and
  12/24-hour decoding. Sampled once by `crate::time` after PIT initialization;
  it must always restore NMI-enabled state and degrade to an error rather than
  stalling boot.
- `context_switch.rs` — naked-asm `switch_*` functions used by the cooperative scheduler.
- `preemption.rs` — naked timer entry plus the privilege-neutral scheduler handoff. The BSP PIT advances global time and timer work; AP LAPIC timers only record local ticks and preempt. CPL=3 saves `UserState`; CPL=0 saves the complete `CpuContext`, including the IA-32e hardware SS:RSP fields.
- `preemption_guard.rs` — nesting-safe `PreemptionGuard` and `PreemptionMutex`. They leave hardware IRQs enabled while deferring local kernel-thread preemption; nesting depth is per-CPU and cross-CPU exclusion comes only from the inner spin lock.
- `interrupt_guard.rs` — RAII guard for `cli`/`sti` regions plus `InterruptMutex`, the required mutex wrapper for state shared by timer-preemptible kernel threads and IF-cleared interrupt/SYSCALL paths. The VirtIO block registry uses it so PCI completion cannot interrupt a queue mutation (see `src/drivers/CLAUDE.md`).

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

Kernel `CpuContext` values use CS=0x08 / SS=0x10 and user transitions derive
their selectors from this fixed ordering. **Do not reorder slots 1 or 2.**
The user_data-before-user_code order is also load-bearing: `syscall`/`sysret`
derives `user_cs = STAR[63:48] + 16` and `user_ss = STAR[63:48] + 8` by
formula. Adding descriptors must append after the TSS.

PCI INTx IOAPIC routes are active-low/level-triggered. VirtIO interrupt
handlers use `try_lock`; an unacknowledged interrupt must remain asserted and
retrigger after EOI rather than being lost as an edge.

Every completed kernel context save is published to the scheduler before the
target entity runs, including direct kernel→ring3 handoffs. Ring3→kernel
transfers restore the permanent kernel CR3 so address-space teardown on a
different CPU cannot invalidate a page table still active in kernel mode.

Every local scheduling-timer edge charges exactly one `CpuLocal` time bucket
before the interrupt path can return: CPL=3 is user, spawned kernel-thread
execution is system, and an idle loop inside its published `sti; hlt` window
is idle. BSP/AP main-loop housekeeping outside that window is system. The
counters are monotonic and never require the scheduler or process-table lock.

## TSS

Each CPU owns a TSS. `privilege_stack_table[0]` (`rsp0`) points at that CPU's
current ring-3 process kernel stack and is updated on migration.
`interrupt_stack_table[0]` points at a private double-fault stack.

## Boot ordering

`gdt::init()` runs *before* `interrupts::init_idt()` in `src/kernel.rs`. The IDT entry for `#DF` references IST index 0 in the TSS; the CPU consults the TSS only at fault time, so loading the IDT before the TSS is in TR is technically safe — but if the very first interrupt arrives between the two calls, IST lookup would fail. Keep the order: GDT → IDT/PIT → `time::init()` RTC anchor → … .

## What the userland platform adds

- `gdt.rs` exposes `selectors()` so the userland's iretq frame can be built with the correct user CS (0x23) and user SS (0x1B) selectors with RPL=3.
- The `#DF` IST stack protects the kernel when exception-handler refactors begin to deal with user-mode faults (see `src/userland/CLAUDE.md` once it lands).

## Cross-references

- Process traits and PCB: `src/process/CLAUDE.md`.
- Memory mapper / paging: `src/mm/CLAUDE.md`.
- `no_std` / panic-handler / testing rules: `.claude/rules/`.
