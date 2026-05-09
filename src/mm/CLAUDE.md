# `src/mm/` — Memory Management

Physical and virtual memory management: frame allocation, virtual paging with demand mapping, and the kernel heap allocator.

## Key files

- `memory.rs` — subsystem entry point and initialization sequence.
- `frame_allocator.rs` — `BootInfoFrameAllocator`. Allocates physical 4 KiB frames from the bootloader's memory map, filtering to "Usable" regions. Skips frame 0 for null-pointer safety.
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

## Gotchas

- **Heap is unavailable until init runs.** Code in the boot path before `heap::init()` cannot use `alloc::*` types.
- **`OffsetPageTable` requires the physical-memory offset.** Don't construct one directly; go through `MemoryMapper`.
- **Frame 0 is intentionally never handed out.** Don't bypass this if you write a new allocator path.

## Debugging

Page faults log details via `debug_info!`. Memory regions are printed during boot. The heap test suite (`src/tests/heap.rs`) validates allocator behavior end-to-end.
