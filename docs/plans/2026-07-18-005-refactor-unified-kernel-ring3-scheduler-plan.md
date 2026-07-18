---
title: "refactor: Unify kernel-thread and ring-3 scheduling"
status: implemented
created: 2026-07-18
completed: 2026-07-18
plan_type: refactor
depth: deep
related_docs:
  - CLAUDE.md
  - src/arch/x86_64/CLAUDE.md
  - src/process/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/window/CLAUDE.md
  - src/net/CLAUDE.md
  - src/tests/CLAUDE.md
  - docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md
  - docs/plans/2026-07-17-002-feat-basic-network-stack-plan.md
---

# refactor: Unify kernel-thread and ring-3 scheduling

## Outcome

Replace the two runnable classes with one scheduler-owned entity registry and
one fair run queue. Kernel threads and ring-3 processes keep different
architecture-specific context storage and resume mechanics, but they use the
same state machine, enqueue/block/wake operations, time accounting, and
selection policy.

Move every PIT-based deadline into one indexed min-heap. The PIT interrupt only
advances time, records the interrupted entity, makes bounded scheduling
decisions, and flags due timer work. A latency-contracted timer-service kernel
thread expires a bounded number of events per pass without scanning process
tables or allocating temporary vectors.

Make the compositor and network worker event/deadline driven, with measurable
dispatch contracts under CPU-bound load. Remove strict ring-3 preference, the
every-second-tick forced return to `KERNEL_CONTEXT`, compositor-owned sleep
expiration, and the main-loop dispatch handoffs.

The end state is:

```text
PIT / yield / block / wake
           |
           v
  Scheduler<EntityId>
  - one current entity
  - one ready queue
  - one blocked state model
  - fair RR + bounded latency overrides
           |
           v
  DispatchAdapter(EntityId)
      /                 \
kernel CpuContext     user UserState + CR3/FS/FPU/rsp0

Timer producers -> indexed min-heap -> bounded timer-service -> exact wake/action
```

## Current-state evidence

The reliability problem is structural rather than a tuning issue:

- `src/process/scheduler.rs:64-81` owns kernel-thread `current`,
  `ready_queue`, and `sleep_queue`, while
  `src/userland/lifecycle.rs:237-245` independently owns `ring3_ready` and
  `ring3_blocked`.
- `Scheduler::next_runnable` at `src/process/scheduler.rs:146-167` gives
  ring-3 strict preference, but most live paths bypass that method and pop the
  ring-3 queue directly.
- The CPL=3 PIT path at `src/arch/x86_64/preemption.rs:312-329` forces the
  current user process back to the kernel main loop every second tick, then
  uses a separate user-to-user decision on intervening ticks.
- Kernel voluntary yield and sleep paths in `src/process/mod.rs` peek into the
  userland queue and sometimes bounce through `KERNEL_CONTEXT` solely so the
  main loop can dispatch a user process.
- The main loop at `src/kernel.rs:758-781` first runs a kernel thread, then
  separately pops and resumes a user process. It is both an idle loop and a
  cross-class dispatcher.
- Kernel-thread sleeps use a deadline-ordered `BTreeMap<u64, Vec<ProcessId>>`,
  but expiration at `src/process/scheduler.rs:483-499` first allocates a
  temporary `Vec` of expired keys in the PIT path.
- Ring-3 `nanosleep` deadlines remain embedded in blocked reasons and are found
  by scanning `ring3_blocked` into another temporary `Vec` at
  `src/userland/lifecycle.rs:826-854`.
- That scan runs from `src/window/compositor.rs:79-86` because the main loop
  can starve. The compositor therefore owns unrelated process lifecycle work.
- `ITIMER_REAL` repeats a full process-table scan in
  `src/userland/lifecycle.rs:762-823`; network timeout expiration is mixed into
  a conservative blocked-process scan at lines 727-759.
- The compositor is permanently runnable and voluntarily yields after every
  pass. The network worker sleeps against a separate kernel sleep queue. Their
  service latency is an emergent function of queue population rather than an
  explicit contract.
- The switch code has four distinct handoff shapes hidden behind global state:
  kernel-to-kernel, kernel-to-user, user-to-user, and user-to-kernel. The last
  two currently depend on `current_user_pid`, `IN_SPAWNED_PROCESS`, and a
  shared `KERNEL_CONTEXT` whose ownership changes by path.

The previous multi-ring-3 plan deliberately minimized risk by retaining the
parallel queues. That was appropriate for enabling fork and multiple shells,
but its deferred fair-share work is now the largest scheduler risk.

## Goals

- Exactly one authoritative scheduler state for every runnable execution
  entity, regardless of privilege level.
- Exactly one ready queue; an entity appears in it at most once.
- Fair round-robin service for ordinary work, with no privilege-class bias.
- A dispatch-latency contract for timer service, compositor work, and network
  polling under CPU-bound workloads.
- One deadline facility for kernel sleeps, `nanosleep`, socket timeouts,
  `ITIMER_REAL`, compositor cadence, and network polling.
- No heap allocation, collection scan, process-table scan, or domain callback
  in the PIT interrupt.
- Bounded timer expiration and bounded latency-override work outside interrupt
  context.
- Direct selection of any entity after a timer preemption, voluntary yield,
  block, exit, or wake; no mandatory trip through the kernel main loop.
- Preserve address-space, FS_BASE, FPU, TSS.rsp0, per-process syscall stack,
  syscall re-fire, and kernel cooperative-context invariants.
- Preserve single-CPU lock safety and make lock ordering explicit.
- Provide scheduler/timer telemetry sufficient to prove fairness, latency, and
  bounded backlog behavior in QEMU.

## Non-goals

- SMP, per-CPU queues, load balancing, CPU affinity, or TLB shootdown.
- Real-time scheduling, arbitrary user priorities, `nice`, cgroups, or POSIX
  scheduling APIs.
- Tickless idle or replacing the 100 Hz PIT.
- Replacing process/resource ownership: kernel PCB storage may stay in
  `src/process`, and ring-3 address spaces/fds/signals stay in `src/userland`.
- Unifying `CpuContext` and `UserState` into one register structure. Their
  layouts and restore requirements are intentionally different.
- Interrupt-driven VirtIO networking.
- Making long IDE PIO transactions preemptible. Those declared atomic epochs
  remain an explicit latency exception and must be measured separately.

## Required invariants

1. Every live schedulable object has one stable `EntityId` and one
   scheduler-owned state: `Ready`, `Running`, `Blocked`, or `Dead`.
2. `Ready` means present exactly once in the run queue; `Running`, `Blocked`,
   and `Dead` mean absent.
3. `Scheduler::current` identifies the execution state actually loaded on the
   CPU. `current_user_pid` may remain temporarily as architecture state, but it
   must agree with `Scheduler::current == UserProcess(pid)` whenever set.
4. A state transition and its queue mutation are atomic under the scheduler
   lock. Wake-before-block and duplicate-wake races cannot lose an entity or
   enqueue it twice.
5. The scheduler never acquires `PROCESS_TABLE`, `NETWORK`, a window-manager
   lock, or the timer-heap lock while holding its own lock.
6. Domain code never calls the scheduler while holding `PROCESS_TABLE` or
   `NETWORK`. It stages the exact `EntityId`/event, releases the domain lock,
   then wakes.
7. The PIT path performs no operation whose cost grows with the number of
   blocked entities or timers and performs no allocation.
8. A timer is identified by `(EntityId, TimerKind)` plus a generation. Rearm
   replaces the existing deadline; cancellation prevents a stale expiry.
9. At most `MAX_TIMER_EXPIRATIONS_PER_PASS` events are delivered before the
   timer service yields. Remaining due work keeps the service runnable.
10. Timer callbacks never execute while the timer heap is locked.
11. An ordinary runnable entity receives one scheduling turn per fair queue
    revolution, except for a bounded number of admitted latency overrides.
12. A latency override grants one bounded service turn, not permanent strict
    priority. The worker must block again when its event/deadline is drained.
13. Context save finishes before the old entity is re-enqueued, and target
    context validation finishes before control transfers to it.
14. The PIC is EOI'd exactly once before any diverging dispatch from the timer
    interrupt.
15. Idle is selected only when the unified run queue and due deferred work are
    empty; idle is not a normal queued entity.

## Core design

### 1. Scheduler-owned runnable entities

Add a tagged identifier and scheduling metadata independent of resource PCBs:

```rust
enum EntityId {
    KernelThread(ProcessId),
    UserProcess(u32),
}

struct SchedEntity {
    id: EntityId,
    state: RunState,
    wait: Option<WaitReason>,
    runtime_ticks: u64,
    slice_remaining: u8,
    ready_since_tick: u64,
    must_run_by_tick: Option<u64>,
    queue_generation: u64,
}
```

`Scheduler` owns the entity registry, current ID, and one `RunQueue`. Kernel
PCBs continue to own `CpuContext`, stack, entry closure, name, and terminal.
User `Process` objects continue to own `UserState`, address space, kernel
stack, FS_BASE/FPU, fds, signals, and ABI state. `ProcessTable` loses
`ring3_ready` and `ring3_blocked`; its `current_user_pid` becomes a temporary
architecture compatibility field and is removed or reduced to a checked cache
in the final unit.

Use scheduler APIs only in terms of `EntityId`:

- `register(id, contract)` / `unregister(id)`;
- `make_ready(id, WakeCause)`;
- `block_current(WaitReason)`;
- `yield_current()`;
- `exit_current()`;
- `on_tick(now)`;
- `pick_next(now) -> DispatchDecision`.

Registration allocates outside interrupt context. Ready/block/wake/tick paths
must only mutate pre-existing entries and pre-reserved queue storage. Convert
`SCHEDULER` from a plain `spin::Mutex` to `InterruptMutex`: it is shared by
timer-preemptible kernel threads, IF-cleared syscall/interrupt paths, and wake
producers. Keep all guarded sections short and allocation-free.

Set and enforce `MAX_ENTITIES`; registration fails before queue storage can
grow past its boot-time reservation. A full registry is an explicit spawn/fork
failure, never an allocation attempt from an interrupt wake path.

`WaitReason` can retain domain-specific payloads (`WaitingForChild`, terminal,
GUI, pipe, network, timer, launcher exit), but it is diagnostic/state data, not
a second blocked queue. Domain wait registries may index exact waiters for
wakes; they may not own runnable state.

### 2. One fair run queue with admitted latency contracts

Use a single pre-reserved `VecDeque<EntityId>` and a one-tick (10 ms) base
quantum. Normal selection pops the front; an entity that remains runnable goes
to the back. Voluntary yield consumes the current turn and also goes to the
back. Wakeups normally join the back, preventing wake-heavy workloads from
continually cutting ahead.

The same queue also carries a small, explicit `LatencyContract`:

```rust
struct LatencyContract {
    max_dispatch_ticks: u8,
    one_shot_on_wake: bool,
}
```

Before the FIFO pop, `pick_next` performs one bounded scan of the single ready
queue for an admitted entity whose `must_run_by_tick` is due, choosing the
earliest deadline. `MAX_ENTITIES` makes the scan bounded; no secondary ready
queue or privilege class is introduced. After one contracted turn, the entity
has no override until it drains work, blocks, and is woken again. This retains
round-robin fairness for sustained CPU users while bounding event-driven
worker dispatch.

Initial contracts, measured from the wake/deadline becoming eligible to the
worker's first instruction under preemptible CPU-bound load:

| Entity | Contract | Required behavior |
|---|---:|---|
| `timer-service` | 1 PIT tick | Drain at most the per-pass timer budget, then yield/block. |
| `net-rx-tx` | 2 PIT ticks | Perform one bounded poll, publish/wake after dropping `NETWORK`, then re-arm and block. |
| `compositor` | 2 PIT ticks | Drain input/terminal invalidation and render at most one frame, then atomically recheck work and block. |

If two contracts become due together, earliest deadline wins and voluntary
blocking lets the next worker run without waiting for another PIT tick. Admit
no new contracted worker unless a scheduler test demonstrates that the set is
feasible with the one-tick quantum. Track misses rather than silently relaxing
deadlines.

The contract applies to CPU-bound contention. Time spent in an explicitly
nonpreemptible IDE PIO epoch is reported separately. Any other preemption-off
section exceeding one tick is a defect and must be split or shortened before
the latency unit can pass.

### 3. One indexed deadline min-heap

Add a scheduler timer facility with an indexed min-heap rather than
`BinaryHeap` plus unbounded stale entries:

```rust
struct TimerKey {
    entity: EntityId,
    kind: TimerKind,
}

struct TimerEntry {
    key: TimerKey,
    deadline_tick: u64,
    generation: u64,
    action: TimerAction,
}
```

The heap stores entries ordered by `(deadline_tick, sequence)` and uses a
fixed-capacity open-addressed index table from `TimerKey` to heap slot. Do not
use a `BTreeMap` here: node allocation/deallocation would defeat the bounded
queue contract. Rearm updates in place and repairs the heap; cancel removes in
`O(log n)`. Pre-allocate an explicit `MAX_TIMERS` at boot so arm, cancel,
expiry, and reheapification cannot allocate. Timer creation/rearm returns a
recoverable error if the bound is exceeded; production code must not silently
drop a deadline.

Producers include:

- kernel `sleep_ticks` -> exact kernel entity wake;
- ring-3 `nanosleep` -> exact user entity wake;
- blocking network timeout -> mark the exact syscall wait expired and wake its
  user entity;
- `ITIMER_REAL` -> raise `SIGALRM`, advance an absolute periodic deadline, and
  wake the exact process only when signal delivery can interrupt its wait;
- `net-rx-tx` -> next smoltcp poll deadline, capped by the existing active/idle
  policy;
- compositor periodic cadence only when a renderer/window actually requests a
  future frame.

The PIT interrupt increments `TIMER_TICKS`, compares `now` with an atomic cached
earliest deadline, and marks `timer-service` ready when due. It never locks the
heap or calls a timer action. The timer service repeatedly:

1. pops one due entry under the timer lock;
2. releases the timer lock;
3. delivers the exact action under its domain's normal lock ordering;
4. stops after `MAX_TIMER_EXPIRATIONS_PER_PASS` (initially 32);
5. yields while remaining due work exists, otherwise blocks until the cached
   earliest deadline is reached.

Periodic timers advance from their prior absolute deadline and skip missed
periods arithmetically, so scheduler delay does not create drift or an
unbounded catch-up storm.

### 4. Separate scheduling policy from context mechanics

Keep distinct context adapters, but make them consume the same
`DispatchDecision`:

| From | To | Save | Resume |
|---|---|---|---|
| kernel | kernel | `CpuContext` cooperative or PIT trap snapshot | kernel full restore |
| kernel | user | save kernel `CpuContext` | CR3 + rsp0 + syscall rsp + FS/FPU + user `iretq` |
| user | user | trap/syscall `UserState` + FS/FPU | existing ring-3 resume sequence |
| user | kernel | `UserState` + FS/FPU | kernel full restore on target stack |

Add one architecture entry point that receives the interrupted frame and
current `EntityId`, saves by entity kind, asks the scheduler once, EOIs once,
then diverges through the selected adapter. Do not try to make `CpuContext` and
`UserState` layout-compatible.

For voluntary block/yield/exit, the caller saves at its natural boundary and
uses the same selection result. A blocking ring-3 syscall may resume a kernel
thread directly; a yielding kernel thread may resume a user process directly.
The main loop is no longer a required trampoline.

Before enabling direct user-to-kernel dispatch, characterize the existing
same-CPL `iretq` frame construction. The replacement kernel full-restore path
must explicitly load the target stack and RIP rather than assuming `iretq`
will pop SS/RSP when returning from CPL0 to CPL0.

### 5. Event-driven compositor and network worker

The compositor must stop being permanently runnable. Introduce compositor work
bits/generations for input, terminal output, window damage, and scheduled frame
cadence. Producers set a bit and wake the compositor entity through the
allocation-free scheduler wake path. After one pass, the compositor performs
an atomic “no work arrived since snapshot” check before blocking, closing the
classic check-then-sleep missed-wake race.

Rendering can still use `PreemptionMutex`, but the latency gate records every
preemption-disabled interval. If normal retained/GPU frame work exceeds one
tick while preemption is disabled, split state snapshot/preparation from the
expensive raster/compose/present work or add safe phase boundaries. A scheduler
contract is not considered met by excluding ordinary compositor critical
sections from measurement.

The network worker performs one `poll_once`, obtains the next absolute smoltcp
deadline, arms its timer, and blocks. Socket syscalls may retain their existing
one bounded opportunistic poll. Network state-change wakes must happen after
dropping `NETWORK`; timeout wakes target the exact waiting process through the
timer event instead of scanning all blocked processes for elapsed deadlines.

## Work sequence

Each unit is a separate review/merge boundary. Do not combine the context
transition cutover with the timer cutover.

### U0 — Characterize behavior and add observability

Add counters and boot-test probes before changing policy:

- per-entity dispatch count, runtime ticks, maximum ready-to-run delay, and
  latency-contract misses;
- switches for each of the four privilege transition pairs;
- timer heap size/high-water, due backlog, events expired per pass, maximum
  deadline-to-delivery delay, cancellation count, and capacity failures;
- maximum preemption-disabled and interrupts-disabled tick duration, tagged by
  subsystem where practical;
- number of forced ring-3-to-kernel handoffs and compositor sleep scans, so the
  later deletion is observable.

Add a deterministic stress fixture with two CPU-bound ring-3 processes, a
kernel counter thread, compositor work injection, and a QEMU-local network
exchange. Record the current failure/latency profile in `.context/`; do not
commit machine-specific timing as a universal baseline.

Gate: the fixture reproduces progress for every participant and the counters
can distinguish starvation from a missed wake or timer delay.

### U1 — Introduce `EntityId` and scheduler-owned state in shadow mode

Files:

- new `src/process/entity.rs` for IDs, run state, wait reason, and contracts;
- new `src/process/run_queue.rs` for the pre-reserved single queue;
- `src/process/scheduler.rs` for the registry and transition assertions;
- `src/process/mod.rs` and `src/userland/lifecycle.rs` to register/unregister
  every kernel thread and user process.

Keep legacy queues authoritative for production dispatch during this unit.
Mirror transitions into the new registry and add test/debug assertions that
legacy and shadow state agree. Do not maintain shadow state after this unit if
the assertions reveal ambiguous ownership; fix the ownership boundary first.

Tests cover unique enqueue, idempotent wake, block/wake/exit transitions,
tagged PID non-collision, and registry cleanup.

Gate: all existing tests pass and a multi-terminal/fork workload reports zero
shadow mismatches.

### U2 — Make the single run queue authoritative

Route spawn, fork, initial user entry, kernel/user voluntary yield, every block
path, and every wake source through the scheduler API. Remove runnable state
from `ProcessTable` and stop `Scheduler` from calling userland queue helpers.

During this bridge unit, context dispatch may still fall back through
`KERNEL_CONTEXT` when the selected target belongs to the other privilege kind.
The key acceptance condition is one queue/state source, not final switch
efficiency. Delete `ring3_ready`, `ring3_blocked`, `pop_next_ring3`,
`peek_next_ring3`, and direct queue mutations once all callers migrate.

Add exact wait indexes where a wake source naturally knows the target (child,
terminal, GUI owner, timer key). Conservative pipe/network event wakes may use
a bounded scheduler wait-index walk temporarily, but may not scan
`PROCESS_TABLE` or allocate a PID vector.

Gate: ordinary round-robin selection alternates kernel and user entities by
queue order; no strict ring-3 preference remains; all existing interactive and
network tests pass.

### U3 — Add the timer heap and bounded timer service

Files:

- new `src/process/timer.rs` for the indexed heap, cache, actions, and bounds;
- a registered `timer-service` kernel entity;
- `src/arch/x86_64/preemption.rs` for the due flag only;
- `src/process/mod.rs`, `src/userland/lifecycle.rs`, and `src/net/mod.rs` for
  producer migration.

Migrate in this order:

1. kernel `sleep_ticks`;
2. ring-3 `nanosleep`;
3. network syscall timeouts;
4. `ITIMER_REAL` including periodic rearm;
5. network worker cadence;
6. any compositor future-frame deadline.

Delete `Scheduler::sleep_queue`, `check_sleep_queue`,
`process_expired_sleeps`, and `process_due_real_timers` after their last caller
moves. Remove lifecycle expiration calls from the compositor, main loop, and
inline userland launcher loop.

Gate: a same-tick timer storm larger than 32 is delivered over multiple passes
without starving a normal entity, and the PIT allocation counter remains flat.

### U4 — Make compositor/network contracts real

Convert compositor wake sources to generation-safe work bits and make the
thread block when idle. Convert the network worker to absolute timer rearm and
block. Enable the latency override only for these event-driven forms; do not
grant it while either worker remains continuously runnable.

Audit preemption-disabled spans under legacy, retained CPU, and strict GPU
rendering plus active TCP traffic. Split any ordinary (non-IDE) span that
prevents the two-tick contract. Add serial diagnostics on every contract miss
in test/debug builds and aggregated counters in release builds.

Gate under two CPU-bound user processes plus normal kernel work:

- timer service starts within one tick of a due flag;
- compositor and network worker start within two ticks of becoming eligible;
- normal entities continue to make progress and their runtime share differs
  by no more than one turn per completed fair-queue revolution.

### U5 — Unify timer-preempt and voluntary dispatch

Refactor the architecture switch boundary around `EntityId` and implement all
four transition pairs. Make the timer handler save the interrupted current
entity once, call the scheduler once, EOI once, and dispatch the returned
target directly.

Route kernel yield/sleep/block/exit and ring-3 syscall block/exit through the
same picker. Preserve the syscall instruction rewind and restart-stable ABI
state. Validate CR3, TSS.rsp0, syscall GS stack top, FS_BASE, and FPU ordering
on every user target.

After the four transition tests pass, delete:

- the every-second-tick `preempt_ring3_to_kernel` policy;
- `try_preempt_ring3` and `Scheduler::next_runnable`;
- `save_kernel_and_resume_ring3` as a main-loop dispatch mechanism;
- cross-class peeks in kernel yield/sleep;
- `IN_SPAWNED_PROCESS` if no architecture-only use remains;
- main-loop ring-3 queue popping and preemption bounce handling.

Idle becomes a minimal `hlt` loop chosen only when the scheduler returns no
runnable target.

Gate: all four transition pairs execute repeatedly in one boot test, including
callee-saved registers, caller-saved registers for PIT-preempted contexts,
user FS_BASE/FPU isolation, and correct kernel stack restoration.

### U6 — Stress, cleanup, and documentation

Run long mixed workloads and fault injection:

- two terminals running CPU-bound loops while Painting animates;
- repeated fork/exec/wait plus nanosleep and interval timers;
- QEMU-local ping/TCP/HTTP while dragging windows and scrolling terminal
  output;
- simultaneous timer cancellation/rearm/exit;
- timer capacity exhaustion and a due storm;
- process exit while ready, blocked on every wait kind, and owning active
  timers;
- scheduler/timer lock contention injection in test builds.

Update subsystem `CLAUDE.md` files and `docs/ARCHITECTURE.md` to name the
single scheduler, timer service, lock order, latency contracts, and remaining
single-CPU limitations. Remove stale U3/U5/U10 comments that describe the
parallel queues as current architecture.

Gate: zero contract misses in the CPU-bound qualification workload, zero stale
timer deliveries, zero duplicate queue entries, and no progress regressions in
the full QEMU suite.

## Tests and validation

Add a dedicated scheduler/timer test topic rather than extending the already
large userland module for pure policy tests.

Pure/in-kernel tests:

- tagged IDs with equal numeric PID values remain distinct;
- enqueue is unique and FIFO order is stable;
- block/wake and wake/wake are idempotent;
- wake concurrent with park cannot be lost;
- normal entities receive equal turns over many queue revolutions;
- a due latency contract overrides FIFO once, then returns to fair order;
- contracted workers that fail to block are demoted and counted as violations;
- min-heap order, equal deadlines, update-in-place, cancel, generation reuse,
  periodic missed-period arithmetic, tick saturation, and capacity failure;
- exactly 32 of 33 due timers expire in the first pass and the last expires in
  the next without allocation;
- exit cancels every timer key for that entity before resource teardown;
- four-way context switching preserves the documented register and CPU state.

Booted qualification:

```sh
cargo fmt --check
cargo check
./test.sh --skip-userland scheduler
./test.sh --skip-userland userland_switch
./test.sh --skip-userland userland
./test.sh --skip-userland network
./test.sh --skip-userland network_userland
./test.sh --skip-userland
./test.sh
```

Add one bounded end-to-end scheduler stress test to `test.sh` that fails on a
latency miss or missing progress rather than relying only on serial inspection.
Interactive qualification should run both retained CPU and the available
qualified GPU path because long compositor critical sections affect the
network contract even when scheduler policy is correct.

## Rollout and review boundaries

- U0 and U1 are behavior-neutral and safe to merge independently.
- U2 is the ownership cutover; retain the bridge dispatcher until its queue
  invariants are proven.
- U3 changes deadline ownership but not the low-level switch matrix.
- U4 enables service contracts only after workers become event driven.
- U5 is the highest-risk assembly change and must not contain timer-policy or
  worker refactors.
- Keep test/debug assertions for queue uniqueness, current-entity agreement,
  timer generation, and latency misses for at least one release after U6.

If U5 exposes an architecture blocker, the acceptable temporary state is the
single authoritative queue plus timer service using the bridge dispatcher.
Do not restore parallel queues or compositor-owned expiration as a rollback.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Same-CPL `iretq` or stack restoration is wrong on user-to-kernel switch | Triple fault or silent stack corruption | Characterize first; add a dedicated full-restore primitive that explicitly loads kernel RSP/RIP; test every transition pair before deleting the bridge. |
| Queue state and owner object disappear in different orders | Resume of freed PCB/address space | Scheduler `Dead`/unregister transition precedes owner drop; dispatch occurs with interrupts disabled on the single CPU; assert target liveness before restore. |
| Missed wake between worker recheck and block | Frozen compositor/network | Work generation is sampled, then `park_current_if_unchanged` performs the final comparison and block transition atomically. |
| Latency overrides starve ordinary work | CPU-bound user regression | One-shot override per wake, admission limited to the three named workers, one-tick quantum, per-revolution fairness assertions. |
| Long compositor or network critical section defeats the contract | Jank or network timeout despite correct queue order | Measure preemption-off spans, split ordinary spans above one tick, keep IDE epochs separately tagged, fail qualification on untagged overruns. |
| Timer cancellation leaves stale events | Wrong process woken after PID/resource reuse | Stable tagged entity identity plus timer generation; indexed removal; unregister cancels all entity keys before owner teardown. |
| Timer storm monopolizes deferred work | Normal work starvation | Fixed 32-event pass, arithmetic periodic catch-up, latency worker yields between passes, backlog telemetry and stress test. |
| Timer capacity failure silently drops a wake | Permanent hang | Pre-reserved explicit cap, fallible arm API, fail/propagate at call site, loud diagnostics and capacity test. |
| New lock ordering deadlocks the single CPU | Full system freeze | `InterruptMutex<Scheduler>`, no scheduler-to-domain lock acquisition, pop timer before callback, staged wake IDs after dropping domain locks, lock-order assertions in debug builds. |
| Watchdog mistakes fair waiting for a hung entity | Healthy process killed | Track runtime/progress per entity; only charge watchdog time while the entity is actually running, not while ready or blocked. |

## Success criteria

- There is one ready queue and one current schedulable entity in production
  code; `ProcessTable` contains no ready/blocked scheduler structures.
- No scheduling decision prefers ring 3 merely because it is ring 3.
- No fixed tick periodically forces user execution through the kernel main
  loop.
- No sleep/timer expiration scans all blocked processes or allocates a
  temporary PID/deadline vector.
- The compositor contains no process sleep-expiration call and blocks when it
  has no work.
- The network worker uses the shared deadline facility and meets its measured
  two-tick dispatch contract under the CPU-bound qualification workload.
- Timer work is bounded to 32 expirations per pass, with backlog visible and
  eventually drained.
- Every kernel/user context transition pair is covered by a booted test.
- Two CPU-bound ring-3 processes, compositor work, network traffic, and normal
  kernel threads all make predictable progress with zero qualification
  contract misses.
- The complete QEMU test suite and interactive multi-terminal/network/GUI
  qualification pass.
