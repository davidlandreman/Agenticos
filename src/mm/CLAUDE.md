# `src/mm/` — Memory Management

Physical and virtual memory management: frame allocation, virtual paging with demand mapping, and the kernel heap allocator.

## Key files

- `memory.rs` — subsystem entry point and initialization sequence.
- `frame_allocator.rs` — `BootInfoFrameAllocator`. Bump-cursor allocator over the bootloader's `MemoryRegions`: per-call cost is amortized O(1), the cursor remembers `(region_idx, next_addr)` so consecutive calls don't rebuild the iterator. Skips frame 0 for null-pointer safety. Emits a periodic info-level summary every 256 frames so a stuck system is still observable. The pure cursor-step (`next_frame`) is exposed via `test_support` for unit tests over synthetic memory maps. The previous implementation rebuilt `usable_frames().nth(self.next)` every call (O(n) per call, O(n²) overall) and was the dominant cost during multi-MiB heap demand-paging.
- `heap.rs` — global allocator. Backed by `linked_list_allocator` v0.10.
- `paging.rs` — `MemoryMapper` over `OffsetPageTable` for virtual ↔ physical translation. Page-fault integration for demand paging.

## Heap

- **Virtual address**: `0x_4444_4444_0000`.
- **Size**: 100 MiB (configurable in `heap.rs`).
- **Backend**: `linked_list_allocator` crate.
- Provides `#[global_allocator]`, which enables `alloc::*` collections (`Vec`, `String`, etc.) — see `.claude/rules/no-std.md`.
- Heap pages are mapped on demand: pages get backing frames only when first accessed.

## Virtual address partition

The kernel address space is partitioned into disjoint regions, each owned by exactly one subsystem:

| Range | Owner | Notes |
|---|---|---|
| `0x0040_0000` – `0x0080_0000` | userland binary + stack | `USER_LOAD_BASE` and `USER_STACK_TOP`; stack grows down. |
| `0x0100_0000` – `0x0100_2000` | userland TLS | TLS image page (`USER_TLS_IMAGE_VA`) + TCB page (`USER_TCB_VA`). FS_BASE points at TCB. Mapped only when the binary has a `PT_TLS` segment. |
| `0x0200_0000` – `0x0280_0000` | userland brk arena | `USER_BRK_BASE` + 8 MiB cap. `brk(0)` returns the current high water; `brk(addr)` grows on demand. |
| `0x0300_0000` – `0x4000_0000` | userland mmap arena | `USER_MMAP_BASE`. Anonymous-private bump arena, no coalescing. Reaches `USER_VA_RANGE_END` at 1 GiB. |
| `0x_4444_4444_0000` + 100 MiB | kernel heap | Demand-mapped on page fault. |
| `0x_5555_0000_0000` + N stacks | process stacks | `src/process/stack.rs`. |

`USER_VA_RANGE_START` / `USER_VA_RANGE_END` constants in `paging.rs` bracket the union of the userland slices; `map_user_region` rejects anything outside them.

## User mapping API

`MemoryMapper::map_user_region(virt_start, num_pages, perms)` and `unmap_user_region` are the only blessed paths into the user-VA partition. They:

- Use `Mapper::map_to_with_table_flags(parent_flags = PRESENT | WRITABLE | USER_ACCESSIBLE)` so the U bit is propagated to every parent table entry on the path, including pre-existing kernel-installed parents (D11). Without this, the first ring-3 access page-faults on the parent walk and the kernel triple-faults if the U2 fault routing isn't perfect.
- Return `Err(UserMapError::PageAlreadyMapped)` rather than swallow a clash; the user range must be empty when load begins.
- Zero-fill freshly allocated frames before mapping so user code never sees stale kernel data.
- Do **not** go through `handle_page_fault`. The page-fault handler short-circuits on CPL=3 in U2; user faults are lifecycle events, not lazy mappings.

`UserPerms` (R-X, R, R-W) bakes in NX/WX hygiene per D11 — `EFER.NXE` is documentary today, but the bits are correct so a future flip is a one-line change.

## Page-fault flow

When code accesses an unmapped heap page:

1. CPU raises a page fault with the unmapped virtual address.
2. The page-fault handler (in `src/arch/x86_64/interrupts.rs`) checks whether the address falls in the heap range.
3. If yes, it allocates a physical frame via `BootInfoFrameAllocator` and maps the virtual page via `MemoryMapper`.
4. Execution resumes transparently.

A page fault outside the heap range is fatal — it indicates a real bug, not lazy mapping.

## Hot-path log levels

The page-fault path runs ~1500 times during a multi-MiB binary load. Per-fault logging at info/debug level burns UART vmexits and dominates wall-clock time under interactive load. Discipline:

- `>>> PAGE FAULT at …, error: …` stays at **info** — one line per fault is the minimum signal a debugger needs.
- `Page fault in heap region at …`, `Handling page fault for address: …`, `Successfully mapped page … to frame …`, `Page X was already mapped` are all at **trace** (silent at the default `Debug` boot level).
- `Allocated frame at PhysAddr(…)` is at **trace** inside `BootInfoFrameAllocator::allocate_frame`. The cursor emits a periodic info-level summary every 256 frames (`frame allocator: N frames issued, region M, next 0x…`) so progress is still visible under load.

If you re-promote any of these to info/debug, the multi-MiB binary load slows back down dramatically. Code comments at `src/mm/paging.rs::handle_page_fault` and `src/arch/x86_64/interrupts.rs::page_fault_handler` reference plan U2 for the rationale.

## Read-into-uninit pattern (`Vec::set_len` after raw read)

`File::read_to_vec` (in `src/fs/file_handle.rs`) reads directly into a `Vec`'s spare capacity instead of pre-zeroing. The pattern: `Vec::with_capacity(size)` + `core::slice::from_raw_parts_mut(ptr, size)` + `read(dst)` + `set_len(bytes_read)`. Each backing page is touched exactly once (by the FAT/IDE copy) instead of twice (zero-fill, then overwrite).

SAFETY contract for any future code reaching for the same pattern:

1. `Vec::with_capacity(size)` allocated `size` uninitialized bytes the caller exclusively owns; `len() == 0` so an early return drops safely.
2. `Vec::with_capacity(0)` returns a dangling pointer — special-case `size == 0` to return `Vec::new()` before the unsafe slice construction.
3. The reader (`File::read` here) must be the SOLE writer — it returns `bytes_read <= size` initialized at the front of the slice.
4. `Vec::set_len(bytes_read)` after the read exposes only the initialized prefix, which is what `set_len`'s precondition requires.

A `debug_assert!(bytes_read <= size)` makes the bound a runtime check in test builds.

## Gotchas

- **Heap is unavailable until init runs.** Code in the boot path before `heap::init()` cannot use `alloc::*` types.
- **`OffsetPageTable` requires the physical-memory offset.** Don't construct one directly; go through `MemoryMapper`.
- **Frame 0 is intentionally never handed out.** The cursor's null-frame skip in `next_frame` is the load-bearing check; don't bypass it if you write a new allocator path.
- **Frame allocator's iteration order is load-bearing for the unit tests.** Frames within a Usable region are issued in ascending physical order; cross-region order matches `MemoryRegions::iter()`. A future allocator swap (bitmap, free-list) that reorders frames will trip the `test_frame_cursor_monotonic_over_4096_calls` test deliberately — that's the signal to revisit the invariant when changing the allocator.

## Debugging

Memory regions are printed during boot. The heap test suite (`src/tests/heap.rs`) validates allocator behavior end-to-end and includes a `test_heap_burst_throughput` that allocates 6 MiB and reports per-page fault cost — useful for spotting regressions in the page-fault path. The frame allocator's diagnostic tests in `src/tests/memory.rs` cover null-frame skip, region-boundary crossing, non-Usable region skip, exhaustion, and 4096-call monotonic ordering.
