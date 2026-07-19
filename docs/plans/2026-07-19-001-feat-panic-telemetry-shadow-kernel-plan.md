---
title: "feat: panic-safe crash telemetry, per-CPU flight recording, and a shadow-kernel invariant engine"
type: feat
status: planned
date: 2026-07-19
depth: large
related_docs:
  - CLAUDE.md
  - src/arch/x86_64/CLAUDE.md
  - src/lib/CLAUDE.md
  - src/mm/CLAUDE.md
  - src/process/CLAUDE.md
  - src/tests/CLAUDE.md
  - src/userland/CLAUDE.md
  - docs/plans/2026-07-18-005-refactor-unified-kernel-ring3-scheduler-plan.md
  - docs/plans/2026-07-18-006-feat-smp-support-plan.md
---

# feat: panic-safe crash telemetry, per-CPU flight recording, and a shadow-kernel invariant engine

## Summary

Replace AgenticOS's present "print a panic line and halt" behavior with a
diagnostic substrate that can answer three questions after a nondeterministic
failure:

1. **What exactly failed?** A versioned, panic-safe crash capsule captures the
   fatal reason, architectural state, build/run identity, every CPU's last
   known execution state, and bounded diagnostic snapshots.
2. **What happened immediately before it failed?** Fixed-size per-CPU binary
   flight recorders preserve semantic scheduler, CR3, page-in, block-token,
   page-table, frame, stack, lock, and interrupt transitions without
   allocating or performing UART I/O on hot paths.
3. **Which invariant broke first?** A shadow kernel independently models the
   legal state machines at the scheduler/MM boundary and the lazy-pager/I/O
   continuation seam, checks cheap transition laws synchronously, reconciles
   itself against production state at bounded checkpoints, and remains
   readable after other CPUs have halted while holding production locks.

The shadow kernel is deliberately **not** a copy of `Scheduler`,
`MemoryMapper`, or `ProcessTable`. Production keeps its existing structures.
The shadow consumes semantic operations such as `BeginSave`, `Publish`,
`Dispatch`, `ActivateL4`, `BeginPageIn`, `CompleteIo`,
`ResumeContinuation`, `MapLeaf`, `ReleaseFrame`, and `RetireStack`, and
maintains a smaller composite model whose legal states do not match the
production representation one-for-one. That difference is what lets it catch
contradictions instead of merely echoing corrupted fields.

The host receives a binary capsule over a dedicated QEMU debug channel,
validates it, symbolizes addresses against the exact kernel ELF, and emits
stable JSON plus a short Markdown report suitable for humans and coding
agents. The first release stops at trustworthy evidence production. Statistical
soak orchestration, scheduler perturbation, deterministic replay, fuzzing, and
an MCP server over the resulting crash database are follow-up plans.

---

## Current state and feasibility findings

### Fatal handling loses most of the state needed to classify a failure

`src/panic.rs` disables local interrupts, broadcasts the existing halt IPI,
prints `PanicInfo`, tries to paint the display, and halts. It does not capture:

- GPRs, CR2, CR3, FS/GS bases, TSS `rsp0`, or a backtrace;
- current scheduler entity / `current_user_pid` on every CPU;
- context-publication and kernel-stack handoff state;
- mapper/frame allocator state or a page walk;
- lock owners and acquisition context;
- the events preceding the panic;
- a build ID or exact run-configuration identity.

The test panic handler exits QEMU after the same one-line report. That is good
for deterministic assertion failures, but it discards the machine state that
is most valuable for intermittent failures.

Fatal exception paths are inconsistent. Page fault and general-protection
handlers print the x86 crate's CPU-pushed interrupt frame before calling
`panic!`; the frame omits general-purpose registers. The double-fault handler
prints and halts directly, so it never takes the test failure exit and can
leave `test.sh` waiting forever. Rust panics have a location/message but no
trap frame at all.

### Current logging cannot be the crash transport

`src/lib/debug.rs::write_line` takes the global `SERIAL_OUTPUT`
`InterruptMutex`. Exception handlers use `debug_error!` before reaching the
panic handler. If a CPU faults while the lock is held locally, or another CPU
halts while owning it, the diagnostic can deadlock before
`write_panic_line` gets its try-lock escape.

Routine page faults are intentionally trace-silent because UART output causes
large QEMU VM-exit overhead. Any useful causal recorder must therefore be
binary, memory-backed, fixed-size, and lock-free on its write path. Serial text
remains a human convenience, never the source of truth.

### SMP already has the placement points needed for per-CPU recording

`src/arch/x86_64/percpu.rs::CpuLocal` is a fixed `MAX_CPUS = 8` array and
already carries:

- logical/LAPIC IDs;
- kernel context and handoff context;
- preemption depth and mapper recursion state;
- current user PID and pending context publication;
- idle/dispatch/tick telemetry.

It is the natural owner of a per-CPU trace ring, crash snapshot slot,
diagnostics recursion guard, lock-wait record, and shadow CPU handoff state.
CPU-local slots are initialized before each CPU enables normal operation and
are already accessed through a stable GS ABI.

The existing `HALT_VECTOR` is a maskable fixed IPI whose handler immediately
halts. It does not acknowledge receipt or save state. A trustworthy crash
rendezvous needs a dedicated panic snapshot path, an acknowledgement bitmap,
and a bounded timeout so an unresponsive CPU never blocks the crash owner.

### The scheduler/MM seam has explicit but distributed invariants

The unified scheduler already represents the key publication rule:

- `SchedEntity.context_published` says whether another CPU may restore a
  context;
- `Scheduler.current[MAX_CPUS]` records the loaded entity per CPU;
- the run queue deduplicates ready entities;
- user CPU affinity preserves one-active-CPU-per-address-space for pthread
  groups.

Architecture code adds a second phase outside the scheduler lock:

- timer preemption marks an entity unpublished;
- assembly changes to the per-CPU handoff stack;
- `publish_pending_context` publishes the abandoned context;
- user dispatch writes CR3, TSS `rsp0`, `gs:[0]`, FS/FPU state, and finally
  `current_user_pid` before `iretq`.

These are valid multi-step transactions, so a naive invariant such as
"scheduler current and CR3 must agree after every individual store" would
raise false positives. The shadow model must represent the handoff phases and
check equality only at defined stable boundaries.

### Lazy ELF faults cross the pager, VirtIO, and scheduler domains

A concrete `git clone` failure provides a scope test for this plan. Several
processes faulted on absent pages whose virtual addresses are inside the
expected `GIT.ELF` executable and read-only PT_LOAD ranges. The current
ring-3 fault path calls `usercopy::ensure_user_page`, which:

1. resolves the current group L4 and VMA;
2. installs a present zeroed leaf;
3. reads the ELF window into the frame through the direct physical alias;
4. may block the in-progress ring-0 page-fault continuation on VirtIO;
5. resumes that continuation after an exact-token wake;
6. leaves the page mapped on success, or unmaps it on population failure.

The existing fatal page-table walk occurs before process teardown. An absent
PTE at that point therefore establishes that fault recovery did not commit,
but it does **not** distinguish no VMA, permission failure, allocation/map
failure, bad file extent, VirtIO submission/completion failure, short read,
premature continuation wake, corrupt continuation restore, or rollback.
Every branch currently collapses to `EFAULT` and then SIGSEGV; the rollback
unmap result is ignored. `read_at` success is also accepted without requiring
the requested byte count.

Inspection exposes a particularly important competing hypothesis:
`wake_ring3_for_signal` removes any `Ring3BlockReason`, including
`WaitingForBlockIo`, and requeues the entity without matching the I/O token.
If a signal arrives while a page-fault continuation is suspended, the task
can resume before its VirtIO request is complete. The caller may then consume
the request's initial status as an I/O failure, causing `ensure_user_page` to
unmap the legitimate ELF page. This could make a signal appear to be an MM or
scheduler race. The diagnostics must capture this causally; it is not enough
to add another textual failure log.

There is a second correctness boundary: the new leaf is present before its
file bytes are populated. Another task sharing the L4 can observe a zero or
partially populated executable page without faulting. The first shadow scope
must therefore include the pager/I/O-continuation seam even though modeling
the full filesystem and VirtIO implementation remains out of scope.

### MM has central mutation points and a usable independent audit path

All live mapper/frame-allocator access is serialized through
`mm::memory::with_memory_mapper`; `CpuLocal.in_mapper` already detects
recursive mapper acquisition. `MemoryMapper` centralizes map, unmap, move,
COW, prune, address-space destroy, and frame retain/release operations.

The physical-memory offset permits a read-only walk of any L4 regardless of
the active CR3. Existing ring-3 fault cleanup already compares live CR3 to the
process's expected L4 and walks raw PTEs. That proves the feasibility of a
bounded auditor which:

1. copies the live L4/process list while holding `PROCESS_TABLE` briefly;
2. releases it;
3. takes the mapper lock alone;
4. incrementally walks page tables and counts actual references;
5. compares those observations with allocator refcounts and shadow ownership.

The auditor must never hold `PROCESS_TABLE` and the mapper simultaneously and
must never yield or touch demand-faultable memory while holding the mapper.
All scratch space is allocated and prefaulted before audits are enabled.

### The test and QEMU launchers provide most host-side plumbing

`test.sh` already fixes the RTC, snapshots the writable disk by default,
accepts exact QEMU/SMP/memory configuration, supports QMP, and exits through
`isa-debug-exit`. `build.sh` already creates dedicated COM2/COM3 channels and
builds QEMU arguments centrally. Adding an `isa-debugcon` chardev at an unused
port provides a dependency-free, panic-safe byte stream without involving the
filesystem, VirtIO, normal UART lock, or host bridge processes.

The host has Python 3 and repository-owned tools already. A standard-library
decoder can validate the binary format and use the pinned toolchain's
`llvm-symbolizer` (falling back to `addr2line`) against the exact kernel ELF.

---

## Goals

1. Route every unrecoverable kernel failure through one bounded crash owner
   state machine, including Rust panic, fatal #PF/#GP/#UD/#SS/#AC/#TS/#NP,
   double fault, watchdog hard lockup, and explicit invariant violation.
2. Preserve a non-halting incident record when a recoverable ring-3 fault
   becomes an abnormal process exit, and allow strict diagnostics to escalate
   only a failed kernel-managed recovery transaction before teardown.
3. Preserve the first failure. A recursive/nested failure records a small
   secondary marker but cannot replace or deadlock the original capsule.
4. Capture useful state even when another CPU has halted while holding the
   scheduler, process-table, mapper, heap, serial, or stack-allocator lock.
5. Emit a versioned, checksummed binary capsule without heap allocation,
   blocking locks, filesystem access, normal debug logging, or pageable data.
6. Record a bounded semantic event history per CPU with no allocation, no
   blocking lock, no string formatting, and no UART I/O in the write path.
7. Give scheduler, CPU continuation, lazy page-in, user-address-space,
   frame/page-table, kernel-stack, and critical lock lifecycles independent
   shadow state machines.
8. Check cheap transition legality synchronously and perform more expensive
   production-vs-shadow reconciliation from a bounded diagnostic worker.
9. Assign stable invariant IDs and crash signatures so repeated manifestations
   deduplicate even when raw addresses or CPU numbers differ.
10. Capture exact build/run identity: git revision and dirty flag, profile,
   diagnostic feature mask, rustc version, kernel symbol-bundle ID, QEMU
   version/binary hash, normalized arguments hash, SMP/RAM/device profile, and
   host-generated run ID.
11. Produce `report.json` and `report.md` artifacts that explicitly separate
    observed facts, shadow violations, missing/unavailable sections, and
    decoder inferences.
12. Prove the system with deliberate negative tests: every supported injected
    corruption must trigger the expected invariant ID and produce a valid
    capsule repeatedly under SMP=1 and SMP=4.
13. Preserve ordinary boot/test behavior when rich diagnostics are disabled;
    the minimal crash core and small recorder remain available.

## Non-goals

- Full QEMU instruction record/replay or reverse debugging integration.
- Scheduler-choice perturbation, bounded-preemption exploration, or seeded
  schedule replay. The recorder schema must accommodate these later.
- A full KASAN/KCSAN/KFENCE implementation, compiler memory-access
  instrumentation, or tracking every kernel object allocation.
- Statistical A/B soak orchestration or a policy deciding how many clean runs
  constitute evidence of a fix.
- A syzkaller-compatible syscall description/fuzzer.
- An LLM/MCP service. This plan emits the stable machine-readable evidence
  such a service will consume.
- Durable in-guest crash persistence. The crash path never writes `/data` or
  `/shared`; the host captures the capsule directly.
- Recovering and continuing after a kernel invariant failure. Diagnostic
  strict mode stops at the first contradiction.
- Modeling filesystem namespace/content semantics, GUI/window state,
  networking protocol state, or all driver internals. The narrow lazy-page
  transaction and block-request token/continuation boundary are in scope
  because they are required to distinguish pager failure from scheduler/MM
  corruption.
- Remote user TLB shootdown or removal of pthread group affinity.

---

## Non-negotiable design rules

1. **No production locks after the crash rendezvous begins.** A halted CPU may
   own any lock. Capsule data must come from per-CPU slots, atomics, committed
   trace records, and crash-readable shadow state.
2. **No crash-path allocations or demand faults.** Every buffer and every byte
   touched by fatal handling is static or allocated once and explicitly
   prefaulted during initialization.
3. **First failure wins.** A single atomic election chooses the crash owner.
   Later entrants capture what they safely can and halt.
4. **Bound every wait and output.** Missing CPU acknowledgements, a failed
   debug channel, corrupt trace slots, and an incomplete shadow transaction
   become flags in the report; none can wedge capsule completion.
5. **Record semantics, not prose.** Hot paths emit numeric event kinds and
   typed operands. Formatting and symbolization occur on the host.
6. **The shadow is independently shaped.** It uses composite legal states,
   transition preconditions, and its own generations. It must not reuse
   `Scheduler::make_ready`, mapper walkers that mutate state, or production
   state enums as its transition implementation.
7. **A transition hook is adjacent to the commit it describes.** Do not emit
   `MapLeaf` before a PTE is installed or `FreeFrame` before refcount reaches
   zero. Rollbacks have explicit events.
8. **Transient windows are modeled, not ignored.** Multi-step handoffs use
   begin/commit/abort epochs. A crash in an odd epoch reports the pending
   operation instead of pretending the system was stable.
9. **Capacity exhaustion is evidence.** Recorder overwrite is expected and
   counted. Shadow-table exhaustion is an explicit `DIAG-CAPACITY` violation,
   never a silent loss of checking.
10. **Observer effect is measurable.** Every report carries diagnostic mode,
    ring sizes, enabled event families, and shadow memory cost.

---

## Architecture and module layout

```text
production mutation
  scheduler / switch / pager / block request / mapper / allocator / stack / tracked lock
        │
        ├── trace::record(SemanticEvent) ──► per-CPU committed ring
        │
        └── shadow::<domain>::apply(Operation)
                   │
                   ├── legal transition: commit shadow epoch
                   └── illegal transition: latch first ViolationRecord

periodic diagnostic worker
  snapshot production under one lock at a time
        └── invariants::reconcile(production_snapshot, shadow_snapshot)

fatal or first strict violation
  crash::begin(reason, trap_fidelity)
        ├── elect owner
        ├── capture local slot
        ├── NMI snapshot/rendezvous other CPUs
        ├── serialize committed rings + shadow snapshots + metadata
        ├── write length-framed capsule to isa-debugcon
        └── test: isa-debug-exit failure / normal: halt

host
  tools/crash_decode.py + exact kernel ELF + run manifest
        ├── capsule.bin
        ├── report.json
        └── report.md
```

Proposed kernel files:

```text
src/diagnostics/
  mod.rs                 initialization, mode, recursion guard
  wire.rs                repr(C) capsule/TLV schema and checksums
  trace.rs               per-CPU rings and event taxonomy
  crash.rs               owner state machine and serialization
  registers.rs           architecture-neutral register snapshots
  shadow/
    mod.rs               domain epochs, first violation, capacity
    scheduler.rs         entity composite state machine
    cpu.rs               dispatch/CR3/stack handoff model
    pager.rs             page-in, I/O token, and suspended-continuation model
    address_space.rs     L4 lifecycle and active-CPU ownership
    memory.rs            frame and mapping ownership ledgers
    stack.rs             kernel-stack slot lifetime
    locks.rs             critical lock owner/order/context model
  invariants.rs          checkpoint/audit reconciliation

src/arch/x86_64/
  crash_entry.rs         local register capture, panic NMI/IST entry

tools/
  crash_decode.py        binary validation, symbolization, JSON/Markdown
  test_crash_decode.py   golden/corruption decoder tests

scripts/
  test-crash-diagnostics.sh  expected-fatal QEMU integration tests
```

`src/main.rs` exports `diagnostics`; `kernel::init` brings up its layers in
three phases:

1. `early_init`: CPU0 static recorder/crash state, before heap;
2. `percpu_init`: attach rings/snapshot slots after GS/per-CPU initialization;
3. `shadow_init`: allocate, prefault, and then enable shadow tables after heap,
   mapper, scheduler, and process sentinel are stable but before APs run work.

### Diagnostic personalities

Keep one codebase with explicit runtime/build personalities:

| Personality | Intended use | Behavior |
|---|---|---|
| minimal | ordinary release | capsule core, small critical-event ring, first-violation latch; no full shadow tables or auditor |
| record | interactive diagnosis / soak | larger event coverage, scheduler/CPU/pager/address-space shadow, lock observation; violations latch and report |
| strict | focused QEMU diagnostics | full MM shadow/auditor, tracked-lock enforcement; first invariant violation enters fatal path |

Add Cargo features `diagnostics` and `diagnostics-strict` (`strict` implies
`diagnostics`) plus `AGENTICOS_DIAGNOSTICS=minimal|record|strict` handling in
the launch/test scripts. Ordinary `./test.sh` stays on its current profile
until strict mode is proven stable; `scripts/test-crash-diagnostics.sh` always
uses strict mode. Every capsule embeds the compiled feature mask and selected
runtime policy.

---

## 1. Panic-safe crash capsule

### Wire format

The kernel emits a little-endian length-framed binary stream. It does not emit
JSON. Use a fixed header followed by independently versioned TLV sections so a
new decoder can read old capsules and an old decoder can skip new sections.

```rust
#[repr(C)]
struct CapsuleHeader {
    magic: [u8; 8],          // b"AGCRASH\0"
    schema_version: u16,
    header_len: u16,
    total_len: u32,
    flags: u64,              // partial CPU set, nested fault, truncation, etc.
    run_id: [u8; 16],
    build_id: [u8; 20],
    owner_cpu: u8,
    online_cpu_mask: u8,
    captured_cpu_mask: u8,
    record_kind: u8,         // fatal, invariant, or non-halting user incident
    record_sequence: u64,
    payload_crc32: u32,
    header_crc32: u32,
}

#[repr(C)]
struct SectionHeader {
    kind: u16,
    version: u16,
    len: u32,
    flags: u32,
    crc32: u32,
}
```

Initial sections:

| Section | Required content |
|---|---|
| `RunMetadata` | git SHA/dirty bit, profile/features, rustc/toolchain hash, symbol bundle ID, run/config hash, boot phase, SMP/RAM/device policy |
| `Trigger` | record kind, invariant/terminal ID if any, panic location/message hash, vector/error, CR2, fault address, fidelity flags |
| `CpuSnapshots` | one fixed slot per CPU: registers, CR0/2/3/4, EFER, FS/GS bases, current entity/PID, phase, rsp0/stack bounds, preempt/IRQ/mapper/diagnostic depths |
| `ProcessIncident` | PID/TGID/parent and generations, executable/source identity, exit phase, pending signals, blocked reason/token, VMA classification, raw fault-page entries |
| `TraceTail` | committed trace slots grouped by CPU with write/overwrite counters |
| `ShadowScheduler` | entity composite states, CPU handoff phases, domain epoch and pending operation |
| `ShadowPager` | active/recent page-in transactions, VMA/source identity, frame and I/O token, continuation state, requested/completed bytes, terminal outcome |
| `ShadowMemory` | address-space states, implicated frame/mapping records, summary counters, audit status/cursor |
| `ShadowLocks` | critical lock owners/waiters, held-lock stacks, observed dependency edges |
| `Violation` | first latched invariant violation, expected/observed operands and causal event references |
| `Backtrace` | bounded raw instruction pointers plus unwind termination reason |
| `Footer` | completion marker, bytes attempted/written, missing/truncated sections |

All enums use explicit `repr(u8/u16/u32)` values. No Rust `Debug` layout or
`Option<T>` representation crosses the wire. `wire.rs` contains compile-time
size/offset assertions and golden-byte tests mirrored by the Python decoder.

### Build and run identity

The capsule must identify the exact symbol source without attempting to embed
the kernel ELF's recursive self-hash:

- `build.sh` / `test.sh` export git commit, dirty state, Rust toolchain string,
  profile, and diagnostic feature mask before invoking Cargo;
- `build.rs` exposes them with `cargo:rustc-env` and rerun-if-env directives;
- the kernel derives a fixed `BuildId` from those inputs;
- the host manifest records SHA-256 for the finished kernel ELF, BIOS image,
  QEMU executable, and normalized QEMU argument vector;
- a host-generated 128-bit run ID and manifest hash enter through QEMU
  `fw_cfg`, are read during early diagnostics init, and appear in the capsule.

The decoder refuses automatic symbolization when capsule `BuildId` and
manifest/ELF metadata do not agree. It may still render raw addresses with a
prominent `symbols_untrusted` flag.

### Crash owner state machine

Use one atomic state and one owner record:

```text
Idle
  └─ CAS winner ─► Electing ─► CapturingLocal ─► Rendezvous
                                      │              │
                                      │              ├─ all acked
                                      │              └─ bounded timeout
                                      ▼
                                Serializing ─► Emitting ─► Complete ─► Halt/Exit

non-owner or recursive entrant
  └─ capture secondary marker if slot is safe ─► acknowledge/halt
```

The owner performs only atomic/static-memory operations after election. It
must not call `debug_*`, `println!`, display code, heap allocation, filesystem,
normal lock wrappers, mapper helpers, or `ProcessTable` helpers.

`PanicInfo` formatting is not used inside the capsule. Record a bounded raw
panic message/location representation available without allocation: file hash,
line/column, and a stable reason hash. Best-effort human text can be printed
after the binary capsule is complete.

### Non-halting user-incident records

A ring-3 SIGSEGV conversion is not automatically a kernel fatality. Before
`cleanup_user_process` mutates mappings, closes descriptors, files zombies, or
wakes parents, commit a bounded `UserIncident` record containing the exact
trap, VMA classification, live raw PTE walk, first/most-recent page-in
transaction, continuation/request state, and local trace tail.

Record mode places this immutable snapshot in a bounded incident queue. A
diagnostic worker later frames it with the same capsule TLV schema and emits it
without stopping other CPUs; queue overflow is counted and emits a compact
loss marker. The snapshot hook itself performs no formatting, allocation,
filesystem I/O, or blocking export.

Strict mode may escalate a user incident to `crash::begin` only when the
kernel had classified the access as recoverable and then violated or failed a
managed recovery transaction—for example `PrematureWake`, invalid saved
continuation, wrong token, short successful read, rollback failure, or an
unpopulated present leaf. A genuine no-VMA/protection fault remains an
ordinary process signal. This policy captures the initiating child failure
before later pipe closures, SIGCHLD wakes, and orphan adoption obscure it.

### SMP snapshot rendezvous

Replace the current fire-and-forget halt broadcast for crash handling:

1. Add a dedicated per-CPU panic IST stack, separate from the double-fault
   IST, and install an NMI crash entry on every CPU.
2. The owner captures its local register snapshot, publishes `CrashState`, and
   sends an all-excluding-self NMI through the LAPIC.
3. The naked NMI entry pushes every GPR before Rust code runs, captures control
   registers/MSRs and per-CPU diagnostic fields into that CPU's fixed slot,
   commits the slot, sets its bit in `CAPTURED_CPUS`, and halts without
   releasing interrupted production locks.
4. An unexpected NMI while `CrashState == Idle` records `UnexpectedNmi` and
   returns. Nested NMI while capturing records a secondary flag and halts.
5. The owner waits using a TSC/instruction budget, not PIT ticks (interrupts
   are disabled). Missing CPUs are represented by their online-but-not-captured
   bits and cannot prevent serialization.

NMI delivery is required because a CPU may have IF cleared in SYSCALL,
exception, or `InterruptMutex` context. If one supported QEMU topology cannot
deliver the NMI rendezvous reliably, retain a fixed-IPI fallback but mark its
lower fidelity in `CpuSnapshots`.

### Register and trap fidelity

Distinguish what was actually captured:

- `ExactTrapFrame`: a diagnostics-owned naked exception entry saved GPRs
  before compiler code;
- `CpuPushedFrame`: RIP/CS/RFLAGS/RSP/SS/error are exact, GPRs are unavailable;
- `HandlerLive`: registers were sampled inside the Rust panic handler;
- `LastTimerSnapshot`: fallback from the recorder, explicitly not exact.

Do not block the first capsule release on replacing every IDT entry. Land in
two steps:

1. unify the current Rust handlers and double fault behind `crash::begin`,
   preserving the x86 crate frames and clearly marking missing GPRs;
2. add layout-asserted naked wrappers for fatal/recoverable exception classes,
   starting with #PF, #GP, #DF, and #UD. Recoverable demand/user page faults
   restore the saved frame and `iretq`; fatal outcomes pass the exact frame to
   the crash owner and diverge.

A Rust `panic!` cannot recover the original pre-panic GPR image without
compiler support. Its evidence is the panic location, handler-live register
image, per-CPU state, backtrace, recorder, and shadow violation.

### Backtrace

Diagnostic builds enable frame pointers. The kernel performs only a bounded
RBP-chain walk:

- maximum 64 frames;
- require canonical, aligned, monotonically increasing frame pointers;
- require each frame to stay within the current known kernel stack;
- use raw reads only from already mapped stack pages;
- record termination (`End`, `OutOfBounds`, `NonCanonical`, `Cycle`,
  `Unavailable`) rather than risking another fault.

The kernel stores only instruction pointers. The host symbolizes them against
the exact ELF and emits function/file/line where available. Qualify kernel
image size before enabling extra debug info globally; retaining a host-side
symbol bundle must not make the BIOS-stage image unbootable.

### Panic transport and host artifact

Attach a dedicated `isa-debugcon` at an unused I/O port (proposed `0xE9`) to a
file or Unix-socket chardev. Each fatal or non-halting incident writes a
self-synchronizing preamble, header, payload, and completion marker using
direct `out` instructions. Output is bounded by a compile-time maximum capsule
size; every section can be truncated at a record boundary with an explicit
flag. The host splits multiple records by run ID and monotonic record
sequence; a partial final record cannot hide earlier completed incidents.

For `test.sh`, capture into a per-run artifact directory. For interactive
`build.sh`, default to `.context/crashes/<run-id>/capsule.bin` when diagnostics
are enabled. If debugcon is unavailable, retain the static image and emit a
small raw header on panic-safe COM1; the report must say the full payload was
not exported.

Artifact layout:

```text
.context/crashes/<run-id>/
  manifest.json       host facts and hashes
  serial.log          ordinary COM1 output
  capsule.bin         exact guest bytes
  incidents/          split non-halting capsule records by sequence
  report.json         decoded/symbolized facts
  report.md           concise human/agent summary
  qemu.log            QEMU diagnostic output when enabled
  kernel.elf.ref      path + SHA-256, not an unbounded binary copy by default
```

`crash_decode.py` must be deterministic, standard-library-only apart from the
external symbolizer, and safe on hostile/truncated lengths. Golden tests cover
unknown sections, bad CRCs, partial output, duplicate sections, invalid enum
values, and symbol mismatch.

---

## 2. Per-CPU binary flight recorder

### Ring layout and publication protocol

Use one static ring per possible CPU. A 64-byte slot avoids torn semantic
records and keeps each slot naturally aligned:

```rust
#[repr(C, align(64))]
struct TraceSlot {
    commit: AtomicU64,     // sequence + IN_PROGRESS/COMMITTED state
    tsc: u64,
    tick: u64,
    causal_epoch: u64,    // 0 for local-only events
    subject: u64,         // encoded entity/L4/frame/lock, event-dependent
    arg0: u64,
    arg1: u64,
    meta: u64,            // kind, CPU, flags, schema packed explicitly
}
```

Each CPU has a monotonic atomic `next_sequence`, overwrite counter, and
recursion/drop counter. Recording:

1. reserve a unique sequence with `fetch_add`;
2. select `sequence % RING_LEN`;
3. store `IN_PROGRESS(sequence)`;
4. write the payload;
5. release-store `COMMITTED(sequence)`.

An interrupt may nest inside a writer and use the next slot safely. A reader
accepts only slots whose committed sequence is stable before and after the
copy. Crash serialization skips in-progress/overwritten slots and records the
gap. No ring lock is ever acquired.

Use per-CPU order for ordinary events. Operations already serialized by the
scheduler, mapper, process table, or stack allocator receive that shadow
domain's `causal_epoch`, allowing the host to order causally meaningful
cross-CPU transitions without imposing one contended global sequence on every
interrupt event.

Default sizing target:

- minimal: 128 slots/CPU (64 KiB total at 8 CPUs);
- record/strict: 1,024 slots/CPU (512 KiB total);
- capsule exports a configurable tail, default 512/CPU in rich modes.

Exact sizes are compile-time constants and embedded in metadata. Recorder
overwrite is normal ring behavior, not an invariant violation.

### Event taxonomy

Every event kind has a documented operand schema in `trace.rs` and the Python
decoder. Initial families:

| Family | Events |
|---|---|
| boot/fatal | boot phase, diagnostics enabled, fatal elected, nested fatal, CPU rendezvous/timeout, capsule section dropped |
| interrupt | entry/exit for timer, reschedule, syscall, page fault, NMI; previous CPL; EOI |
| scheduler | register, ready request, enqueued/dequeued, dispatch, save begin, publish, preempt-no-alternative, block, yield, unregister, affinity change |
| process/signal | fork/clone/exec identity, parent change/orphan adoption, signal raised/wake attempted/delivered, exit/zombie/reap, group fd close |
| CPU handoff | handoff phase begin/commit/abort, CR3 write, rsp0/gs stack install, current PID set/clear, pending context publish set/take |
| pager | fault classified, VMA/backing resolved, page-in begin, leaf reserved, read requested/returned, population committed, rollback begin/result, terminal reason |
| block I/O | request allocated/submitted/completed, waiter token published, continuation save begin/commit, blocked visible, wake queued/drained/accepted/rejected, resume selected/entered/returned |
| address space | L4 allocate/build/live/activate/deactivate/destroy begin/complete, group/L4 association |
| memory | frame allocate/retain/release/free/pin, leaf map/unmap/move, COW share/copy/upgrade, table allocate/prune, rollback |
| kernel stack | slot allocate, activate, handoff away, retire request, free/reuse, RSP bounds failure |
| locks | attempt, acquired, try-failed, released, wait duration bucket, lock-order edge |
| invariant | transition accepted/rejected, checkpoint begin/end, reconciliation mismatch, capacity exhausted, first violation latched |

Do not record every successful heap access or routine PTE read. The first
release records mutations and boundary decisions. Later targeted modes may
add sampled memory accesses or watched-address events.

### Subject encodings and generations

Raw PID/frame/slot reuse can make old events appear related to new objects.
Use compact stable encodings:

- entities retain the kernel/user tag and PID;
- address spaces use `(L4 physical frame, shadow generation)`;
- page-ins use `(address-space generation, virtual page, transaction generation)`;
- block requests use the driver's monotonic token plus queue/device identity;
- frames use `(compact allocator index, allocation generation)`;
- kernel stacks use `(slot index, reuse generation)`;
- locks use a static `LockClassId` plus optional instance ID.

Generations increment on reuse and are included in shadow keys, trace events,
and violation operands.

### Instrumentation placement

Place recorder calls at central commit points, not scattered call sites:

- `Scheduler` methods and `RunQueue` operations for entity/queue state;
- `publish_pending_context`, preemption handoff, `resume_ring3_inner`, and
  kernel-thread restore for CPU state;
- `ensure_user_page`, `File::read_at`, VirtIO request submit/completion, the
  bounded wake queues, and `block_current_ring3_on_io` for page-in causality;
- `AddressSpace::{new,activate,drop}` and fork clone for L4 lifecycle;
- `MemoryMapper` map/unmap/move/COW/destroy/prune methods plus
  `BootInfoFrameAllocator` reference mutations;
- `StackAllocator` and user `KernelStack` ownership changes;
- tracked `InterruptMutex`/`PreemptionMutex` wrappers for selected locks;
- exception/syscall/timer/NMI entry/exit at architecture boundaries.

The recorder API accepts only integers/copyable enums. It cannot call
`format_args!`, debug logging, `Vec`, `BTreeMap`, mapper translation, or any
subsystem lock.

### Early boot and recursion behavior

Before GS/per-CPU initialization, CPU0 records into slot 0 through an explicit
early path. After `percpu_init`, ordinary recording uses `cpu_id()`. Each
`CpuLocal` has a diagnostics nesting counter:

- first-level diagnostics record normally;
- recorder faults/recursion increment a drop counter and return;
- fatal handling may read rings but never emits ordinary events through them
  while serializing.

Recorder buffers are static and therefore mapped with the kernel image. Rich
shadow/audit buffers allocated later are fully written once at init to prefault
every page before their addresses become reachable from hot hooks.

---

## 3. Executable invariants and the shadow kernel

### What the shadow kernel is

The shadow kernel is a collection of bounded diagnostic state machines with
four jobs:

1. reject semantically illegal operations at the moment they are attempted;
2. preserve a compact crash-readable account of ownership and handoff state;
3. reconcile that account periodically against independent production
   snapshots/page-table walks;
4. identify the earliest violated law with a stable invariant ID.

It does **not** make scheduling decisions, map pages, change refcounts, own
processes, or repair production state. A shadow failure cannot be used as a
fallback kernel operation. In minimal/record mode it latches evidence; in
strict mode it deliberately enters the unified fatal path.

### Domain transaction protocol

Scheduler mutations are serialized by `SCHEDULER`; MM mutations by `MAPPER`;
stack mutations by the stack allocator. Reuse those production serialization
domains without taking an additional shadow lock:

```text
shadow domain epoch E (even, stable)
  write pending operation + operands
  epoch = E + 1 (odd, in flight)
  apply independent shadow transition
  publish trace event carrying E + 2
  epoch = E + 2 (even, committed)
  clear pending marker
```

Shadow fields needed in a crash are atomics or fixed POD cells published under
the epoch. A crash snapshot reads `epoch`, copies, and rereads it:

- same even epoch: stable snapshot;
- changed epoch: retry a bounded number of times;
- odd/stuck epoch: include the last committed state plus `pending operation`
  and mark `transition_interrupted`.

This avoids blocking if the crash NMI interrupted a CPU while it held the
production serialization lock or was midway through its shadow hook.

### Scheduler entity shadow

Production represents one entity across `entities`, `run_queue`,
`current[cpu]`, PCB state, and `context_published`. The shadow collapses those
fields into one legal composite state:

```text
Absent
  └─ Register ─► Blocked

Blocked
  └─ MakeReady(published) ─► ReadyQueued

Running(cpu)
  ├─ BeginSave ─► ReadyUnpublished(previous_cpu=cpu)
  ├─ Block ─► Blocked
  ├─ Yield ─► ReadyQueued
  └─ BeginExit ─► Dying(cpu)

ReadyUnpublished
  ├─ Publish ─► ReadyQueued
  └─ ResumeSameCpu ─► Running(previous_cpu)

ReadyQueued
  ├─ Dispatch(cpu) ─► Running(cpu)
  ├─ Block/Cancel ─► Blocked
  └─ Unregister ─► Dead

Dying ─► Dead
```

Each fixed `ShadowEntitySlot` stores encoded entity key, generation, composite
state, affinity, last causal epoch, and last transition site. A bounded
open-addressed registry (capacity at least `2 * MAX_ENTITIES`) maps stable
entity keys to slots. PIDs are monotonic but kernel/user namespaces remain
tagged.

Transition preconditions independently enforce:

- `Dispatch` consumes only `ReadyQueued` and only on an eligible CPU;
- `BeginSave` names the entity running on the calling CPU;
- an unpublished entity cannot enter the ready queue;
- one entity cannot be `Running` on two CPUs;
- one CPU cannot run two entities;
- `Publish` must match a prior save and pending publication owner;
- `Unregister` cannot silently discard an active entity or live stack;
- kernel PCB and tagged scheduler lifecycles terminate together at a stable
  checkpoint.

### CPU handoff shadow

Per-CPU state has valid transient phases that span scheduler and architecture
code. Model them explicitly:

```rust
enum ShadowCpuPhase {
    Boot,
    KernelIdle,
    KernelRunning { entity, stack },
    Saving { outgoing, frame_kind },
    HandoffStack { outgoing, pending_publish, target },
    LoadingUser { target, expected_l4, stack },
    UserStable { target, l4, stack },
    LoadingKernel { target },
    Exiting { outgoing, l4, stack },
    Crashed,
}
```

The shadow observes typed operations rather than arbitrary field stores:

```text
BeginDispatch(target)
InstallCr3(target_l4)
InstallKernelStack(stack_top)
RestoreExtendedState(target)
SetCurrentUserPid(target)
CommitUserEntry(target)
BeginReturnToKernel(outgoing)
InstallKernelCr3
ClearCurrentUserPid
CommitKernelEntry(target)
```

At `UserStable`, enforce all cross-subsystem equality:

- scheduler current is `UserProcess(pid)` on that CPU;
- shadow entity is `Running(cpu)`;
- `CpuLocal.current_user_pid == pid`;
- live CR3 equals the thread group's live L4;
- TSS `rsp0` and `gs:[0]` equal the process's live kernel-stack top;
- FS/FPU restoration is marked complete;
- no pending outgoing context publication remains.

During `LoadingUser` those values may temporarily differ, but only in the
declared order. A timer/NMI/fault in that window records the exact phase and
last completed step rather than emitting a generic CR3 mismatch.

At `KernelIdle`/`KernelRunning`, enforce permanent kernel CR3 and no loaded
user PID. Kernel-thread context publication and safe stack retirement use the
same handoff phase rather than a separate ad hoc checker.

### Lazy page-in and suspended-continuation shadow

Model page recovery as a cross-domain transaction rather than treating a
page fault, mapping, block request, and scheduler wake as unrelated events.
The key is `(address_space_generation, virtual_page, page_in_generation)`;
the record owns explicit links to PID/TGID, VMA generation, backing-source
identity, frame generation, block token, continuation generation, and kernel
stack generation.

Use a bounded table for active transactions plus a small per-CPU/per-process
cache of recent terminal transactions. Active entries are never evicted. If
capacity is exhausted, latch `DIAG-CAPACITY-PAGER`; do not silently perform an
untracked strict-mode page-in.

The desired page transaction is:

```text
Absent
  └─ ClassifyFault ─► Classified(vma_generation, backing, permissions)
Classified
  └─ ReserveFrame ─► FrameReserved(frame_generation, leaf_not_visible)
FrameReserved
  ├─ AnonymousZero ─► Populated
  └─ SubmitRead(token, offset, expected_bytes) ─► IoPending
IoPending
  └─ CompleteRead(token, status, actual_bytes) ─► IoComplete
IoComplete
  ├─ exact success + zero tail ─► Populated
  └─ error/short read ─► RollingBack
Populated
  └─ RevalidateVmaAndInstallLeaf ─► PresentCommitted
Classified | FrameReserved | IoPending | IoComplete
  └─ Abort(reason) ─► RollingBack ─► Aborted(reason)
```

`PresentCommitted` is the only state from which user execution may consume
the page. The current implementation installs a present zero leaf before the
read. Instrument that behavior first as `LeafExposedUnpopulated` so the
diagnostics can characterize it, then change the production commit protocol
to populate a privately held frame and install the leaf only after I/O,
zero-tail, and VMA revalidation succeed. Until that change lands, strict mode
must report the exposure rather than claiming atomic page-in coverage.

VMA revalidation is necessary because the page-in drops all VM locks while
blocked. Add a monotonically increasing address-space/VMA mutation generation
and verify after I/O that:

- the same generated address space is live;
- the virtual page is still covered by the same backing and permissions;
- the expected file window is unchanged and arithmetically valid;
- no other transaction committed a leaf for the key;
- the destination frame and owning kernel stack are still live generations.

Represent the blocking request independently:

```text
Allocated(token)
  └─ Submitted(device, queue_head, requested_bytes, waiter) ─► InFlight
InFlight
  └─ DeviceCompleted(status, used_bytes) ─► Completed
Completed
  └─ QueueWake(token, waiter) ─► WakePending
WakePending
  └─ AcceptExactWake ─► WakeAccepted
WakeAccepted
  └─ ConsumeResult ─► Consumed
```

The IRQ completion fact and the scheduler wake fact are distinct. A wake is
legal only when PID/entity generation, token, page-in transaction, blocked
reason, and continuation generation all match. A signal may become pending
while a page-fault I/O continuation is blocked, but it must not convert that
continuation to runnable. If page-in is made interruptible later, cancellation
needs an explicit request-cancel/ack/rollback protocol; a generic signal wake
is insufficient.

Model the saved kernel continuation as:

```text
None
  └─ Allocate(owner, stack_generation) ─► Saving
Saving
  └─ SaveComplete(rip, rsp, rflags) ─► PublishedBlocked(token)
PublishedBlocked
  └─ ExactWakeAccepted(token) ─► Runnable
Runnable
  └─ Dispatch(cpu) ─► Resuming
Resuming
  └─ ConsumeOnce ─► None(next_generation)
```

Before `switch_to_context`, validate that RIP is canonical and in kernel text,
RSP is aligned and inside the declared live kernel-stack generation, the
context save is committed, the entity is the unique scheduler target, and
the matching request is completed. Record the full saved RIP/RSP/RFLAGS in
the shadow/capsule; the ordinary event ring may use compact values. Migration
to another CPU is permitted only if all CPU-local handoff state has been
reinstalled and no source-CPU pending-publication marker remains.

Replace the page-fault path's undifferentiated `Result<(), i64>` internally
with a diagnostic result carrying a stable terminal reason. The syscall ABI
may still receive `EFAULT`. Initial reasons:

```text
NoAddressSpace, NoVma, PermissionDenied, CowNotApplicable, CowOutOfFrames,
MapperUnavailable, FrameAllocationFailed, MapFailed, PhysicalAliasFailed,
FileExtentOverflow, FileOffsetInvalid, IoSubmitFailed, IoCompletionError,
ShortRead, WrongWakeToken, PrematureWake, ContinuationInvalid,
VmaChangedDuringIo, LeafCollision, PopulationChecksumMismatch,
RollbackUnmapFailed, StackGrowthRejected
```

Record requested and actual byte counts and a bounded checksum of populated
bytes. The host manifest/decoder may compare the page checksum with the exact
ELF artifact when available; otherwise it remains an observed value, not a
claim that the source bytes were correct.

### Address-space shadow

Represent each user L4 with a generation and independent lifecycle:

```text
Absent
  └─ AllocateRoot ─► Building
Building
  ├─ InstallKernelSlots / BuildUserTrees
  ├─ PublishProcessOwner ─► LiveInactive
  └─ Abort ─► Destroying ─► Dead
LiveInactive
  ├─ Activate(cpu) ─► Active(cpu)
  └─ BeginDestroy ─► Destroying
Active(cpu)
  └─ Deactivate(cpu) ─► LiveInactive
Destroying
  └─ ReleaseRoot ─► Dead
```

Pthread tasks may share a group L4. The address-space owner is the TGID plus a
set/count of member tasks, not a one-PID assumption. The current no-shootdown
rule permits `active_cpu_mask.count_ones() <= 1`; if group affinity is removed
later, this invariant must be versioned and replaced with TLB-generation/ack
tracking before multi-CPU activation is accepted.

Address-space hooks enforce:

- activation only of `LiveInactive`/same-CPU `Active` state;
- no destroy while any CPU is active or loading that L4;
- kernel L4 is a distinguished immortal root and never enters user destroy;
- L4 frame generation still matches the allocator's live allocation;
- copied kernel-reserved PML4 slots are not treated as user-owned references;
- every live task resolves to exactly one live group L4.

### Frame ownership ledger

Allocate one compact `ShadowFrame` per usable allocator frame, indexed by the
same compact memory-map ordinal as `BootInfoFrameAllocator`. Proposed logical
fields (packed/atomic implementation may be smaller):

```rust
struct ShadowFrame {
    allocation_generation: u32,
    state: Free | Pinned | Live | Quarantined,
    expected_refs: u32,
    leaf_refs: u32,
    page_table_refs: u32,
    transient_refs: u32,
    kind: RootL4 | PageTable | UserLeaf | KernelHeap | KernelStack | Other,
    last_alloc_site: u16,
    last_release_site: u16,
    last_epoch: u64,
}
```

Replace untyped diagnostic ambiguity with typed ownership reasons at central
allocator call sites:

```rust
enum FrameRefReason {
    RootL4 { asid },
    PageTable { asid, level },
    LeafMapping { asid, vpn },
    CowShare { parent, child, vpn },
    KernelHeap { vpn },
    KernelStack { slot },
    Trampoline,
    Mmio,
    Transient { site },
}
```

The production allocator may retain its numeric refcount internally, but
allocate/retain/release entry points used by mapper/address-space code gain a
typed diagnostic reason in rich modes. `Unknown` is permitted during staged
migration and counted; strict completion requires no unknown user-page-table
or COW mutations.

Rules:

- allocate requires `Free`, increments generation, and establishes exactly
  one typed reference;
- retain requires a live matching generation and increments both the reason
  bucket and expected total;
- release requires a matching outstanding reason and cannot underflow;
- transition to `Free` requires expected total zero and no mapping ledger
  entries;
- pinned frames never become free;
- a page-table frame cannot simultaneously be classified as a leaf;
- a reused physical frame never inherits ownership from its prior generation.

At 2 GiB, 524,288 usable frames make a 24- to 32-byte ledger cost roughly
12-16 MiB; at the 256 MiB test default it costs roughly 1.5-2 MiB. Allocate
only in rich modes, cap total diagnostic memory (initial target 32 MiB or 2%
of guest RAM, whichever is lower), and report exact bytes. Failure to provision
the full ledger downgrades MM shadow explicitly; strict mode refuses to start
rather than pretending it has full coverage.

### Mapping ledger

Frame totals alone cannot detect "unmapped the wrong VA but balanced the
refcount." Strict mode adds a bounded open-addressed mapping table keyed by:

```text
(address_space_generation, virtual_page) ->
    (frame_index, frame_generation, flags, mapping_generation, last_epoch)
```

Semantic operations:

- `MapLeaf` requires the key absent, a live address space, a live frame, and
  legal USER/W^X flags; then it adds one leaf reference;
- `UnmapLeaf` requires exact key/frame/generation agreement and removes it;
- `MoveLeaf` atomically changes the key without changing frame ownership;
- `CowShare` installs a second key and retain;
- `CowCopy` replaces one key's frame and releases the old reason;
- `DestroySubtree` removes every mapping owned by that ASID before root free.

Size the initial table to the diagnostic budget and expose load factor,
maximum probe distance, and rejected insertions. If it fills, latch
`DIAG-CAPACITY-MAPPING` and preserve all existing entries; do not evict live
ownership facts.

### Independent page-table/allocator reconciliation

The mapping and frame ledgers are driven by production hooks, so they can
share an omitted-hook bug. The auditor supplies independence:

1. snapshot up to the bounded process/task/L4 inventory under
   `PROCESS_TABLE`, then release it;
2. snapshot scheduler entities/current slots under `SCHEDULER`, then release;
3. enter mapper alone with prefaulted scratch counters;
4. walk the kernel root once and each live user-owned lower-half tree;
5. validate raw flags and classify every table/leaf frame reference;
6. compare actual PTE references with allocator refcounts, mapping ledger,
   frame ledger, and address-space states;
7. exit mapper before reporting/latching any result.

The scan is incremental by address space/table budget. It never yields while
holding the mapper; instead it records a cursor, releases the mapper, and
continues on the next diagnostic-worker slice. A full pass receives an audit
generation and only compares inventories that remained stable across the pass;
otherwise it restarts with a bounded retry counter. Transition checks still
run while an audit restarts, so churn cannot disable all checking.

Raw audit laws include:

- every present user leaf has USER set on all ancestors;
- no writable+executable user leaf;
- no huge entry in user-owned trees until explicitly supported;
- user-owned paths never enter reserved kernel PML4 slots;
- each reachable user table/root frame is live and typed correctly;
- actual leaf multiplicity matches allocator refcount after accounting for
  roots/tables/kernel/transient owners;
- every strict mapping-ledger entry exists in the raw tree with exact frame and
  permission flags;
- no raw user leaf is absent from the strict ledger after a stable full pass.

### Kernel-stack shadow

Model both the fixed kernel-thread `StackAllocator` slots and ring-3
`KernelStack` objects with generations:

```text
Free ─Allocate(owner)► Reserved ─Activate(cpu)► Active(cpu)
Active ─BeginHandoff► Retiring ─PublishAway► Reserved
Reserved ─Free(owner)► Free
```

Rules:

- an active CPU RSP must remain inside its declared stack, except on a known
  per-CPU main/handoff/IST stack;
- `Free`/reuse cannot occur while a CPU is active, loading, exiting, or has a
  pending context publication for the owner;
- slot owner/generation must match PCB/process ownership;
- guard pages remain unmapped and stack bounds are page-aligned;
- stack retirement occurs only after the architecture has switched to the
  per-CPU handoff stack;
- a newly allocated generation cannot retain a previous owner's live context.

This domain is essential because premature stack reuse can corrupt a saved
context and later manifest as an apparently unrelated scheduler or mapper
fault.

### Critical-lock shadow

Extend `InterruptMutex` and `PreemptionMutex` with an optional static
`LockClassId` while preserving `new(value)` for untracked locks. Add
`new_tracked(value, class)` for the first critical set:

- scheduler;
- process table;
- memory mapper/frame allocator;
- kernel/user stack allocators;
- heap allocator;
- serial logger.

Each tracked wrapper publishes owner CPU/entity, acquire site/TSC, recursion
depth, failed try-lock count, and waiter summaries through atomics/per-CPU
slots. Guard drop records release before restoring IF/preemption, matching the
current load-bearing field order.

The shadow checks:

- no recursive acquisition unless a class explicitly permits it;
- owner is empty before successful acquire and matches on release;
- `InterruptMutex` acquisition has local IF masked after its guard is taken;
- `PreemptionMutex` ownership has positive local preemption depth;
- mapper acquisition is nonrecursive and no forbidden yield/fault boundary is
  crossed while `in_mapper`;
- observed lock-order edges do not create a cycle in strict mode.

Roll out lock ordering in observe-first mode: record the clean baseline graph,
review and codify the allowed partial order, then make cycles fatal. Do not
auto-learn an allowlist from a potentially corrupt run.

### Invariant execution modes

| Mode | Timing | Cost | Purpose |
|---|---|---|---|
| transition | every semantic mutation | O(1), no allocation | reject illegal state-machine edges near cause |
| stable checkpoint | end of scheduler/CPU/MM transaction | bounded scan of affected objects | compare cross-fields after transient window closes |
| incremental audit | diagnostic worker | expensive but sliced | independent raw production-vs-shadow reconciliation |
| crash snapshot | fatal owner, no locks | bounded copy | preserve last stable and in-flight state |

Each check returns a typed `InvariantResult`; it never calls `panic!` or logs
directly. `diagnostics::violation::latch` atomically preserves the first:

```rust
struct ViolationRecord {
    invariant_id: u32,
    severity: u8,
    cpu: u8,
    mode: u8,
    domain: u8,
    epoch: u64,
    subject: u64,
    expected0: u64,
    observed0: u64,
    expected1: u64,
    observed1: u64,
    trace_sequence: u64,
}
```

Record mode continues after latching unless the violation is marked
unrecoverable. Strict mode immediately calls `crash::begin(Invariant(id))`
with the current transition context. The violation path has its own recursion
guard and cannot attempt a second shadow transition.

### Initial invariant catalog

Stable IDs are documentation/API. Never reuse an ID for a new meaning.

| ID | Law |
|---|---|
| `SCHED-001` | an entity is current/running on at most one CPU |
| `SCHED-002` | one CPU has at most one current entity |
| `SCHED-003` | `ReadyQueued` appears exactly once in the run queue |
| `SCHED-004` | unpublished/saving entity is not dispatchable or queued |
| `SCHED-005` | dispatch respects affinity and online CPU bounds |
| `SCHED-006` | scheduler entity state and kernel PCB state agree at checkpoint |
| `SCHED-007` | unregister/exit does not discard a current entity |
| `CPU-001` | stable user PID, scheduler current, and shadow running entity agree |
| `CPU-002` | stable user CR3 equals the live group L4 |
| `CPU-003` | stable kernel phase uses permanent kernel CR3 and no user PID |
| `CPU-004` | pending context publication has one matching outgoing entity |
| `CPU-005` | CR3/rsp0/GS/current-PID operations follow declared handoff order |
| `PAGER-001` | every recoverable user fault has one typed terminal outcome |
| `PAGER-002` | a page-in key has at most one active transaction/committer |
| `PAGER-003` | committed file page matches the live VMA generation, extent, and permissions |
| `PAGER-004` | successful file population returns the exact requested byte count and zeroes the declared tail |
| `PAGER-005` | abort releases the private frame/leaf exactly once and records rollback failure |
| `PAGER-006` | no present user leaf is exposed before population commits |
| `IO-001` | request token/queue-head has one live request and one terminal completion |
| `IO-002` | result is not consumed before device completion and exact-token wake acceptance |
| `IO-003` | blocked reason, waiter identity, request token, and page-in transaction agree |
| `IO-004` | queue capacity/wake loss/completion status and byte count are explicit |
| `CONT-001` | saved kernel continuation is dispatched only after its context save commits |
| `CONT-002` | continuation RIP/RSP belong to kernel text and the live owner stack generation |
| `CONT-003` | continuation is consumed once by its owning entity generation |
| `CONT-004` | signal delivery cannot make a page-in I/O continuation runnable without cancellation acknowledgement |
| `AS-001` | a user L4 is active on at most one CPU under current affinity rules |
| `AS-002` | only a live L4 may be activated |
| `AS-003` | active/loading L4 cannot be destroyed or freed |
| `AS-004` | every live task resolves to one live group address space |
| `MM-001` | mapping key is unique and unmap/move targets the exact live generation |
| `MM-002` | expected typed frame references do not underflow/overflow |
| `MM-003` | allocator refcount equals reconstructed ownership after stable audit |
| `MM-004` | every user leaf has USER ancestors and legal W^X permissions |
| `MM-005` | user page-table topology stays out of reserved kernel slots |
| `MM-006` | page-table/root frames are live and correctly typed |
| `MM-007` | mapper is nonrecursive and mutations occur inside its serialization domain |
| `STACK-001` | active/retiring stack cannot be freed or reused |
| `STACK-002` | active RSP belongs to the declared owner/generation or known per-CPU stack |
| `STACK-003` | stack publication/free ordering follows handoff completion |
| `LOCK-001` | tracked lock has one owner and matching release |
| `LOCK-002` | tracked nonrecursive lock is not reacquired by its owner |
| `LOCK-003` | IF/preemption context matches mutex class requirements |
| `LOCK-004` | enforced lock-order graph remains acyclic |
| `DIAG-001` | recorder/shadow capacity loss is explicit and mode policy is honored |
| `DIAG-002` | a domain transaction did not remain odd past its bounded progress window |

### Keeping the shadow honest

The shadow itself can contain bugs. Apply four defenses:

1. **Different representation:** composite entity/CPU/AS states rather than
   production's separate maps, flags, and queues.
2. **Independent audit:** raw page-table walks and copied production snapshots
   do not consume shadow transition results.
3. **Pure model tests:** shadow transition functions take plain input/state and
   return `Result<NewState, InvariantId>`; exhaustive small state/action
   sequences test allowed and forbidden edges without booting workloads.
4. **Fault injection:** test-only hooks deliberately omit/reorder publication,
   duplicate queue/current ownership, alter CR3 expectations, prematurely
   wake or corrupt a page-in continuation, short-complete block I/O,
   leak/release frame reasons, reuse active stacks, and create lock cycles.
   Each must trigger exactly the intended ID.

Never "repair" shadow state from production after a reconciliation mismatch.
Preserve the mismatch and crash/latch it. A separately invoked test reset may
reinitialize shadow state between synthetic test cases only.

---

## Agent-facing decoded report

`report.json` is evidence, not an LLM narrative. Initial top-level shape:

```json
{
  "schema": 1,
  "run": { "id": "...", "build_id": "...", "manifest_trusted": true },
  "trigger": { "kind": "invariant", "id": "CPU-002", "owner_cpu": 3 },
  "signature": "CPU-002:resume_user:cr3_mismatch",
  "process": { "pid": 14, "tgid": 14, "parent_pid": 13, "exe": "GITRHTTP.ELF" },
  "first_violation": {
    "expected": { "l4": "0x...", "pid": 42 },
    "observed": { "cr3": "0x...", "pid": 42 },
    "epoch": 913
  },
  "cpus": [],
  "timeline": [],
  "shadow": { "stable": false, "pending_transition": "InstallCr3" },
  "fault_recovery": {
    "classification": "elf_page_in",
    "terminal_reason": "PrematureWake",
    "vma_generation": 7,
    "virtual_page": "0x5c0000",
    "frame_generation": 19,
    "io_token": 481,
    "request_state": "InFlight",
    "continuation_state": "Resuming",
    "requested_bytes": 4096,
    "completed_bytes": 0
  },
  "missing": [],
  "inferences": []
}
```

The decoder computes a stable signature from invariant ID or vector,
symbolized top frame, fault-address class, and first shadow contradiction. It
must keep inferences labeled separately from capsule facts. `report.md`
contains the fatal summary, earliest violation, per-CPU table, last causal
timeline, missing evidence, and exact reproduction manifest reference.

---

## Work sequence

Each unit is independently reviewable and keeps the default kernel bootable.
Rich hooks remain behind the diagnostic personality until their focused tests
and overhead checks pass.

### U0 — Schema, build/run identity, and host decoder

**Files:** new `src/diagnostics/{mod.rs,wire.rs}`, `src/main.rs`, `build.rs`,
`build.sh`, `test.sh`, new `tools/crash_decode.py`, decoder tests.

- Freeze capsule header/TLV v1, enum IDs, size assertions, CRC implementation,
  and golden byte vectors.
- Inject `BuildId`, read host run/manifest IDs from `fw_cfg`, and write matching
  host `manifest.json`.
- Implement hostile-input-safe decoder and symbolizer trust checks before the
  kernel emits real capsules.
- Qualify `isa-debugcon` syntax on stock and pinned VirGL QEMU binaries.

**Exit:** Python parses Rust-generated golden capsules byte-for-byte; corrupt
length/CRC/enum tests fail safely; a normal boot prints matching build/run IDs.

### U1 — Per-CPU flight recorder foundation

**Files:** new `src/diagnostics/trace.rs`, `percpu.rs`, initial hooks in
interrupt/scheduler/switch paths, decoder support, new `src/tests/diagnostics.rs`.

- Add static minimal/rich rings, commit protocol, early CPU0 path, recursion
  handling, and trace-tail serialization helpers.
- Land event taxonomy and operand documentation.
- Instrument boot, timer/syscall/exception boundaries, scheduler publication,
  dispatch, CR3 writes, current PID transitions, page-in terminal reasons,
  and block-request token/wake boundaries first.

**Exit:** nested synthetic writes never produce torn accepted records; wrap
and overwrite counters are exact; SMP tests show valid independent sequences
from every CPU; routine events perform no UART I/O/allocation.

### U2 — Local crash owner, capsule assembly, and debugcon transport

**Files:** new `crash.rs`, `registers.rs`, `arch/x86_64/crash_entry.rs`,
`panic.rs`, `interrupts.rs`, `lapic.rs`, launch scripts.

- Implement first-failure election, static capsule arena, section truncation,
  local register/control-state capture, bounded frame-pointer trace, debugcon
  output, and test exit/normal halt policies.
- Add bounded non-halting incident snapshots, worker-side export, and
  multi-record host stream splitting before relying on them for pager cases.
- Route Rust panic and all current fatal handlers, including double fault,
  through `crash::begin` without first using blocking debug logging.
- Keep best-effort screen/COM1 text strictly after capsule completion.

**Exit:** explicit panic, fatal page fault, double-fault, and synthetic
non-halting user-fault integration boots all produce decodable capsules or an
explicit partial-export result and never hang the harness indefinitely.

### U3 — SMP NMI rendezvous and exact exception frames

**Files:** GDT/TSS panic IST support, LAPIC NMI broadcast, naked crash NMI and
selected exception entries, layout tests.

- Capture/ack/halt every responsive CPU with bounded owner wait.
- Preserve exact GPRs on panic NMI.
- Convert #PF/#GP/#DF/#UD first to exact diagnostics-owned frames while
  retaining recoverable page-fault return semantics; expand to remaining
  fatal exceptions after focused qualification.

**Exit:** injected crashes on BSP and AP capture all four CPUs in 100 repeated
SMP=4 runs; a test CPU intentionally refusing acknowledgement yields a valid
partial capsule with the correct missing-CPU bit instead of a hang.

### U4 — Scheduler and CPU-handoff shadow

**Files:** new `shadow/{mod.rs,scheduler.rs,cpu.rs}`, `invariants.rs`, hooks in
`scheduler.rs`, `run_queue.rs`, preemption/context-switch/user-switch paths.

- Implement entity composite states, CPU handoff phases, domain epochs,
  pending operations, first-violation latch, transition/checkpoint policies.
- Reconcile bounded scheduler snapshots against the shadow after stable
  operations.
- Add exact negative tests for duplicate-running, queued-unpublished,
  affinity, wrong publication, CR3/current PID order, and stuck handoff.

**Exit:** clean scheduler/SMP/userland-switch tests produce no violations;
every injected corruption triggers its documented `SCHED-*`/`CPU-*` ID at the
earliest intended hook and appears in the capsule.

### U5 — Lazy page-in, block-token, and kernel-continuation shadow

**Files:** new `shadow/pager.rs`, typed page-in results in `usercopy.rs`, hooks
in ring-3 page-fault handling, `File::read_at`, VirtIO block request/IRQ code,
ring-3 I/O block/wake/resume paths, decoder and focused tests.

- Add a stable page-fault classification/terminal-reason schema without
  changing the userspace `EFAULT`/SIGSEGV ABI.
- Correlate VMA/address-space generation, virtual page, backing file window,
  private frame, request token/queue head, continuation/stack generation, and
  every wake/resume decision in one shadow transaction.
- Require exact read length, retain I/O status/used length, report rollback
  failure, and optionally checksum the populated page for host comparison.
- Detect generic signal wake of `WaitingForBlockIo`, premature result consume,
  wrong token, lost wake, double completion, incomplete/zero continuation,
  and process/address-space/stack retirement with an outstanding transaction.
- After observation hooks prove the current sequence, change page commit to
  populate a private frame and install the present leaf only after successful
  read, zero-tail, and VMA-generation revalidation.

**Exit:** a forced failure at every `ensure_user_page` branch produces its
exact terminal reason; an injected signal during pending page-in triggers
`CONT-004` before the request is consumed; short read and device error remain
distinguishable; successful text/rodata faults show `PresentCommitted` with
matching leaf/frame generations; no user task can observe an unpopulated
present ELF leaf.

### U6 — Address-space and kernel-stack shadow

**Files:** new `shadow/address_space.rs`, `shadow/stack.rs`, hooks in
`address_space.rs`, `lifecycle.rs`, `switch.rs`, kernel-stack/stack allocators.

- Model group-shared L4 ownership, activation mask, create/abort/destroy, and
  stack generations/retirement.
- Tie CPU stable phases to expected L4, current PID, TSS/GS stack top, and
  pending publication.
- Add destroy-active-L4, premature stack reuse, wrong stack owner/RSP, and
  pthread home-CPU negative tests.

**Exit:** fork/exec/pthread/SMP suites remain clean in record and strict mode;
negative tests distinguish address-space lifetime from stack lifetime rather
than collapsing both into a later page fault.

### U7 — Frame/mapping shadow and incremental raw auditor

**Files:** new `shadow/memory.rs`, mapper/frame allocator typed reasons,
diagnostic worker, memory/address-space tests.

- Allocate/prefault bounded frame/mapping ledgers.
- Instrument allocator, page-table, leaf, COW, move, rollback, prune, and
  destroy commit points with generations and typed ownership.
- Implement process/L4 inventory copy and independent incremental raw walker.
- Remove `Unknown` reasons from every user page-table/COW path before strict
  completion.

**Exit:** mapper, VM, userland, COW/fork, low-memory rollback, and address-space
drop tests pass strict audits; injected leak, double release, wrong unmap,
missing USER ancestor, W+X, stale generation, and destroy-live-tree cases
produce exact `MM-*` IDs.

### U8 — Critical tracked locks and lock-context invariants

**Files:** lock wrappers, new `shadow/locks.rs`, tracked static declarations,
lock-focused tests.

- Add optional lock classes, owner/wait state, acquisition context, and edge
  recording without recursive recorder/shadow locking.
- Observe and review the baseline dependency graph.
- Codify the allowed partial order, then enable cycle enforcement in strict
  mode.

**Exit:** known clean workloads produce the reviewed graph; injected
recursion, wrong-owner release, wrong mutex context, and A→B/B→A cycle produce
`LOCK-*`; a crash while a remote CPU owns each critical lock still exports a
complete shadow/lock section.

### U9 — End-to-end policies, artifact tests, and documentation

**Files:** `scripts/test-crash-diagnostics.sh`, test fixtures/injections,
CLAUDE subsystem docs, root guidance, decoder/report docs.

- Add expected-fatal QEMU cases with timeout and artifact assertions.
- Test signature stability and missing-section behavior.
- Document mode selection, overhead, artifact interpretation, invariant ID
  ownership, schema versioning, and how agents must report partial evidence.
- Record final memory/runtime overhead for minimal/record/strict.

**Exit:** the full acceptance matrix below passes, artifacts survive clean
workspace reruns, and a fresh agent can distinguish capsule fact, decoder
inference, and absent evidence from the checked-in docs.

---

## Tests and validation

### Pure/unit tests

- Capsule layout/offset/CRC and golden Rust↔Python encoding.
- Trace commit, nested writer, wrap, overwrite, partial slot, and snapshot
  retry behavior.
- Every legal/illegal scheduler, CPU, address-space, stack, frame, mapping,
  pager/request/continuation, and lock transition from compact tables.
- Small exhaustive sequences (bounded entity/CPU/frame counts) to prove no
  legal path reaches contradictory composite state and forbidden paths return
  stable IDs.
- Mapping-ledger hash collision, tombstone, capacity, generation reuse, and
  no-eviction behavior.

### In-kernel positive tests

- Existing `scheduler`, `smp`, `interrupts`, `memory`, `heap`, `vm`,
  `userland`, `userland_switch`, and `pthreads` modules under diagnostics.
- Context save/publish/migrate and ring3↔kernel handoff with zero shadow
  violations.
- Fork+COW+unmap+exit audits with exact reconstructed refcounts.
- Lazy text/rodata faults that suspend on VirtIO and resume on the same and a
  different eligible CPU, with exact page bytes and terminal transaction.
- Low-memory injected rollback leaves production and shadow empty/consistent.
- Kernel-thread and ring-3 stack allocate/handoff/retire/reuse generations.

### Negative/injection tests

Each test asserts the first invariant ID, not only "some panic occurred":

- duplicate entity on two CPUs;
- `Ready` entity queued while unpublished;
- dispatch on wrong affinity CPU;
- CR3 from process A with `current_user_pid`/scheduler process B;
- no VMA, permission failure, allocation/map failure, invalid ELF extent,
  VirtIO submission/device error, short read, and rollback-unmap failure;
- signal wake while `WaitingForBlockIo`, wrong-token wake, wake before blocked
  publication, result consumption before completion, duplicate completion,
  and lost bounded wake-slot publication;
- continuation with zero/noncanonical RIP, RSP outside its live stack
  generation, double consume, or dispatch after owner/address-space teardown;
- concurrent same-L4 access to a page whose population has not committed;
- destroy L4 during loading/active phase;
- missing/double frame retain/release;
- unmap/move wrong frame generation;
- raw PTE omitted from mapping ledger and ledger entry omitted from raw PTEs;
- missing USER ancestor, user W+X leaf, user tree in reserved slot;
- free/reuse current or pending-publication kernel stack;
- recursive/wrong-context/wrong-owner lock operation and dependency cycle;
- shadow/mapping capacity exhaustion;
- crash during odd shadow epoch;
- nested panic during capsule serialization;
- missing CPU acknowledgement and unavailable debugcon.

### Case-driven differential experiments

Use the Git incident as an acceptance case, but do not encode the proposed
root cause as an expected answer:

1. Repeat the clone under SMP=1 and SMP=4 with identical disk/network images.
2. Run a large HTTPS transfer with `/host/CURL.ELF` under SMP=1 and SMP=4,
   recording faults in curl's own text/rodata separately from file/network
   I/O. A matching typed page-in failure strongly generalizes beyond Git; a
   clean run is weak evidence because it may not reproduce the same late code
   pages, signals, or process topology.
3. Run a local, network-free workload that repeatedly executes cold paths in
   a large `/host` ELF while VirtIO completion is delayed. This isolates
   sparse ELF + block continuation from TLS/network and Git pipes.
4. Add a diagnostic-only eager-ELF personality and interleave eager/lazy Git
   runs with the same seed/topology. A statistically material failure-rate
   collapse in eager mode implicates the lazy-page path without claiming
   which branch failed.
5. Inject an actionable signal after `Submitted`/`PublishedBlocked` but before
   `DeviceCompleted`. The result must remain blocked with the signal pending;
   the old generic-wake behavior must reproduce `CONT-004` deterministically.
6. Delay completion and independently inject wrong token, short transfer,
   I/O status error, VMA mutation, and frame-allocation failure. Each report
   must name a different terminal reason and causal timeline.

For every run, compare the first failed page-in transaction, not merely the
last process exit. Child pipe errors, orphan adoption, missing EOF, and parent
SIGCHLD are downstream timeline facts unless their event precedes the first
pager/continuation contradiction.

### Expected-fatal QEMU integration

`scripts/test-crash-diagnostics.sh <case>` launches a fresh QEMU with a hard
host timeout and expects failure. For each case it validates:

- QEMU exits/finalizes within the bound;
- capsule header/footer/CRC and build/run IDs agree;
- expected CPU capture bitmap and trap fidelity;
- expected invariant/vector/signature;
- symbolized top frames use the matching ELF;
- `report.json` declares every missing/truncated section;
- no blocking serial line is required for success.

Run matrix:

| Dimension | Values |
|---|---|
| CPUs | 1, 4, 8 (8 for focused rendezvous only) |
| RAM | 128M, 256M, 2G |
| mode | minimal, record, strict |
| fault CPU | BSP, AP |
| context | kernel thread, ring 3, timer interrupt, lazy ELF page-in, suspended I/O continuation, mapper transaction, tracked-lock owner |
| fatal | panic, #PF, #GP/#UD, #DF, invariant, nested fatal |

### Overhead and perturbation budgets

- Minimal/rich event recording performs no allocations, locks, formatting, or
  port I/O; verify by code review and instrumentation counters.
- Minimal static recorder budget: ≤64 KiB at 8 CPUs; rich recorder target:
  ≤512 KiB.
- Rich shadow allocation: report exact bytes, target ≤2% guest RAM and hard
  cap 32 MiB unless a later measured plan changes it.
- Record mode full-suite wall time target: no more than 20% above identical
  diagnostic-off QEMU topology after removing build time.
- Strict mode may cost up to 2× for focused suites; audits must remain sliced
  and must not cause watchdog kills or UART floods.
- Capsule output is size-bounded and timeout-bounded. Truncation is preferable
  to a hung crash owner.

### Acceptance soak for the diagnostics themselves

- 100 repeated clean boots of the focused SMP/scheduler/userland-switch/MM
  set in record mode at SMP=4: zero false invariant latches.
- 100 repetitions per synthetic injected race/corruption: expected invariant
  is the first signature every time.
- 100 successful text/rodata page-ins with forced I/O suspension at SMP=1 and
  SMP=4: exact-token completion, exact byte count, committed page, and no
  premature signal wake.
- 100 AP panic rendezvous runs: no missing CPU in the normal case.
- Deliberate missing-CPU case: 100 valid partial capsules, zero host hangs.

This soak validates diagnostic reliability. It does not claim the underlying
kernel is generally panic-free.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Recorder/shadow changes timing and hides a race | keep minimal/record/strict personalities; embed mode; measure failure rates across modes; no UART in hot hooks |
| Shadow repeats the same bug as production | different composite representation, semantic preconditions, raw independent audits, deliberate omitted-hook tests |
| False positives inside valid multi-step handoff | explicit CPU/domain phases, odd/even transaction epochs, stable checkpoint checks only after commit |
| Typed failure logging still misses the causal wake | correlate page-in, request token, IRQ completion, blocked publication, wake acceptance, and continuation generation in one shadow transaction |
| Present-before-populate behavior is treated as normal forever | instrument it explicitly, then make private-frame population plus atomic leaf commit a U5 exit gate; strict mode reports incomplete coverage until then |
| Signal semantics intentionally interrupt some waits | distinguish restartable userspace syscalls from an in-progress kernel page-fault continuation; require explicit cancel/ack before the latter becomes runnable |
| Host ELF differs from mounted backing | record build/run source identity and page checksum; decoder labels comparison unavailable/untrusted instead of asserting corruption |
| Crash CPU blocks on a lock held by halted CPU | categorical ban on production locks after election; crash-readable atomics/static shadow only |
| NMI interrupts shadow/trace write | committed-slot protocol, domain pending operation, panic IST, bounded retry/partial flags |
| NMI rendezvous is unreliable on one QEMU build | qualify both supported binaries; capture bitmap/timeout; fixed-IPI lower-fidelity fallback if required |
| Panic path faults recursively | static/prefaulted arena, bounded raw reads, recursion guard, first-failure state, secondary minimal marker |
| Debugcon output is truncated or missing | CRC/footer, byte/section counts, host timeout, partial report; optional static memory extraction/QMP follow-up |
| Symbolization uses wrong ELF | build/run/manifest hashes; refuse trusted symbols on mismatch |
| Full MM ledger consumes too much RAM | size from usable frames, hard percentage/byte cap, explicit downgrade; strict refuses incomplete coverage |
| Mapping table fills under large workloads | no eviction; latch capacity violation; tune from measured load factors in a separate change |
| Lock instrumentation recursively uses locks | atomics/per-CPU slots only; recorder/shadow code forbidden from tracked locks; recursion counter |
| Incremental audit observes moving target forever | audit generations, bounded restart count, transition checks remain live, report starvation as `DIAG-002` |
| Exact exception-entry refactor destabilizes demand paging | land capsule with partial fidelity first; convert #PF/#GP/#DF/#UD separately with layout and recovery tests |
| Diagnostic info enlarges BIOS kernel past bootloader limits | frame pointers first, retain host symbol bundle separately, measure every profile before enabling extra debuginfo |
| Plan scope turns into a full verification kernel | first boundary is scheduler/CPU/pager-token-continuation/AS/MM/stack/critical locks only; full filesystem/driver semantics remain follow-ups |

---

## Success criteria

1. Every supported fatal path produces a valid or explicitly partial capsule
   and never relies on production locks, heap, filesystem, display, or normal
   logger for completion.
2. Every abnormal ring-3 exit first commits a bounded non-halting incident
   snapshot; strict mode escalates failed kernel-managed recovery without
   treating genuine userspace protection faults as kernel panics.
3. SMP crash rendezvous captures every responsive CPU and reports missing CPUs
   without hanging.
4. Per-CPU rings show a committed, symbolizable semantic timeline through
   scheduler context publication, CR3 switching, mapping, frame ownership,
   stack retirement, and critical lock state.
5. The shadow kernel distinguishes at least these root classes before a later
   generic page fault: scheduler publication, CPU/CR3 handoff, lazy page
   classification/population, I/O token/wake, suspended-continuation validity,
   address-space lifetime, frame/PTE ownership, kernel-stack reuse, and lock
   misuse.
6. All initial invariant IDs have positive and deliberate-negative tests; the
   negative tests name the expected first ID.
7. A full strict raw audit reconciles live user page tables, mapping ledger,
   address-space inventory, frame ledger, and allocator refcounts after
   fork/COW/unmap/exit stress.
8. Every recoverable ring-3 page fault records one typed terminal result. A
   committed ELF page has exact read length, live VMA/frame generations, and
   no interval in which an unpopulated present leaf is user-visible.
9. Crashes while remote CPUs own the scheduler, process table, mapper, stack,
   heap, or serial lock still emit readable CPU/shadow/lock evidence.
10. Decoder output is deterministic, schema-versioned, CRC-validated,
   build-symbol trusted, and directly usable by an agent without parsing
   free-form boot logs.
11. Record-mode clean soak has zero false violations; injected failures produce
   stable signatures in 100/100 trials.
12. Root and subsystem guidance documents the diagnostic modes, transition
    hook rules, invariant catalog, crash-path prohibitions, and how to add new
    shadow domains without weakening independence.

---

## Follow-up plans enabled by this work

1. Statistical soak runner and evidence ledger with interleaved baseline /
   candidate / revert trials and confidence bounds.
2. Seeded scheduler/interrupt perturbation and bounded-preemption schedule
   exploration using the event/epoch schema defined here.
3. QEMU record/replay and reverse-GDB qualification for a replay-friendly TCG
   topology.
4. Coverage-guided syscall/workload corpus, crash deduplication, and automatic
   reproducer minimization.
5. MCP/agent tools such as `crash.list`, `crash.show`, `trace.slice`,
   `state.diff`, `experiment.run`, and `patch.validate` backed by the decoded
   artifact database.
6. Targeted memory sanitizers: sampled guarded allocations, poison/quarantine,
   unsafe-access provenance, and KCSAN-like sampled race watchpoints.

Design references for those follow-ups and the choices above:

- [Linux KASAN](https://docs.kernel.org/dev-tools/kasan.html)
- [Linux KCSAN](https://docs.kernel.org/dev-tools/kcsan.html)
- [Linux KFENCE](https://docs.kernel.org/dev-tools/kfence.html)
- [Linux lockdep design](https://docs.kernel.org/locking/lockdep-design.html)
- [syzkaller internals](https://github.com/google/syzkaller/blob/master/docs/internals.md)
- [QEMU execution record/replay](https://qemu.readthedocs.io/en/latest/devel/replay.html)
