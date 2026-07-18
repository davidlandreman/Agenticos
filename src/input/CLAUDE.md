# `src/input/` — Input Processing Pipeline

Three-layer architecture for interrupt-safe input handling: hardware interrupt handlers push raw events to a lock-free queue; a processing layer turns them into typed events; typed events feed the window system.

## Key files

- `mod.rs` — central `InputProcessor`; converts raw events to typed events and routes them.
- `queue.rs` — lock-free SPSC (Single-Producer, Single-Consumer) ring buffer, 256 entries (power of 2 for cheap modulo). Atomic `Release`/`Acquire` ordering.
- `keyboard_driver.rs` — PS/2 scancode-set-2 → `KeyCode` state machine. Handles extended (`0xE0`) prefixes and modifier tracking (Shift, Ctrl, Alt).
- `mouse_driver.rs` — PS/2 mouse 3-byte packet state machine. Validates packet integrity and produces `MouseEvent` (delta + buttons).

## Three layers

```
1. Hardware (src/drivers/, src/arch/x86_64/interrupts.rs)
     interrupt handler pushes RawInputEvent → queue
2. Processing (this folder)
     InputProcessor pops from queue, runs scancode/packet state machines
3. Event delivery
     Typed events handed to src/window/ for routing through the window tree
```

The processor runs in the kernel's idle loop:

```rust
input::process_pending_events();   // drain queue, route to windows
window::render_frame();            // render any changes
```

## Why lock-free (don't break this)

The SPSC queue exists because earlier code used `try_lock` from interrupt context, which caused stalls when the lock was held by the consumer. **Do not add a `Mutex` here.** The interrupt-handler producer must never block. The atomic `compare_exchange` in `push()` returns `false` on full rather than blocking; dropping the rare overflow event is correct behavior, blocking the interrupt handler is not.

Producer count: exactly one logical producer on the BSP. IOAPIC routing pins
PS/2/device IRQs to CPU 0, and `InputQueue::push` debug-asserts that affinity.
Consumer count remains exactly one (the BSP idle-loop processor).
Multi-producer or multi-consumer use is a redesign, not a parameter tweak.

## Cross-references

- Raw event sources (PS/2 controllers, VirtIO tablet) live in `src/drivers/` — see `src/drivers/CLAUDE.md`.
- Typed events flow into the window tree — see `src/window/CLAUDE.md`.
