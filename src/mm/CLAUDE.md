# `src/mm/` — Memory Management

Physical and virtual memory management: frame allocation, virtual paging with demand mapping, and the kernel heap allocator.

## Key files

- `memory.rs` — subsystem entry point and initialization sequence.
- `frame_allocator.rs` — reusable bitmap allocator with compact per-usable-frame `u32` refcounts, next-fit search, pin/retain/release APIs, stats, and test-only failure injection. A small pre-heap cursor reserves the contiguous metadata extent and pins frame zero/metadata.
- `heap.rs` — global allocator. Backed by `linked_list_allocator` v0.10.
- `paging.rs` — `MemoryMapper` over `OffsetPageTable` for virtual ↔ physical translation. Page-fault integration for demand paging.

## Heap

- **Virtual address**: `0x_4444_4444_0000`.
- **Size**: 512 MiB of virtual address space (configurable in `heap.rs`).
- **Backend**: `linked_list_allocator` crate.
- Provides `#[global_allocator]`, which enables `alloc::*` collections (`Vec`, `String`, etc.) — see `.claude/rules/no-std.md`.
- Heap pages are mapped on demand: pages get backing frames only when first accessed.

## Virtual address partition

The kernel address space is partitioned into disjoint regions, each owned by exactly one subsystem:

| Range | Owner | Notes |
|---|---|---|
| `0x0040_0000` – canonical lower-half ceiling | per-process user VMAs | Sparse ELF, TLS, heap, anonymous/private-file mmap, and a 64 MiB grow-down stack ending at `0x0000_7fff_ffff_f000`. Only eight stack pages are initially resident; a 1 MiB guard separates the reservation from lower mappings. |
| `0x0100_0000` – `0x0100_2000` | userland TLS | TLS image page (`USER_TLS_IMAGE_VA`) + TCB page (`USER_TCB_VA`). FS_BASE points at TCB. Mapped only when the binary has a `PT_TLS` segment. |
| derived from highest PT_LOAD | userland brk | Heap VMA grows metadata-only and releases complete resident pages on shrink. |
| reusable top-down gaps | userland mmap | Anonymous-private and readable file-private mappings; reservations are nonresident until touched. |
| `0x_4444_4444_0000` + 512 MiB | kernel heap | Demand-mapped on page fault. Freed objects are reused, but resident heap pages remain a high-water mark. |
| `0x_5555_0000_0000` + N stacks | process stacks | `src/process/stack.rs`. |

Lower-half PML4 slots 136 (kernel heap) and 170 (kernel-thread stacks) are shared kernel holes. VMA validation and targeted mapping reject them; upper-half slots remain shared.

## User mapping API

Address-space-targeted `MemoryMapper` operations accept an L4 frame and map,
inspect, protect, COW-resolve, unmap/prune, or destroy a user tree without
requiring that CR3 be active. Compatibility range wrappers remain for the ELF
loader and older kernel fixtures. Mapping operations:

- Use `Mapper::map_to_with_table_flags(parent_flags = PRESENT | WRITABLE | USER_ACCESSIBLE)` so the U bit is propagated to every parent table entry on the path, including pre-existing kernel-installed parents (D11). Without this, the first ring-3 access page-faults on the parent walk and the kernel triple-faults if the U2 fault routing isn't perfect.
- Return `Err(UserMapError::PageAlreadyMapped)` rather than swallow a clash; the user range must be empty when load begins.
- Zero-fill freshly allocated frames before mapping so user code never sees stale kernel data.
- Roll back leaves and newly empty page tables on partial failure.
- Release leaf references and empty L1/L2/L3 tables during unmap; AddressSpace drop also releases its L4 root.

`UserPerms` (R-X, R, R-W) bakes in NX/WX hygiene. `EFER.NXE` is enabled during architecture initialization, so NX is enforced.

All runtime access to the global `MemoryMapper` goes through
`memory::with_memory_mapper`; do not call `paging::get_mapper` outside the
`mm` module. Mapper closures must stay bounded and must not yield. Do not add a
spin mutex here: mapper access from page-fault context would make a same-core
spin lock deadlock.

## Page-fault flow

When code accesses an unmapped heap page:

1. CPU raises a page fault with the unmapped virtual address.
2. The page-fault handler (in `src/arch/x86_64/interrupts.rs`) checks whether the address falls in the heap range.
3. If yes, it allocates a physical frame via `BootInfoFrameAllocator` and maps the virtual page via `MemoryMapper`.
4. Execution resumes transparently.

A kernel heap fault demand-maps one page. A ring-3 fault first resolves COW,
then checks VMA protection, then pages in anonymous/heap/stack/private-file
backing. Addresses outside VMAs are fatal to that user process.

### Ring-3 stack-grow path

A ring-3 page fault (CPL=3) is routed through `lifecycle::try_grow_user_stack` *before* the standard cleanup path. If the fault address falls inside the active process's grow window (`stack_max_growth_floor ≤ addr < stack_bottom`) and the per-process growth budget allows, the handler maps one fresh page R+W, lowers `stack_bottom` and `stack_mapped_bottom`, widens `validate_user_slice`'s bounds via `set_user_va_bounds`, and returns so the CPU retries the instruction. Everything else (overflow, budget exhaustion, lock contention, map failure, fault outside the window) falls through to `cleanup_user_process` with vector 14 / SIGSEGV.

The stack-grow helper holds the Process lock via `try_lock` only — blocking `lock()` from interrupt context would deadlock the single core. Bookkeeping mutations are split across two short critical sections so `map_user_region`'s mapper-lock acquisition never nests with the Process lock.

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
- **Frame 0 and allocator metadata are pinned.** Releasing pinned, unmanaged, already-free, or overflowing references is rejected rather than corrupting the bitmap.
- **Every installed user leaf owns one frame reference.** COW fork retains leaves; every unmap/drop path must release exactly once. Never hide release calls inside `debug_assert!`, because release builds erase the assertion expression.

## Debugging

Memory regions are printed during boot. The heap test suite (`src/tests/heap.rs`) validates allocator behavior end-to-end and includes a `test_heap_burst_throughput` that allocates 6 MiB and reports per-page fault cost — useful for spotting regressions in the page-fault path. The frame allocator's diagnostic tests in `src/tests/memory.rs` cover null-frame skip, region-boundary crossing, non-Usable region skip, exhaustion, and 4096-call monotonic ordering.
