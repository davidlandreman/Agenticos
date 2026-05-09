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
