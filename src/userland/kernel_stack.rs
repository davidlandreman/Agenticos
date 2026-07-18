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

/// Size of each per-process kernel stack. 64 KiB.
///
/// 16 KiB (the original budget) is not enough for the deepest paths the
/// kernel reaches today: zsh's musl init drives nested syscalls
/// (open → FAT chain walk → block I/O → allocator) that walk past the bottom
/// of a 16 KiB stack and stomp adjacent heap memory. Because the kernel
/// stack lives in the heap (boxed slice), the stomped memory is whatever
/// the allocator placed next to this buffer — typically a free-list node
/// or another live allocation. The corruption then surfaces non-locally:
/// PR #22's "linked_list_allocator drops holes from its free list during
/// zsh init" was this overflow trashing the allocator's free-chunk
/// headers. Single-process tests like `test_fork_execve_badpath_returns_to_parent`
/// fail in isolation for the same reason — whichever path happens to
/// straddle the stack bottom.
///
/// 64 KiB gives 4× headroom — comfortable for current paths and the
/// per-process cost is acceptable while the kernel runs one app at a time.
/// A future improvement would be a guard page below the stack so the
/// overflow faults immediately instead of corrupting silently.
pub const KERNEL_STACK_BYTES: usize = 64 * 1024;

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
