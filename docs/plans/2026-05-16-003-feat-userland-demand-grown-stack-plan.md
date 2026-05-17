---
title: "feat: Demand-grown user stack"
status: active
created: 2026-05-16
plan_type: feat
depth: standard
related_docs:
  - CLAUDE.md (Deferred from zsh-interactive bring-up, item #1)
  - docs/plans/2026-05-08-004-feat-userland-app-platform-plan.md
  - docs/solutions/learnings/2026-05-09-multi-mib-user-binary-load.md
---

# feat: Demand-grown user stack

## Summary

Replace the user stack's eager 64-page (256 KiB) mapping with a small initial commit plus fault-driven growth in the ring-3 page-fault handler, capped by a per-process growth limit. Stack-growth metadata lives on `Process`; the fault handler checks whether the faulting address is a stack-grow candidate before routing to `cleanup_user_process`.

---

## Problem Frame

`src/userland/loader.rs` currently maps `USER_STACK_PAGES = 64` (256 KiB) eagerly for every process. The bump from 8 → 64 was made because zsh's post-fork prep blew past 32 KiB and got terminated by the page-fault handler's blanket "ring-3 fault → cleanup" policy. Any binary that needs more than 256 KiB of stack hits the same wall — its first stack-overflow-style access faults at an unmapped address below `stack_bottom`, the handler routes it to `cleanup_user_process`, and the process dies with `SIGSEGV`. The CLAUDE.md "Deferred from zsh-interactive bring-up" item #1 names the proper fix verbatim: grow the stack on faults below the current bottom up to a `RLIMIT_STACK`-style cap, mirroring Linux. Until that's in place, commands that consume more stack than the static cap silently die instead of returning.

The user-VA partition adds a tight constraint: `USER_LOAD_BASE = 0x40_0000` to `USER_STACK_TOP = 0x80_0000` is only 4 MiB (1024 pages), shared between the binary's text/data/bss and the stack. The growth cap can't be Linux's 8 MiB without first repartitioning the user VA layout — that repartition is explicitly out of scope here.

---

## Goals

- A ring-3 fault below the current mapped stack bottom, but above the growth limit and above the highest PT_LOAD segment, grows the stack by one page and resumes the user instruction.
- A ring-3 fault below the growth limit (true overflow) still terminates the process via the existing `cleanup_user_process` path with `SIGSEGV`.
- Fresh processes commit only a small initial stack (a few pages); growth fills in the rest on demand.
- Fork-spawned children inherit the parent's stack window (current bottom + growth limit) and can grow further independently.
- No regression in zsh interactive boot or any existing userland test.

## Non-Goals

- Real `getrlimit`/`setrlimit` syscall plumbing — the cap is a kernel constant for this plan.
- Repartitioning the user VA layout to give the stack 8 MiB or more (would require moving `USER_TLS_IMAGE_VA`, `USER_BRK_BASE`, `USER_MMAP_BASE` in `src/mm/paging.rs`).
- Frame reclamation on stack shrink. The bump-only `BootInfoFrameAllocator` can't return frames anyway, and stacks don't shrink in this plan.
- Heap-side fault handling — already works (`MemoryMapper::handle_page_fault` for the kernel heap region).
- Items #2 (signal-mask restore on `rt_sigreturn`) and #3 (`WIFSIGNALED` encoding) from the same CLAUDE.md "Deferred" list.

---

## Requirements

- **R0.** Before claiming the original symptom ("commands aren't coming back") is fixed, capture CR2 and faulting RIP for one previously-hanging command and confirm the fault address sits below the current `stack_bottom`. If the address is elsewhere (e.g., a heap dereference, a kernel-side wedge, or a non-fault hang), the cause is not stack-related and this plan does not address it — investigate items #2/#3 from CLAUDE.md or escalate.
- **R1.** Ring-3 page-fault handler distinguishes stack-grow candidates from true faults using per-process stack-window metadata.
- **R2.** Stack-window metadata (`stack_top`, current `stack_bottom`, `stack_mapped_bottom`, `stack_max_growth_floor`, `growth_faults_remaining`) lives on `Process` only, is populated by the loader, and survives fork by copying all five fields onto the child `Process` constructed in `fork_handler` (the existing eager PML4[0] copy already carries the actual stack pages).
- **R3.** Initial mapped stack commit is a small constant (default: 8 pages / 32 KiB), restored from the pre-zsh value.
- **R4.** Growth path uses `MemoryMapper::map_user_region` with `UserPerms::ReadWrite`, consistent with the loader's stack mapping.
- **R5.** Growth-limit floor is `max(USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES * 0x1000, highest_pt_load_end + 16 * 0x1000)` (16-page guard), computed per-process at load time and stored on `Process`.
- **R6.** Stack-overflow faults below the growth limit, lock-contended growth attempts, and frame-allocator failures all route through `cleanup_user_process` with vector 14 / SIGSEGV — the existing cleanup path is the single termination route.
- **R7.** `validate_user_slice`'s bounds are updated on each successful growth via `set_user_va_bounds` so the validated range tracks what's actually mapped. The bounds never accept pointers into pages that aren't currently backed.
- **R8.** Growth path holds no growable per-fault heap state — no `Vec::push` on every fault. The single `Vec<PhysFrame>` allocation inside `map_user_region` is bounded (capacity 1 per growth call) and is acceptable because the kernel heap auto-maps at any CPL via `MemoryMapper::handle_page_fault`.
- **R9.** Per-fault growth logs stay at `debug_trace!` level (hot-path discipline from `src/mm/CLAUDE.md`); an info-level summary may emit every N growths.
- **R10.** Grown stack frames are unmapped at process teardown via a new `unmap_user_stack(process)` helper called from `cleanup_user_process` and `cooperative_exit` — not via `UserImage::Drop`, which doesn't have visibility into Process state.
- **R11.** A per-process growth-fault counter caps the number of successful growths to `USER_STACK_MAX_GROWTH_PAGES`. Once reached, all further growth-window faults route to cleanup — guards against frame exhaustion via a fault-storm attack pattern (bump allocator never reclaims).
- **R12.** Fault handler acquires `CURRENT_PROCESS` via `try_lock` only; lock contention returns `GrowOutcome::LockContended` which routes through cleanup (defensive — no other path should hold the mutex when a stack-grow fault fires, but a contention here would otherwise deadlock).
- **R13.** Initial constants `USER_STACK_INITIAL_PAGES` and `USER_STACK_MAX_GROWTH_PAGES` are validated against measured stack peaks for zsh interactive, `BB.ELF find /`, and `BB.ELF awk` deep recursion before merge (U2 verification step).

---

## Key Technical Decisions

**All per-process stack metadata lives on `Process`, including `stack_mapped_bottom`.** Rationale: the fault handler runs in interrupt context and needs to mutate `stack_bottom` and `stack_mapped_bottom` per growth. `Process` is the interrupt-context entry point. Putting `stack_mapped_bottom` on `UserImage` (an earlier draft of this plan) breaks fork: `fork_handler` constructs the child `Process` with `image: None` (see `src/userland/syscalls.rs::fork_handler`), so the child's fault handler would have nowhere to record growth. Teardown unmaps `[stack_mapped_bottom, USER_STACK_TOP)` from a helper that reads Process state (called from `cleanup_user_process` and `cooperative_exit`), not from `UserImage::Drop`. `UserImage::Drop` is unchanged — it still walks `mappings: Vec<MappingRange>` for PT_LOAD / TLS regions, and the stack mapping is **not** recorded in that list (see U1).

**`Process` carries `stack_mapped_bottom` directly — no per-page heap allocation in the growth path.** Rationale: `Vec::push` from interrupt context can fault on the kernel heap, and a page fault inside a page fault is a hardware double fault. A single mutable `u64` on Process avoids that. The chosen API for the per-page map call (`map_user_region`) does allocate a small `Vec<PhysFrame>` internally (`src/mm/paging.rs:286`); this is acknowledged in U4 as a bounded amortized allocation whose fallback is kernel heap auto-mapping at any CPL, but R8 below is scoped accordingly (no growable per-fault state) rather than claiming zero allocation.

**Fault handler uses `try_lock` on `CURRENT_PROCESS`, not blocking `lock`.** Rationale: `src/userland/lifecycle.rs:84-88` explicitly documents the try-lock contract for interrupt context — blocking would deadlock on a single core if any other path holds the mutex when the fault fires. `try_grow_user_stack` does `CURRENT_PROCESS.try_lock()`; on `None` it returns `GrowOutcome::LockContended`, the handler falls through to `cleanup_user_process`. Lock contention is treated as overflow (defensive; if the kernel is mid-syscall holding the Process mutex when a stack-grow fault fires, that's a real bug, but the fallthrough keeps the system halt-clean rather than wedged).

**Growth-limit floor = `max(global cap, per-binary code-end + 16-page guard)`.** Rationale: a global `USER_STACK_MAX_GROWTH_PAGES` constant caps how deep the stack can grow. A per-binary floor prevents the stack from clobbering the binary even when the global cap would allow it. The 16-page (64 KiB) guard between code-end and the deepest possible stack page is larger than the bare-minimum 1-page guard — Linux post-Stack-Clash (CVE-2017-1000364) defaults to a 1 MiB `STACK_GUARD_GAP`; 64 KiB is the minimum-viable bar for a kernel that may later host untrusted binaries. Document as "minimum viable; revisit when untrusted code lands."

**Initial values for the global constants are measurement-driven, not picked from memory.** `USER_STACK_INITIAL_PAGES` and `USER_STACK_MAX_GROWTH_PAGES` are tuned by U2's verification, not assumed. The plan proposes starting points (8 / 768 pages) but requires U2's verification to either confirm them by measuring stack-peak on the known workloads (zsh interactive startup, `BB.ELF find /`, `BB.ELF awk` deep recursion) or adjust before merging. The 3 MiB cap claim is unverified at plan time.

**Single termination path for stack overflow.** When the fault is below the growth limit (or the lock is contended), the handler falls through to the existing `cleanup_user_process(AbnormalExit { vector: 14, .. })` call — no parallel cleanup route. This preserves the SS-restore invariant guarded by `test_kernel_ss_after_user_fault` (see post-mortem).

**`bounds_start` is updated on each growth, not pre-expanded at load time.** Rationale: an earlier draft of this plan pre-expanded `bounds_start` to the growth floor so syscall pointer validation would accept deep-stack pointers up front. That accepts pointers into pages that have not been mapped — a syscall reading or writing through such a pointer would fault in **kernel mode**, where the existing fault path does not call `try_grow_user_stack` (the call lives inside the ring-3 branch). The fault would re-enter the mapper while it holds its own lock, producing a deadlock or panic. To avoid that, `bounds_start` starts at the initial commit bottom; every successful `GrowOutcome::Grew` re-calls `set_user_va_bounds` to widen the bounds by the freshly mapped page. This costs one extra MSR-class write per growth and keeps user pointer validation honest about what's mapped.

**Initial commit stays at 8 pages.** Rationale: matches the pre-zsh default. The original 32 KiB was enough for everything except zsh's post-fork prep, which is exactly the case the growth path now handles. zsh's post-fork path will take a handful of growth faults; U4's per-binary timing assertion catches a regression if the fault path is slower than expected.

---

## High-Level Technical Design

*Directional sketch — not implementation specification. The implementer should treat this as context, not code to reproduce.*

Decision flow inside `page_fault_handler` when `frame_is_user(CS)` is true:

```
ring-3 fault at addr (CR2)
  └─ try_grow_user_stack(addr):
       ├─ with_current_process |p|:
       │    addr < p.stack_bottom          ── no  → GrowOutcome::NotStackGrow
       │    addr >= p.stack_max_growth_floor ── no → GrowOutcome::Overflow
       │    map_user_region(page_of(addr), 1, RW) ── err → GrowOutcome::MapFailed
       │    p.image.stack_mapped_bottom = page_of(addr)
       │    trace!("stack grew to {:#x}", page_of(addr))
       │    return GrowOutcome::Grew
  ├─ Grew      → return (CPU retries instruction)
  ├─ NotStackGrow / Overflow / MapFailed → cleanup_user_process(vector=14, ...)
```

Stack-window state transitions on `Process`:

```
install_new_process_opt(image, ...)
  │
  ├─ stack_top              = USER_STACK_TOP                            (constant)
  ├─ stack_bottom           = USER_STACK_TOP - INITIAL_PAGES * 0x1000   (mutable)
  └─ stack_max_growth_floor = max(USER_STACK_TOP - MAX_PAGES * 0x1000,
                                  highest_pt_load_end + 0x1000)         (constant)

stack-grow fault at addr
  └─ stack_bottom ← page_of(addr)                                       (monotonic decrease)

cleanup_user_process / cooperative_exit
  └─ UserImage::Drop unmaps [stack_mapped_bottom, USER_STACK_TOP)       (single range)

fork (clone_for_child + Process clone)
  └─ child inherits (stack_top, stack_bottom, stack_max_growth_floor)   (verbatim)
     parent retains same fields in PARENT_STASH                         (unchanged)
```

---

## Scope Boundaries

In scope:
- Stack-window metadata on `Process` and `UserImage`.
- Loader changes: initial commit shrink, growth-limit floor computation, bounds-start expansion, parse_pt_load spill check updated.
- New ring-3 page-fault branch in `src/arch/x86_64/interrupts.rs` calling a new `try_grow_user_stack` helper in `src/userland/lifecycle.rs`.
- Fork plumbing: stack metadata copied to the child Process; parent stash retains its copy.
- Tests: stack-grow happy path, overflow guard, post-fork growth, and bounds-start regression.
- CLAUDE.md cleanup: mark deferred item #1 as resolved.

Out of scope (true non-goals):
- User VA layout repartition.
- `getrlimit`/`setrlimit` syscalls.
- Frame reclamation / stack shrink.
- Items #2 and #3 of the "Deferred" list.

### Deferred to Follow-Up Work

- Per-process configurable growth limit driven by future `setrlimit(RLIMIT_STACK, ...)` — when that syscall lands, replace the kernel constant in the floor computation with the Process's recorded rlimit.
- Repartitioning the user VA layout to give the stack a real 8 MiB cap — separate PR, will touch `USER_TLS_IMAGE_VA`/`USER_BRK_BASE`/`USER_MMAP_BASE`.
- Info-level summary cadence for stack-growth — start at trace-only; add the every-N summary if a future load reveals a need for observability.

---

## Output Structure

This plan modifies existing files only; no new directories or files are introduced. New tests are appended to `src/tests/userland.rs` and (if needed) `src/tests/userland_fixtures.rs`.

---

## Implementation Units

### U1. Stack-window fields on `Process` + teardown helper

**Goal:** Add per-process stack-window metadata to `Process` (only — not `UserImage`) and a teardown helper that unmaps the grown stack range. No heap allocation in the fault path.

**Requirements:** R2, R8, R10, R12.

**Dependencies:** none.

**Files:**
- `src/userland/lifecycle.rs` (extend `Process` struct, static initializer, add `unmap_user_stack` helper)
- `src/tests/userland.rs` (extend existing process-install tests)

**Approach:**
- On `Process`: add five fields, all `u64`:
  - `stack_top` (constant per-process; equals `USER_STACK_TOP` for now)
  - `stack_bottom` (mutable; the lowest committed page from the loader's initial commit, lowered on each growth)
  - `stack_mapped_bottom` (mutable; the lowest currently-mapped page — used by teardown to compute the unmap range)
  - `stack_max_growth_floor` (constant per-process; lowest page the stack may ever reach)
  - `growth_faults_remaining` (mutable; initialized to `USER_STACK_MAX_GROWTH_PAGES`, decremented on each successful growth)
- Initialize all five to `0` in the static `CURRENT_PROCESS` initializer (kernel-sentinel slot — never reached by the fault handler's ring-3 branch).
- Provide `Process::set_stack_window(top, bottom, mapped_bottom, max_growth_floor, growth_faults_remaining)` used by U3 to install metadata from the loader.
- Add `pub fn unmap_user_stack(process: &mut Process)` that calls `with_memory_mapper(|m| m.unmap_user_region(VirtAddr::new(p.stack_mapped_bottom), (p.stack_top - p.stack_mapped_bottom) / 0x1000))` and zeroes the four mutable fields. Called from `cleanup_user_process` and `cooperative_exit` (U4 + U6 wire this in).
- **Do not** change `UserImage` — the stack is no longer recorded in `mappings` (U2 removes the existing `record_mapping` call for the stack) and `UserImage::Drop` continues to handle PT_LOAD/TLS regions only.

**Patterns to follow:**
- Field ordering and naming style of existing `Process` fields (see `brk_current`, `mmap_next`).

**Test scenarios:**
- After `install_new_process_opt` for a synthesized image, `with_current_process(|p| ...)` reports `stack_top == USER_STACK_TOP`, `stack_bottom == stack_mapped_bottom == USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000`, a non-zero `stack_max_growth_floor`, and `growth_faults_remaining == USER_STACK_MAX_GROWTH_PAGES`.
- Calling `unmap_user_stack` on a `Process` with `stack_mapped_bottom = USER_STACK_TOP - N * 0x1000` unmaps exactly `N` pages, then leaves the four mutable fields at `0`.
- Calling `unmap_user_stack` on a sentinel `Process` (PID 0, all stack fields `0`) is a no-op (no unmap call).
- Sentinel `Process` (PID 0) leaves stack fields at `0` after the kernel boots without entering a user process.

**Verification:** New tests pass; existing loader/lifecycle tests untouched.

---

### U2. Loader: initial commit + growth-floor computation + bounds expansion

**Goal:** Shrink the eager stack commit, compute the per-process growth floor, expand `bounds_start` to cover the full growable range, and update `parse_pt_load` so PT_LOAD spill into the *grown* stack range is rejected.

**Requirements:** R3, R5, R7.

**Dependencies:** U1.

**Files:**
- `src/userland/loader.rs`
- `src/userland/image.rs` (add `stack_initial_bottom`, `stack_max_growth_floor` read-only fields used by U3)
- `src/mm/paging.rs` (constants only — `USER_STACK_INITIAL_PAGES` and `USER_STACK_MAX_GROWTH_PAGES` are best colocated with `USER_STACK_TOP`)
- `src/tests/userland.rs` (extend loader tests)

**Approach:**
- Replace `USER_STACK_PAGES` with two constants in `src/mm/paging.rs`:
  - `USER_STACK_INITIAL_PAGES: u64 = 8` (matches pre-zsh default; ~32 KiB).
  - `USER_STACK_MAX_GROWTH_PAGES: u64 = 768` (≈3 MiB; leaves ~1 MiB of the 4 MiB user-VA slice for code/data + headroom).
- In `load_elf`, after PT_LOAD parsing:
  - Compute `highest_pt_load_end_page` from `pt_loads` (max of `page_va + page_count * 0x1000`).
  - `stack_max_growth_floor = max(USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES * 0x1000, highest_pt_load_end_page + 16 * 0x1000)` — the `+ 16 * 0x1000` is the 64 KiB guard between code and the deepest possible stack page.
  - `initial_stack_bottom = USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000`.
- Map only `USER_STACK_INITIAL_PAGES` pages (RW) at `initial_stack_bottom`. **Do not** call `image.record_mapping` for the stack — Process owns stack teardown via `unmap_user_stack` (U1).
- Surface the stack metadata to the caller. `UserImage` gets two read-only fields used by U3 to populate Process: `stack_initial_bottom: u64` and `stack_max_growth_floor: u64`. `stack_top` is already present.
- `bounds_start = min(bounds_start, initial_stack_bottom)` — bounds start at the initial commit, not the growth floor. Growth widens bounds via `set_user_va_bounds` in U4. This keeps `validate_user_slice` honest about what's actually mapped.
- Update `parse_pt_load`'s spill check: reject PT_LOAD where `region_end > stack_max_growth_floor && page_va < USER_STACK_TOP`. This is stricter than today's check against the initial commit and protects against ELFs whose data section sits where a grown stack would land. The check needs access to the growth-floor value, which depends on other PT_LOADs — restructure as a post-pass over `pt_loads` (compute floor first, then validate each segment against it), or push the check out of `parse_pt_load` into a separate `check_no_pt_load_in_grown_stack` helper called after `check_no_overlap`.
- **Measurement-driven verification (R13).** Before merging, measure stack peak for: (a) zsh interactive startup and first ten commands; (b) `BB.ELF find /` over the FS root; (c) `BB.ELF awk` running a 10k-line deep-recursion script. If any peak approaches `USER_STACK_MAX_GROWTH_PAGES * 0x1000`, raise the constant before merge. Record the measurements in the implementation PR description.

**Patterns to follow:**
- Existing `bounds_start`/`bounds_end` reduction at `src/userland/loader.rs:298-320`.
- Existing post-parse pass like `check_no_overlap` at `loader.rs:559`.

**Test scenarios:**
- `test_loader_happy_path` still passes; `image.mapping_count()` now reflects no stack entry (PT_LOAD + TLS only).
- New test: load a fixture with a PT_LOAD whose end is at, say, `USER_LOAD_BASE + 0x300000` (3 MiB). Confirm `stack_max_growth_floor` is the per-binary floor (`PT_LOAD end + 0x1000`), not the global cap.
- New test: load a fixture whose PT_LOAD would extend into the growth region. Loader returns `LoaderError::VaOutOfRange`.
- New test: `image.bounds_start <= stack_max_growth_floor` for a fixture with the global cap binding.
- Update `test_loader_rollback_unmaps_on_reloc_failure` (`src/tests/userland.rs:1034`): replace `8 * 0x1000` with `USER_STACK_INITIAL_PAGES * 0x1000`.

**Verification:** All existing loader tests pass; new tests pass; no caller still references the removed `USER_STACK_PAGES` constant.

---

### U3. Wire stack window into `Process` at install + execve

**Goal:** Plumb the loader-computed stack-window values into `Process` at process install and execve (fork handled by U5).

**Requirements:** R2.

**Dependencies:** U1, U2.

**Files:**
- `src/userland/lifecycle.rs` (`install_new_process_opt`)
- `src/userland/mod.rs` (`enter_user_mode_with_aspace` call sites if they read stack metadata)
- `src/userland/syscalls.rs` (`execve_handler` — fork covered by U5)
- `src/tests/userland.rs`

**Approach:**
- Change `install_new_process_opt` to read `stack_top` (already on `UserImage`), `stack_initial_bottom`, and `stack_max_growth_floor` (added by U2) from the `UserImage` and call `Process::set_stack_window(stack_top, stack_initial_bottom, stack_initial_bottom, stack_max_growth_floor, USER_STACK_MAX_GROWTH_PAGES)` — initial `stack_bottom == stack_mapped_bottom == initial_bottom`; full growth budget.
- For execve: the new image's metadata replaces the old (no preservation across exec — POSIX semantics).

**Patterns to follow:**
- Existing `install_new_process_opt` field assignments at `src/userland/lifecycle.rs:260-275`.

**Test scenarios:**
- After `run /HOST/HELLO.ELF`, the live `Process` reports `stack_bottom == USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000` (8 pages, not 64).
- After a `fork` syscall, child `Process` shares the parent's `stack_top` / `stack_max_growth_floor`; child `stack_bottom` equals parent's at the moment of fork.
- After `execve`, the new `Process`'s stack-window fields reflect the new image, not the caller's.

**Verification:** Tests pass; no Process slot is created without populated stack-window fields except the kernel sentinel (PID 0).

---

### U4. Ring-3 stack-grow branch in the page-fault handler

**Goal:** Detect stack-grow candidates in the ring-3 page-fault path and grow the stack one page at a time. Fall through to `cleanup_user_process` for true overflow, lock contention, growth-budget exhaustion, or frame-allocator failure.

**Requirements:** R1, R4, R6, R7, R9, R10, R11, R12.

**Dependencies:** U1, U2, U3.

**Files:**
- `src/arch/x86_64/interrupts.rs` (`page_fault_handler`)
- `src/userland/lifecycle.rs` (new `try_grow_user_stack` helper)
- `src/userland/abi.rs` (the growth helper widens validated bounds via `set_user_va_bounds`)
- `src/tests/userland.rs` + `src/tests/userland_fixtures.rs` (new fixture + tests)

**Approach:**
- Add `pub enum GrowOutcome { Grew, NotStackGrow, Overflow, BudgetExhausted, LockContended, MapFailed }`.
- Add `pub fn try_grow_user_stack(fault_addr: VirtAddr) -> GrowOutcome` in `lifecycle.rs`. Inside the helper:
  - `let mut guard = CURRENT_PROCESS.try_lock();` — on `None` return `LockContended` immediately.
  - Read `stack_top`, `stack_bottom`, `stack_max_growth_floor`, `growth_faults_remaining`. Classify:
    - `NotStackGrow` if `fault_addr >= stack_bottom` or `fault_addr >= stack_top` (above the current bottom — wrong handler).
    - `Overflow` if `fault_addr < stack_max_growth_floor`.
    - `BudgetExhausted` if `growth_faults_remaining == 0`.
  - Otherwise compute `new_page = fault_addr & !0xFFF`. Drop the guard before calling `with_memory_mapper` to avoid nested locking (the mapper is a different lock; explicit ordering keeps it sane). Call `map_user_region(VirtAddr::new(new_page), 1, UserPerms::ReadWrite)`. On error return `MapFailed`.
  - On success re-acquire `CURRENT_PROCESS.try_lock()` (on `None` the page is mapped but the bookkeeping is stale — log at debug, return `Grew` anyway; the next fault on the same page will be `NotStackGrow`-cum-leak — preferable to leaving the page unmapped). Update `Process.stack_bottom = new_page`, `Process.stack_mapped_bottom = new_page`, decrement `growth_faults_remaining`. Call `set_user_va_bounds(new_bounds_start = new_page, end unchanged)` to widen the validated user-pointer bounds (R7).
  - Emit `debug_trace!` for the per-growth line. Return `Grew`.
- In `page_fault_handler`, inside the existing `if frame_is_user(...)` branch: call `try_grow_user_stack(accessed_addr)` **before** logging the EXCEPTION line. On `Grew`, return. On any other outcome, fall through to `cleanup_user_process(AbnormalExit { vector: 14, .. })`. Update the "Ring-3 page faults are lifecycle events" comment at `interrupts.rs:244-246` to reflect the stack-growth exception.
- The `>>> PAGE FAULT at ...` info line is suppressed only for the `Grew` case (move it into the fallthrough branches).
- Wire `unmap_user_stack(p)` (U1) into `cleanup_user_process` and `cooperative_exit` so the grown stack range is released on every exit path.

**Patterns to follow:**
- `route_user_fault_or_panic` shape in `src/arch/x86_64/interrupts.rs:12` for shared ring-3 cleanup wiring.
- `with_current_process` and `with_memory_mapper` usage in existing handlers (but use `try_lock` here, not `lock`).
- `BootInfoFrameAllocator`'s every-256-frames info summary as a model if a stack-growth summary is added later (deferred per Scope).

**Test scenarios:**
- *Covers R1, R10.* Synthesized ELF in `userland_fixtures.rs` that, starting from RSP, writes one byte every 4 KiB downward for ~32 pages. With `USER_STACK_INITIAL_PAGES = 8` and growth-floor well below, expect cooperative exit 0. After exit, the `Process` reports `stack_bottom` equal to the lowest page touched. Confirm a follow-up process load doesn't show leaked mappings (count the mappings reported by the mapper after teardown).
- *Covers R6 (Overflow).* Synthesized ELF that writes one byte every 4 KiB downward for a number of pages that exceeds `USER_STACK_MAX_GROWTH_PAGES`. Expect `ExitKind::Abnormal { vector: 14, .. }`.
- *Covers R6 (per-binary floor).* Synthesized ELF with a large PT_LOAD whose end forces `stack_max_growth_floor` to the per-binary floor. Stack write just below that floor triggers Abnormal vector 14.
- *Covers R11.* Synthesized ELF that fault-storms into the growth window (touches the same page many times before letting `stack_bottom` advance — easier said than done in user code, but at minimum: run a fixture that grows exactly `USER_STACK_MAX_GROWTH_PAGES` times and confirm the next growth attempt triggers Abnormal vector 14 with `BudgetExhausted` recorded as the inner reason (use a test-only `LAST_GROW_OUTCOME` cell).
- *Covers R7 (bounds widen on growth).* After a grow that lowers `stack_bottom`, call `validate_user_slice` with a pointer at the new bottom; assert `Ok`. Before the grow, the same call returns `EFAULT`.
- *Covers R9.* `test_stack_growth_log_level_default` — run a 32-page-growth fixture at the default `Debug` log level and assert serial output contains no per-fault `grew stack` line (only the eventual exit summary).
- *Covers R12 (try_lock).* Manufacture a `CURRENT_PROCESS.lock()` held by a test driver, then directly invoke `try_grow_user_stack(addr_in_window)`; expect `GrowOutcome::LockContended` returned in <1 µs (no spin).
- *Covers R1 (no false positive).* A genuine bad-pointer dereference (e.g., `*0x10_0000_0000`) still terminates the process via the existing `test_run_fault_pf` path — confirm it still passes unchanged.
- *Covers R0 (diagnostic capture).* For the originally-flagged hanging command, capture CR2 and faulting RIP. Document in the implementation PR description before declaring R0 met.

**Test fixture note.** New fixtures need a loop+counter+arithmetic-on-RSP instruction stream — larger than existing fixtures in `src/tests/userland_fixtures.rs` (which top out at maybe a dozen instructions). Two options for the implementer: (a) hand-assemble the loops following the precedent in `fork_then_wait_with_status_elf`; (b) introduce a tiny test helper that takes a Rust closure expressed as a byte template and stamps in immediates. Pick (a) if loops stay short (under 30 bytes); pick (b) if you need three or more fixtures with similar shape.

**Verification:** New tests pass; `test_run_fault_pf`, `test_run_hellocpp_end_to_end`, and the zsh-interactive smoke path (manual + `./test.sh`) still pass. zsh startup-to-first-prompt time (manual stopwatch) is within 1.5× of the pre-change baseline.

---

### U5. Fork interaction: stack window carried into child

**Goal:** Make `fork_handler` populate the child's stack-window fields from the parent's current values, and verify parent post-fork resumes with intact stack state.

**Requirements:** R2.

**Dependencies:** U3, U4.

**Files:**
- `src/userland/syscalls.rs` (`fork_handler` — explicit code change to copy fields)
- `src/userland/lifecycle.rs` (parent stash audit; likely no change beyond U1)
- `src/tests/userland.rs`

**Approach:**
- Audit confirmed in plan-time research: `src/userland/syscalls.rs::fork_handler` constructs the child `Process` from scratch with `image: None` (the child shares the parent's mapped pages via `AddressSpace::clone_for_child` eager PML4[0] copy, not via the UserImage). The child Process needs the parent's `stack_top`, `stack_bottom`, `stack_mapped_bottom`, `stack_max_growth_floor`, and `growth_faults_remaining` copied verbatim. This is a real code change, not just an invariant check — promote out of "audit" framing.
- The parent (in `PARENT_STASH`) retains its own copy of the five fields. Single-app-synchronous semantics mean the parent's stack can't change while the child runs; the post-restore path is unchanged.
- Document in `src/userland/syscalls.rs::fork_handler` the symmetry: "child's growth_faults_remaining inherits the parent's remaining budget at fork time — a deeply-grown parent gives the child less headroom" (deliberate; matches Linux's per-process RLIMIT_STACK semantics where children inherit the rlimit not a fresh budget).

**Patterns to follow:**
- Existing child-Process construction in `fork_handler` (`src/userland/syscalls.rs::fork_handler` — same place `parent_pid`, `fd_table`, `cwd`, `signal_state` are copied).

**Test scenarios:**
- *Covers R2.* Synthesized ELF that grows the stack by 16 pages, then `fork`s. The child writes one byte at the parent-pre-fork lowest-stack address (proving it inherited the page from the eager PML4[0] copy). Both exit 0.
- *Covers R2.* After a fork+wait cycle in the test, the parent's `Process.stack_bottom` matches what it was pre-fork (no clobbering from child growth).
- Cooperative-exit confirms no stack-window field on the parent or child is `0` post-install.

**Verification:** Tests pass; existing fork tests (`test_fork_*` in `src/tests/userland.rs`) still pass.

---

### U6. Cleanup: stale references, constant rename, CLAUDE.md update

**Goal:** Remove or update everything that still references the old single `USER_STACK_PAGES` constant; mark the CLAUDE.md deferred item as resolved.

**Requirements:** none — pure cleanup tied to the above units.

**Dependencies:** U1, U2.

**Files:**
- `src/userland/loader.rs` (remove the `USER_STACK_PAGES` constant and its comment; replace with a docstring on the new `USER_STACK_INITIAL_PAGES`/`USER_STACK_MAX_GROWTH_PAGES` in `src/mm/paging.rs`)
- `src/tests/userland.rs` (update `test_loader_rollback_unmaps_on_reloc_failure` per U2 — verify nothing else still references the old constant)
- `CLAUDE.md` (strike item #1 from "Deferred from the zsh-interactive bring-up"; line-number reference for item #2's `syscalls.rs:1214` may shift if any of the above units edit `syscalls.rs` — re-anchor if so)
- `src/mm/CLAUDE.md` (extend the user-VA partition table to name `USER_STACK_INITIAL_PAGES` and `USER_STACK_MAX_GROWTH_PAGES` and the per-binary growth floor)
- Possibly `docs/plans/2026-05-09-001-feat-userland-linux-abi-cpp-hello-plan.md` and `docs/plans/2026-05-09-003-feat-zsh-on-agenticos-plan.md` if they reference `USER_STACK_PAGES` — update to point at the new constants (or leave a note that the plan landed and the constant was split).

**Test scenarios:**
- `cargo build --features test` succeeds with no `USER_STACK_PAGES` references remaining.
- `grep -r USER_STACK_PAGES src/` returns no results.
- CLAUDE.md no longer lists demand-grown stack under "Deferred" (or moves it into a "Recently resolved" subsection if that convention exists when this lands).

**Verification:** Grep clean; test build clean; CLAUDE.md reads correctly.

---

## Verification

End-to-end:
- `./build.sh` boots into the desktop, terminal launches zsh — confirm interactive shell still works after multiple commands.
- `./test.sh` exits 33 (all tests passed).
- `./test.sh stack` (filter on new stack-growth modules) exits 33.
- Manual: `run /HOST/HELLOCPP.ELF` still completes in under a second (no regression from the multi-MiB load post-mortem).
- Manual: invoke a previously-hanging command (the one the user originally flagged) and confirm it returns.

## Risks & Mitigations

- **RK1: Heap fault during growth-path bookkeeping.** Stack-grow path uses a single `u64` field on Process (no `Vec::push`). The `map_user_region` internal `Vec<PhysFrame>` of capacity 1 is acknowledged as bounded amortized allocation; kernel heap auto-mapping handles any recursive fault. R8 is scoped accordingly.
- **RK2: Kernel-mode user-pointer faults to ungrown stack pages.** Earlier design pre-expanded `bounds_start` to the growth floor, which would have admitted user pointers into unmapped pages — a syscall reading or writing such a pointer would fault in kernel mode where the existing fault path does not call `try_grow_user_stack` and would re-enter the mapper under its own lock. *Mitigation:* bounds_start tracks the actual mapped bottom; `try_grow_user_stack` widens it via `set_user_va_bounds` on each successful growth (R7). User pointers into unmapped pages now return EFAULT, matching today's heap/mmap behavior.
- **RK3: Frame allocator exhaustion via fault-storm.** A binary that fault-storms the growth window could chew through frames before its `stack_bottom` advances enough to tighten the natural check (bump allocator never reclaims). *Mitigation:* per-process `growth_faults_remaining` counter (R11), checked on every growth attempt.
- **RK4: Deadlock if `CURRENT_PROCESS` is held when a stack-grow fault fires.** Blocking `lock()` from interrupt context would deadlock on a single core. *Mitigation:* `try_lock`-only (R12); contention treated as overflow.
- **RK5: 4 MiB user VA slice may still be too small for some binaries.** A binary whose code section is 3 MiB+ leaves <1 MiB for the grown stack. *Mitigation:* the per-binary growth floor surfaces a `LoaderError::VaOutOfRange` at load time if PT_LOAD would force the floor above the initial commit. Future repartitioning is the real fix (deferred).
- **RK6: `restore_continuation` SS-restore regression.** Any new path that returns from the ring-3 fault handler must not bypass kernel SS restoration. *Mitigation:* the grew-case returns directly from the page-fault handler without long-jumping; only `cleanup_user_process` long-jumps, and we reuse it verbatim. The existing `test_kernel_ss_after_user_fault` regression covers this.
- **RK7: Symptom not actually caused by stack overflow.** The originally-flagged hanging command may be caused by items #2/#3 from CLAUDE.md (signal-mask restore, WIFSIGNALED encoding), not by a stack fault. *Mitigation:* R0 requires capturing CR2 + RIP from the actual failure before declaring this plan a fix. If the fault address isn't below `stack_bottom`, this plan doesn't address the user's reported problem and the work should pivot to items #2/#3.
- **RK8: zsh startup perf regression from per-fork fault burst.** Initial commit dropping from 64 to 8 pages means zsh fork takes ~10-14 growth faults to repopulate its working set. *Mitigation:* U4 verification requires zsh startup-to-first-prompt within 1.5× of baseline. If exceeded, raise `USER_STACK_INITIAL_PAGES` (e.g., to 16 or 32) before merge — the constant is tunable.
- **RK9: SMP/preemption breaks the per-process growth lock.** Currently `CURRENT_PROCESS` is a single global, single-app-synchronous. *Mitigation:* not addressed in this plan; the kernel is single-core. A future SMP move needs per-process locks for several reasons.

---

## Stale References to Update at Implementation Time

- `src/userland/loader.rs:79` — `USER_STACK_PAGES` comment block (~7 lines anticipating this fix). Remove.
- `src/userland/loader.rs:297` — `let stack_pages = USER_STACK_PAGES;`. Replace with `USER_STACK_INITIAL_PAGES`.
- `src/userland/loader.rs:362` — stack `map_user_region` call. Use initial commit count; do **not** `record_mapping`.
- `src/userland/loader.rs:541-544` — `parse_pt_load` spill check. Refactor as described in U2.
- `src/tests/userland.rs:1034` — `let stack_bottom = ... - 8 * 0x1000`. Reference the new constant.
- `src/arch/x86_64/interrupts.rs:244-246` — "Ring-3 page faults are lifecycle events — never auto-map them" comment. Update to reflect the stack-growth exception.
- `CLAUDE.md` — "Deferred from the zsh-interactive bring-up" item #1. Strike.
- `src/mm/CLAUDE.md` — partition table. Add `USER_STACK_INITIAL_PAGES` and `USER_STACK_MAX_GROWTH_PAGES`.
