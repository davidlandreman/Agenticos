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
//! ## The BinaryLoadGuard interaction
//!
//! The IDE PIO atomicity concern (multi-MiB binary loads contending
//! with framebuffer writes) is preserved: the compositor checks
//! `binary_load_in_progress()` and skips input + render while a
//! binary is being loaded. Terminal output processing always runs
//! (kernel-side `print!` macros accumulate into the console buffer;
//! we want the post-load redraw to catch up).

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
        let user_active = crate::userland::lifecycle::binary_load_in_progress();

        // Input processing — skip while a binary is being loaded so the
        // ~3.7 MiB framebuffer writes from render_frame don't contend
        // with PIO IDE reads. (See the
        // 2026-05-09-multi-mib-user-binary-load learning.)
        if !user_active {
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
        }

        // U10/bugfix: invalidate terminal windows that have buffered
        // output from ring-3 writes. The write path itself doesn't
        // touch WINDOW_MANAGER (would deadlock against an
        // in-progress render); the invalidation happens here under
        // the compositor's own lock.
        crate::window::terminal::invalidate_dirty_terminals();

        // Terminal output processing keeps running so the user app's
        // `write` syscall bytes still reach the terminal buffer; the
        // compositing catches up after the binary exits.
        crate::window::process_terminal_output();

        // Render frame (early-exit inside if compositor has no dirty
        // regions). Skip while binary loading for the IDE PIO reason
        // above.
        if !user_active {
            crate::window::render_frame();
        }

        // A one-tick cadence bounds input/render latency while allowing the
        // thread to leave the run queue completely between passes.
        crate::process::sleep_ticks_with_contract(
            1,
            Some(crate::process::entity::LatencyContract::new(2)),
        );
    }
}
