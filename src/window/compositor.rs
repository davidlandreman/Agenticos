//! U10: compositor kernel thread.
//!
//! Pre-U10, the kernel main loop did input processing + terminal output +
//! rendering inline. With multi-ring-3 scheduling (U5-U8) live, a CPU-
//! bound ring-3 process (e.g., a tight loop, BusyBox `yes`) would
//! monopolize the timer's CPL=3 path: U5 keeps round-robining among
//! ring-3 processes but never returns to the main loop, so input
//! events queue up unprocessed and the screen never redraws. The user
//! sees a frozen desktop until the ring-3 process voluntarily yields
//! (syscall block, exit).
//!
//! The compositor is a kernel entity in the same queue as ring-3 processes.
//! PIT preemption therefore rotates directly between user work, compositor,
//! network, and other workers without a main-loop handoff.
//!
//! ## What stays in the main loop
//!
//! - Watchdog housekeeping.
//! - Initial/idle dispatch through the unified selector.
//! - `hlt` for true CPU idle.
//!
//! The compositor is deadline-driven through the shared scheduler timer. It
//! blocks for one PIT tick after each bounded pass and is woken with a two-tick
//! dispatch contract. Timer expiration is owned by `timer-service`, never by
//! this rendering loop.
//!
//! Storage uses interrupt-driven VirtIO DMA, so binary loading never pauses
//! compositor input processing or rendering.

use crate::input::{InputProcessor, INPUT_QUEUE};
use spin::Mutex;

/// Per-CPU processor state. Single-CPU kernel — one global Mutex is
/// fine. Initialized lazily on first call to `run` since the
/// compositor's stack hasn't been allocated when this static is
/// declared.
static PROCESSOR: Mutex<Option<InputProcessor>> = Mutex::new(None);

/// Compositor entry point. Loops forever, processing input + rendering
/// + yielding the CPU.
///
/// Spawned at boot from `src/kernel.rs::init_kernel` via
/// `crate::process::spawn_process("compositor", None, run)`.
pub fn run() {
    // Lazy-init the InputProcessor. The screen dimensions match the
    // boot framebuffer (the pre-U10 main loop used the same constants
    // — see kernel.rs).
    {
        let mut g = PROCESSOR.lock();
        if g.is_none() {
            *g = Some(InputProcessor::new(1280, 720));
        }
    }

    let using_virtio = crate::drivers::mouse::is_virtio_tablet();

    loop {
        let mut g = PROCESSOR.lock();
        if let Some(processor) = g.as_mut() {
            if using_virtio {
                crate::drivers::mouse::poll();
                if let Some(event) = processor.check_virtio_tablet() {
                    crate::window::process_event(event);
                }
            }
            for event in processor.process_pending(&INPUT_QUEUE) {
                crate::window::process_event(event);
            }
        }
        drop(g);

        // Render frame (early-exit inside if compositor has no dirty regions).
        crate::window::render_frame();
        crate::system_control::drain_pending_notifications();

        // A one-tick cadence bounds input/render latency while allowing the
        // thread to leave the run queue completely between passes.
        crate::process::sleep_ticks_with_contract(
            1,
            Some(crate::process::entity::LatencyContract::new(2)),
        );
    }
}
