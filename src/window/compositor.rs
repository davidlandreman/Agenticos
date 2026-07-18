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
//! U10 moves the input + render path into its own kernel thread spawned
//! at boot. The kernel-thread scheduler round-robins it alongside other
//! kernel threads (the terminal launchers, future workers) AND U5's
//! timer ISR splits ring-3 slices via `try_preempt_ring3`. So the
//! compositor gets CPU even while a ring-3 process is busy:
//!
//! - Ring-3 runs for one timer tick.
//! - Timer at CPL=3 → `try_preempt_ring3` checks ring3_ready. If
//!   another ring-3 is queued, switch. Otherwise iretq back.
//! - Eventually the CPL=0 kernel-thread scheduler gets a slice (when
//!   ring-3 syscalls or yields). `try_run_scheduled_processes` picks
//!   the compositor; it processes input + renders + yields.
//!
//! ## What stays in the main loop
//!
//! - Preemption check + watchdog (low-latency housekeeping).
//! - `try_run_scheduled_processes` (the kernel-thread scheduler tick).
//! - `save_kernel_and_resume_ring3` (the U8 ring-3 dispatcher).
//! - `hlt` for true CPU idle.
//!
//! ## Why `process_expired_sleeps` runs here
//!
//! Ring-3 `nanosleep` deadlines are expired from this loop, not only the main
//! loop. Under U10 the main loop is the idle task and barely runs once this
//! compositor thread is always ready, so a self-timed ring-3 animation
//! (`PAINTING.ELF`) that blocks in `nanosleep` each frame would be woken only
//! every few seconds. The compositor is scheduled every round-robin revolution,
//! so waking sleepers here gives them frame-cadence resumption. Dispatch of the
//! woken process is already fast (`scheduler::next_runnable` pops `ring3_ready`
//! on every context switch).
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
        // Expire ring-3 `nanosleep` deadlines here, not just in the kernel
        // main loop: under U10 the main loop is the idle task and barely runs
        // once other kernel threads (this compositor) are always ready, so a
        // self-timed ring-3 animation loop would only get woken every few
        // seconds. The compositor is scheduled every round-robin revolution,
        // so waking sleepers here gives them frame-cadence resumption.
        crate::userland::lifecycle::process_expired_sleeps();

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

        // Render frame (early-exit inside if compositor has no dirty regions).
        crate::window::render_frame();

        // Yield. With round-robin scheduling, the compositor gets one
        // slice per scheduler revolution. When idle (no dirty
        // regions), each iteration is cheap — early-exits inside
        // process_terminal_output and render_frame.
        crate::process::yield_current();
    }
}
