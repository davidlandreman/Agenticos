//! `UserImage` (U6 / D8).
//!
//! Program metadata returned by the ELF loader on success.
//!
//! - Construction-time mapping metadata used to seed the AddressSpace VMA
//!   set. Production setup then transfers page-table ownership to the
//!   AddressSpace and clears this list.
//! - Legacy bounds retained for kernel-only fixtures; real syscall pointer
//!   validation queries the current AddressSpace's VMAs.
//! - The entry-point virtual address and the user stack top.
//!
//! AddressSpace is the whole-tree teardown owner and returns user leaves,
//! page tables, and its root to the reusable frame allocator. The fallback
//! `Drop` cleanup below exists only for older tests that load directly into
//! the kernel L4 without creating an AddressSpace.
//!
//! ## Why a `Vec<MappingRange>` rather than per-segment fields
//!
//! A static-PIE binary may legitimately have several PT_LOAD segments with
//! distinct permission profiles (.text R-X, .rodata R, .data R-W). The loader
//! pushes one entry per allocated region — PT_LOAD segments and the user
//! stack — and the drop path is identical for all of them.

use alloc::vec::Vec;
use x86_64::VirtAddr;

use crate::fs::File;
use crate::lib::arc::Arc;

#[cfg(feature = "test")]
use crate::mm::paging::UserMapError;

/// One installed user-VA mapping. The pair is exactly what
/// `unmap_user_region` consumes.
#[derive(Debug, Clone, Copy)]
pub struct MappingRange {
    pub virt_start: VirtAddr,
    pub page_count: u64,
    pub perms: crate::mm::paging::UserPerms,
}

/// File source for a sparse PT_LOAD VMA.
#[derive(Clone)]
pub struct ElfBacking {
    pub start: u64,
    pub file: Arc<File>,
    pub file_offset: u64,
    pub file_len: u64,
    pub zero_tail: u64,
}

impl core::fmt::Debug for ElfBacking {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ElfBacking")
            .field("start", &self.start)
            .field("file_offset", &self.file_offset)
            .field("file_len", &self.file_len)
            .field("zero_tail", &self.zero_tail)
            .finish()
    }
}

/// Metadata handle for a loaded user binary. The loader records mappings as
/// it constructs them; normal process setup consumes those records into VMAs.
#[derive(Debug)]
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
    /// Page-aligned initial program break derived from the highest PT_LOAD.
    pub brk_base: u64,
    /// FS_BASE for `arch_prctl(ARCH_SET_FS)` — the address of the TCB the
    /// loader allocated when the binary has a `PT_TLS` segment. `None`
    /// when the binary has no TLS image. Consumers (the initial-stack
    /// builder in U8 and the `arch_prctl` handler in U9) read this field
    /// to install the per-process FS_BASE.
    pub tls_fs_base: Option<VirtAddr>,
    /// Raw program-header bytes from the input ELF, captured at load time.
    ///
    /// The Linux initial-stack contract requires `AT_PHDR` to point at
    /// the in-memory program-header table. musl-cross-make binaries
    /// usually place phdrs inside the first `PT_LOAD`, so AT_PHDR could
    /// point at `USER_LOAD_BASE + e_phoff`. The kernel's hand-rolled
    /// fixtures have phdrs *outside* any PT_LOAD though, so we capture
    /// the bytes here at load time and the initial-stack builder copies
    /// them onto the user stack — the same code path works for both.
    pub phdr_bytes: Vec<u8>,
    /// `e_phnum` from the ELF header — paired with `phdr_bytes` for
    /// `AT_PHNUM` and to size the on-stack phdr copy.
    pub e_phnum: u16,
    /// Demand-grown stack — VA of the lowest initially-committed stack
    /// page. The loader maps `USER_STACK_INITIAL_PAGES` pages at this
    /// address and stops; growth is driven from the page-fault handler.
    /// Consumed by U3 (`install_new_process_opt`) to populate Process's
    /// `stack_bottom` / `stack_mapped_bottom`.
    pub stack_initial_bottom: u64,
    /// Demand-grown stack — lowest address the stack may grow into for
    /// this binary. `max(USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES *
    /// 0x1000, highest_pt_load_end + USER_STACK_GUARD_PAGES * 0x1000)`.
    /// Consumed by U3.
    pub stack_max_growth_floor: u64,
    /// Every region the loader mapped, in mapping order. The user stack
    /// is **not** recorded here — Process owns stack teardown via
    /// `unmap_user_stack` so the per-fault growth path doesn't need to
    /// push to a heap-allocated `Vec` from interrupt context.
    mappings: Vec<MappingRange>,
    elf_backings: Vec<ElfBacking>,
    /// Set to `false` by the destructor to make Drop idempotent in case a
    /// future refactor tries to drop twice.
    dropped: bool,
}

impl UserImage {
    pub fn new(entry: VirtAddr, stack_top: VirtAddr, bounds_start: u64, bounds_end: u64) -> Self {
        Self {
            entry,
            stack_top,
            bounds_start,
            bounds_end,
            brk_base: crate::mm::paging::USER_BRK_BASE,
            tls_fs_base: None,
            phdr_bytes: Vec::new(),
            e_phnum: 0,
            stack_initial_bottom: 0,
            stack_max_growth_floor: 0,
            mappings: Vec::new(),
            elf_backings: Vec::new(),
            dropped: false,
        }
    }

    pub fn set_brk_base(&mut self, brk_base: u64) {
        self.brk_base = brk_base;
    }

    /// Record the loader-computed stack window. Consumed by U3 to
    /// install the values on `Process`.
    pub fn set_stack_window(&mut self, initial_bottom: u64, max_growth_floor: u64) {
        self.stack_initial_bottom = initial_bottom;
        self.stack_max_growth_floor = max_growth_floor;
    }

    /// Record the FS_BASE address for the TCB the loader allocated. Called
    /// from the loader when a `PT_TLS` segment is present.
    pub fn set_tls_fs_base(&mut self, fs_base: VirtAddr) {
        self.tls_fs_base = Some(fs_base);
    }

    /// Record the program-header bytes and count for AT_PHDR / AT_PHNUM.
    pub fn set_phdrs(&mut self, bytes: Vec<u8>, e_phnum: u16) {
        self.phdr_bytes = bytes;
        self.e_phnum = e_phnum;
    }

    /// Record a mapping that has just been installed via `map_user_region`.
    /// The drop path replays this list in reverse to call `unmap_user_region`
    /// for each.
    pub fn record_mapping(&mut self, virt_start: VirtAddr, page_count: u64) {
        self.record_mapping_with_perms(
            virt_start,
            page_count,
            crate::mm::paging::UserPerms::ReadWrite,
        );
    }

    pub fn record_mapping_with_perms(
        &mut self,
        virt_start: VirtAddr,
        page_count: u64,
        perms: crate::mm::paging::UserPerms,
    ) {
        self.mappings.push(MappingRange {
            virt_start,
            page_count,
            perms,
        });
    }

    /// Number of recorded mappings. Test-visible.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn mapping_count(&self) -> usize {
        self.mappings.len()
    }

    pub fn mappings(&self) -> &[MappingRange] {
        &self.mappings
    }

    pub fn record_elf_backing(
        &mut self,
        start: u64,
        file: Arc<File>,
        file_offset: u64,
        file_len: u64,
        zero_tail: u64,
    ) {
        self.elf_backings.push(ElfBacking {
            start,
            file,
            file_offset,
            file_len,
            zero_tail,
        });
    }

    pub fn elf_backing(&self, start: u64) -> Option<&ElfBacking> {
        self.elf_backings
            .iter()
            .find(|backing| backing.start == start)
    }

    /// Total user pages mapped (PT_LOAD + stack). Test-visible.
    #[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
    pub fn total_pages(&self) -> u64 {
        self.mappings.iter().map(|m| m.page_count).sum()
    }

    /// Test-only: peek at a recorded mapping without consuming it.
    #[cfg(feature = "test")]
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn mapping(&self, idx: usize) -> Option<MappingRange> {
        self.mappings.get(idx).copied()
    }

    /// Transfer all page-table ownership to the surrounding AddressSpace.
    /// After this point Drop is metadata-only and cannot target the wrong CR3.
    pub fn transfer_mapping_ownership(&mut self) {
        self.dropped = true;
        self.mappings.clear();
        self.elf_backings.clear();
    }
}

impl Drop for UserImage {
    fn drop(&mut self) {
        // Production page-table ownership always belongs to AddressSpace.
        // The feature-test fallback keeps historical fixtures isolated: they
        // deliberately load into the kernel L4 without an AddressSpace.
        #[cfg(feature = "test")]
        self.drop_legacy_test_mappings();
    }
}

#[cfg(feature = "test")]
impl UserImage {
    fn drop_legacy_test_mappings(&mut self) {
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

        // Demand-grown stack: the initial commit lives outside `mappings`
        // so the ring-3 page-fault growth path doesn't need to push to a
        // heap-allocated Vec. Clean it up here when nobody else already
        // has (the runtime exit path clears `stack_initial_bottom` to
        // signal "Process already handled the stack range" so we don't
        // double-unmap).
        if self.stack_initial_bottom != 0 && self.stack_top.as_u64() > self.stack_initial_bottom {
            let page_count = (self.stack_top.as_u64() - self.stack_initial_bottom) / 0x1000;
            let res = crate::mm::memory::with_memory_mapper(|m| {
                m.unmap_user_region(x86_64::VirtAddr::new(self.stack_initial_bottom), page_count)
            });
            if let Some(Err(e)) = res {
                if !matches!(e, UserMapError::PageNotMapped) {
                    crate::debug_warn!("UserImage::drop: stack unmap failed: {:?}", e);
                }
            }
        }
    }
}
