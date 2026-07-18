---
title: "feat: Make physical memory reusable and user address spaces sparse"
status: completed
created: 2026-07-17
plan_type: feat
depth: deep
related_docs:
  - src/mm/CLAUDE.md
  - src/process/CLAUDE.md
  - src/userland/CLAUDE.md
  - src/tests/CLAUDE.md
  - docs/plans/2026-05-09-002-perf-frame-allocator-and-page-fault-hot-path-plan.md
  - docs/plans/2026-05-16-003-feat-userland-demand-grown-stack-plan.md
  - docs/plans/2026-05-16-005-feat-multi-ring3-process-scheduling-plan.md
  - docs/conductor-workflow.md
---

# feat: Make physical memory reusable and user address spaces sparse

## Summary

Replace the current allocate-once user-memory model with a reclaiming virtual
memory subsystem suitable for compiler workloads. Land the work in this strict
dependency order:

1. A bitmap physical-frame allocator with per-frame reference counts and real
   deallocation.
2. Address-space-owned page-table teardown that releases user leaves, page
   tables, and the root even when the address space is not active.
3. Per-process VMA tracking and a sparse lower-half user layout, including a
   stack near the canonical user-space ceiling.
4. Copy-on-write fork using the frame reference counts.
5. VMA-correct `munmap`, shrinking `brk`, and enforced `mprotect`.
6. Demand paging for anonymous, file-backed, heap, stack, and large executable
   mappings, plus user-copy helpers that can safely fault pages in from kernel
   context.
7. Only after reclamation and stress tests are green, raise normal QEMU RAM to
   2 GiB by default and enlarge the demand-backed kernel heap VA arena.

Each numbered phase is a separate review unit and depends on the preceding
phase's acceptance gate. In particular, do not begin COW while address-space
drop still leaks, and do not add lazy mappings while user-pointer validation is
still a single broad min/max bound.

## Evidence and problem frame

### Physical pages are never reusable

`src/mm/frame_allocator.rs:46-91` implements a forward-only cursor. The
allocator can issue a frame but has no ownership state, refcount, or
deallocation API. `MemoryMapper::unmap_user_region` at
`src/mm/paging.rs:371-407` returns leaf frames in a `Vec`, but callers discard
them. Parent page-table frames allocated by `map_to_with_table_flags` are not
tracked for teardown either.

There are also smaller transactional leaks today:

- `map_user_region` allocates a leaf before mapping it and does not roll back
  earlier leaves or newly-created page tables when a later page fails.
- `handle_page_fault` allocates a frame before learning that a page is already
  mapped; its `PageAlreadyMapped` branch returns without releasing that frame.
- The live allocator throughput test deliberately consumes 256 permanent
  frames on every test boot (`src/tests/memory.rs:157-203`).

### Process teardown abandons the owning address space

`AddressSpace::Drop` at `src/userland/address_space.rs:256-272` switches away
from an active L4 but leaks the L4 and every user table below it.
`remove_process` at `src/userland/lifecycle.rs:439-457` must call
`UserImage::abandon` because `UserImage::Drop` unmaps against the current CR3,
which may belong to another process. This prevents cross-process corruption but
turns every reaped process into an intentional physical-memory leak.

The canonical teardown owner must be `AddressSpace`, because it knows the root
frame to walk. A list of ranges attached to `UserImage`, and an API that always
targets the active CR3, cannot safely tear down an inactive child.

### Fork cost is proportional to resident memory

`AddressSpace::clone_for_child` at `src/userland/address_space.rs:99-157`
recursively creates a new user subtree, and `clone_pt` at lines 222-253
allocates and copies every present 4 KiB leaf. Forking a compiler driver thus
doubles all resident code, stack, heap, TLS, and mmap pages before the child can
`execve`.

### There is no virtual-memory source of truth

The process stores only `brk_current` and a forward-only `mmap_next`
(`src/userland/lifecycle.rs:91-98`). `UserImage` has a construction-time list of
mapped ranges, but `munmap` does not remove or split those records. The current
syscalls show the consequences:

- `mmap` accepts only anonymous private mappings and eagerly maps every page.
- `munmap` ignores mapper errors, does not update ownership metadata, and never
  reuses the VA hole (`src/userland/syscalls.rs:465-486`).
- `mprotect` is an unconditional success stub (lines 488-496).
- `brk` grows eagerly and treats shrink as a no-op (lines 498-545).
- `validate_user_slice` accepts any pointer inside one global min/max range,
  including unmapped gaps (`src/userland/abi.rs:62-160`).

Correct partial unmap, protection changes, lazy faults, and file-backed mmap
all require a sorted, non-overlapping VMA map.

### The current layout is still a 4 MiB executable box

`USER_LOAD_BASE` is `0x400000` and `USER_STACK_TOP` is `0x800000`
(`src/mm/paging.rs:14-18`). The stack-growth code therefore has to squeeze code,
data, the stack, and a guard into the same roughly 4 MiB slice. Fixed TLS at
16 MiB and fixed brk at 32 MiB become the next collisions as soon as binaries
grow.

Address-space isolation currently clears only PML4 entry 0 and copies entries
1..511 from the kernel L4 (`src/userland/address_space.rs:45-81`). The kernel
heap at `0x4444_4444_0000` and kernel-thread stacks at
`0x5555_0000_0000` occupy lower-canonical PML4 slots 136 and 170, so the new
layout must explicitly reserve those slots instead of assuming the entire
lower half is empty.

### The machine and heap caps are development-sized

Normal boot uses `-m 128M` at `build.sh:247`, while the kernel heap exposes a
100 MiB demand-backed VA arena at `src/mm/heap.rs:6-7`. Increasing either value
before reclamation merely postpones exhaustion. After the leak tests are green,
the normal developer profile should provide enough memory for a GCC-class
workload and an override for larger rustc experiments.

## Goals

- Every managed physical frame has one auditable state: free, exclusively
  owned, shared with a positive refcount, or permanently pinned.
- Destroying a user address space returns all of its leaf and page-table frames
  to the allocator, regardless of which CR3 is active.
- Repeated launch/exit and fork/exec/wait cycles reach a stable frame count
  rather than consuming memory monotonically.
- Fork cost is proportional to page-table structure, not resident data size;
  writable private leaves copy only on the first write.
- A process has a sorted set of VMAs for ELF segments, TLS, stack, brk,
  anonymous mmap, and private file mmap.
- `munmap`, `brk`, and `mprotect` mutate VMAs and present PTEs consistently,
  including partial-range splitting.
- Large reservations consume virtual address space only. Physical usage grows
  with touched pages.
- ELF code may still begin at `0x400000`, but PT_LOAD size is no longer bounded
  by a stack at 8 MiB.
- The user stack begins near the top of the canonical lower half and grows down
  through a large sparse reservation with a real guard gap.
- The normal QEMU profile defaults to 2 GiB after all reclamation gates pass,
  with an easy 4 GiB override for rustc work.

## Non-goals

- Swap, compression, overcommit heuristics, or an OOM killer.
- SMP-safe TLB shootdown. The kernel remains single-CPU; every API must make
  the future shootdown boundary explicit, but this epic performs local flushes.
- Transparent huge pages or user 1 GiB/2 MiB mappings. User page-table walkers
  reject huge leaves during this epic.
- `MAP_SHARED` writeback or a global file page cache. File mappings are
  read-only or `MAP_PRIVATE`; dirty private pages are anonymous after COW.
- Threads or `CLONE_VM`.
- ASLR. The layout becomes sparse and collision-aware but remains deterministic
  for reproducible tests.
- Returning empty kernel-heap pages to the physical allocator. Freed heap
  objects remain reusable by `linked_list_allocator`; physical heap residency
  is still a high-water mark. User pages and user page tables are the critical
  reclamation target here.
- Relocating the kernel heap and kernel-thread stack regions. Their two
  lower-half PML4 slots are reserved holes in user space for this iteration.

## Core design decisions

### 1. Use a bitmap plus compact refcount table

Replace `BootInfoFrameAllocator`'s cursor state with a reusable allocator that
keeps:

- one allocation bitmap bit per usable 4 KiB frame;
- one `u32` reference count per usable frame;
- a next-fit word hint for amortized O(1) allocation;
- counters for total, free, pinned, exclusive, and shared frames;
- the bootloader memory map for compact-index to physical-frame translation.

Index metadata by the ordinal of a frame within `MemoryRegionKind::Usable`
regions, not by the largest physical frame number. This avoids a huge metadata
hole on machines whose memory map contains a high sparse region. Translating a
physical frame back to its compact index may scan the small boot memory-region
list; allocation scans bitmap words from the saved hint.

The allocator is needed before the Rust heap exists. During initialization,
use the existing bump cursor only as a bootstrap mechanism to reserve a
physically contiguous metadata block from a sufficiently large usable region.
Reach that block through the bootloader physical-memory offset, initialize it,
and mark its frames pinned. If no usable region can hold the metadata block,
fail boot with the required byte count and largest available extent rather
than silently falling back to a leaking allocator.

At 4 GiB, a bitmap plus `u32` count costs roughly 4.1 MiB, about 0.1% of RAM.
Null frame remains permanently unavailable. Frames outside usable regions are
never managed.

Expose a narrow API:

- `allocate_frame()` returns a zero-to-one ownership transition and count 1.
- `retain_frame(frame)` increments an owned frame for sharing.
- `release_frame(frame)` decrements and returns it to the free bitmap at zero.
- `pin_frame(frame)` is initialization-only and uses a non-releasable state.
- `stats()` returns stable counters for tests and diagnostics.

Implement the `x86_64` crate's `FrameAllocator<Size4KiB>` and
`FrameDeallocator<Size4KiB>` traits, but keep retain/release explicit because a
plain deallocator cannot express COW sharing.

Debug/test builds detect release of an unmanaged frame, double release,
refcount overflow, and attempts to release pinned metadata. Release builds log
and refuse the invalid transition rather than corrupting the bitmap.

### 2. Address spaces own page-table trees

Add address-space-targeted primitives that accept an L4 frame instead of
constructing an `OffsetPageTable` from the live CR3:

- map one page into a specified L4;
- look up a PTE and its parent path;
- change leaf flags while preserving software bits;
- unmap one page if present and release its leaf reference;
- prune empty PT/PD/PDPT frames;
- walk and destroy every user-owned subtree.

All tables are reachable through the physical-memory offset. User page-table
frames are exclusive and have refcount 1; only leaf data frames may be shared.
The teardown walker clears entries bottom-up, releases leaves, releases empty
L1/L2/L3 tables, and finally releases the L4 root. Encountering a huge user
entry is an invariant failure, not a frame to guess about.

`AddressSpace::Drop` is the only whole-space teardown owner. If its L4 is live,
it switches to the kernel L4 first; it then destroys its user-owned slots and
the root without touching any other address space. This deletes the need for
`UserImage::abandon` and removes physical mapping ownership from
`UserImage::Drop`.

Mapping operations must be transactional. If a multi-page map fails, unwind
the leaves installed by that call, release the not-yet-installed leaf, and
prune page tables created by the failed transaction. A partially built child
or exec address space can then be dropped normally.

### 3. Split lower-half slots by ownership

Use the canonical lower half (`0x0000_0000_0000_0000` through
`0x0000_7fff_ffff_ffff`) for user virtual memory, with these exceptions:

- keep a null/low-address guard below the ELF load area;
- reserve the PML4 slot containing the kernel heap (`HEAP_START`, slot 136);
- reserve the PML4 slot containing kernel-thread stacks
  (`STACK_REGION_START`, slot 170).

PML4 entries 256..511 remain shared kernel entries. In entries 0..255,
`AddressSpace::new` copies only the explicit kernel-reserved slots and leaves
all other slots per-process and initially empty. User mapping validation rejects
the reserved holes, and the VMA gap allocator skips them. Add boot-time
assertions that every required lower-half kernel mapping falls in the declared
reserved set; an unexpected present lower-half kernel slot must be added to the
policy deliberately.

This is safer for this epic than moving bootloader and kernel mappings at the
same time, while still giving programs tens of terabytes of sparse VA.

### 4. Make VMAs the source of truth

Add `src/userland/vm.rs` with a sorted, non-overlapping `Vec<Vma>`. A vector is
appropriate at the current process/mapping scale, available under `alloc`, and
easier to split transactionally than a new tree implementation. The API must
centralize insert, remove, split, merge, gap search, full-range coverage, and
access checks so syscall code never edits the vector directly.

Each VMA records:

- page-aligned `[start, end)`;
- logical protection (`READ`, `WRITE`, `EXEC`, including `PROT_NONE`);
- private/shared policy and growth flags;
- a backing kind:
  - ELF segment with `Arc<File>`, file offset/length, and zero-fill tail;
  - TLS/TCB anonymous pages;
  - grow-down stack with maximum floor and guard gap;
  - brk/heap anonymous range;
  - anonymous mmap;
  - private file mapping with `Arc<File>`, file offset, and captured file size.

The `Arc<File>` keeps a mapping alive after `close(fd)`. Add an atomic
`File::read_at(offset, dst)` operation that does not change the descriptor's
shared position; page faults must not implement pread as separate
seek/read/seek calls.

`AddressSpace` owns the VMA set alongside its L4. `Process` keeps scalar policy
state such as the byte-granular current brk, but `brk_current` and any mmap
cursor are not substitutes for VMAs. Fork clones VMA metadata; exec builds a
complete replacement `AddressSpace` off to the side and swaps it only after
loading and initial-stack construction succeed.

Retire the global `USER_VA_BOUNDS`. User-pointer checks query the current
address space's VMA coverage and required access, so a hole between two valid
VMAs is rejected.

### 5. Use a conventional sparse layout

Keep static ET_EXEC PT_LOAD addresses and the common `0x400000` load base, but
remove the artificial `0x800000` collision:

- `USER_MIN`: retain a low guard and require ELF mappings at or above the
  current `0x400000` policy unless the ELF format support is expanded.
- PT_LOAD: accept any canonical, non-reserved, non-overlapping user range.
- brk base: derive from the page-aligned highest PT_LOAD end instead of the
  fixed 32 MiB constant; keep a guard before later mappings.
- TLS/TCB: allocate as VMAs rather than pinning them at 16 MiB.
- mmap: choose reusable gaps, preferably top-down below the stack reservation;
  honor a page-aligned hint only when the full range is free. Continue to
  reject `MAP_FIXED` initially rather than destructively replacing mappings.
- `USER_STACK_TOP`: `0x0000_7fff_ffff_f000`.
- stack reservation: initially 64 MiB, with only the top 8 pages committed and
  at least a 1 MiB unmapped guard gap below the maximum-growth floor. The
  reservation consumes no physical frames.

The exact stack limit is a policy constant, not an architectural ceiling. It
can later come from `RLIMIT_STACK` without another page-table redesign.

### 6. Mark COW in a software PTE bit

Use `PageTableFlags::BIT_9` as `PTE_COW`. Fork walks only present user leaves:

- read-only leaves are shared and retained without changing logical VMA
  permissions;
- writable private leaves have `WRITABLE` cleared and `PTE_COW` set in parent
  and child, then the frame is retained;
- writable shared mappings are not supported in this epic;
- non-present leaves are represented only by cloned VMA metadata and cost no
  frame references.

After parent PTE changes, flush the parent's affected TLB entries (one full
local flush is acceptable for fork). Child entries have never been in the TLB.

On a user-mode protection fault caused by write, resolve COW before ordinary
protection handling:

1. Require a present `PTE_COW` leaf and a VMA that is logically writable.
2. If the physical refcount is 1, clear `PTE_COW`, set `WRITABLE`, and flush.
3. Otherwise allocate a frame, copy 4 KiB through physical aliases, replace the
   PTE atomically, release the old frame, and flush the faulting page.
4. If allocation fails, leave the old mapping untouched and terminate with the
   normal user OOM/fault result.

`mprotect(PROT_WRITE)` must preserve COW: logical write permission does not
mean a shared COW PTE can become hardware-writable.

### 7. Resolve faults from VMAs in a fixed order

The ring-3 page-fault path classifies in this order:

1. COW write fault.
2. Protection violation against an existing VMA (`SIGSEGV`; do not allocate).
3. Non-present page in a valid VMA:
   - anonymous/brk/TLS: allocate and zero;
   - stack: enforce grow-down floor/guard/budget, then allocate and zero;
   - file/ELF: allocate, zero, read the intersecting file bytes at the VMA's
     offset, and leave the BSS/tail zeroed;
   - file access wholly beyond the captured file length: report `SIGBUS`.
4. Address outside all VMAs: `SIGSEGV`.

The fault handler takes a small immutable `FaultPlan` snapshot from the current
process, drops the process-table lock, performs frame/page-table/file work, and
then records any accounting update. Never hold `PROCESS_TABLE` while acquiring
the frame allocator, mapper, or filesystem locks.

User-mode faults arrive while no syscall-side filesystem lock is held, so
synchronous read-only page-in is safe on the current single CPU. Document that
this assumption must be redesigned before SMP or blocking storage is added.

### 8. Replace raw user dereferences with user-copy helpers

Demand paging makes today's min/max validation insufficient: a syscall can
legitimately receive a pointer into a VMA whose page is not resident, and a
kernel-mode dereference would otherwise enter the fatal kernel-fault path.

Add `src/userland/usercopy.rs`:

- `copy_from_user(dst, src_user, len)` requires readable VMA coverage and
  faults in each source page before copying;
- `copy_to_user(dst_user, src, len)` requires writable coverage, resolves COW
  if necessary, and faults in each destination page;
- typed unaligned reads/writes and C-string/vector helpers are layered on those
  primitives;
- every range uses checked arithmetic and rejects reserved kernel holes.

Migrate syscall, signal-frame, argv/envp, and path parsing code away from direct
`ptr as *const/*mut` user accesses. Loader writes into a not-yet-active address
space through physical aliases and does not use the current-CR3 user-copy path.

Enable `EFER.NXE` before calling `mprotect` real. The existing NX bits are only
documentary today; without NXE, `PROT_EXEC`/non-exec distinctions are not
enforced.

## Implementation sequence

### Phase 0 — Baseline, counters, and failure injection

**Goal:** Make frame leaks measurable before changing ownership.

1. Add a read-only frame-allocator stats snapshot to the existing kernel-state
   diagnostics and test API: total usable, pinned, allocated, shared, and free.
   The initial forward allocator can report issued frames until Phase 1 fills
   in the complete counters.
2. Add test-only fail-after-N allocation injection at the allocator boundary.
   It must default off and reset through an RAII guard so one test cannot poison
   the next.
3. Record baseline deltas for:
   - fresh `AddressSpace::new` then drop;
   - load then exit;
   - fork then child exit/reap;
   - failed multi-page mapping;
   - failed child clone.
4. Keep these tests diagnostic/expected-leak until Phase 1/2 flips them to
   equality assertions. Do not weaken the final assertions to tolerate a
   fixed leak.

**Files:** `src/mm/frame_allocator.rs`, `src/mm/paging.rs`,
`src/tools/kernel_state.rs`, `src/tests/memory.rs`, `src/tests/userland.rs`.

**Gate:** The tests deterministically expose the current leak counts and the
full pre-epic `./test.sh` baseline remains green.

### Phase 1 — Reusable physical-frame allocator

**Goal:** Allocate, retain, release, and reuse managed physical frames safely.

1. Preserve the cursor as a private bootstrap scanner; replace the live
   allocator state with bitmap/refcount metadata initialized before heap init.
2. Pass the physical-memory offset into allocator initialization and reserve
   the metadata extent before marking ordinary usable frames free.
3. Implement next-fit allocation, retain, release, pin, and stats.
4. Add mapper wrappers so every allocation site has a matching error-path
   release. Fix `handle_page_fault(PageAlreadyMapped)` and transactional
   multi-page mapping immediately.
5. Change live throughput tests to release their frames and assert that the
   next allocation reuses released capacity; retire the monotonic-order
   invariant that was intentionally allocator-specific.
6. Keep zero-fill at the mapping/page-in boundary, not in the raw allocator,
   so page-table frames are not needlessly cleared twice. Explicitly zero every
   newly allocated page-table frame before installing it.

**Tests:**

- synthetic memory maps with holes, frame zero, non-usable regions, and
  metadata reservation;
- allocate-to-exhaustion/release/reallocate on a small synthetic pool;
- `1 -> 2 -> 1 -> 0` refcount transition and reuse only at zero;
- pinned/unmanaged/double-release rejection;
- allocation failure leaves stats unchanged;
- live allocate/release stress with the free count returning to baseline.

**Gate:** Raw frames are genuinely reusable and every mapper error path added
or touched by this phase has zero net frame delta.

### Phase 2 — Address-space-targeted mapping and complete teardown

**Goal:** Drop any address space and recover all of its page-table and resident
user frames without depending on active CR3.

1. Add the lower-half PML4 ownership policy and reserve kernel slots 136 and
   170. Copy those plus the upper kernel half into each new L4; zero all other
   lower entries.
2. Replace active-CR3-only user map/unmap helpers with L4-targeted page-table
   primitives. Keep current-address wrappers only as thin adapters for callers
   not yet migrated.
3. Implement bottom-up user subtree destruction and empty-table pruning.
4. Move whole-space ownership into `AddressSpace::Drop`; switch to kernel CR3
   first only when dropping the active space.
5. Remove `UserImage::abandon`. Make `UserImage` program metadata rather than a
   page owner; its `Drop` must not touch page tables.
6. Make exec transactional: create/map/populate a new address space without
   destroying the old image, atomically activate/install it on success, and
   drop the untouched old address space afterward. A failed load simply drops
   the new space and resumes the old program.
7. Ensure fork/clone failure injection drops the partial child and unwinds all
   retained/allocated frames.

**Tests:**

- drop active and inactive address spaces with the same frame baseline result;
- map leaves across PT, PD, and PDPT boundaries, then drop and reclaim every
  table level;
- another process's overlapping VA and the shared kernel entries remain
  unchanged;
- partial map and partial clone failures leave no frame delta;
- repeated failed exec preserves the old image and frame count;
- unexpected huge-page or reserved-slot traversal fails loudly in tests.

**Gate:** 1,000 create/map/drop cycles and 1,000 launch/exit cycles under a
128 MiB test profile show no monotonic frame loss.

### Phase 3 — VMAs and sparse executable/stack layout

**Goal:** Establish authoritative virtual-memory metadata before optimizing
fork or changing memory syscalls.

1. Add `vm.rs` and its split/merge/gap/coverage APIs.
2. Move VMA ownership into `AddressSpace`; remove `mmap_next` as an allocation
   authority.
3. Teach the loader to register PT_LOAD, TLS/TCB, stack, and initial brk VMAs.
   In this phase it may still populate pages eagerly.
4. Move `USER_STACK_TOP` to `0x0000_7fff_ffff_f000`, reserve the 64 MiB stack
   window/guard, and keep the 8-page initial commit.
5. Derive brk from the loaded image end, allocate TLS from free VA, and replace
   `USER_VA_RANGE_START/END` checks with canonical-user plus reserved-hole
   checks.
6. Add private file-backed mmap metadata. Implement `File::read_at`; mappings
   retain an `Arc<File>` across fd close. Continue eagerly populating in this
   phase so faulting behavior changes only in Phase 6.
7. Replace global `USER_VA_BOUNDS` validation with VMA coverage checks, while
   leaving raw copying in place until the user-copy phase.

**Tests:**

- insert/merge/split/trim and first-fit/top-down gap reuse;
- overlap and integer-wrap rejection;
- a synthetic PT_LOAD larger than 4 MiB coexists with the high stack;
- stack, TLS, brk, mmap, and reserved kernel holes never overlap;
- VMA validation rejects a gap between two valid ranges;
- closing a mapped fd does not invalidate the VMA backing;
- exec replaces the complete VMA set, while fork clones it by value/reference.

**Gate:** Existing zsh/BusyBox/compiler-compat tests pass with the high stack,
and a test ELF with a PT_LOAD end above the old `0x800000` ceiling loads.

### Phase 4 — Copy-on-write fork

**Goal:** Fork shares resident data and copies only a page that either side
writes.

1. Replace eager leaf copies in `clone_for_child` with shared leaf installs and
   refcount increments.
2. Mark writable private leaves read-only+COW in both address spaces; share
   read-only leaves without a COW marker.
3. Clone non-present VMA metadata without allocating a leaf.
4. Add the COW write-fault resolver before stack/demand fault handling.
5. Flush the parent's local TLB after write-protecting it.
6. Audit `mprotect`, loader relocation writes, signal-frame writes, and kernel
   user copies: no kernel path may bypass COW and modify a shared frame.
7. Route `vfork` through ordinary COW fork until true vfork semantics exist;
   update its stale “full eager copy” comment.

**Tests:**

- child and parent initially translate a writable VA to the same frame;
- fork frame delta is page tables only, not resident leaf count;
- first child write creates one private frame and preserves parent bytes;
- first parent write does the symmetric operation;
- refcount-1 COW fault upgrades in place without a copy;
- read-only write is rejected rather than mistaken for COW;
- nested forks and every parent/child exit ordering return to baseline;
- fork with hundreds of MiB reserved but only a few resident pages remains
  cheap.

**Gate:** A fork/exec/wait stress loop under 128 MiB completes at least 10,000
iterations with stable free-frame count and no cross-process data corruption.

### Phase 5 — Correct `munmap`, shrinking `brk`, and real `mprotect`

**Goal:** Make memory syscalls mutate the same VMA/PTE ownership model used by
fork and teardown.

1. `mmap`:
   - use VMA gap search and reuse holes;
   - support anonymous private and readable private file mappings;
   - require page-aligned file offsets and a readable file fd;
   - keep `MAP_SHARED` and `MAP_FIXED` explicitly unsupported;
   - roll back VMA and resident pages atomically on failure.
2. `munmap`:
   - require aligned address/nonzero length and round length up;
   - split or trim every intersecting VMA;
   - unmap present leaves, release references, and prune empty tables;
   - succeed over valid holes, matching Linux behavior;
   - adjust file offsets on the retained right-hand VMA.
3. `brk`:
   - update one dedicated heap VMA;
   - on shrink, release only complete pages above the rounded new end;
   - preserve a partially used boundary page;
   - on regrowth, return zero-filled memory and never expose old data;
   - return the old break if a grow request cannot be satisfied.
4. `mprotect`:
   - require full VMA coverage or return `ENOMEM`;
   - split VMAs at both boundaries and update logical protection;
   - update flags on present leaves and flush locally;
   - preserve `PTE_COW` for logically writable shared leaves;
   - implement `PROT_NONE` as inaccessible to user mode;
   - enforce W^X unless a documented compatibility exception is required.
5. Enable `EFER.NXE` during architecture initialization and add an executable
   permission test.

**Tests:**

- unmap prefix/suffix/middle/multiple VMAs and a range containing holes;
- remap reuses the exact released gap;
- unmap of COW leaves decrements but does not prematurely free shared frames;
- brk grow/shrink/regrow across partial/full pages with zero-fill checks;
- mprotect read-only/write/execute/none transitions and cross-VMA failure;
- mprotect-write on COW does not make the shared physical frame writable;
- file-map right split computes the correct new file offset.

**Gate:** Syscall-level and live ring-3 fixtures agree with VMA/PTE state, and
all released resident frames return to the allocator.

### Phase 6 — Demand paging and safe user copies

**Goal:** Large mappings reserve VA cheaply and acquire frames only as pages
are touched.

1. Add the VMA fault classifier and fixed resolution order.
2. Make anonymous mmap and brk growth metadata-only. Keep the top stack commit
   small; grow/fill stack pages through the same resolver.
3. Make private file mmap populate one page per fault using `read_at`, with a
   zero-filled partial tail and `SIGBUS` beyond EOF.
4. Convert PT_LOAD segments to file-backed VMAs where possible. Refactor ELF
   parsing/loading to bounded `read_at` operations so a large executable does
   not require one whole-file kernel `Vec`. Eagerly materialize only pages that
   relocation or initial-stack construction must modify; the rest fault in.
5. Remove the 16 MiB `MAX_USER_BINARY_BYTES` implementation ceiling in favor
   of checked ELF/VMA limits and available storage.
6. Add `usercopy.rs` and migrate every syscall/path/signal direct user-pointer
   dereference. `copy_to_user` explicitly resolves COW and lazy destination
   pages before writing.
7. Keep loader writes to inactive address spaces on physical aliases; never
   activate a half-built exec image merely to copy bytes.
8. Replace the old special-case stack hook with the unified fault resolver,
   retaining stack guard/budget outcome observability for tests.

**Tests:**

- reserving a 512 MiB anonymous mapping consumes zero leaf frames;
- touching N sparse pages consumes N leaves plus only required page tables;
- untouched pages remain non-present across fork;
- lazy file pages contain correct offset bytes and tail zeros;
- private writes do not change the file or a sibling mapping;
- a mapped fd may be closed before the first page fault;
- syscall read/write buffers in nonresident pages fault in safely from kernel
  context;
- invalid/protected pointers return `EFAULT` rather than panicking the kernel;
- a large ELF starts with resident memory proportional to touched pages;
- munmap/exit after sparse faults returns to the pre-mapping frame baseline.

**Gate:** A large sparse-memory fixture, repeated compiler-compat launches, and
the fork/exec/wait stress loop pass under constrained RAM with no frame drift.

### Phase 7 — Capacity defaults and operational visibility

**Goal:** Raise development capacity only after reclamation is proven.

1. Add `AGENTICOS_QEMU_MEMORY`, defaulting normal `build.sh` runs to `2G`.
   Document `AGENTICOS_QEMU_MEMORY=4G ./build.sh` for rustc experiments and
   `1G` as the supported GCC floor.
2. Add an explicit `AGENTICOS_TEST_MEMORY` to `test.sh`, defaulting to 256 MiB
   for the full suite. Keep dedicated reclamation stress runs at 128 MiB so a
   larger default cannot hide leaks.
3. Increase the demand-backed kernel heap VA arena from 100 MiB to 512 MiB.
   This does not preallocate physical RAM. Record the remaining high-water
   physical-residency limitation in `src/mm/CLAUDE.md`.
4. Derive page-fault heap bounds from `heap::HEAP_START/HEAP_SIZE` rather than
   duplicating 100 MiB constants in the interrupt handler.
5. Extend memory diagnostics with free/shared/pinned frames, resident pages by
   VMA kind, COW faults/copies/upgrades, demand faults, and page-in failures.
6. Update subsystem CLAUDE files, architecture docs, build help, and stale
   comments that describe the bump allocator, 1 GiB user ceiling, fixed TLS,
   fixed brk, eager fork, or 4 MiB code/stack slice.

**Gate:**

```sh
cargo fmt --check
cargo check
./test.sh memory vm userland
AGENTICOS_TEST_MEMORY=128M ./test.sh memory vm userland compiler_compat
./test.sh --skip-userland compiler_compat
./test.sh
AGENTICOS_QEMU_MEMORY=2G ./build.sh
```

The 128 MiB stress boot is a permanent leak regression gate; the 2 GiB normal
boot is the developer/compiler capacity check.

## Delivery strategy

Use one dependent branch/PR per numbered phase, following
`docs/conductor-workflow.md`. These phases are deliberately sequential because
they redefine ownership contracts. Within a phase, pure data-structure tests,
documentation, and live fixtures can be developed in separate workspaces once
the phase API is fixed, but later phases must not merge before the earlier
acceptance gate.

Recommended commit boundaries inside each phase:

1. Pure data structure/API plus synthetic tests.
2. Integration into mapper/process/syscall paths.
3. Failure-path and stress tests.
4. Documentation and stale-comment cleanup.

Keep commits bisectable: no commit may leave two competing owners of a frame
or a `Drop` implementation that can target the wrong CR3.

## Acceptance criteria

- The allocator can reuse a released frame and reports exact refcounts for COW
  leaves.
- Allocator metadata and frame zero remain pinned/unavailable.
- `AddressSpace::Drop` releases all user leaf, L1, L2, L3, and L4 frames from
  active or inactive spaces.
- `UserImage::Drop` no longer edits page tables, and `UserImage::abandon` is
  gone.
- Repeated process launch/exit and fork/exec/wait have zero steady-state frame
  drift.
- Fork does not copy resident leaves until write; parent and child isolation is
  preserved after either writes.
- VMAs cover ELF, TLS, stack, brk, anonymous mmap, and private file mmap, with
  no overlaps or hidden bump cursor authority.
- `munmap` splits metadata, releases present pages, tolerates holes, and makes
  the VA reusable.
- Shrinking brk releases complete pages and regrowth observes zeros.
- `mprotect` changes hardware permissions, preserves COW, and NX is enabled.
- A 512 MiB untouched mapping uses no leaf frames; sparse touches consume only
  proportional frames.
- Kernel reads/writes of lazy user buffers use user-copy helpers and never turn
  a valid user demand fault into a kernel panic.
- The stack top is `0x0000_7fff_ffff_f000`; an executable larger than the old
  4 MiB slice loads without colliding with stack/TLS/brk.
- Normal QEMU boot defaults to 2 GiB, supports a 4 GiB override, and constrained
  128 MiB reclamation tests remain green.

## Risks and mitigations

- **Allocator metadata bootstrapping becomes recursive.** Reserve and map the
  metadata block with the old cursor before exposing the reusable allocator;
  never allocate metadata from the heap.
- **Compact frame lookup is wrong across memory-map holes.** Centralize ordinal
  translation and test both directions over synthetic discontiguous regions.
- **A teardown walker frees shared kernel tables.** Encode lower-slot ownership
  in one policy, skip reserved/upper slots unconditionally, and compare kernel
  PML4 entries before/after destructive tests.
- **Unmapping a shared COW frame frees it early.** Every leaf removal calls
  `release_frame`, never raw deallocation; only count zero clears the bitmap.
- **Parent remains writable in a stale TLB after fork.** Flush the local TLB
  after installing parent COW flags before returning from fork.
- **COW and mprotect disagree.** Keep logical protection in the VMA and physical
  sharing in PTE/refcount state. Hardware writable is allowed only when the VMA
  permits write and the leaf is not shared COW.
- **A file page fault deadlocks in filesystem code.** User faults snapshot/drop
  `PROCESS_TABLE` before I/O, use one atomic `read_at`, and rely explicitly on
  the single-CPU/no-kernel-preemption storage model. Add lock-order assertions
  where practical.
- **Kernel-mode user copies fault recursively.** Preflight/fault-in through
  `usercopy` before dereferencing. The generic kernel page-fault handler never
  guesses that an arbitrary kernel fault is user demand paging.
- **`PROT_NONE` loses the physical frame address.** Preserve the PTE address and
  software bits while clearing user accessibility/presence as chosen by the
  implementation; the page-table walker, not `translate_addr`, is the source
  for later unmap/protect.
- **The new lower-half layout aliases kernel mappings.** Reject VMAs in reserved
  PML4 slots and assert the boot kernel has no undeclared required lower-half
  entries.
- **A larger QEMU default masks leaks.** Do not change the default before Phase
  7, and retain the 128 MiB 10,000-cycle reclamation gate permanently.
- **The 512 MiB kernel heap can retain too much physical memory at high water.**
  It is demand-backed and freed objects are reusable within the heap, but page
  decommit remains documented follow-up work. Monitor resident heap pages in
  the new diagnostics.
- **Scope expands into a complete Linux VM.** Keep `MAP_SHARED`, writeback,
  swap, ASLR, huge pages, and SMP shootdown out of this epic; reject unsupported
  flags explicitly and test those errno paths.
