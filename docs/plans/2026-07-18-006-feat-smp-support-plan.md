---
title: "feat: SMP bring-up — LAPIC/IOAPIC, per-CPU state, AP boot, multi-core scheduling"
status: implemented
created: 2026-07-18
plan_type: feat
depth: deep
related_docs:
  - CLAUDE.md
  - src/process/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/mm/CLAUDE.md
  - src/input/CLAUDE.md
  - docs/plans/2026-07-18-005-refactor-unified-kernel-ring3-scheduler-plan.md
  - docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md
---

# feat: SMP bring-up — LAPIC/IOAPIC, per-CPU state, AP boot, multi-core scheduling

## Outcome

The kernel boots on the BSP, enumerates CPUs from the ACPI MADT, starts each
AP through an INIT-SIPI-SIPI trampoline, and schedules kernel threads and
ring-3 processes across all cores from the existing single unified run queue.
Interrupt delivery moves from the legacy 8259 PIC to LAPIC + IOAPIC, with all
device IRQs pinned to the BSP and a per-CPU LAPIC timer driving preemption on
every core. Idle cores `hlt` and are woken by a reschedule IPI when work is
enqueued.

End state on `-smp 4`:

```text
BSP (cpu 0)                     AP (cpu 1..3)
  boot, MADT parse, AP start      trampoline: real → long mode
  IOAPIC: all device IRQs         load kernel CR3, per-CPU GS/GDT/TSS
  PIT wall clock (time.rs)        LAPIC timer @ 100 Hz
  LAPIC timer @ 100 Hz            pick from shared run queue
        \                          /
         one SCHEDULER lock, one run queue, one timer heap
         per-CPU: current entity, KERNEL_CONTEXT, PerCpu, rsp0, idle loop
```

A `./build.sh` boot with `AGENTICOS_QEMU_SMP=1` (or on a machine with no
usable MADT) behaves exactly as today — every unit must keep the single-CPU
path green.

## Current-state evidence

The unified scheduler refactor
(`docs/plans/2026-07-18-005-refactor-unified-kernel-ring3-scheduler-plan.md`,
implemented) already provides the SMP-shaped core: one `EntityId`-keyed
registry, one privilege-neutral `RunQueue` (`src/process/run_queue.rs:9`,
`MAX_ENTITIES = 256`), one indexed timer min-heap
(`src/process/timer.rs:305-308`), and `SCHEDULER` behind `InterruptMutex`
(`src/process/scheduler.rs:68`). What remains single-CPU is the machine layer
and a set of globals:

**Interrupts: legacy PIC, no APIC anywhere.**

- `pic8259::ChainedPics` with offsets 32/40 (`src/arch/x86_64/interrupts.rs:5`,
  `:37-41`); PIT channel 0 at 100 Hz (`interrupts.rs:31-35`, `:172-200`);
  EOI via `PICS.lock().notify_end_of_interrupt(..)` in every handler.
- A full-tree grep for `apic|lapic|ioapic|x2apic` finds nothing. There is no
  IPI mechanism, no LAPIC timer, no interrupt routing beyond PIC masks
  (`interrupts.rs:252-264`).
- `boot_info.rsdp_addr` is captured and only logged
  (`src/kernel.rs:58`, `:116-118`). No ACPI parser exists
  (`src/arch/x86_64/rtc.rs:140-141` notes this explicitly).

**Per-CPU state that is a singleton today.** Each of these carries a code
comment saying SMP must change it:

- `static mut PERCPU` — the one GS-based per-CPU block the SYSCALL stub uses
  to find the kernel stack (`src/arch/x86_64/syscall.rs:76-91`; stub at
  `:148-150` does `swapgs; mov gs:[8], rsp; mov rsp, gs:[0]`).
  `src/arch/x86_64/msr.rs:12-14` spells out the per-AP re-programming needed.
- `PREEMPTION_DISABLE_DEPTH: AtomicUsize` is global "because AgenticOS
  currently has one CPU" (`src/arch/x86_64/preemption_guard.rs:9-15`).
- One GDT/TSS/`KERNEL_RSP0_STACK`/`DOUBLE_FAULT_STACK`
  (`src/arch/x86_64/gdt.rs:24-53`); `set_kernel_rsp0` mutates the single TSS
  via raw pointer (`gdt.rs:122-125`).
- `static mut KERNEL_CONTEXT: CpuContext` — the shared kernel shadow context
  every user→kernel return path targets (`src/arch/x86_64/preemption.rs:24`).
- Two "current" pointers, both single-slot: `Scheduler.current`
  (`scheduler.rs:79`) and `ProcessTable.current_user_pid`
  (`src/userland/lifecycle.rs:252`, doc at `:53` calls it "the single active
  per-CPU user-process slot").
- `InterruptMutex` documents that a plain spin mutex "is not safe" only
  because the kernel is single-CPU (`src/arch/x86_64/interrupt_guard.rs:24-30`).
  Its actual construction — IF-mask + `spin::Mutex` — is already the correct
  SMP shape.

**Memory layer.**

- The heap allocator is already `InterruptMutex`-guarded (`src/mm/heap.rs:9-12`).
- The frame allocator and page-table mapper have **no lock at all**:
  `static mut MAPPER: Option<*mut MemoryMapper>` reached via `get_mapper()`
  (`src/mm/paging.rs:156-160`) and `with_memory_mapper`
  (`src/mm/memory.rs:223-227`). `src/mm/CLAUDE.md` forbids a naive mutex
  because the page-fault handler also takes this path on the same core.
- Per-process L4s share the kernel half by PML4-entry copy
  (`src/userland/address_space.rs:198-209`); kernel heap and kernel-thread
  stacks live in fixed shared PML4 slots. All TLB maintenance is local-CPU
  (`paging.rs:351`, `:383`, `:467`; full flush after fork clone at
  `address_space.rs:186`). No shootdown exists.
- The custom `Arc` refcounts are already SMP-correct atomics
  (Relaxed inc, Release dec + Acquire fence — `src/lib/arc.rs:30-31`,
  `:162`, `:186-199`).

**Scheduling and idle.**

- The PIT handler branches on interrupted CPL, saves context, and dispatches
  via `preempt_and_pick` (`src/arch/x86_64/preemption.rs:144-333`).
- Every blocking path's terminal fallback is a local `hlt`
  (`src/userland/switch.rs:152-154`, `src/kernel.rs:815-825`). Wakes are
  driven entirely by this CPU's ISRs; nothing can nudge another core.
- Signal delivery happens only at the syscall dispatcher tail of the process
  that just trapped (`src/userland/syscalls.rs:1900-1956`); `kill(2)` from
  another process relies on the target eventually trapping on the same CPU.

**Assumptions that break with a second CPU.**

- `INPUT_QUEUE` is a lock-free SPSC ring whose single-producer guarantee is
  "interrupts are disabled during ISR" (`src/input/queue.rs:76-105`);
  `src/input/CLAUDE.md` says multi-producer is a redesign. Two cores taking
  keyboard IRQs concurrently would corrupt it.
- `static mut VFS` via `get_vfs() -> &'static mut`
  (`src/fs/vfs.rs:153-156`) and the mounted-fs backing arrays
  (`vfs.rs:11-26`) are unsynchronized.
- COM1 debug logging takes no lock (`src/lib/debug.rs:21-69`); concurrent
  cores would interleave bytes mid-line. `DEBUG_LEVEL` is a non-atomic
  `static mut` (`debug.rs:11-19`).
- `PICS` is a plain `spin::Mutex` locked from the timer ISR
  (`interrupts.rs:40`) — goes away with the PIC itself.

## Goals

1. Enumerate CPUs from the ACPI MADT (RSDP handed over by the bootloader) and
   boot up to `MAX_CPUS = 8` application processors; degrade gracefully to
   single-CPU when the MADT is missing or `-smp 1`.
2. Replace the 8259 PIC with LAPIC (EOI, timer, IPIs) + IOAPIC (device IRQ
   routing), with every device IRQ pinned to the BSP so existing
   single-producer/driver assumptions hold unchanged.
3. Give every CPU its own: GS-based per-CPU block (cpu id, syscall stack top,
   scratch), GDT + TSS + rsp0 + double-fault IST stack, `KERNEL_CONTEXT`,
   preemption-disable depth, current-entity slot, and idle loop.
4. Schedule the existing unified run queue across all cores under the existing
   `SCHEDULER` lock — no per-CPU queues, no load balancer in this plan — with
   a reschedule IPI so idle (`hlt`) cores pick up newly enqueued work and
   pending fatal signals promptly.
5. Preserve all invariants from the unified-scheduler plan, extended
   cross-CPU: an entity's context save must complete (and be published under
   the scheduler lock) before any other CPU can pick it.
6. Keep the memory layer correct: a lock policy for the mapper/frame
   allocator that is cross-CPU safe and still deadlock-free from page-fault
   context, plus explicit TLB rules that exploit "one address space is active
   on at most one CPU at a time."
7. `./test.sh` passes with `-smp 4` and with `-smp 1`; new tests cover AP
   bring-up, per-CPU ticks, cross-CPU dispatch, and a fork/exec stress.

## Non-goals

- Per-CPU run queues, work stealing, affinity, `nice`, or any load-balancing
  policy beyond "any idle CPU may pop the shared queue."
- User-level threads (`clone` with shared VM). Processes stay single-threaded;
  this is what keeps TLB shootdown out of scope.
- Distributing device IRQs across cores, MSI/MSI-X, or interrupt-driven
  VirtIO-net (still polled by the net worker, which may now run on any core).
- Fine-grained locking of the VFS, window system, terminal, or net stack.
  They keep their current coarse locks; the only requirement is that those
  locks become *real* (no unsynchronized `static mut` reachable from two
  CPUs).
- NUMA, CPU hotplug, deep C-states, tickless idle, x2APIC (xAPIC MMIO only —
  works everywhere QEMU runs, including TCG and HVF).
- SMP-parallel in-kernel test framework: tests still run sequentially on the
  BSP; APs participate only as scheduling targets.

## Required invariants

1. **Save-before-steal.** A preempted or blocking entity's full context
   (kernel `CpuContext` or user `UserState` + FS/FPU) is written back before
   the entity becomes visible in the run queue, and both happen under the
   scheduler lock. No CPU may resume an entity whose save is in flight.
2. **One CPU per address space.** A user process is `Running` on at most one
   CPU. Since processes are single-threaded and all mutations of a process's
   page tables happen from its own syscall/fault context on the CPU running
   it, local `invlpg`/CR3-reload is sufficient; no shootdown IPI is needed
   for user mappings.
3. **Kernel mappings only grow.** Shared-kernel-half mappings transition
   invalid→valid only (heap is fully mapped at boot; kernel-thread stacks are
   pre-faulted on allocate, `src/process/stack.rs:109-114`). x86 does not
   cache not-present entries, so no cross-CPU flush is required. Any future
   kernel-VA unmap/remap (e.g. reclaiming kernel stacks) must first add a
   broadcast flush IPI — until then, stack slots are recycled without
   unmapping, exactly as today.
4. **All shared kernel PML4 slots exist before the first user L4 is built**
   (they are copied by value into each process L4). Creating a *new* kernel
   PML4 slot after boot is a bug; assert it.
5. **IF-masking is per-CPU; exclusion is the lock.** `InterruptMutex` keeps
   its shape (local cli + spin), but no code may assume `cli` alone provides
   exclusion. The mapper's `static mut` access is the one holdout and is
   fixed in U2.
6. **Lock-hold discipline for the mapper:** no path may page-fault while
   holding the mapper lock (kernel heap and stacks are pre-mapped, so this
   holds today; assert via a per-CPU "in mapper" flag in debug builds).
   The page-fault handler may take the mapper lock — it spins only on
   *another* CPU's holder, never self-deadlocks, because of this rule.
7. **Device-IRQ affinity.** IOAPIC redirection entries for keyboard, mouse,
   PIT, and PCI INTx target the BSP only. The `INPUT_QUEUE` SPSC contract
   (one producer = one ISR on one core) is thereby preserved without redesign.
8. **Per-CPU dispatch state.** `KERNEL_CONTEXT`, `PerCpu`, TSS.rsp0,
   `current_user_pid`, and `Scheduler.current` become per-CPU slots indexed
   by `cpu_id()` (read from GS). `current_user_pid[cpu]` must agree with
   `Scheduler.current[cpu] == UserProcess(pid)` whenever set.
9. **EOI exactly once, now via LAPIC.** Same single-EOI-before-diverge rule
   as the unified plan, retargeted from `PICS` to the local APIC's EOI
   register.
10. **Panic freezes the machine.** The panic handler sends a halt IPI (NMI or
    fixed vector with a "halted" flag) to all other CPUs before printing, so
    serial output is not interleaved and `isa-debug-exit` codes stay
    deterministic in test mode.
11. **Timer heap has one driver.** The PIT tick on the BSP remains the sole
    wall-clock (`TIMER_TICKS`, `src/time.rs` anchor) and the sole trigger for
    timer-service wakes. AP LAPIC timers drive *preemption only*; they do not
    advance global time or touch the heap.
12. **Single-CPU mode is first-class.** MADT absent, `MAX_CPUS = 1`, or
    `-smp 1` must produce today's behavior with the only difference being
    LAPIC/IOAPIC instead of PIC.

## Core design

### 1. CPU discovery — minimal MADT walk (U0)

Hand-roll a small parser (`src/arch/x86_64/acpi.rs`, ~200 lines) rather than
pulling the `acpi` crate: RSDP (from `boot_info.rsdp_addr`, already plumbed at
`src/kernel.rs:58`) → validate checksum → RSDT/XSDT → MADT (signature
`APIC`) → iterate entries, collecting type-0 Local APIC records
(processor present + enabled flags) and the type-1 I/O APIC record (MMIO base,
GSI base). All physical reads go through the existing
`phys_to_virt` (`src/mm/memory.rs:154-156`)… when covered by the bootloader's
physical-memory map; ACPI tables live in reserved RAM regions that the
bootloader's dynamic mapping covers. If any checksum or signature fails, log
and fall back to `cpus = [bsp]`.

Output: `struct CpuTopology { bsp_lapic_id, cpus: ArrayVec<CpuInfo, MAX_CPUS>,
ioapic: Option<IoApicInfo>, lapic_mmio_base: u64 }`, stored once at boot.

### 2. Interrupt controller cutover: PIC → LAPIC + IOAPIC (U1)

This lands and soaks **while still single-CPU** — it is the highest-risk
mechanical change and must be bisectable independently of AP boot.

- **LAPIC (xAPIC MMIO).** Map the 4 KiB LAPIC page (default `0xFEE0_0000`,
  MADT-provided) uncached via the mapper — it is above the 2 GiB RAM top, so
  it is *not* covered by the physical-memory offset mapping and needs an
  explicit kernel mapping. Wrap in `src/arch/x86_64/lapic.rs`: enable via
  SVR (spurious vector `0xFF`), error vector `0xFE`, EOI register, ICR for
  IPIs, timer LVT.
- **IOAPIC.** Map its MMIO page (typically `0xFEC0_0000`); program
  redirection entries for the IRQs the kernel actually uses — 0 (PIT),
  1 (keyboard), 12 (mouse), and PCI INTx lines 3-15 as currently unmasked by
  `enable_pci_irq` — keeping the existing vector numbers 32..47 so the IDT
  (`interrupts.rs:61-166`) is untouched. Destination: BSP LAPIC id, fixed
  delivery, edge/active-high for ISA (honor MADT interrupt-source-override
  entries for IRQ0→GSI2 etc.).
- **PIC disposal.** Remap then mask both 8259s entirely (`0xFF` to both data
  ports). Delete `PICS` and replace every `notify_end_of_interrupt` with
  `lapic::eoi()` — one store, no lock, resolving the "spin::Mutex locked from
  ISR" wart at `interrupts.rs:40` for free.
- **Timer.** PIT keeps ticking IRQ0→vector 32 on the BSP as the wall clock
  and BSP preemption source, unchanged. LAPIC timer stays off in this unit.

### 3. Per-CPU state (U2)

New `src/arch/x86_64/percpu.rs`:

```rust
#[repr(C)]
pub struct CpuLocal {
    // offsets 0/8 are ABI: the SYSCALL stub reads gs:[0]/gs:[8]
    kernel_rsp_top: u64,          // gs:[0]
    user_rsp_scratch: u64,        // gs:[8]
    cpu_id: u32,                  // gs:[16]
    preemption_disable_depth: AtomicUsize,
    kernel_context: CpuContext,   // replaces static mut KERNEL_CONTEXT
    current_user_pid: Option<u32>,
    tss: TaskStateSegment,
    // rsp0 stack, double-fault IST stack, idle-thread bookkeeping
}
```

- One `CpuLocal` per CPU, heap-allocated at bring-up (BSP's during
  `kernel::init`, replacing `PERCPU`); `GsBase`/`KernelGsBase` point at it
  (`msr.rs:71-75` already does this for the singleton). `cpu_id()` is a
  `mov eax, gs:[16]`.
- Per-CPU GDT+TSS: each CPU needs its own TSS descriptor (busy-flag and
  rsp0 are per-CPU) — build one GDT per CPU with identical selector layout
  (0x08/0x10/0x18/0x20/0x28, load-bearing in the asm stubs, `gdt.rs:10-16`).
  `set_kernel_rsp0` becomes a write through GS to the local TSS. The IDT
  stays a single shared read-only table; each CPU executes `lidt` on it.
- `PreemptionGuard`/`PreemptionMutex` switch from the global
  `PREEMPTION_DISABLE_DEPTH` to `gs`-relative depth. The timer handler's
  `kernel_preemption_allowed()` check (`preemption.rs:216`) reads the local
  CPU's depth — which is the correct semantic: preemption-off is a per-CPU
  property; cross-CPU exclusion is only ever provided by locks.
- **Mapper lock.** Replace the raw `static mut MAPPER` access with a
  dedicated `InterruptMutex<MemoryMapper>` honoring invariant 6. Fault-path
  usage keeps working: with IF cleared it can still spin on a remote holder,
  and the no-fault-while-held rule prevents self-deadlock. `with_memory_mapper`
  keeps its signature so ~all call sites are untouched.
- Scheduler: `Scheduler.current: Option<EntityId>` becomes
  `current: [Option<EntityId>; MAX_CPUS]`; `preempt_and_pick`/`pick_next`
  take the calling `cpu_id`. `ProcessTable.current_user_pid` likewise becomes
  per-CPU (it's already documented as aspirationally per-CPU,
  `lifecycle.rs:53`).

All of U2 is refactoring that runs on one CPU; behavior must be identical.

### 4. AP boot (U3)

- **Trampoline.** A ≤4 KiB blob assembled into the kernel image
  (`global_asm!`), copied at boot to a reserved low page (target `0x8000`).
  Reserve it by claiming the frame from the boot memory map before the frame
  allocator initializes; if no usable sub-1 MiB page exists (it does under
  QEMU/BIOS and UEFI alike, but be defensive), SMP is disabled with a log
  line. Sequence: 16-bit real → protected (temp GDT in the blob) → enable
  PAE+LM with the kernel's CR3 (physical address patched into the blob) →
  64-bit, load per-AP RSP and jump to `ap_main(cpu_id)`. The kernel L4's
  identity/physical-offset mapping covers the trampoline page for the paged
  hop.
- **Kick.** BSP sends INIT, waits 10 ms, SIPI (vector = trampoline page >> 12),
  optional second SIPI after 200 µs if the AP hasn't checked in (standard
  MP-spec dance; use the PIT for the delays). Per-AP handshake word with
  timeout → "cpu N failed to start" is a boot warning, not a panic.
- **`ap_main`:** install shared IDT, own GDT/TSS, program `IA32_EFER`
  SCE+NXE, STAR/LSTAR/SFMASK, GS bases (re-running
  `program_syscall_msrs`/`init_gs_base` per `msr.rs:12-14` exactly as that
  comment prescribes), enable its LAPIC, calibrate the LAPIC timer against
  the BSP's PIT tick count, then park in the per-CPU idle loop with
  interrupts on — **not yet scheduling** in this unit.
- LAPIC timer on each AP (and optionally the BSP, later) fires the existing
  timer vector path; in U3 the AP handler only EOIs and increments a per-CPU
  diagnostic counter.

### 5. Multi-core scheduling (U4)

- **Dispatch.** The timer/yield/block paths already funnel through
  `preempt_and_pick` + the dispatch adapters. Under SMP the same code runs on
  each CPU against the shared `SCHEDULER` lock; the pick loop skips entities
  whose save hasn't been published (impossible by invariant 1's
  lock-ordering, but assert it) and writes `current[cpu]`.
- **Idle.** `kernel::run()`'s tail (`src/kernel.rs:815-825`) becomes the
  BSP's idle loop; APs run the same function body. The existing
  `cli → recheck → enable_and_hlt` sequence is already race-free against the
  local CPU's interrupts; the missing piece is remote wake.
- **Reschedule IPI** (vector `0xF0`): `make_ready` (and the timer service's
  wake delivery) sends a fixed IPI to one hlt'ed CPU when the queue was
  empty-ish, tracked by a per-CPU "idle and interruptible" atomic flag set
  around the `hlt`. The IPI handler is EOI-only — its entire job is punching
  the target out of `hlt` so its idle loop re-runs the pick.
- **Signals/kill.** `wake_ring3_for_signal` additionally checks whether the
  target pid is *currently running* on some CPU (`current[cpu]` scan under
  the scheduler lock) and sends that CPU a reschedule IPI. The IPI forces a
  trip through the timer-interrupt-shaped preemption path, whose
  next-dispatch runs the pending-fatal-signal check — this closes (on SMP
  *and* shrinks on UP) the documented "CPU-bound process outruns SIGKILL"
  gap. Delivery itself still happens at the target's own dispatcher tail;
  no cross-CPU frame surgery.
- **Migration.** Nothing special: a user process resumed on a different CPU
  gets its CR3 loaded fresh (`switch.rs:266-269`), its rsp0/gs:[0] written to
  the *local* TSS/`CpuLocal` (`switch.rs:273-276` retargeted through GS), and
  stale TLB entries on the old CPU die on that CPU's next CR3 load
  (non-global mappings). Kernel threads have no CR3 of their own and migrate
  trivially. `IN_SPAWNED_PROCESS` and other one-slot dispatch flags
  (`src/process/mod.rs:20`) move into `CpuLocal` or are retired where the
  unified scheduler already made them redundant.
- **Timer service / compositor / net worker** stay ordinary contracted
  entities and may run on any core; their latency contracts get *easier* with
  more CPUs. `PENDING_IO_WAKES` (`mod.rs:23`) is already atomic; drains move
  to wherever dispatch happens, unchanged.

### 6. Shared-state hardening (U5)

Scoped strictly to "reachable from two CPUs, currently unsynchronized":

- **VFS**: `get_vfs() -> &'static mut` becomes a lock. Given every consumer
  is a syscall or kernel thread (never an ISR), a `PreemptionMutex` or plain
  spin::Mutex wrapper with today's coarse granularity suffices. The mounted-fs
  arrays fold under the same lock.
- **Serial/debug**: one spin lock around COM1 line output with a
  `try_lock`-and-print-anyway escape in the panic path (garbled output beats
  a deadlocked panic). `DEBUG_LEVEL` → `AtomicU8`.
- **`NEXT_PID` static mut** (`src/process/process.rs:3`) → atomic, matching
  the ring-3 one (`lifecycle.rs:231`).
- **Audit sweep**: re-grep `static mut` and per-subsystem CLAUDE.md notes;
  everything already behind `InterruptMutex`/`PreemptionMutex`/spin locks
  (scheduler, process table, timers, drivers registry, window manager, GUI
  state maps, PTY registry, net) is SMP-correct once preemption depth is
  per-CPU and needs no change. Document the survivors that are safe by
  construction (boot-once-then-read-only) as such where they're declared.
- **Input**: no change needed — invariant 7 pins the producers to the BSP.
  Add a debug assertion in `InputQueue::push` that `cpu_id() == 0`.

### 7. What stays deliberately coarse

One scheduler lock, one run queue, one timer heap, BSP-only device IRQs,
coarse subsystem locks. At ≤8 CPUs with a 100 Hz quantum, lock hold times
(bounded, allocation-free by the unified plan's invariants) are a few
microseconds against a 10 ms slice — contention is noise. Per-CPU queues,
IRQ steering, and lock splitting are all follow-up plans with this one's
telemetry as their justification.

## Work sequence

Each unit is a separate review/merge boundary and keeps `-smp 1` green.

### U0 — MADT enumeration (report-only)
`acpi.rs` parser + `CpuTopology`; boot log prints discovered CPUs/IOAPIC.
No behavior change. Tests: parser unit tests against hand-built table bytes;
boot test asserting ≥1 CPU found under QEMU.

### U1 — LAPIC + IOAPIC cutover, PIC retired
Still single-CPU. LAPIC/IOAPIC drivers, redirection entries for the live
IRQs, LAPIC EOI everywhere, 8259 masked, `PICS` deleted. Soak: full
`./test.sh`, interactive boot with keyboard/mouse/net/disk exercised.

### U2 — Per-CPU state refactor (one CPU still)
`CpuLocal`, per-CPU GDT/TSS, GS-based preemption depth, per-CPU
`KERNEL_CONTEXT`/`current`/`current_user_pid` (arrays sized `MAX_CPUS`, only
slot 0 used), mapper behind `InterruptMutex`. Pure refactor; identical
behavior is the acceptance bar.

### U3 — AP boot to parked idle
Trampoline, INIT-SIPI-SIPI, `ap_main` through MSR/GDT/LAPIC-timer setup into
a diagnostic idle loop. `AGENTICOS_QEMU_SMP` env plumbs `-smp N` (default 4)
in `build.sh`/`test.sh`. Tests: all APs check in; per-CPU LAPIC tick counters
advance; whole suite still passes (APs parked).

### U4 — SMP dispatch
APs enter the shared pick loop; reschedule IPI; per-CPU idle; signal-kick
IPI; migration paths (rsp0/GS through `CpuLocal`). Tests below.

### U5 — Shared-state hardening + audit
VFS/serial/PID-allocator locks and the `static mut` sweep. Stress tests run
here.

### U6 — Docs and telemetry
Per-CPU counters in the render-stats/scheduler-telemetry style
(dispatches per CPU, IPIs sent/received, lock-contention samples), CLAUDE.md
updates (root "No SMP" limitation, `src/process/`, `src/userland/`,
`src/arch/` notes, `src/input/` producer contract), and this plan's
implementation notes.

## Implementation notes

Implemented on 2026-07-18. The delivered machine layer includes the bounded
MADT parser, xAPIC/IOAPIC cutover with PIC fallback, the low-memory AP
trampoline, per-CPU GS/GDT/TSS/IST and scheduler state, AP-local preemption
timers, reschedule and panic-stop IPIs, and the shared-run-queue AP idle loop.
The mapper, VFS, stack allocator, serial logger, PID allocator, and the other
single-CPU globals identified above were hardened or moved into `CpuLocal`.

The context-switch implementation also gained an SMP-specific publication
protocol: an entity is not re-enqueued until its save is complete, scheduling
claims are protected through the architecture handoff, and every kernel
restore consumes the complete GPR image. This last rule is load-bearing when
a timer-preempted thread is resumed by a voluntary block/exit path on another
CPU. Kernel-thread termination likewise changes to the per-CPU main-loop stack
before returning the abandoned stack to the shared allocator, so a concurrent
spawn cannot reuse memory that still contains live termination frames. A
cross-CPU watchdog kill is deferred to the owning CPU's next safe timer
boundary for the same reason.

Interactive GUI-launch qualification exposed three additional ordering rules:
the CR3-sensitive ELF setup interval is locally non-preemptible and restores
the kernel L4 before publication; every ring3→kernel transfer restores that
kernel L4 before remote reaping; and PCI INTx remains active-low/level so a
contended `try_lock` ISR retriggers. Ring-3 block completions are deferred until
the scheduler's Blocked transition is visible, closing wake-before-block.

Automated coverage is in `src/tests/smp.rs`. Validation completed with
`AGENTICOS_QEMU_SMP=1` and `AGENTICOS_QEMU_SMP=4`: all 863 kernel tests pass
in both configurations, including the remote-termination regression. The
combined SMP, scheduler, and interrupt topics,
all 184 userland tests under four CPUs, and repeated one-/four-CPU VirtIO
block wake tests also pass.
The interactive desktop/workload and repeated
full-suite soak scenarios below remain release qualification procedures rather
than assertions made by the focused in-kernel tests.

## Tests and validation

- **Unit (in-kernel):** MADT parser fixtures; trampoline patch-site checks;
  `CpuLocal` offset asserts (gs:[0]/[8]/[16] are ABI); IOAPIC redirection
  readback.
- **Bring-up:** with `-smp 4`, all 4 CPUs check in and tick; with `-smp 1`
  and with MADT parsing force-failed, boot equals today's.
- **Dispatch:** spawn N CPU-bound kernel threads, assert >1 CPU accumulates
  runtime; ring-3 process observed running on different CPUs across slices
  (per-CPU dispatch counters); reschedule-IPI latency: wake of a blocked
  entity while all CPUs hlt'ed dispatches within 2 ticks.
- **Kill:** SIGKILL a CPU-bound busy-loop ring-3 process from another shell —
  terminates within one slice (the IPI-kick path).
- **Stress:** fork/exec storm (zsh loop spawning BusyBox), TinyCC compile in
  `/work` while GLGAME runs, `./test.sh` full suite at `-smp 4` repeated ×10
  with no hangs — the historical failure mode for this class of change.
- **Single-CPU regression:** full suite at `-smp 1` after every unit.

## Risks and mitigations

- **PIC→APIC cutover breaks an existing driver's IRQ** (wrong polarity/
  trigger from missing interrupt-source-override handling). Mitigation: U1 is
  isolated and soaked interactively; MADT ISO entries honored; keep vector
  numbers identical so only routing changes.
- **Low-memory trampoline page unavailable or clobbered.** Mitigation:
  reserve before frame-allocator init; verify blob checksum after copy;
  SMP-off fallback path.
- **Hidden single-CPU assumptions surface as rare corruption.** The audit
  (U5) is a long tail by nature. Mitigation: keep device IRQs on the BSP and
  subsystem locks coarse so the *only* new concurrency is scheduler-mediated;
  U4→U5 ordering means stress tests run after the sweep; debug-build
  assertions (`cpu_id()==0` in input push, mapper no-fault flag, save-before-
  steal).
- **`hlt`-vs-wake races on idle CPUs** (lost reschedule IPI → stalled work).
  Mitigation: idle loop rechecks the queue under `cli` before `hlt`
  (already the pattern at `kernel.rs:815-825`); IPI is level-equivalent
  (re-sent whenever queue is non-empty and a CPU is flagged idle).
- **LAPIC timer calibration drift on APs.** Only affects slice length, not
  wall time (invariant 11). Calibrate against PIT over 100 ms at bring-up;
  ±10% is acceptable.
- **QEMU-specific behavior masking real-hardware bugs** (e.g. HVF vs TCG
  APIC modeling). Accepted: QEMU is the only supported target today; note in
  docs that real-hardware SMP needs re-validation.

## Success criteria

1. `AGENTICOS_QEMU_SMP=4 ./build.sh` boots to the desktop; Task Manager's
   Performance tab shows work spread across CPUs (follow-up: per-CPU graphs).
2. Kernel + ring-3 workloads demonstrably run concurrently (compile in one
   terminal while GLGAME stays playable).
3. `./test.sh` passes at `-smp 4` and `-smp 1`, including the new SMP topic
   module, ten consecutive runs.
4. No remaining `static mut` reachable from two CPUs without a lock or a
   documented boot-once/read-only justification.
5. Root and subsystem CLAUDE.md "No SMP" sections rewritten to describe the
   new model and its deliberate coarseness.
