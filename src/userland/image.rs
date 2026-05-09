//! `UserImage` (U6 / D8).
//!
//! Transactional handle returned by the ELF loader on success. Owns:
//!
//! - The list of `(virt_addr, page_count)` mapping ranges installed in the
//!   user VA window — `Drop` walks this list and calls `unmap_user_region` on
//!   each, so a partial-load failure does not leak page-table state.
//! - The user-VA bounds the loader sized for the image. U7 stamps these into
//!   `crate::userland::abi::USER_VA_BOUNDS` so syscall pointer-validation
//!   will accept user-supplied pointers within the image.
//! - The entry-point virtual address and the user stack top.
//!
//! ## Frame freeing
//!
//! The kernel's frame allocator (`BootInfoFrameAllocator`) is bump-only — it
//! never returns frames to the pool. `unmap_user_region` returns the freed
//! `PhysFrame` list, but `UserImage::Drop` discards it. The plan calls this
//! out explicitly: per-PID frame tracking is not in scope for U6. The
//! transactional invariant is "no page-table state survives a failed load,"
//! not "every frame is reclaimable."
//!
//! ## Why a `Vec<MappingRange>` rather than per-segment fields
//!
//! A static-PIE binary may legitimately have several PT_LOAD segments with
//! distinct permission profiles (.text R-X, .rodata R, .data R-W). The loader
//! pushes one entry per allocated region — PT_LOAD segments and the user
//! stack — and the drop path is identical for all of them.

use alloc::vec::Vec;
use x86_64::VirtAddr;

use crate::mm::paging::UserMapError;

/// One installed user-VA mapping. The pair is exactly what
/// `unmap_user_region` consumes.
#[derive(Debug, Clone, Copy)]
pub struct MappingRange {
    pub virt_start: VirtAddr,
    pub page_count: u64,
}

/// Transactional handle for a loaded user binary. Drop unmaps every recorded
/// range. Constructed via `UserImage::new()`; the loader pushes mappings as
/// it makes them so a mid-load failure that drops the image still cleans up.
#[derive(Debug)]
#[allow(dead_code)] // bounds_{start,end} are read by U7 when entering ring 3
pub struct UserImage {
    /// Resolved RIP for `iretq` to user mode.
    pub entry: VirtAddr,
    /// Top of the user stack (exclusive). The loader sets the user RSP to
    /// `stack_top - 8` before entering ring 3 (pre-aligned for an
    /// `alignment-after-call` System V invariant; in practice `_start` is
    /// `extern "C"` and that convention applies).
    pub stack_top: VirtAddr,
    /// Inclusive-lower / exclusive-upper user-VA bounds covering every PT_LOAD
    /// segment plus the user stack and (when present) the TLS region. Used
    /// to populate the syscall pointer-validation bounds before entering
    /// ring 3.
    pub bounds_start: u64,
    pub bounds_end: u64,
    /// FS_BASE for `arch_prctl(ARCH_SET_FS)` — the address of the TCB the
    /// loader allocated when the binary has a `PT_TLS` segment. `None`
    /// when the binary has no TLS image. Consumers (the initial-stack
    /// builder in U8 and the `arch_prctl` handler in U9) read this field
    /// to install the per-process FS_BASE.
    pub tls_fs_base: Option<VirtAddr>,
    /// Every region the loader mapped, in mapping order.
    mappings: Vec<MappingRange>,
    /// Set to `false` by the destructor to make Drop idempotent in case a
    /// future refactor tries to drop twice.
    dropped: bool,
}

impl UserImage {
    pub fn new(
        entry: VirtAddr,
        stack_top: VirtAddr,
        bounds_start: u64,
        bounds_end: u64,
    ) -> Self {
        Self {
            entry,
            stack_top,
            bounds_start,
            bounds_end,
            tls_fs_base: None,
            mappings: Vec::new(),
            dropped: false,
        }
    }

    /// Record the FS_BASE address for the TCB the loader allocated. Called
    /// from the loader when a `PT_TLS` segment is present.
    pub fn set_tls_fs_base(&mut self, fs_base: VirtAddr) {
        self.tls_fs_base = Some(fs_base);
    }

    /// Record a mapping that has just been installed via `map_user_region`.
    /// The drop path replays this list in reverse to call `unmap_user_region`
    /// for each.
    pub fn record_mapping(&mut self, virt_start: VirtAddr, page_count: u64) {
        self.mappings.push(MappingRange {
            virt_start,
            page_count,
        });
    }

    /// Number of recorded mappings. Test-visible.
    pub fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    /// Total user pages mapped (PT_LOAD + stack). Test-visible.
    pub fn total_pages(&self) -> u64 {
        self.mappings.iter().map(|m| m.page_count).sum()
    }

    /// Test-only: peek at a recorded mapping without consuming it.
    #[cfg(feature = "test")]
    pub fn mapping(&self, idx: usize) -> Option<MappingRange> {
        self.mappings.get(idx).copied()
    }
}

impl Drop for UserImage {
    fn drop(&mut self) {
        if self.dropped {
            return;
        }
        self.dropped = true;

        // Unmap in reverse order. The mapper does not require this — each
        // range is independent — but reverse-of-construction is the standard
        // teardown discipline for transactional handles.
        let mut errs: u32 = 0;
        for range in self.mappings.iter().rev() {
            let res = crate::mm::memory::with_memory_mapper(|m| {
                m.unmap_user_region(range.virt_start, range.page_count)
            });
            match res {
                Some(Ok(_)) => {}
                Some(Err(UserMapError::PageNotMapped)) => {
                    // The range was never finalized (loader failed between
                    // `record_mapping` and the actual `map_user_region`
                    // returning Ok). Treat as a no-op.
                }
                _ => errs += 1,
            }
        }
        if errs > 0 {
            crate::debug_warn!(
                "UserImage::drop: {} mapping(s) failed to unmap cleanly",
                errs
            );
        }
        self.mappings.clear();
    }
}
