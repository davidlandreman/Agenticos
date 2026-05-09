# `no_std` Discipline

**This is a `no_std` kernel. The Rust standard library is NOT available.**

## What you can use

- **`core::*`** — always available.
- **`alloc::*`** — available *after* the heap allocator initializes during boot. `Vec`, `String`, `Box`, `BTreeMap`, etc. work normally once init has run. Heap is sized at 100 MiB at virtual address `0x_4444_4444_0000`.

## What you cannot use

- **No `std::*` imports.** Never. Even when an `alloc::*` equivalent doesn't exist, find another way — do not reach for `std`.
- **No `HashMap` from `std`.** Use `alloc::collections::BTreeMap`, or implement a small custom map.
- **No file I/O, threads, sockets, or any OS-provided primitive.** This *is* the OS.

## Common gotchas

- **Custom `Arc`** — the kernel uses its own `Arc<T>` implementation in `src/lib/arc.rs`, not `alloc::sync::Arc`. Always import via `crate::lib::arc::Arc`.
- **Static slices vs. `Vec`** — for hot paths, interrupt handlers, and any code that runs before heap init, prefer `&'static [T]`. Heap allocation works but adds latency and depends on the allocator being initialized.
- **Panic in interrupt context** is fatal. Validate inputs in interrupt handlers; don't `unwrap()`.

For deeper memory subsystem context (heap internals, page-fault flow, frame allocator) see `src/mm/CLAUDE.md`. For the custom `Arc` see `src/lib/CLAUDE.md`.
