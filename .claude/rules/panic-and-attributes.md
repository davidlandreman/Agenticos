# Kernel Attributes and Panic Handler

This is a freestanding bare-metal kernel. Several attributes and rules are non-negotiable.

## Required crate-level attributes

- **`#![no_std]`** — no Rust standard library.
- **`#![no_main]`** — no Rust runtime entry point. The bootloader calls our entry symbol directly.

## Function-level

- **`#[no_mangle]`** — required on any function the bootloader or assembly stubs call by name. Without this, the linker drops or renames the symbol.
- **`#[panic_handler]`** — exactly one panic handler in the crate. It lives in `src/panic.rs`.

## Panic handler behavior

The custom panic handler in `src/panic.rs` differs by build mode:

- **Normal build** — prints panic info to serial, then halts.
- **Test build** (`cargo build --features test`) — prints, then exits QEMU with the failure code (`0x11 << 1 | 1` = 35) so the test harness sees a failure rather than a hang.

Never write a second panic handler. Never `panic!()` in an interrupt handler — validate inputs and recover or halt cleanly.

For testing-flow specifics see `.claude/rules/testing-flow.md`.
