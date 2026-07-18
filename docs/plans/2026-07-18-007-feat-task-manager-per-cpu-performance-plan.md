---
title: "feat: Task Manager per-CPU performance graphs"
status: implemented
created: 2026-07-18
plan_type: feat
depth: medium
related_docs:
  - docs/plans/2026-07-18-003-feat-ring3-task-manager-and-procfs-plan.md
  - docs/plans/2026-07-18-006-feat-smp-support-plan.md
  - src/arch/x86_64/CLAUDE.md
  - src/process/CLAUDE.md
  - src/userland/CLAUDE.md
---

# feat: Task Manager per-CPU performance graphs

## Outcome

When AgenticOS boots with more than one logical processor, Task Manager's
Performance tab shows a live history graph for each online processor and an
accurate aggregate CPU percentage. A single-processor boot keeps the same
compact experience with one graph.

The implementation also repairs the SMP semantics of `/proc/stat`. Today its
`cpu0` line is a copy of aggregate process/thread runtime, and aggregate idle
time is derived from the BSP wall clock. Once several CPUs execute in
parallel, that can undercount capacity, clamp real aggregate work at 100%, and
attribute every CPU's work to CPU 0. The new source of truth is monotonic
per-CPU timer accounting; `/proc/stat`, `/proc/uptime`, shell tools, and Task
Manager all consume the same counters.

Target experience on the default four-CPU boot:

```text
+---------------------- Performance ----------------------+
| +----------------------+  +----------------------+       |
| | CPU 0          18.2% |  | CPU 1          72.0% |       |
| |       history        |  |       history        |       |
| +----------------------+  +----------------------+       |
| +----------------------+  +----------------------+       |
| | CPU 2           4.0% |  | CPU 3          51.5% |       |
| |       history        |  |       history        |       |
| +----------------------+  +----------------------+       |
| +-------------------- Memory ---------------------+       |
| |                     history                     |       |
| +-------------------------------------------------+       |
| CPU total 36.4%   4 logical processors   Up 0:12:08      |
+----------------------------------------------------------+
```

## Current-state evidence

- `userland/apps/taskmgr/src/main.rs` owns one `cpu_graph` and computes CPU
  usage as `(delta user + delta system) / delta wall ticks`, capped at 100%.
  That was valid only while one CPU supplied all execution capacity.
- `userland/apps/taskmgr/src/sampler.rs` parses only the aggregate `cpu` line
  from `/proc/stat`; it discards every `cpuN` line.
- `src/userland/procfs.rs::gen_stat` emits one aggregate line and one `cpu0`
  line with identical values. `cpu_totals()` sums live ring-3 process time and
  live kernel-thread runtime, so totals can drop when an entity is reaped.
- The SMP work already provides fixed logical CPU slots, an online CPU count,
  a 100 Hz PIT tick on CPU 0, and calibrated 100 Hz LAPIC timer ticks on the
  APs. `CpuLocal` is therefore the natural lock-free home for monotonic CPU
  time counters.
- `TimeSeriesGraph` already supports the required fixed 0..100 history. No
  GUI ABI or shared toolkit extension is needed; Task Manager can own a
  bounded vector of graph instances, one per online CPU (maximum eight).

## Requirements

### R1 — Monotonic per-CPU accounting

1. Add per-CPU `user_ticks`, `system_ticks`, and `idle_ticks` atomics to
   `src/arch/x86_64/percpu.rs`, plus a small snapshot accessor for any logical
   CPU slot. Counters start at zero when that CPU initializes and are never
   reset or decremented.
2. Charge exactly one bucket on every local 100 Hz scheduling timer edge,
   before any `try_lock`, early return, signal exit, or preemption decision:
   - interrupted CPL 3 -> `user`;
   - a running spawned kernel thread -> `system`;
   - the per-CPU idle loop sleeping in `hlt` -> `idle`;
   - BSP/AP main-loop housekeeping outside `hlt` -> `system`.
3. Mark the BSP's final `enable_and_hlt` region with the same
   `idle_interruptible` state already used by AP idle loops. This makes idle
   classification explicit instead of inferring it from the shared scheduler
   after the interrupt arrives.
4. Keep existing per-process `utime_ticks` and kernel-thread runtime counters
   for the Processes table. They answer "which task used time"; the new
   per-CPU counters answer "how much capacity did each CPU use" and remain
   monotonic across process exit.
5. Do not take the scheduler or process-table lock to charge CPU totals. The
   timer path updates only the executing CPU's cache-line-aligned `CpuLocal`
   atomics.

### R2 — Linux-shaped `/proc` output

1. Replace `cpu_totals()` as the source for `/proc/stat` and `/proc/uptime`
   with snapshots of all online CPUs.
2. Emit one `cpuN` line for every online logical CPU and no lines for unused
   `MAX_CPUS` slots. Each line uses Linux's existing ten-field shape:

   ```text
   cpuN <user> 0 <system> <idle> 0 0 0 0 0 0
   ```

3. Emit aggregate `cpu` fields as the saturating sum of the corresponding
   online `cpuN` fields. This makes an aggregate interval's total delta close
   to `elapsed_ticks * online_cpus`, which is the correct denominator for a
   0..100 system-wide percentage.
4. Keep `/proc/uptime`'s first value as BSP wall-clock uptime. Report its idle
   value as the sum of per-CPU idle ticks, matching Linux semantics (idle can
   exceed wall uptime on SMP).
5. Preserve snapshot-at-open behavior: gather all atomic counters once while
   generating the virtual file, then format from that owned local snapshot.
   No lock remains held across user reads.

### R3 — Task Manager sampling and rate math

1. Add a bounded `CpuTimes { id, user, system, idle }` vector to
   `sampler::Snapshot`. Parse the aggregate `cpu` row and all `cpuN` rows from
   the same `/proc/stat` open.
2. Derive utilization from counter deltas, not wall time:

   ```text
   busy_delta  = delta(user + system)
   total_delta = delta(user + system + idle)
   percent     = busy_delta / total_delta * 100
   ```

   Apply the same helper to the aggregate row and each logical processor.
   This tolerates small PIT/LAPIC calibration differences and reports the
   aggregate as the average utilization of available processors.
3. Match previous and current CPU samples by logical ID. A new/missing row,
   zero total delta, or counter rollback produces 0% for that interval rather
   than a spike or divide-by-zero.
4. Keep the existing wall-tick denominator for per-process and per-kernel-
   thread CPU columns. AgenticOS processes are single-threaded, so each row's
   maximum remains one processor (100%).
5. If per-CPU rows are absent or malformed, retain a single aggregate graph
   instead of rendering an empty Performance tab. This provides graceful
   compatibility during mixed kernel/userland builds.

### R4 — Responsive Performance tab

1. Replace `cpu_graph: TimeSeriesGraph` with bounded per-CPU history state:
   logical ID, latest percentage, and one `TimeSeriesGraph` with the existing
   120-sample/two-minute capacity.
2. Lay CPU graphs out in a maximum-two-row grid: one graph for one CPU, two
   columns for two to four CPUs, and `ceil(cpu_count / 2)` columns for five to
   eight CPUs. At the 420 px minimum window width, eight processors still get
   four compact graphs per row; at the default 640 px size they remain easy
   to distinguish.
3. Label every graph `CPU N` with its current percentage. Keep aggregate CPU
   utilization in the always-visible status strip and add a Performance tile
   such as `CPU total 36.4% (4 logical processors)`.
4. Keep the memory graph and current uptime/process/kernel-heap/memory/socket
   details below the CPU grid. Recompute all graph geometry on resize and
   whenever the first sample establishes the processor count.
5. Preserve history for an existing logical ID when rebuilding layout state.
   CPU hotplug is out of scope, but malformed/transient samples must not wipe
   every other processor's history.
6. Reuse `TimeSeriesGraph` unchanged unless implementation exposes a concrete
   drawing defect at compact sizes. This feature does not add colors per CPU,
   legends, or a new GUI syscall.

### R5 — Documentation and validation

1. Update the root current-state description and `src/userland/CLAUDE.md` to
   document per-CPU `/proc/stat` and Task Manager histories. Update
   `src/arch/x86_64/CLAUDE.md` with the timer-accounting ownership and idle
   classification invariant.
2. Extend procfs tests to parse every `cpu`/`cpuN` line, assert one per online
   CPU, and assert that the aggregate fields equal the sum of the per-CPU
   fields from the same opened snapshot.
3. Extend SMP tests to observe total CPU ticks advancing independently on
   every online CPU. Keep the test delta-based so earlier tests and normal
   boot activity do not matter.
4. Build both kernel and Task Manager, then smoke-test one-, four-, and
   eight-CPU boots. Exercise resize and sustained workloads on several CPUs.

## Detailed design

### Accounting boundary

The architecture timer handler is the only place guaranteed to observe one
sample on every online CPU at the same nominal frequency. Charging there
avoids reconstructing CPU ownership from process totals, which loses history
when tasks exit and cannot identify where migratable entities ran.

Classification uses state owned by the executing CPU:

```text
local timer interrupt
        |
        +-- saved CS is ring 3 ------------------------> user_ticks++
        |
        +-- CpuLocal.in_spawned_process --------------> system_ticks++
        |
        +-- CpuLocal.idle_interruptible --------------> idle_ticks++
        |
        `-- main-loop housekeeping -------------------> system_ticks++
```

The accounting call must occur near the top of `timer_handler_inner`, after
the CPU ID and frame are available but before either the ring-3 or ring-0 path
can return. `idle_interruptible` must stay true for the complete `sti; hlt`
wait and become false immediately after it returns, on both BSP and APs.

This is sampled tick accounting, not cycle-accurate accounting. Interrupt
handler time is attributed to the context it interrupted. That matches the
resolution and tradeoff of the existing process accounting and is sufficient
for one-second monitor samples.

### `/proc/stat` snapshot

Introduce a small internal value type in `procfs.rs` (or reuse the public
per-CPU snapshot type if it remains architecture-neutral):

```rust
struct CpuTimeSnapshot {
    id: usize,
    user: u64,
    system: u64,
    idle: u64,
}
```

`online_cpu_times()` reads `0..smp::online_cpu_count()` exactly once. Both
`gen_stat()` and `gen_uptime()` format from one local vector per open. Avoid
mixing the legacy live-process sum with these counters: doing so would make
aggregate and per-CPU lines disagree and reintroduce counter rollback.

### Userland history ownership

Keep the file-format boundary in `sampler.rs`; `main.rs` receives typed CPU
rows. A small `CpuHistory` in Task Manager owns UI state:

```rust
struct CpuHistory {
    id: usize,
    pct10: u64,
    graph: TimeSeriesGraph,
}
```

On every sample, match a typed row to the prior snapshot, calculate its
percentage, create history only for a newly seen ID, and push one sample.
Layout mutates only each graph's rectangle. The status strip continues to use
`cpu_pct10`, now calculated from the aggregate `/proc/stat` row with the same
delta helper.

## Expected file changes

| File | Change |
|---|---|
| `src/arch/x86_64/percpu.rs` | Add monotonic user/system/idle counters and snapshot accessors. |
| `src/arch/x86_64/preemption.rs` | Classify and charge every local scheduling timer tick before divergent paths. |
| `src/kernel.rs` | Mark the BSP `hlt` interval as idle, matching AP behavior. |
| `src/userland/procfs.rs` | Generate aggregate plus online `cpuN` rows from per-CPU snapshots; fix SMP uptime idle. |
| `userland/apps/taskmgr/src/sampler.rs` | Parse aggregate/per-CPU time rows into typed snapshot data. |
| `userland/apps/taskmgr/src/main.rs` | Calculate delta percentages and render/reflow the per-CPU graph grid. |
| `src/tests/procfs.rs` | Verify `/proc/stat` shape, online row count, and aggregate sums. |
| `src/tests/smp.rs` | Verify independent per-CPU accounting progresses. |
| `CLAUDE.md`, subsystem `CLAUDE.md` files | Record the delivered data contract and UI behavior. |

No changes are expected in `userland/libs/gui`, the GUI syscall ABI, the
scheduler policy, process layout, or build manifests.

## Work sequence

Each unit should leave `AGENTICOS_QEMU_SMP=1` working.

### U1 — Per-CPU counter source

Add `CpuLocal` counters and the timer-handler charge point; mark BSP idle.
Validate that each online CPU's total increases and that offline slots are not
reported. This is the load-bearing unit for everything after it.

### U2 — `/proc` contract

Switch `/proc/stat` and `/proc/uptime` to the new snapshots. Add procfs tests
for online rows, aggregate equality, monotonic snapshots, and Linux-shaped
field counts. Verify BusyBox `uptime` and any available `ps` output remain
compatible.

### U3 — Sampler and percentage derivation

Teach `sampler.rs` to retain aggregate and per-CPU rows, then centralize the
delta calculation in Task Manager. First keep one aggregate graph and verify
that aggregate percentages stay in 0..100 under parallel load.

### U4 — Per-CPU UI grid

Introduce `CpuHistory`, replace the aggregate graph panel with the responsive
grid, retain aggregate status, and reflow on resize/count discovery. Exercise
one, four, and eight graph layouts before changing documentation.

### U5 — Documentation and qualification

Update current-state docs, run focused and full tests, and perform the manual
SMP workload checklist. Mark this plan implemented only after the one-CPU
fallback and compact eight-CPU layout are both verified.

## Validation matrix

| Configuration | Automated checks | Manual checks |
|---|---|---|
| `AGENTICOS_QEMU_SMP=1` | procfs aggregate equals `cpu0`; focused procfs/SMP tests | One graph, no layout regression, CPU load reaches near 100% |
| Default SMP 4 | four `cpuN` rows; aggregate equals their sum; all counters advance | Four histories react independently while several CPU-bound tasks run |
| SMP 8 | eight rows; no unused slots; full test suite | Eight graphs remain legible at default/minimum size and after resize |
| Idle SMP | totals advance mostly in idle buckets | Aggregate and per-CPU graphs settle near zero |
| Mixed kernel/user load | monotonic counters across process exits | GUI/compile/network activity appears without percentages exceeding 100% |

Recommended commands during implementation:

```sh
./test.sh procfs smp
AGENTICOS_QEMU_SMP=1 ./test.sh procfs smp
AGENTICOS_QEMU_SMP=8 ./test.sh procfs smp
cargo check
./build.sh -n
```

Run the complete `./test.sh` suite after the focused matrix passes.

## Non-goals

- Per-CPU run queues, affinity controls, work stealing, NUMA, or scheduler
  policy changes.
- A "CPU" column showing where a process last ran. Processes can migrate, so
  a sampled last-CPU field needs a separate semantics/UI decision.
- Per-process sparklines, system/user stacked colors, interrupt/softirq/iowait
  splits, frequency, temperature, or hardware performance counters.
- CPU hotplug. The parser and history matching degrade safely, but the online
  topology remains boot-time fixed.
- Percentages above 100 for a process. AgenticOS processes remain
  single-threaded in this plan.

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| A timer path returns before charging | A CPU graph freezes or undercounts | Place one unconditional charge before CPL/preemption branches; delta-based SMP test for every CPU. |
| BSP main-loop sleep is counted as system | Idle system appears busy on CPU 0 | Use the same explicit `idle_interruptible` window as APs around `enable_and_hlt`. |
| Aggregate mixes wall ticks with CPU ticks | Parallel load clamps at 100 or exceeds it | Derive aggregate and each CPU from their own busy/total counter deltas. |
| Reaped tasks make counters go backward | One-sample utilization drop/spike | `/proc/stat` uses only monotonic per-CPU counters; task totals remain a separate table concern. |
| Eight graphs crowd the 640x480 window | Labels/history become unreadable | Cap at two rows, preserve the 420 px minimum, shorten titles to `CPU N`, and validate the minimum-size layout. |
| Task Manager and kernel are built from different revisions | Performance tab has no CPU graphs | Fall back to the aggregate row and keep `/proc/stat` text backward-compatible. |

## Success criteria

1. `/proc/stat` contains exactly one `cpuN` line for every online logical CPU,
   and its aggregate user/system/idle fields equal the sum of those lines.
2. Aggregate and per-CPU counters are monotonic across task exit and advance
   on all online processors without taking scheduler/process-table locks.
3. Task Manager displays one history per online processor, preserves the
   aggregate status percentage, and remains usable at 420x300 through 640x480
   for one, four, and eight CPUs.
4. Parallel CPU-bound workloads visibly distribute across graphs; an idle
   machine settles near zero; no system or CPU graph reports above 100%.
5. Focused procfs/SMP tests pass at one, four, and eight CPUs, followed by the
   complete test suite and an interactive default-SMP smoke test.

## Implementation notes

Implemented on 2026-07-18. `CpuLocal` now owns monotonic user/system/idle
timer counters, the timer handler charges exactly one bucket before any
divergent path, and the BSP publishes the same idle window as APs. `/proc/stat`
emits an exact aggregate plus one row per online logical CPU; Task Manager
parses those rows, derives utilization from busy/total deltas, and renders a
responsive maximum-two-row graph grid while keeping aggregate CPU in the
status/details area.

The Task Manager release build, kernel/test compilation, formatting, diff
checks, and focused procfs/SMP suites passed at 1, 4, and 8 CPUs. The complete
866-test suite also passed at the default four CPUs with
`AGENTICOS_FORCE_DIRTY_MOUNT=1`; the override was needed because the existing
target `/data` image's dirty bit otherwise activates the intentional read-only
mount gate. QEMU test snapshot mode kept that generated image unchanged.

A follow-up interactive Task Manager smoke exposed an SMP scheduler handoff
race: a completed save was published while the outgoing CPU was still using
the entity's kernel stack. The handoff now stages the destination context in
per-CPU storage, changes to the per-CPU main stack, and only then publishes the
source entity. Timer preemption follows the same deferred-publication rule.
Scheduler state tests and a repeated cross-CPU sleep/wake stress test cover the
regression; the stress passes at both four and eight CPUs.
