# Kernel Attributes and Panic Handler

This is a freestanding bare-metal kernel. Several attributes and rules are non-negotiable.

## Required crate-level attributes

- **`#![no_std]`** — no Rust standard library.
- **`#![no_main]`** — no Rust runtime entry point. The bootloader calls our entry symbol directly.

## Function-level

- **`#[no_mangle]`** — required on any function the bootloader or assembly stubs call by name. Without this, the linker drops or renames the symbol.
- **`#[panic_handler]`** — exactly one panic handler in the crate. It lives in `src/panic.rs`.

## Panic handler behavior

The custom panic handler in `src/panic.rs` differs by diagnostics/test mode:

- **Minimal diagnostics** — preserves the ordinary serial panic report and
  halt/test-exit behavior.
- **Record or strict diagnostics** — elects one crash owner, rendezvouses CPUs,
  emits the allocation-free capsule through debugcon, then halts or exits.
- **Test build** (`cargo build --features test`) — exits QEMU with the failure
  code (`0x11 << 1 | 1` = 35) after ordinary failures so the harness does not
  hang. Expected-fatal capsule tests deliberately require a non-success exit.

Never write a second panic handler. Never `panic!()` in an interrupt handler — validate inputs and recover or halt cleanly.

After crash ownership is elected, never allocate, format, touch a filesystem
or display, use the normal logger, or acquire a production lock. Crash-readable
state must use static/prefaulted storage and bounded atomic copies. See
`docs/crash-diagnostics.md` for schema and shadow-domain rules.

For testing-flow specifics see `.claude/rules/testing-flow.md`.
