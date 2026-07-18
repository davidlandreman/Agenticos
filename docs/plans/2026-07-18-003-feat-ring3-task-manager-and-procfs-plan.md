---
title: "feat: ring-3 Task Manager (TASKMGR.ELF) with resource graphs, backed by a minimal /proc"
type: feat
status: complete
date: 2026-07-18
---

# feat: ring-3 Task Manager (TASKMGR.ELF) with resource graphs, backed by a minimal /proc

## Summary

Retire the kernel-side `tasks` app (`src/commands/tasks/`, ~530 lines of ring-0
window plumbing) and replace it with a standalone ring-3 application following
notepad's pattern — but substantially richer:

- **A tabbed Task Manager** (`userland/apps/taskmgr/` → `TASKMGR.ELF`) with
  three tabs: **Processes** (sortable multi-column list, End Task),
  **Performance** (real-time CPU / memory history graphs, uptime, counts), and
  **Network** (RX/TX throughput graphs, interface totals).
- **A minimal synthetic `/proc` filesystem** as the data plane. The kernel
  already synthesizes `/bin` and manages `/etc`; `/proc` follows the same
  pattern. This is the key leverage point: the same files that feed taskmgr
  make BusyBox `ps`, `uptime`, and `free` work in zsh for free, and give
  every future debugging session (and the eventual Agentic runtime) a
  scriptable window into the kernel.
- **Kernel stats plumbing that doesn't exist yet**: per-ring-3-process CPU
  tick accounting and resident-page (RSS) counting, plus read-only exposure
  of kernel threads, frame-allocator/heap stats, and NIC byte counters.
- **Toolkit growth in `userland/libs/gui`**: `TabBar`, `ColumnListView`
  (multi-column, sortable, scrollable), and `TimeSeriesGraph` (ring-buffer
  area chart) — widgets the next migrations (explorer, painting) and future
  monitoring UIs will reuse.

The GUI syscall ABI stays frozen at 5001-5004. All new kernel surface is
read-only files under `/proc`; the only mutating action (End Task) uses the
existing `kill(2)`.

---

## Problem Frame

### What exists

- `src/commands/tasks/mod.rs` — kernel-side Task Manager: one
  `MultiColumnList` of **kernel threads only** (`process::get_process_list()`
  reads the kernel-thread `SCHEDULER`), 500 ms refresh via a spin/yield loop,
  right-click → Kill. It cannot see ring-3 processes at all — on today's OS,
  where the interesting workloads (zsh, BusyBox pipelines, notepad) are all
  ring 3, the current app shows the *least* interesting half of the system.
- The ring-3 GUI platform: syscalls 5001-5004, `libs/gui` (Canvas, Window,
  MenuBar, Button, TextField, ListView), `libs/dialogs` (FileDialog,
  MessageBox, ColorPicker), notepad as the migration reference
  (`docs/plans/2026-07-18-001` and `-002`).
- Kernel stats that exist today: kernel-thread `ProcessInfo` (name, state,
  `total_runtime` ticks, stack size, `cpu_percentage`), `mm::heap::stats()`
  (`HeapStats`), `FrameAllocator::stats()` (`FrameStats`), 100 Hz
  `get_timer_ticks()`, smoltcp device/interface state in `src/net/`.
- Precedent for kernel-synthesized namespaces: `/bin`
  (`src/userland/bin_namespace.rs` — stat/access/open/getdents64 all
  recognize it) and the kernel-managed `/etc` (DNS plan). An inline micro-proc
  already answers `readlink("/proc/self/exe")` and `/proc/self/fd/N`
  (`src/userland/syscalls.rs:3686`).

### What's missing

1. **Ring-3 processes are invisible.** `PROCESS_TABLE` has no CPU-time
   accounting (nothing increments a per-process tick counter) and no
   resident-page counter (RSS). Names exist only as the launch path
   (`/proc/self/exe` support); argv is built onto the user stack and not
   retained.
2. **No read path from ring 3 to kernel stats.** A ring-3 taskmgr can't call
   `process::get_process_list()`; it needs either new private syscalls or
   files.
3. **No graphing/table widgets in `libs/gui`.** The kernel-side
   `MultiColumnList` is ring-0 and dies with the old app.
4. **BusyBox `ps`/`top`/`free`/`uptime` are broken today** (no `/proc`) — a
   standing gap for anyone debugging in zsh.

### Why now, and why this shape

Every subsystem landed this quarter (network stack, multi-ring-3 scheduling,
sparse address spaces) increased the amount of invisible kernel state. As the
OS heads toward the Agentic runtime — many concurrent ring-3 processes doing
autonomous work — a live, trustworthy view of "what is running, what is it
costing" stops being a toy and becomes the primary development instrument.
Building it as *files + a userland app* rather than *syscalls + a kernel app*
means every future consumer (shell scripts, tests, agents introspecting their
own host) rides the same rails.

---

## Requirements

### R1 — Ring-3 process accounting (kernel)

- **R1.1 CPU ticks.** Add `utime_ticks: u64` to `Process`
  (`src/userland/lifecycle.rs`). The 100 Hz timer path that observes a
  CPL=3 interrupt charges one tick to `current_user_pid`. No user/system
  split in v1 (`stime` reported as 0).
- **R1.2 RSS.** Add a `resident_pages: usize` counter to `AddressSpace`,
  incremented wherever a user leaf becomes resident (VMA fault resolver,
  loader eager paths, stack growth, COW copy) and decremented on
  munmap/brk-shrink/teardown paths that free leaves. VSZ is derived from the
  existing VMA set (sum of `page_count`).
- **R1.3 Command name.** Retain the ring-3 process's argv[0] (basename) and a
  bounded copy of the full argv (cap: 256 bytes) on the `Process` at
  exec/launch time, for `/proc/<pid>/comm`-style naming and `cmdline`.
- **R1.4** All counters are maintained under the existing `PROCESS_TABLE`
  `InterruptMutex` discipline — no new locks, no counter updates that require
  taking the table from an ISR path that doesn't already hold it.

### R2 — Minimal synthetic `/proc` (kernel)

- **R2.1 Mechanism.** `/proc` is a kernel-synthesized read-only namespace in
  the style of `/bin`: `stat`, `access`, `open`, `read`, `getdents64`
  recognize it. File content is **generated once at `open()`** into a heap
  buffer owned by the fd (snapshot semantics — no lock held across user
  reads, consistent view per open).
- **R2.2 Linux-compatible files** (formats scoped to what BusyBox `ps`,
  `uptime`, and `free` actually parse — not full Linux fidelity):
  - `/proc/uptime` — `<seconds.centis> <idle-seconds.centis>` from
    `get_timer_ticks()`.
  - `/proc/meminfo` — `MemTotal`, `MemFree`, `MemAvailable` (from
    `FrameStats`), plus `KernelHeapTotal`/`KernelHeapUsed` extension lines
    (from `HeapStats`; harmless to Linux parsers).
  - `/proc/stat` — aggregate `cpu` line (user/system/idle jiffies) + `btime`
    + `processes`. User jiffies = Σ ring-3 utime; system = kernel-thread
    runtime; idle = remainder.
  - `/proc/<pid>/stat`, `/proc/<pid>/status`, `/proc/<pid>/cmdline`,
    `/proc/<pid>/statm` — one directory per **ring-3** PID, fields beyond
    (pid, comm, state, ppid, utime, vsize, rss) zero-filled.
  - `/proc/net/dev` — the standard two-header + one-interface-line format
    with RX/TX bytes and packet counts from the VirtIO-net driver
    (add counters in `src/net/` if not already tracked).
- **R2.3 AgenticOS extension files** under `/proc/agenticos/` (no Linux
  format constraints — line-oriented `key value` / TSV):
  - `kthreads` — kernel-thread table: tid, name, state, runtime ticks, stack
    size (the data the old tasks app showed).
  - `gui` — per-PID window count and event-queue depth (from
    `src/userland/gui.rs`) — debugging aid for queue-overflow issues.
  - `sockets` — socket registry summary (proto, state, local/remote) from
    `src/net/`'s bounded registry.
- **R2.4 Directory listing.** `getdents64("/proc")` enumerates the static
  files, `agenticos/`, `self`, and live ring-3 PIDs; `/proc/self` resolves to
  the calling process (the existing `readlink` micro-proc folds into this
  namespace rather than remaining a string special-case).
- **R2.5 Read-only.** `open` with write intent returns `-EACCES`. No
  `/proc/sys`, no writable knobs in v1.

### R3 — Toolkit widgets (`userland/libs/gui`)

- **R3.1 `TabBar`** — horizontal tab strip; `draw(canvas, active)`,
  `hit(x, y) -> Option<usize>`. Keyboard: Ctrl+Tab cycles (host-routed).
- **R3.2 `ColumnListView`** — multi-column upgrade in the `ListView` idiom:
  `Vec<Column { title, width, align }>`, rows as `Vec<Vec<String>>`,
  scrolling/selection/activation semantics identical to `ListView`,
  **click-on-header sorts** by that column (numeric-aware comparator flag per
  column, toggling asc/desc). Selection is tracked by a caller-supplied row
  key (the PID), not row index, so refreshes and re-sorts don't move the
  user's selection.
- **R3.3 `TimeSeriesGraph`** — fixed-capacity ring buffer (default 120
  samples) of `f32`; `push(sample)`; `draw(canvas, rect)` renders a filled
  area chart with a 1-px line, horizontal gridlines at 25/50/75 %, and a
  right-aligned current-value label. Two y-modes: fixed 0..100 (percent) and
  autoscaling max (throughput). Supports two series in one plot (RX/TX) with
  distinct colors.
- **R3.4** All widgets stay manually-positioned plain structs (no layout
  engine), matching `Button`/`ListView` precedent.

### R4 — The taskmgr application (`userland/apps/taskmgr/`)

- **R4.1 Shell.** ~640×480 default, resizable; `TabBar` with
  **Processes | Performance | Network**; a status strip at the bottom
  (process count, CPU %, mem %, uptime) visible on every tab — the modern
  Task Manager "footer" idiom.
- **R4.2 Event/refresh loop.** Drain GUI events with `GUI_NONBLOCK`, then
  `nanosleep` ~100 ms; re-sample `/proc` once per second (tick counter), push
  graph samples, re-render. No busy spinning — the process sleeps between
  frames. CPU % is computed app-side from consecutive utime snapshots
  (deleting the kernel's `update_cpu_percentages(50)` fixed-window hack).
- **R4.3 Processes tab.** `ColumnListView` with columns: PID, Name, State,
  CPU %, CPU Time, Mem (RSS), Threads-equivalent slot left out (no threads).
  Two sections in one list: ring-3 processes first, kernel threads below with
  bracketed names (`[compositor]`) sourced from `/proc/agenticos/kthreads`.
  Sort by any column; default CPU % descending.
- **R4.4 End Task.** A button (and Delete key) enabled only for ring-3 rows;
  confirmation via `dialogs::MessageBox::confirm`; on confirm, `kill(pid,
  SIGTERM)`, escalating offer to SIGKILL if the row is still present two
  refreshes later. **Kernel threads are view-only** — the old app's ability
  to kill arbitrary kernel threads (including, disastrously, the compositor)
  is deliberately dropped (KTD4).
- **R4.5 Performance tab.** Two large `TimeSeriesGraph` panels — CPU %
  (total, 0..100 fixed) and memory (used MiB, autoscale to MemTotal) — over a
  two-minute window, plus stat tiles: uptime, ring-3 process count, kernel
  thread count, kernel heap used/total, frames used/total.
- **R4.6 Network tab.** One dual-series throughput graph (RX/TX bytes/sec,
  autoscale) from `/proc/net/dev` deltas; totals line (bytes/packets since
  boot); socket list from `/proc/agenticos/sockets` in a `ColumnListView`.
- **R4.7 Visuals.** Client-area rendering is fully app-owned: flat panels,
  a light background consistent with notepad, one accent color for graphs
  and selection, and the bundled 8×8 bitmap font. Server-side chrome
  (Win98/Aero theme) frames it; the app does not attempt to match chrome
  pixel-for-pixel.

### R5 — Migration and integration

- **R5.1** Manifest row for `taskmgr` (built-every-run, Rust, staged as
  `TASKMGR.ELF`); workspace member; `build.rs` via
  `userland_build_support::configure`.
- **R5.2** `bin_namespace.rs`: remove `"tasks"` from `GUI_APPLETS`; add
  direct rewrites `tasks` and `taskmgr` → `/host/TASKMGR.ELF` (both names,
  so muscle memory and discoverability each work).
- **R5.3** Start menu (`src/commands/guishell/`): add a **Task Manager**
  entry launching via `terminal_factory::spawn_gui_user_app`, following the
  notepad arm.
- **R5.4** Delete `src/commands/tasks/`, its `gui_launch_table` arm, and the
  now-unused kernel-side pieces it uniquely consumed
  (`update_cpu_percentages` if no other caller remains).
- **R5.5** Docs: root `CLAUDE.md` current-state paragraph + known-issues,
  `src/commands/CLAUDE.md`, `src/userland/CLAUDE.md` (the `/proc`
  namespace), `userland/README.md` layout/`/bin` sections, this plan's
  status flip.

### R6 — Tests

- **R6.1** Kernel tests (`src/tests/`) for `/proc`: getdents64 enumeration,
  `uptime`/`meminfo`/`stat` parse-shape assertions, `/proc/<pid>/stat` for a
  synthetic process, snapshot-at-open semantics (content stable across a
  process exiting mid-read), unknown path → `-ENOENT`, write intent →
  `-EACCES`.
- **R6.2** Kernel tests for accounting: `utime_ticks` increments under a
  synthetic timer charge; `resident_pages` up on fault-in, down on munmap,
  zero after teardown.
- **R6.3** Manual smoke (userland has no automated harness): BusyBox `ps`,
  `uptime`, `free` in zsh; taskmgr full checklist (sort, kill flow both
  buttons, graphs advance, resize each tab, two instances side by side).

---

## Scope Boundaries

### Outside scope

- **New GUI syscalls or ABI changes.** 5001-5004 stay frozen; no event
  timestamps, no kernel timers for apps (nanosleep suffices).
- **BusyBox `top` full compatibility.** `top` wants `/proc` fields and
  terminal behaviors beyond v1; `ps`/`uptime`/`free` are the compat bar.
  `top` working partially is a bonus, not a requirement.
- **Killing kernel threads from ring 3** (KTD4).
- **Per-process network accounting** (needs per-socket-owner byte counters;
  the Network tab is system-wide in v1).
- **Writable `/proc`** (priorities, oom-style knobs) and `/sys`.
- **Historical/logging features** (App history tab) — nothing persists.

### Deferred to follow-up

- **Per-process CPU graphs / sparkline column** — cheap once
  `TimeSeriesGraph` exists; wants per-row state management in
  `ColumnListView` first.
- **`stime` (system-time) split** — charge syscall-path ticks separately.
- **GPU/compositor frame-time panel** — pairs with the retained GPU
  compositor plan (`2026-07-17-002`).
- **`/proc/agenticos/overlay`** (overlay dirty-page stats) — useful for the
  `sync` workflow; needs fs-side counters.
- **Migrating `calc`/`painting`/`explorer`** — separate plans; they inherit
  `TabBar`/`ColumnListView` from this one.

---

## High-Level Technical Design

### Data plane: why `/proc` files instead of private syscalls

| | Private syscall (e.g. 5005 snapshot) | Synthetic `/proc` |
|---|---|---|
| Consumers | taskmgr only | taskmgr, BusyBox ps/uptime/free, shell scripts, tests, future agents |
| Kernel precedent | gui_syscalls range | `/bin` synthesis, kernel-managed `/etc`, existing `/proc/self/*` readlink |
| Versioning | packed-struct ABI churn per field added | add a line/file; text is self-describing |
| Cost | one handler | path recognition + content generators |

Files win on every axis that matters for an OS whose stated direction is
agent-based computing: agents and scripts read files; nobody links against a
bespoke snapshot struct. The marginal cost over `/bin` (which already proves
the stat/open/getdents64 recognition pattern) is the content generators —
straight-line `format!` code.

**Snapshot-at-open** is the load-bearing concurrency decision: generating the
full file into a heap buffer while briefly holding the relevant lock
(`PROCESS_TABLE` / scheduler / net), then serving `read()` from that buffer,
means no kernel lock is ever held across a user copy or a yield, and a
process exiting mid-read can't corrupt a half-read table. This matches the
existing rule "do not hold process/network locks or user pointers across a
yield" (`src/userland/CLAUDE.md`).

### Accounting: where ticks and pages get counted

- **utime**: the U5 timer path already distinguishes "timer fired at CPL=3"
  and knows `current_user_pid` (it's what `try_preempt_ring3` reads). One
  `saturating_add(1)` on the current process under the already-taken
  `PROCESS_TABLE.try_lock()` — no new lock acquisition. If the try_lock
  misses (contended tick), the tick is dropped; sampling error at 100 Hz is
  acceptable for a monitor.
- **RSS**: `AddressSpace` is the single owner of every resident user leaf
  (fault resolver materializes; munmap/brk/teardown release), so a counter
  maintained at those choke points is exact, not sampled. The teardown path
  already walks and frees leaves — decrement rides along.

### App architecture

```
taskmgr/src/
  main.rs        — _start, window + TabBar + status strip, event/refresh loop
  sampler.rs     — reads/parses /proc files into a `Snapshot` struct;
                   keeps previous snapshot for rate derivation (CPU %, B/s)
  proc_tab.rs    — ColumnListView wiring, sort state, kill flow (+ MessageBox)
  perf_tab.rs    — graphs + stat tiles
  net_tab.rs     — throughput graph + socket list
```

The sampler is the only module that knows `/proc` formats; tabs consume the
typed `Snapshot`. Rates are computed from consecutive snapshots
(`Δutime / Δuptime`), which is both more honest and simpler than the old
kernel-side fixed-50-tick estimator.

Loop shape (no blocking `next_event` — the app must animate):

```
loop {
    while gui_next_event(&mut ev, GUI_NONBLOCK) == 0 { route(ev); }
    if now - last_sample >= 1s { snapshot = sample(); push_graphs(); dirty = true; }
    if dirty { render_active_tab(); present(); dirty = false; }
    nanosleep(100ms);
}
```

At ~10 wakeups/sec doing nothing but a nanosleep syscall when idle, taskmgr
itself stays honest on its own CPU column — a monitor that dominates its own
measurements is a classic failure mode this design avoids.

### The unified process list

Ring-3 processes (from `/proc/<pid>/…`) and kernel threads (from
`/proc/agenticos/kthreads`) are different beasts — different ID spaces,
different lifecycle, different kill semantics. The UI merges them into one
list (that's what "what is my OS doing?" means) but tags kernel threads with
bracketed names and disables actions on them. The two-source design keeps the
Linux-shaped files strictly Linux-shaped: BusyBox `ps` sees only real
processes, never a kernel thread masquerading with a fake PID.

---

## Implementation Units

### U1. Ring-3 accounting (kernel)
`utime_ticks` + timer charge; `resident_pages` counter through
fault/munmap/teardown choke points; retained argv[0]/cmdline (R1). Tests
R6.2. Verify: `./test.sh userland` green; counters visible in U2's files.

### U2. `/proc` namespace (kernel)
Path recognition (stat/access/open/read/getdents64), snapshot-at-open fd
backing, Linux-file generators, `agenticos/` extension files, `self`
resolution, NIC byte counters if missing (R2). Tests R6.1. Verify:
`cat /proc/uptime`, `ps`, `free`, `uptime` in zsh.

### U3. Widgets (`libs/gui`)
`TabBar`, `ColumnListView` (sort, key-stable selection), `TimeSeriesGraph`
(R3). Verify: `cargo build --release` in `userland/`; consumed by U4-U5
(guidemo hook optional but cheap for `TimeSeriesGraph`).

### U4. taskmgr app — shell + Processes tab
App scaffold, manifest row, sampler, event/refresh loop, Processes tab with
sort + End Task flow (R4.1-R4.4, R5.1). Verify: launch from zsh via
`/host/TASKMGR.ELF`; kill a `sleep 100` BusyBox child; confirm kernel-thread
rows are inert.

### U5. taskmgr app — Performance + Network tabs
Graphs, stat tiles, net throughput + sockets (R4.5-R4.7). Verify: `wget` a
file and watch RX spike; `yes > /dev/null`-equivalent loop and watch CPU.

### U6. Migration cutover
`/bin` rewrites, Start-menu entry, delete `src/commands/tasks/` + launch-table
arm + dead kernel helpers (R5.2-R5.4). Verify: `tasks` in zsh launches the
ELF; Start → Task Manager works; `./test.sh` full run green.

### U7. Docs
All CLAUDE.md / README touchpoints, plan status (R5.5). Verify:
`./build.sh -n` clean.

Sequencing: U1 → U2 → {U3 ∥ (nothing)} → U4 → U5 → U6 → U7. U3 can start in
parallel with U1/U2 (no kernel dependency). The old kernel app keeps working
until U6 flips atomically — same staging as the notepad migration.

---

## Key Technical Decisions

### KTD1. `/proc` files, not private syscalls
Triple payoff (taskmgr + BusyBox + scripts/agents), text self-versioning,
and namespace-synthesis precedent already proven twice. See the design
comparison table.

### KTD2. Snapshot-at-open content generation
No kernel lock held across user reads or yields; consistent per-open view;
trivially testable. Cost: a transient heap buffer per open — bounded (process
count is small; cap generators at a sane size).

### KTD3. Linux-shape for real processes, `agenticos/` files for everything else
BusyBox parsers must never trip over AgenticOS concepts (kernel threads,
GUI queues, frame allocator). Extensions live in their own directory with a
relaxed format; Linux files stay minimal-but-well-formed.

### KTD4. Kernel threads are view-only
The old app could kill the compositor from a context menu — a footgun, not a
feature. Ring-3 kill goes through the real `kill(2)`/signal path (TERM first,
KILL on escalation), which exercises the same machinery every other consumer
uses. If a kernel-thread kill switch is ever genuinely needed, it should be a
deliberate `/proc/agenticos` *write* interface designed with a denylist — out
of scope here.

### KTD5. App-side rate computation from consecutive snapshots
The kernel exports monotonic counters only (ticks, bytes, pages); every rate
(CPU %, B/s) is a userland delta. Kills the kernel's fixed-window
`update_cpu_percentages` estimator and matches how Linux tools work.

### KTD6. Poll-and-sleep loop instead of blocking `next_event`
A monitor must animate without input. `GUI_NONBLOCK` + `nanosleep(100ms)`
needs no ABI change (vs. adding an event-wait-with-timeout syscall) and keeps
idle cost at ~10 syscalls/sec. If a future app needs tighter event latency
while animating, a timeout flag on 5003 is a compatible extension — not
needed here.

### KTD7. Selection tracked by PID, not row index
Refresh-every-second plus user sorting means row indices are unstable;
key-based selection in `ColumnListView` prevents the classic "my selection
jumped to a different process right as I clicked End Task" hazard — which
would be a correctness bug in a kill UI, not just polish.

## Risks and Mitigations

### RK-1. BusyBox parser expectations
BusyBox `ps`/`free`/`uptime` are lenient but not infinitely so (field counts
in `/proc/<pid>/stat` matter). Mitigation: scope compat to those three
applets, zero-fill to the documented field counts, and smoke-test in-guest as
part of U2's verify — before the app is even started.

### RK-2. Lock discipline around content generation
`/proc` generators read `PROCESS_TABLE` (InterruptMutex) and scheduler/net
state. Mitigation: KTD2 confines each lock to a short, non-yielding,
non-allocating-while-held critical section (collect raw numbers, format after
release); the pattern is written once as a helper and reused by every
generator.

### RK-3. RSS counter drift
Missing one materialize/release path makes RSS silently wrong forever.
Mitigation: counter updates live inside the two-or-three central leaf
materialize/release helpers (not scattered at call sites), plus an R6.2
teardown-reaches-zero test and a debug assertion on AddressSpace drop.

### RK-4. Widget scope creep in `ColumnListView`
Column resize, per-cell colors, sparkline cells all beckon. Mitigation: pin
v1 to what R4.3/R4.6 consume (same demand-driven rule as the dialogs plan);
the deferred list explicitly parks per-row graphs.

### RK-5. taskmgr perturbing its own measurements
A 10 Hz repainting GUI over a full-surface-copy present is not free.
Mitigation: repaint only on dirty (sample tick or input), present only the
active tab's surface, and check taskmgr's own CPU column stays low
single-digit % in U4's verify.

## System-Wide Impact

- **`src/commands/` shrinks again** — three kernel GUI apps remain
  (painting, calc, explorer); each subsequent migration gets `TabBar` +
  `ColumnListView` + graphs for free.
- **`/proc` becomes load-bearing infrastructure** — BusyBox observability
  applets start working; kernel tests gain a user-visible introspection
  surface; the future Agentic runtime gets a self-inspection API without new
  ABI.
- **`PROCESS_TABLE`/`AddressSpace` gain small hot-path counters** — one add
  per timer tick, one add/sub per page materialize/release; negligible.
- **Userland toolkit** grows the three most-reusable remaining widgets.
- **A feature is deliberately dropped**: killing kernel threads (KTD4) —
  called out in the root `CLAUDE.md` update.

## Open Questions

- `/proc/<pid>` for **kernel threads too** (Linux shows kthreads as bracketed
  PIDs)? Planned no (KTD3 keeps ID spaces separate) — revisit if BusyBox `ps`
  output looking "empty" on an idle system reads as broken.
- Should `tasks` in zsh keep working as an alias, or force the new name?
  Planned: both `tasks` and `taskmgr` rewrite to `TASKMGR.ELF` (R5.2).
- Graph window length (120 samples @ 1 s = 2 min) vs. sample rate — tune in
  U5 by eye; ring capacity is a constant.
- Does the VirtIO-net driver already count bytes/packets, or does U2 add
  counters? (Scoping found interface state in `src/net/` but did not confirm
  counter granularity — resolve at U2 start.)
- Ctrl+Shift+Esc as a global taskmgr shortcut (the Windows reflex) — needs a
  compositor-side global hotkey concept that doesn't exist; parked, not
  designed here.

## Origin

Requested 2026-07-18: "Plan to pull taskmgr out as a userland elf similar to
how notepad works. Make it a much richer / full fledged app including
resource graphs etc. Brainstorm and think about how to make this useful as we
continue to develop this operating system but is also nice and modern."
Scoped against the completed ring-3 GUI platform (`2026-07-18-001`), dialogs
library (`2026-07-18-002`), and multi-ring-3 scheduling
(`2026-05-16-005`) plans. Exploration confirmed: the kernel tasks app sees
only kernel threads; ring-3 processes have no CPU/RSS accounting; a
readlink-only micro-`/proc` exists; `/bin` + `/etc` prove the synthetic
namespace pattern; `nanosleep` + `GUI_NONBLOCK` suffice for an animating app
with no ABI change.
