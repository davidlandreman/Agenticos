---
title: "perf: Frame allocator and page-fault hot path for multi-MiB binaries"
type: refactor
status: active
date: 2026-05-09
---

# perf: Frame allocator and page-fault hot path for multi-MiB binaries

## Summary

Replace the O(n²) frame-allocator iterator walk with an O(1) cursor, demote the chatty per-fault/per-allocation logging to trace level, and rewrite `File::read_to_vec` so the FAT/IDE copy is the only writer to each file-backing page. The combined effect makes a 5–6 MiB ET_EXEC binary loadable in seconds instead of stalling indefinitely.

---

## Problem Frame

In a prior debugging session, loading the 5.79 MiB `HELLOCPP.ELF` user binary stalled: after ~30 wall-clock minutes (12 min QEMU CPU), only ~27 of the ~1414 expected heap page faults for the file Vec had been serviced; QEMU went mostly idle and `read_to_vec` never returned. The kernel works fine for the 8 KiB Rust hello binary, so the symptom only emerges at multi-MiB scale. The cost compounds across three hot paths — frame allocation, page-fault logging, and pre-zeroing the read buffer — and shows up as a hang once the heap demand-paging count crosses ~hundreds of pages.

---

## Requirements

- R1. A 5–6 MiB ET_EXEC binary (e.g. `HELLOCPP.ELF`) loaded via `run /host/<NAME>.ELF` reaches the userland entry point in under 30 seconds wall-clock under QEMU.
- R2. The 8 KiB `HELLO.ELF` binary continues to load and run with no observable regression in correctness or perceived speed.
- R3. The kernel heap remains demand-paged via the existing page-fault handler — no eager pre-mapping of the 100 MiB heap region.
- R4. Hot-path code stays `no_std`-compliant per `.claude/rules/no-std.md` and `.claude/rules/panic-and-attributes.md` — no `std::*`, no panicking in interrupt context, no allocation in the page-fault handler.
- R5. Frame allocation is amortized O(1) per call; the `usable_frames().nth(self.next)` rebuild is gone.
- R6. Per-fault and per-allocation logging on the heap-fault hot path is off by default (trace level), with a low-frequency summary still visible at info level for operational visibility.
- R7. `File::read_to_vec` does not zero-fill its output buffer before the FAT/IDE copy.

---

## Scope Boundaries

- FAT cluster-walk caching — separate issue; the `src/fs/CLAUDE.md` "revisit if performance bites" note tracks it.
- VFS case-insensitivity / `/host` vs `/HOST` mount handling — separately planned fix.
- Switching IDE off PIO mode (DMA, IRQ-driven, virtio-blk) — out of scope.
- Pre-mapping the heap or replacing `linked_list_allocator` — out of scope.
- Per-process or NUMA-aware frame allocation — single-CPU kernel today; no value yet.

### Deferred to Follow-Up Work

- Investigation of the suspected page-fault → preemption stall (bottleneck (5) from the prior session): defer to a follow-up plan **iff** the verification in U5 shows the symptom persists after U1+U2+U3 land. The prior evidence is consistent with bottlenecks (1)–(4) being the entire cause, so this is gated on measurement, not pre-committed.

---

## Context & Research

### Relevant Code and Patterns

- `src/mm/frame_allocator.rs` — `BootInfoFrameAllocator`. Single struct, single `allocate_frame` impl, log call inside `usable_frames`. Owned by `MemoryMapper` constructed in `src/mm/paging.rs::MemoryMapper::new`.
- `src/mm/paging.rs` — `MemoryMapper::handle_page_fault` (heap demand-mapping) and `MemoryMapper::map_user_region` (loader path, doesn't go through fault handler).
- `src/arch/x86_64/interrupts.rs::page_fault_handler` — entry point, ring-3 short-circuit to `cleanup_user_process`, ring-0 dispatch into `handle_page_fault`.
- `src/fs/file_handle.rs::read_to_vec` (line 197) — current implementation does `Vec::with_capacity` + `Vec::resize(size, 0)` then `seek(0)` + `read(&mut buffer)`.
- `src/lib/debug.rs` — five-level macros (`debug_error!` … `debug_trace!`) with a runtime `DEBUG_LEVEL` set to `Debug` at boot in `src/kernel.rs:14`. Level `Trace` is below `Debug` so demoting hot-path logs to `debug_trace!` makes them silent in default builds without removing them.
- `src/tests/memory.rs` — existing in-kernel `Testable` test module for memory primitives; new frame-allocator tests slot in here.
- `src/tests/filesystem.rs` — existing in-kernel test module for the FAT stack; `read_to_vec` regression coverage slots in here.
- `src/tests/CLAUDE.md` — convention for adding a test (`Testable` impl, `get_tests()` registration).

### Institutional Learnings

- `docs/solutions/` is empty (only `.gitkeep`). No prior learnings to apply. Worth capturing the resulting fix via `/ce-compound` after landing — both the page-fault hot-path tuning and the `Vec::set_len` + uninit-read pattern are subtle and likely to recur.

### External References

- Rust standard library `Vec::spare_capacity_mut` and `Vec::set_len` semantics — the canonical safe-then-unsafe pattern for reading into uninitialized capacity. See the `Vec` documentation in `core` / `alloc`. The kernel uses `alloc::vec::Vec`, which exposes both APIs.

---

## Key Technical Decisions

- **Stateful frame allocator with a region cursor, not a precomputed Vec or a bitmap.** The heap is initialized after the frame allocator (`src/kernel.rs` boot order), so the allocator cannot allocate a `Vec` for itself. A bump cursor `(region_index, next_addr_in_region)` over `&'static MemoryRegions` filtered to `Usable` is enough: O(1) per allocation, zero per-call heap traffic, trivial to reason about.
- **Skip the zero frame at iteration time, not via post-filter.** Move the `addr != 0` check into the cursor's advance step so the per-call path doesn't need to materialize-then-discard. Behavior identical, slightly cleaner.
- **Demote hot-path logs, don't delete them.** `Usable region: …`, `Allocated frame at …`, `Page fault in heap region at …`, `Handling page fault for address: …`, and `Successfully mapped page … to frame …` move from `debug_debug!`/`debug_info!` to `debug_trace!`. The opening `>>> PAGE FAULT at …` line stays at `debug_info!` — it's the only line a debugger needs to see that a fault happened. A periodic `debug_info!` summary (every N=256 frame allocations, configurable constant) emits "frame allocator: N frames issued, region M, next 0x…" so a stuck system is still observable.
- **`read_to_vec` reads directly into uninitialized capacity.** Allocate with `Vec::with_capacity(size)`, take a `&mut [u8]` view of the spare capacity via `spare_capacity_mut().as_mut_ptr() as *mut u8` (length = `size`), call `read()` into that slice, then `set_len(bytes_read)`. Each page is touched exactly once — by the FAT layer's `copy_from_slice` into the cluster boundary — instead of twice (zero-fill, then overwrite). Halves the page-fault count for the read path and removes a 5.79 MiB memset.
- **Verification is end-to-end timing, not microbench.** The whole point is wall-clock to userland entry; an isolated frame-allocator microbench would prove the right thing for the wrong reason. U5 captures the timing in the same auto-run instrumentation pattern used in the prior debug session.
- **Defer bottleneck (5) (page-fault → preemption stall) until measurement justifies it.** Three independent O(n²)-ish costs were identified; the most parsimonious model is that fixing (1)–(4) eliminates the apparent stall. If U5 shows the load completes but is still much slower than expected, open a separate plan; if it completes in seconds, (5) was a downstream symptom and needs no further work.

---

## Open Questions

### Resolved During Planning

- **Should the frame allocator pre-build a `Vec<PhysFrame>` of all usable frames at init?** No — the boot order initializes the frame allocator before the heap (the heap allocator itself needs the frame allocator), so the allocator cannot use `alloc::*`. The cursor design avoids this dependency.
- **Should we also demote `Successfully mapped page …` from `handle_page_fault`?** Yes — it's part of the per-fault hot path. Demoted alongside the others in U2.
- **Does the existing `MemoryRegions` reference outlive the allocator?** Yes — `src/mm/memory.rs::STATIC_MEMORY_REGIONS` holds the `'static` reference; the allocator already takes `&'static MemoryRegions`.

### Deferred to Implementation

- Exact constant for the periodic frame-allocator summary cadence — start at `N = 256`, adjust if the resulting log rate is too sparse or too noisy under real workloads.
- Whether the frame-allocator state should expose a `frames_issued()` accessor for `kernel_state` MCP tool reporting — decide once U1 is in and the field is named.

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

**Frame allocator state machine (U1):**

```text
struct BootInfoFrameAllocator {
    memory_map: &'static MemoryRegions,
    region_idx: usize,        // index into memory_map of the current Usable region
    next_addr:  u64,           // next 4 KiB-aligned phys addr to hand out within that region
    frames_issued: u64,        // monotonically increasing; drives periodic summary logs
}

allocate_frame():
    loop:
        if region_idx >= len(memory_map): return None
        region = memory_map[region_idx]
        if region.kind != Usable:
            region_idx += 1; reset next_addr to next region.start (clamped to 4 KiB align); continue
        if next_addr >= region.end:
            region_idx += 1; reset next_addr to next region.start; continue
        if next_addr == 0:                  // skip the null frame
            next_addr += 4096; continue
        frame = PhysFrame::containing_address(next_addr)
        next_addr += 4096
        frames_issued += 1
        if frames_issued % 256 == 0:
            debug_info!("frame allocator: {} frames issued, region {}, next {:#x}",
                        frames_issued, region_idx, next_addr)
        return Some(frame)
```

**Per-page-fault log volume (U2):**

| Line                                                | Before    | After          |
|-----------------------------------------------------|-----------|----------------|
| `>>> PAGE FAULT at … error: …`                      | INFO      | INFO           |
| `Page fault in heap region at …`                    | INFO      | TRACE          |
| `Handling page fault for address: …`                | INFO      | TRACE          |
| `Usable region: 0x… - 0x…` (×2 from frame allocator)| DEBUG     | (gone — no per-call iterator) |
| `Allocated frame at PhysAddr(…)`                    | DEBUG     | TRACE          |
| `Successfully mapped page … to frame …`             | INFO      | TRACE          |

Net per heap fault drops from ~6 lines (~280 bytes UART) at default level to 1 line (~50 bytes), with a periodic summary every 256 allocations.

**`read_to_vec` shape (U3):**

```text
read_to_vec():
    let size = self.size()
    let mut buffer = Vec::with_capacity(size)
    seek(0)
    let dst: &mut [u8] = unsafe slice from buffer.as_mut_ptr() with length size  // uninit
    let n = self.read(dst)?     // FAT layer copies bytes into dst; pages fault once each
    unsafe { buffer.set_len(n) }
    Ok(buffer)
```

`File::read` already takes `&mut [u8]` and only writes into it (FAT's `copy_from_slice`); the uninit-then-write pattern is sound because the bytes are written before `set_len` exposes them as initialized. The `unsafe` block is two lines and lives behind the same `read_to_vec` API as today.

---

## Implementation Units

### U1. Stateful frame allocator with O(1) cursor

**Goal:** Replace the per-call `usable_frames().nth(self.next)` walk in `BootInfoFrameAllocator::allocate_frame` with a stateful cursor that advances one frame per call. Per-allocation cost becomes O(1) amortized; the `Usable region: …` log lines disappear from the per-call hot path entirely.

**Requirements:** R3, R5

**Dependencies:** None

**Files:**
- Modify: `src/mm/frame_allocator.rs`
- Test: `src/tests/memory.rs`

**Approach:**
- Carry `region_idx`, `next_addr`, `frames_issued` on the struct.
- Initialize `region_idx`/`next_addr` lazily on first `allocate_frame` (or eagerly in `init` — either works; lazy keeps `init` cheap).
- Skip zero frame in the cursor advance, not via post-filter.
- Emit the periodic `debug_info!` summary every `N = 256` allocations from `allocate_frame`.
- Preserve the existing `unsafe impl FrameAllocator<Size4KiB>` signature so `MemoryMapper` callers don't change.
- Remove `usable_frames()` entirely (it has no other caller after this unit).

**Patterns to follow:**
- Same crate-level imports (`bootloader_api::info::*`, `x86_64::structures::paging::*`).
- Same `unsafe { … }::init(memory_map)` constructor shape — single static instantiation in `MemoryMapper::new`.

**Test scenarios:**
- Happy path: first call returns the lowest non-zero usable frame; second call returns frame + 4 KiB. Asserts the cursor advances by exactly 0x1000 within a region.
- Edge case: cursor crosses a region boundary — exhaust region 0 (`0x0 – 0x9fc00`), next call returns the first frame of region 3 (`0x100000 – 0x1000000`), not a frame in the gap.
- Edge case: zero frame is never returned. Force the cursor to start at 0x0 (region 0 begins there) and assert the first `allocate_frame` returns the frame at 0x1000, not 0x0.
- Edge case: non-Usable regions (`Bootloader`, `UnknownBios`, `UnknownUefi`) are skipped — none of the addresses returned across N allocations fall inside a non-Usable region's range.
- Edge case: exhaustion — once every usable byte has been issued, `allocate_frame` returns `None` and continues to return `None` on subsequent calls (no underflow, no wrap).
- Integration: drive 4096 sequential `allocate_frame` calls and verify the returned frame addresses are strictly monotonically increasing, every address is 4 KiB-aligned, and none repeats.

**Verification:**
- `cargo build --release` succeeds.
- `./test.sh` passes the new frame-allocator tests alongside the existing memory tests.
- A single `allocate_frame` call performs no calls into `MemoryRegions::iter()` and no debug log output (verified by inspecting the diff and the new test fixtures' debug-output snapshot, not by counting at runtime).

---

### U2. Demote page-fault and frame-allocator hot-path logs

**Goal:** Move the per-fault and per-allocation log lines from `debug_info!`/`debug_debug!` to `debug_trace!` so they're silent at the default `Debug` level, and add a periodic summary to `BootInfoFrameAllocator` so a stuck system is still observable. Net per-heap-fault UART traffic drops from ~280 bytes to ~50 bytes.

**Requirements:** R6

**Dependencies:** U1 (the frame-allocator summary is emitted from the new cursor; demoting the orphaned `Usable region: …` log is moot once U1 lands)

**Files:**
- Modify: `src/mm/frame_allocator.rs` (already touched by U1; finalize log levels here)
- Modify: `src/mm/paging.rs` (`handle_page_fault`)
- Modify: `src/arch/x86_64/interrupts.rs` (`page_fault_handler`)

**Approach:**
- Per the table in High-Level Technical Design, demote: `Page fault in heap region at …`, `Handling page fault for address: …`, `Allocated frame at …`, `Successfully mapped page … to frame …` to `debug_trace!`.
- Keep the opening `>>> PAGE FAULT at …, error: …` at `debug_info!` so any unexpected fault is still visible at default level.
- Keep the ring-3 `EXCEPTION: PAGE FAULT (ring 3)` and the ring-0 unhandled-fault `debug_error!` paths untouched — those are real failures, not hot-path noise.
- Keep all `debug_error!` panic-precursors as is.
- The U1 periodic summary line is the only added log surface; emitted from the allocator at info level.

**Patterns to follow:**
- Existing `debug_trace!` macro definition in `src/lib/debug.rs:62-69`.
- Existing convention from `c439f61 chore(process): demote per-switch try_run logs to trace` — same demotion pattern, same justification.

**Test scenarios:**
- Test expectation: none — no behavioral change. Coverage is the existing memory and userland test suites continuing to pass with the demoted logs invisible at default level. Manual verification by running `./build.sh` and confirming the `Page fault in heap region` lines are absent from serial output during normal heap demand-paging.

**Verification:**
- `cargo build --release` succeeds.
- `./test.sh` passes (no test should be parsing the demoted log lines).
- An interactive `./build.sh` run boots cleanly; serial log no longer carries a `Page fault in heap region`/`Allocated frame`/`Successfully mapped page` line per heap fault. The periodic frame-allocator summary appears every N=256 allocations.

---

### U3. `File::read_to_vec` reads into uninitialized capacity

**Goal:** Drop the `buffer.resize(size, 0)` zero-fill from `File::read_to_vec` and read the FAT bytes directly into uninitialized `Vec` capacity. Halves the page-fault count for large file reads (one fault per page instead of two — the zero-fill and the read overwrite collapse into a single touch).

**Requirements:** R3, R7 — and contributes to R1 by removing 5.79 MiB of redundant memset on the C++ binary load path.

**Dependencies:** None (independent of U1/U2; can land in any order)

**Files:**
- Modify: `src/fs/file_handle.rs` (`read_to_vec`, around line 197)
- Test: `src/tests/filesystem.rs`

**Approach:**
- Allocate `Vec::with_capacity(size)`.
- Construct a `&mut [u8]` view of length `size` over `buffer.as_mut_ptr()` via `core::slice::from_raw_parts_mut` inside an `unsafe` block.
- `seek(0)` then `self.read(dst)?` — the FAT layer's `copy_from_slice` is the sole writer, so no byte is read before being initialized.
- `unsafe { buffer.set_len(bytes_read) }` to expose only the bytes actually written.
- Add a SAFETY comment explaining the invariant: `read()` is the sole writer, and `set_len` is bounded by the returned `bytes_read` (which is in turn bounded by `size`).
- Empty-file path: when `size == 0`, return `Vec::new()` directly to avoid a zero-length unsafe slice construction.

**Patterns to follow:**
- The kernel already uses raw-pointer writes through `core::ptr::write_bytes` and `copy_nonoverlapping` in `src/userland/loader.rs::copy_segment_into_user_va` — same `unsafe` discipline.
- `alloc::vec::Vec::set_len` semantics are identical to std's; `no_std` does not change anything here.

**Test scenarios:**
- Happy path: reading `/HELLO.TXT` (existing fixture) returns the same bytes as before. Compare output byte-for-byte against a literal expected value.
- Edge case: a zero-byte file returns `Vec::new()` with length 0 and capacity 0 (or 0 — implementation-defined; the assertion is `len() == 0`).
- Edge case: short read (FAT returns fewer bytes than `file.size()`) leaves the Vec at length `bytes_read`, with the trailing capacity neither initialized nor exposed. Assert `buffer.len() == bytes_read` and that reading past `len()` would be UB (i.e., the test must not index past `len()`).
- Integration: `RunProcess::new_with_args(["/host/HELLO.ELF"]).run()` end-to-end still succeeds and the userland app prints "Hello" — verifies that nothing downstream relied on the buffer being zeroed past the file's end.

**Verification:**
- `cargo build --release` succeeds.
- `./test.sh` passes the existing FAT tests plus the new `read_to_vec` coverage.
- An interactive `./build.sh` run loads `/host/HELLO.ELF` end-to-end without regression — same observable behavior as before this unit lands.

---

### U4. Heap demand-page log demotion regression guard

**Goal:** Lock in the U2 expectation with a regression guard: the kernel test suite's existing memory tests run without producing per-fault `[INFO]`/`[DEBUG]` chatter for routine heap allocation. Catches an accidental level re-promotion in a future change.

**Requirements:** R6

**Dependencies:** U2

**Files:**
- Modify: `src/tests/memory.rs`

**Approach:**
- Add a small `Testable` that allocates ~1 MiB of heap (256 pages) and asserts via the kernel's existing log-level read (`debug::get_debug_level()`) that the level is `Debug` (not `Trace`). This makes the test environment match the default boot level.
- The test's value is documentary — it does not snapshot serial output (the test framework doesn't capture it), but the surrounding allocation forces the page-fault path to run, and a developer rerunning `./test.sh` after a level-promotion regression will see the expected silence by inspection. Pair with a code comment in `src/mm/paging.rs::handle_page_fault` and `src/arch/x86_64/interrupts.rs::page_fault_handler` pointing back to U2 so the level choice is discoverable from the call site.

**Test scenarios:**
- Happy path: allocate `Vec<u8>` of ~1 MiB, write a sentinel byte at offset 0 and `size-1`, drop. Assert the writes succeed and no panic occurs.
- Edge case: `debug::get_debug_level()` returns `DebugLevel::Debug` (the default), confirming the test environment matches the boot environment U2 was tuned for.

**Verification:**
- `./test.sh` passes including the new test.
- The companion code comments in `paging.rs` and `interrupts.rs` reference U2 so the trace-level choice is not orphan knowledge.

---

### U5. End-to-end load-time verification for HELLOCPP.ELF

**Goal:** Verify R1 and the deferred-bottleneck-(5) hypothesis by measuring how long `run /host/HELLOCPP.ELF` takes from `RunProcess::run` entry to the userland's first `write` syscall. Captures the result so the deferred-follow-up decision is evidence-based rather than speculative.

**Requirements:** R1, R2

**Dependencies:** U1, U2, U3

**Files:**
- Modify (temporarily, then revert before final commit): `src/kernel.rs` to add an autorun spawn after boot that runs `RunProcess` against `/host/HELLOCPP.ELF` and logs a single `debug_info!` line with elapsed timer ticks.
- The instrumentation is a *measurement scaffold* and is not committed — it is the same shape used in the prior debug session (kernel.rs:467-474 area).

**Approach:**
- Stage `host_share/HELLOCPP.ELF` per the existing `build.sh` flow.
- Boot under QEMU with the autorun instrumentation, capture the elapsed-tick log, divide by 100 (PIT tick rate) to get seconds.
- Decision tree:
  - **Elapsed < 30 s and userland prints "Hello":** R1 met. Bottleneck (5) was a downstream symptom; close out without a follow-up plan. Revert the autorun instrumentation; do not commit it.
  - **Elapsed < 30 s but userland faults / hangs:** loader or userland regression introduced by one of U1/U2/U3. Open a focused fix plan against whichever unit's diff is the most likely cause; do not ship U1–U4 until the regression is understood.
  - **Elapsed ≥ 30 s but progressing:** load completes but is still slow. Profile the dominant remaining cost (likely FAT cluster walk per `src/fs/CLAUDE.md`) and route to a follow-up plan; this plan ships as a partial improvement only after sign-off.
  - **No completion (true hang):** bottleneck (5) is real and independent. Open a separate plan targeted at the page-fault/preemption interaction; this plan does not ship until that one lands.
- Also rerun the small-binary path (`/host/HELLO.ELF`) to confirm R2 — no regression.

**Test scenarios:**
- Happy path (manual): `HELLOCPP.ELF` loads and the terminal shows "Hello from C++!" (or whatever the binary's `main` writes) within 30 s.
- Happy path (manual): `HELLO.ELF` loads and the terminal shows the Rust binary's output with no observable slowdown vs. baseline.
- Test expectation: no automated test — this is a wall-clock measurement that requires QEMU and a multi-MiB binary, neither of which fits the in-kernel `Testable` framework. Capture the measurement in the PR description and in `docs/solutions/` (per the institutional-learnings recommendation) for future regressions.

**Verification:**
- The autorun-instrumented kernel produces an elapsed-tick log line for `HELLOCPP.ELF` and the wall-clock-equivalent is ≤ 30 s.
- The autorun-instrumented kernel also runs `HELLO.ELF` cleanly with comparable timing to baseline (within an order of magnitude — this is a sanity check, not a benchmark).
- The autorun instrumentation is removed from `src/kernel.rs` before the unit's final commit; `git diff` against the parent commit shows no `autorun` debug spawn or measurement scaffolding remaining.
- The measurement (elapsed seconds for `HELLOCPP.ELF`, plus the disposition per the decision tree) is recorded in the PR description and in `docs/solutions/` so future regressions are detectable against a baseline.

---

## System-Wide Impact

- **Interaction graph:** The page-fault handler is on every heap demand-page path and every process-stack demand-page path (both go through `handle_page_fault`). The frame allocator is on every heap fault, every user-region map (loader's `map_user_region`), and every kernel-stack pre-fault. Both are universal hot paths.
- **Error propagation:** No change. `allocate_frame` still returns `Option<PhysFrame>`; `handle_page_fault` still returns `Result<(), MapToError>`; `read_to_vec` still returns `FileResult<Vec<u8>>`.
- **State lifecycle risks:** `Vec::set_len` is unsafe and can expose uninitialized memory if `bytes_read > capacity`. The bound is enforced by `read()` returning at most the slice length, but the SAFETY comment must spell this out and `bytes_read` should be sanity-asserted in debug builds (`debug_assert!(bytes_read <= size)`). The `read()` contract is documented but not type-enforced.
- **API surface parity:** None. Public APIs of `File`, `MemoryMapper`, and `BootInfoFrameAllocator` are unchanged.
- **Integration coverage:** The `RunProcess` end-to-end path exercised in U5 is the only integration that proves U1+U3 don't subtly corrupt loader state — unit tests can't catch a Vec whose set_len exposed garbage to the ELF parser.
- **Unchanged invariants:** Heap remains demand-paged. Frame 0 is never handed out. The frame allocator's iteration order matches today's (lowest usable region first, then ascending) — anything that depended on observed physical addresses (nothing should) continues to see the same sequence.

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| `Vec::set_len` exposes uninitialized memory if `read()` returns more bytes than the slice length. | Pre-condition: the slice is constructed with `len = size`; `read()` is contractually bounded by the slice. Add `debug_assert!(bytes_read <= size)` and a SAFETY comment. The kernel is already trusting this same contract everywhere `read(&mut buf)` is called today. |
| Demoting the per-fault logs hides a real fault when a page-fault storm happens during a regression. | The opening `>>> PAGE FAULT at …` line stays at info, so a fault storm is still visible — only the routine 5-line follow-up is silenced. The U1 periodic frame-allocator summary further bounds blind spots. |
| The frame allocator cursor reads `MemoryRegions` directly; if a future refactor changes the layout (sorted/unsorted, gaps, overlapping), monotonic-frame ordering could break. | Keep the assumption explicit in the cursor's comment ("regions are visited in `MemoryRegions::iter()` order; within a region, frames are issued in ascending physical order"). The U1 monotonic-ordering test catches a regression on the next `./test.sh`. |
| `read_to_vec` on a zero-byte file with `Vec::with_capacity(0)` then `from_raw_parts_mut(ptr, 0)` is technically sound (zero-length slices have no requirements on the pointer) but linters may flag it. | Special-case `size == 0` to return `Vec::new()` early — the unsafe slice construction never runs in the trivial case. |
| Bottleneck (5) turns out to be real and independent. | U5's decision tree explicitly handles this: a follow-up plan is opened, this plan ships as a partial improvement only after sign-off. |

---

## Documentation / Operational Notes

- After U5 ships green, capture the load-time measurement and the page-fault hot-path tuning lessons in `docs/solutions/` per the `ce-learnings-researcher` recommendation. The `Vec::set_len` + uninitialized-read pattern in particular is subtle and likely to recur in any future "read large blob into Vec" path.
- Update `src/mm/CLAUDE.md` if the frame allocator's invariants ("regions visited in MemoryRegions iter order; frames ascending within a region") are not already stated — they should be once the cursor design lands, since they're now load-bearing for ordering tests.

---

## Sources & References

- Prior debug session in this workspace: identified bottlenecks (1)–(5), measured the 30-min stall on `HELLOCPP.ELF`, confirmed the path-case mismatch as the original user-visible failure (separately tracked).
- Related code: `src/mm/frame_allocator.rs`, `src/mm/paging.rs`, `src/arch/x86_64/interrupts.rs`, `src/fs/file_handle.rs`, `src/lib/debug.rs`.
- Related convention: commit `c439f61 chore(process): demote per-switch try_run logs to trace` — the precedent for hot-path log demotion in this kernel.
