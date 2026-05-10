//! Per-process kernel stack.
//!
//! Phase 5 PR-C1. Each ring-3 user process gets its own kernel stack —
//! the buffer that the SYSCALL stub switches `RSP` to on entry and
//! that interrupt gates use as their TSS-`rsp0` target while the
//! process is running.
//!
//! This replaces the PR-C2 hack of shifting a single 64 KiB shared
//! rsp0 stack down 32 KiB so parent and child syscall handlers
//! wouldn't trample each other. With a fresh per-process stack, the
//! parent's frame stays on its own buffer while the child runs on a
//! new one, no arithmetic required.
//!
//! The buffer lives on the kernel heap (boxed). `KernelStack` owns it
//! exclusively; the buffer is freed when the `Process` slot drops.

use alloc::boxed::Box;
use x86_64::VirtAddr;

/// Size of each per-process kernel stack. 16 KiB matches the global
/// boot-time stack and is comfortable for the deepest path we have
/// today (execve → load_elf → FAT cluster walk + IDE PIO).
pub const KERNEL_STACK_BYTES: usize = 16 * 1024;

/// A kernel-side stack buffer for one user process.
///
/// Stack grows downward, so `top()` returns the high end of the
/// buffer (one byte past `base + size`). The CPU writes `RSP` to this
/// value on ring 3 → ring 0 transitions and pushes downward from
/// there.
pub struct KernelStack {
    buffer: Box<[u8]>,
}

impl KernelStack {
    pub fn new() -> Self {
        // Allocate via Vec to keep the buffer contents zeroed (harmless
        // but cheap with a fresh heap page) and to stay within the
        // existing heap allocator's path. The conversion to a boxed
        // slice drops length tracking we don't need.
        Self {
            buffer: alloc::vec![0u8; KERNEL_STACK_BYTES].into_boxed_slice(),
        }
    }

    /// Stack top (one past the highest valid byte) — the value the
    /// SYSCALL stub and IRQ gates load into RSP.
    pub fn top(&self) -> VirtAddr {
        let base = self.buffer.as_ptr() as u64;
        VirtAddr::new(base + self.buffer.len() as u64)
    }
}

impl Default for KernelStack {
    fn default() -> Self {
        Self::new()
    }
}
