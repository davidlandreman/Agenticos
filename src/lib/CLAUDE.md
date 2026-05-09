# `src/lib/` — Core Kernel Libraries

Shared utilities used across subsystems: the kernel's custom `Arc`, debug logging, and the `no_std` test framework primitives.

## Key files

- `arc.rs` — kernel `Arc<T>` and `Weak<T>`. **NOT `alloc::sync::Arc`** — this is a custom implementation tuned for the kernel.
- `debug.rs` — 5-level debug logging: `error`, `warn`, `info`, `debug`, `trace` (macros: `debug_error!` … `debug_trace!`).
- `debug_breakpoint.rs` — debug breakpoint helpers.
- `test_utils.rs` — `Testable` trait and `test_runner()` function used by the `no_std` test framework.

## Custom `Arc` — important

This crate's `Arc` is the kernel's own implementation. **Always import as**:

```rust
use crate::lib::arc::{Arc, Weak};
```

Never `use alloc::sync::Arc` — the alloc version exists but the project deliberately uses the custom one. They are not interchangeable in this codebase.

Features:

- Thread-safe atomic reference counting.
- Weak references via `Arc::downgrade(&arc)`.
- Compatible with `!Sized` types.
- Integrated with the kernel heap allocator.

```rust
let data = Arc::new(vec![1, 2, 3]);
let data2 = data.clone();
assert_eq!(Arc::strong_count(&data), 2);

let weak = Arc::downgrade(&data);
assert!(weak.upgrade().is_some());
```

The `Arc`-based file API in `src/fs/` uses this — see `src/fs/CLAUDE.md`.

## Cross-references

- `Testable` and `test_runner()` are consumed by `src/tests/` — see `src/tests/CLAUDE.md` for how to author tests.
- `Arc<T>` is heap-allocated; `alloc::*` rules apply — see `.claude/rules/no-std.md`.
