use super::frame_allocator::BootInfoFrameAllocator;
use crate::{debug_error, debug_info, debug_trace};
use alloc::vec::Vec;
use bootloader_api::info::MemoryRegions;
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, OffsetPageTable, Page, PageTable,
        PageTableFlags, PhysFrame, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

#[cfg(feature = "test")]
use x86_64::structures::paging::page_table::PageTableLevel;

/// Base virtual address where a static non-PIE user binary is loaded.
pub const USER_LOAD_BASE: u64 = 0x0000_0000_0040_0000;

/// Exclusive ceiling of canonical lower-half user virtual memory.
pub const USER_CANONICAL_END: u64 = 0x0000_8000_0000_0000;

/// Lower-half PML4 entries that remain kernel-owned in every address space.
pub const KERNEL_HEAP_PML4_SLOT: usize = 136;
pub const KERNEL_STACK_PML4_SLOT: usize = 170;

pub const fn is_kernel_reserved_slot(slot: usize) -> bool {
    slot == KERNEL_HEAP_PML4_SLOT || slot == KERNEL_STACK_PML4_SLOT
}

/// Top of the user stack (exclusive). The stack grows down from here.
pub const USER_STACK_TOP: u64 = 0x0000_7fff_ffff_f000;

/// Demand-grown stack — pages committed at process creation. The loader
/// maps exactly this many pages immediately below `USER_STACK_TOP`; the
/// rest is filled in on demand by the ring-3 page-fault handler (see
/// `lifecycle::try_grow_user_stack`).
///
/// 8 pages / 32 KiB matches the pre-zsh default. Common programs
/// amortize at most a few stack-grow faults at startup; zsh's
/// post-fork prep takes ~10-14, which the growth path handles
/// transparently.
pub const USER_STACK_INITIAL_PAGES: u64 = 8;

/// Demand-grown stack — hard per-process 64 MiB reservation.
/// Once a process has grown its stack by this many pages, any further
/// fault into the growth window is treated as overflow and the process
/// is terminated.
///
/// Only the initial eight pages are committed; the rest costs no physical
/// memory until faulted in.
pub const USER_STACK_MAX_GROWTH_PAGES: u64 = (64 * 1024 * 1024) / 0x1000;

/// Demand-grown stack — guard region between the highest mapped PT_LOAD
/// page and the deepest the stack may ever grow into. A Stack-Clash-
/// style write that steps past the current stack bottom must land in
/// unmapped territory and #PF, not silently corrupt a PT_LOAD page.
///
/// One MiB of unmapped space separates the stack reservation from lower
/// mappings.
pub const USER_STACK_GUARD_PAGES: u64 = (1024 * 1024) / 0x1000;

/// Preferred virtual address of the per-process TLS region.
///
/// Layout (x86-64 TLS variant II):
/// - `[USER_TLS_IMAGE_VA, USER_TLS_IMAGE_VA + 0x1000)` — TLS image (tdata + tbss)
/// - `[USER_TCB_VA, USER_TCB_VA + 0x1000)` — TCB. FS_BASE points at this page;
///   `%fs:0` returns the self-pointer recorded at offset 0.
///
/// Sized at 1 page each for the milestone — libstdc++'s static TLS image
/// (errno + `__cxa_eh_globals` slots, etc.) is well under 4 KiB. The
/// loader rejects with `TlsUnsupported` if `p_memsz` exceeds this.
///
/// The loader uses these deterministic addresses when free and relocates the
/// two-page block above PT_LOAD when a large executable occupies them.
pub const USER_TLS_IMAGE_VA: u64 = 0x0000_0000_0100_0000;
pub const USER_TCB_VA: u64 = 0x0000_0000_0100_1000;

/// Initial brk anchor. `brk(0)` returns this; subsequent `brk(addr)` calls
/// grow up to `addr`, mapping pages on demand. Sized so musl's mallocng
/// initial heap fits without colliding with the mmap arena above.
pub const USER_BRK_BASE: u64 = 0x0000_0000_0200_0000; // 32 MiB

/// Lower search bound for the per-process mmap arena. `mmap` searches free
/// VMA gaps top-down above this address, rounding lengths to page granularity.
/// The separation from the initial brk anchor leaves room for heap growth.
pub const USER_MMAP_BASE: u64 = 0x0000_0000_0300_0000; // 48 MiB

/// Compatibility aliases for the canonical user range. VMA validation also
/// rejects the two reserved lower-half kernel slots.
pub const USER_VA_RANGE_START: u64 = USER_LOAD_BASE;
pub const USER_VA_RANGE_END: u64 = USER_CANONICAL_END;

/// Permission profile applied to a user-mode mapping. Values are explicit
/// rather than packed flags so the loader cannot accidentally hand `WRITABLE`
/// to a `.text` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPerms {
    /// `.text` and the trampoline: present, executable, USER, no write.
    /// `NO_EXECUTE` is left clear; EFER.NXE is enabled during x86-64 init.
    ReadExecute,
    /// `.rodata`: present, USER, no write, NX.
    ReadOnly,
    /// `.data`, `.bss`, stack, GOT: present, writable, USER, NX.
    ReadWrite,
}

impl UserPerms {
    pub fn leaf_flags(self) -> PageTableFlags {
        let base = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        match self {
            UserPerms::ReadExecute => base,
            UserPerms::ReadOnly => base | PageTableFlags::NO_EXECUTE,
            UserPerms::ReadWrite => base | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE,
        }
    }
}

/// Errors from the user-mapping API. Distinct from `MapToError` so the loader
/// (U6) can wrap them in `LoaderError` without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserMapError {
    /// The supplied virtual range is not 4 KiB aligned, has zero pages, or
    /// crosses outside the user VA range (e.g. into the kernel heap).
    VaOutOfRange,
    /// `Mapper::map_to_with_table_flags` returned `PageAlreadyMapped`. The
    /// user-mapping API treats this as a hard error — the user range must be
    /// empty when load begins (D11; risk row "swallow PageAlreadyMapped").
    PageAlreadyMapped,
    /// Backing frame pool exhausted (either for a user page or for a parent
    /// page table the mapper had to allocate).
    OutOfFrames,
    /// `Mapper::unmap` returned `PageNotMapped`. The unmap API only frees what
    /// was previously mapped; callers should pass the same range they mapped.
    PageNotMapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowOutcome {
    NotCow,
    Upgraded,
    Copied,
    OutOfFrames,
}

impl<S: x86_64::structures::paging::PageSize> From<MapToError<S>> for UserMapError {
    fn from(err: MapToError<S>) -> Self {
        match err {
            MapToError::FrameAllocationFailed => UserMapError::OutOfFrames,
            MapToError::PageAlreadyMapped(_) => UserMapError::PageAlreadyMapped,
            MapToError::ParentEntryHugePage => UserMapError::PageAlreadyMapped,
        }
    }
}

// Global mapper for page fault handling
pub static mut MAPPER: Option<*mut MemoryMapper> = None;

pub(super) unsafe fn get_mapper() -> Option<&'static mut MemoryMapper> {
    MAPPER.map(|ptr| &mut *ptr)
}

/// Frame holding the kernel's L4 (PML4). Captured once at boot so user
/// processes can switch back to it on exit.
///
/// Phase 4 PR-B: each user process runs on its own L4 with the kernel
/// half (PML4 indices 1..512) shared by reference. When the user
/// process exits, we must switch CR3 back here before dropping the
/// per-process L4 frame.
pub static mut KERNEL_L4_FRAME: Option<PhysFrame<Size4KiB>> = None;

/// Capture the active CR3 value as the kernel's permanent L4. Called
/// exactly once at boot, after `MemoryMapper::new` has been built.
pub fn capture_kernel_l4() {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    unsafe {
        KERNEL_L4_FRAME = Some(frame);
    }
    debug_info!("captured kernel L4 frame: {:?}", frame);
}

/// Read-only accessor for the captured kernel L4 frame. Returns `None`
/// if `capture_kernel_l4` has not run.
pub fn kernel_l4_frame() -> Option<PhysFrame<Size4KiB>> {
    unsafe { KERNEL_L4_FRAME }
}

/// Switch CR3 to the kernel L4. Used after the user process exits and
/// before its per-process L4 frame is dropped.
///
/// SAFETY: `capture_kernel_l4` must have run, and CR3 must currently
/// point at a valid L4 sharing the kernel half with this one (so the
/// kernel-side code path that issues the write itself stays mapped
/// across the switch). All process L4s built by `AddressSpace::new`
/// satisfy that.
pub unsafe fn activate_kernel_l4() {
    use x86_64::registers::control::{Cr3, Cr3Flags};
    let frame = kernel_l4_frame().expect("kernel L4 not captured at boot");
    Cr3::write(frame, Cr3Flags::empty());
}

/// Build a fresh `OffsetPageTable` over whatever L4 is currently
/// active (per CR3). Used by `map_user_region` and `unmap_user_region`
/// so that, after a user process activates its `AddressSpace`,
/// mappings land in that process's per-process L4 instead of the
/// boot-time kernel L4 captured by the global `MemoryMapper`.
///
/// SAFETY: the caller must hold mutable access to the live page-table
/// state (i.e. they're the sole owner of the `MemoryMapper` borrow)
/// and the bootloader's physical-memory offset must already be set up
/// so the L4 frame is reachable through it.
pub unsafe fn active_offset_page_table(phys_offset: VirtAddr) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    let l4_va = phys_offset.as_u64() + frame.start_address().as_u64();
    let l4: &'static mut PageTable = &mut *(l4_va as *mut PageTable);
    OffsetPageTable::new(l4, phys_offset)
}

/// Translate a virtual address to a physical address
/// Returns None if the address is not mapped
pub fn translate_virt_to_phys(virt_addr: u64) -> Option<u64> {
    crate::mm::memory::with_memory_mapper(|mapper| {
        mapper
            .translate_addr(VirtAddr::new(virt_addr))
            .map(|phys| phys.as_u64())
    })
    .flatten()
}

pub struct MemoryMapper {
    mapper: OffsetPageTable<'static>,
    frame_allocator: BootInfoFrameAllocator,
    physical_memory_offset: VirtAddr,
}

impl MemoryMapper {
    pub unsafe fn new(
        physical_memory_offset: VirtAddr,
        memory_map: &'static MemoryRegions,
    ) -> Self {
        debug_info!(
            "Creating memory mapper with offset: {:?}",
            physical_memory_offset
        );

        let level_4_table = active_level_4_table(physical_memory_offset);
        let mapper = OffsetPageTable::new(level_4_table, physical_memory_offset);
        let frame_allocator = BootInfoFrameAllocator::init(memory_map, physical_memory_offset);

        Self {
            mapper,
            frame_allocator,
            physical_memory_offset,
        }
    }

    pub fn translate_addr(&self, addr: VirtAddr) -> Option<PhysAddr> {
        // Phase 4 PR-B: walk whatever L4 is active. The boot-time
        // `self.mapper` is bound to the kernel L4, but after a process
        // activates its `AddressSpace` user mappings live elsewhere —
        // and `translate_addr` is the lookup the loader uses to copy
        // file bytes into freshly-mapped user pages.
        let phys_offset = self.physical_memory_offset;
        let active_mapper = unsafe { active_offset_page_table(phys_offset) };
        active_mapper.translate_addr(addr)
    }

    /// Allocate a single physical frame from the boot-time pool. Used by
    /// `AddressSpace::new` to claim a fresh L4 frame for a user process.
    pub fn allocate_one_frame(&mut self) -> Option<PhysFrame> {
        self.frame_allocator.allocate_frame()
    }

    pub fn release_frame(&mut self, frame: PhysFrame<Size4KiB>) -> bool {
        self.frame_allocator.release_frame(frame).is_ok()
    }

    pub fn retain_frame(&mut self, frame: PhysFrame<Size4KiB>) -> bool {
        self.frame_allocator.retain_frame(frame).is_ok()
    }

    pub fn frame_refcount(&self, frame: PhysFrame<Size4KiB>) -> Option<u32> {
        self.frame_allocator.refcount(frame)
    }

    pub fn frame_stats(&self) -> super::frame_allocator::FrameStats {
        self.frame_allocator.stats()
    }

    fn table_ptr(&self, frame: PhysFrame<Size4KiB>) -> *mut PageTable {
        (self.physical_memory_offset.as_u64() + frame.start_address().as_u64()) as *mut PageTable
    }

    pub fn zero_frame(&self, frame: PhysFrame<Size4KiB>) {
        unsafe { core::ptr::write_bytes(self.table_ptr(frame) as *mut u8, 0, 0x1000) }
    }

    fn active_l4_frame(&self) -> PhysFrame<Size4KiB> {
        use x86_64::registers::control::Cr3;
        Cr3::read().0
    }

    fn leaf_entry_ptr(
        &self,
        l4_frame: PhysFrame<Size4KiB>,
        addr: VirtAddr,
    ) -> Option<*mut x86_64::structures::paging::page_table::PageTableEntry> {
        let indices = page_indices(addr);
        let mut table_frame = l4_frame;
        for level in 0..3 {
            let table = unsafe { &*self.table_ptr(table_frame) };
            let entry = &table[indices[level]];
            if entry.is_unused()
                || !entry.flags().contains(PageTableFlags::PRESENT)
                || entry.flags().contains(PageTableFlags::HUGE_PAGE)
            {
                return None;
            }
            table_frame = PhysFrame::containing_address(entry.addr());
        }
        let table = unsafe { &mut *self.table_ptr(table_frame) };
        Some(&mut table[indices[3]] as *mut _)
    }

    pub fn leaf_info(
        &self,
        l4_frame: PhysFrame<Size4KiB>,
        addr: VirtAddr,
    ) -> Option<(PhysFrame<Size4KiB>, PageTableFlags)> {
        let entry = unsafe { &*self.leaf_entry_ptr(l4_frame, addr)? };
        if entry.is_unused() || !entry.flags().contains(PageTableFlags::PRESENT) {
            return None;
        }
        Some((PhysFrame::containing_address(entry.addr()), entry.flags()))
    }

    pub fn set_leaf_flags(
        &mut self,
        l4_frame: PhysFrame<Size4KiB>,
        addr: VirtAddr,
        flags: PageTableFlags,
    ) -> Result<(), UserMapError> {
        let entry = unsafe {
            &mut *self
                .leaf_entry_ptr(l4_frame, addr)
                .ok_or(UserMapError::PageNotMapped)?
        };
        entry.set_flags(flags);
        if self.active_l4_frame() == l4_frame {
            x86_64::instructions::tlb::flush(addr);
        }
        Ok(())
    }

    pub fn resolve_cow(&mut self, l4_frame: PhysFrame<Size4KiB>, addr: VirtAddr) -> CowOutcome {
        let Some((old_frame, mut flags)) = self.leaf_info(l4_frame, addr) else {
            return CowOutcome::NotCow;
        };
        if !flags.contains(PageTableFlags::BIT_9) {
            return CowOutcome::NotCow;
        }
        flags.remove(PageTableFlags::BIT_9);
        flags.insert(PageTableFlags::WRITABLE);
        if self.frame_allocator.refcount(old_frame) == Some(1) {
            let _ = self.set_leaf_flags(l4_frame, addr, flags);
            return CowOutcome::Upgraded;
        }
        let Some(new_frame) = self.frame_allocator.allocate_frame() else {
            return CowOutcome::OutOfFrames;
        };
        unsafe {
            let source = (self.physical_memory_offset.as_u64() + old_frame.start_address().as_u64())
                as *const u8;
            let destination = (self.physical_memory_offset.as_u64()
                + new_frame.start_address().as_u64()) as *mut u8;
            core::ptr::copy_nonoverlapping(source, destination, 0x1000);
            let entry = &mut *self.leaf_entry_ptr(l4_frame, addr).unwrap();
            entry.set_addr(new_frame.start_address(), flags);
        }
        let _ = self.frame_allocator.release_frame(old_frame);
        if self.active_l4_frame() == l4_frame {
            x86_64::instructions::tlb::flush(addr);
        }
        CowOutcome::Copied
    }

    /// Install one fresh zeroed user leaf in the specified address space.
    pub fn map_zeroed_page_into(
        &mut self,
        l4_frame: PhysFrame<Size4KiB>,
        addr: VirtAddr,
        flags: PageTableFlags,
    ) -> Result<PhysFrame<Size4KiB>, UserMapError> {
        let page_addr = VirtAddr::new(addr.as_u64() & !0xfff);
        let page = Page::<Size4KiB>::containing_address(page_addr);
        let frame = self
            .frame_allocator
            .allocate_frame()
            .ok_or(UserMapError::OutOfFrames)?;
        self.zero_frame(frame);
        let l4 = unsafe { &mut *self.table_ptr(l4_frame) };
        let mut target = unsafe { OffsetPageTable::new(l4, self.physical_memory_offset) };
        let parent_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        let result = unsafe {
            target.map_to_with_table_flags(
                page,
                frame,
                flags,
                parent_flags,
                &mut self.frame_allocator,
            )
        };
        match result {
            Ok(flush) => {
                if self.active_l4_frame() == l4_frame {
                    flush.flush();
                } else {
                    flush.ignore();
                }
                Ok(frame)
            }
            Err(error) => {
                let _ = self.frame_allocator.release_frame(frame);
                self.prune_empty_path(l4_frame, page_addr);
                Err(UserMapError::from(error))
            }
        }
    }

    pub fn frame_bytes_mut(&mut self, frame: PhysFrame<Size4KiB>) -> &mut [u8; 0x1000] {
        unsafe {
            &mut *((self.physical_memory_offset.as_u64() + frame.start_address().as_u64())
                as *mut [u8; 0x1000])
        }
    }

    /// Unmap one present 4 KiB leaf from an arbitrary address space, release
    /// its frame reference, and prune empty page tables bottom-up.
    pub fn unmap_page_from(
        &mut self,
        l4_frame: PhysFrame<Size4KiB>,
        addr: VirtAddr,
    ) -> Result<PhysFrame<Size4KiB>, UserMapError> {
        let indices = page_indices(addr);
        let mut tables = [l4_frame; 4];
        for level in 0..3 {
            let table = unsafe { &*self.table_ptr(tables[level]) };
            let entry = &table[indices[level]];
            if entry.is_unused() || !entry.flags().contains(PageTableFlags::PRESENT) {
                return Err(UserMapError::PageNotMapped);
            }
            if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                return Err(UserMapError::PageAlreadyMapped);
            }
            tables[level + 1] = PhysFrame::containing_address(entry.addr());
        }

        let leaf_table = unsafe { &mut *self.table_ptr(tables[3]) };
        let leaf = &mut leaf_table[indices[3]];
        if leaf.is_unused() || !leaf.flags().contains(PageTableFlags::PRESENT) {
            return Err(UserMapError::PageNotMapped);
        }
        if leaf.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Err(UserMapError::PageAlreadyMapped);
        }
        let leaf_frame = PhysFrame::containing_address(leaf.addr());
        leaf.set_unused();
        let released = self.frame_allocator.release_frame(leaf_frame);
        debug_assert!(released.is_ok(), "user leaf must be allocator-owned");
        self.prune_empty_path(l4_frame, addr);
        if self.active_l4_frame() == l4_frame {
            x86_64::instructions::tlb::flush(addr);
        }
        Ok(leaf_frame)
    }

    fn prune_empty_path(&mut self, l4_frame: PhysFrame<Size4KiB>, addr: VirtAddr) {
        let indices = page_indices(addr);
        let mut tables = [l4_frame; 4];
        let mut depth = 1usize;
        while depth < 4 {
            let parent = unsafe { &*self.table_ptr(tables[depth - 1]) };
            let entry = &parent[indices[depth - 1]];
            if entry.is_unused()
                || !entry.flags().contains(PageTableFlags::PRESENT)
                || entry.flags().contains(PageTableFlags::HUGE_PAGE)
            {
                break;
            }
            tables[depth] = PhysFrame::containing_address(entry.addr());
            depth += 1;
        }

        while depth > 1 {
            let child_depth = depth - 1;
            let child = unsafe { &*self.table_ptr(tables[child_depth]) };
            if child.iter().any(|entry| !entry.is_unused()) {
                break;
            }
            let parent = unsafe { &mut *self.table_ptr(tables[child_depth - 1]) };
            parent[indices[child_depth - 1]].set_unused();
            let released = self.frame_allocator.release_frame(tables[child_depth]);
            debug_assert!(released.is_ok(), "user page table must be allocator-owned");
            depth -= 1;
        }
    }

    /// Destroy all user-owned lower-half subtrees and finally release the L4.
    pub fn destroy_user_address_space(&mut self, l4_frame: PhysFrame<Size4KiB>) {
        let l4 = unsafe { &mut *self.table_ptr(l4_frame) };
        for slot in 0..256 {
            if is_kernel_reserved_slot(slot) || l4[slot].is_unused() {
                continue;
            }
            assert!(
                !l4[slot].flags().contains(PageTableFlags::HUGE_PAGE),
                "huge entry in user-owned PML4 slot {}",
                slot
            );
            let child = PhysFrame::containing_address(l4[slot].addr());
            l4[slot].set_unused();
            unsafe { self.destroy_user_table(child, 3) };
        }
        let released = self.frame_allocator.release_frame(l4_frame);
        debug_assert!(released.is_ok(), "user L4 must be allocator-owned");
    }

    unsafe fn destroy_user_table(&mut self, frame: PhysFrame<Size4KiB>, level: u8) {
        let table = &mut *self.table_ptr(frame);
        for entry in table.iter_mut() {
            if entry.is_unused() || !entry.flags().contains(PageTableFlags::PRESENT) {
                entry.set_unused();
                continue;
            }
            assert!(
                !entry.flags().contains(PageTableFlags::HUGE_PAGE),
                "huge page in user-owned level {} table",
                level
            );
            let owned = PhysFrame::containing_address(entry.addr());
            entry.set_unused();
            if level == 1 {
                let released = self.frame_allocator.release_frame(owned);
                debug_assert!(released.is_ok(), "user leaf must be allocator-owned");
            } else {
                self.destroy_user_table(owned, level - 1);
            }
        }
        let released = self.frame_allocator.release_frame(frame);
        debug_assert!(released.is_ok(), "user page table must be allocator-owned");
    }

    /// Read-only accessor for the bootloader's physical-memory offset.
    /// Used when mapping a freshly-allocated frame through the
    /// kernel-visible alias to zero or copy into it.
    pub fn physical_memory_offset(&self) -> VirtAddr {
        self.physical_memory_offset
    }

    /// Test-only: allocate one physical frame from the live frame
    /// allocator. Used by `tests::memory::test_live_frame_allocator_throughput`
    /// to confirm the U1 cursor's O(1) claim against the real memory map.
    #[cfg(feature = "test")]
    pub fn allocate_test_frame(&mut self) -> Option<PhysFrame> {
        use x86_64::structures::paging::FrameAllocator;
        self.frame_allocator.allocate_frame()
    }

    #[cfg(feature = "test")]
    pub fn release_test_frame(&mut self, frame: PhysFrame) -> bool {
        self.frame_allocator.release_frame(frame).is_ok()
    }

    /// Test-only: total frames issued by the live frame allocator.
    #[cfg(feature = "test")]
    #[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
    pub fn frames_issued(&self) -> u64 {
        self.frame_allocator.frames_issued()
    }

    /// Map `num_pages` consecutive 4 KiB pages starting at `virt_start` into
    /// the user-accessible address space. Allocates fresh frames, zeroes them,
    /// and uses `map_to_with_table_flags(parent_flags = PRESENT | WRITABLE |
    /// USER_ACCESSIBLE)` so the USER bit is propagated to every parent entry
    /// on the path (PML4 -> PDPT -> PD -> PT) — even ones that already exist
    /// because of an earlier kernel mapping (D11). `PageAlreadyMapped` is
    /// returned as a hard error rather than swallowed: a clash with an
    /// existing kernel mapping is a real bug, not a no-op.
    ///
    /// This path does **not** go through `handle_page_fault`. The fault
    /// handler in U2 already short-circuits on CPL=3 before reaching its
    /// auto-map branch; user faults are lifecycle events, not lazy mappings.
    pub fn map_user_region(
        &mut self,
        virt_start: VirtAddr,
        num_pages: u64,
        perms: UserPerms,
    ) -> Result<Vec<PhysFrame>, UserMapError> {
        validate_user_range(virt_start, num_pages)?;

        let leaf_flags = perms.leaf_flags();
        // Parent flags are uniform across all permission profiles: a parent
        // table may need to host both R-X and R-W leaves on different paths.
        let parent_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

        let phys_offset = self.physical_memory_offset;
        let mut frames: Vec<PhysFrame> = Vec::with_capacity(num_pages as usize);

        // Phase 4 PR-B: build the OffsetPageTable from the *current* CR3
        // for each call so that after a process activates its
        // AddressSpace, mappings land in that process's L4 rather than
        // the boot-time kernel L4 the global mapper was constructed
        // with.
        let mut active_mapper = unsafe { active_offset_page_table(phys_offset) };

        for i in 0..num_pages {
            let page_addr = VirtAddr::new(virt_start.as_u64() + i * 0x1000);
            let page = Page::<Size4KiB>::containing_address(page_addr);

            let Some(frame) = self.frame_allocator.allocate_frame() else {
                let l4 = self.active_l4_frame();
                for installed in (0..frames.len()).rev() {
                    let va = VirtAddr::new(virt_start.as_u64() + installed as u64 * 0x1000);
                    let _ = self.unmap_page_from(l4, va);
                }
                return Err(UserMapError::OutOfFrames);
            };

            // Zero the freshly allocated frame so user code never observes
            // stale data. The bootloader's offset mapping lets us reach
            // physical memory directly.
            unsafe {
                let virt = phys_offset.as_u64() + frame.start_address().as_u64();
                core::ptr::write_bytes(virt as *mut u8, 0u8, 0x1000);
            }

            let result = unsafe {
                active_mapper.map_to_with_table_flags(
                    page,
                    frame,
                    leaf_flags,
                    parent_flags,
                    &mut self.frame_allocator,
                )
            };
            match result {
                Ok(flush) => flush.flush(),
                Err(error) => {
                    let _ = self.frame_allocator.release_frame(frame);
                    let l4 = self.active_l4_frame();
                    self.prune_empty_path(l4, page_addr);
                    for installed in (0..frames.len()).rev() {
                        let va = VirtAddr::new(virt_start.as_u64() + installed as u64 * 0x1000);
                        let _ = self.unmap_page_from(l4, va);
                    }
                    return Err(UserMapError::from(error));
                }
            }

            frames.push(frame);
        }

        debug_info!(
            "map_user_region: {} pages at {:?} ({:?})",
            num_pages,
            virt_start,
            perms
        );
        Ok(frames)
    }

    /// Unmap a previously mapped user region. Returns the physical frames
    /// that backed the leaf pages so the caller can verify (in tests) or hand
    /// them off to a future per-process frame reclaimer. The bump frame
    /// allocator does not actually return frames to a pool; the returned list
    /// is the loader's transactional-teardown handle (U6).
    pub fn unmap_user_region(
        &mut self,
        virt_start: VirtAddr,
        num_pages: u64,
    ) -> Result<Vec<PhysFrame>, UserMapError> {
        validate_user_range(virt_start, num_pages)?;

        let mut frames: Vec<PhysFrame> = Vec::with_capacity(num_pages as usize);
        let l4 = self.active_l4_frame();

        for i in 0..num_pages {
            let page_addr = VirtAddr::new(virt_start.as_u64() + i * 0x1000);
            frames.push(self.unmap_page_from(l4, page_addr)?);
        }
        Ok(frames)
    }

    /// Walk the page-table hierarchy for `addr` and confirm every parent
    /// entry on the path (PML4 -> PDPT -> PD -> PT) has `USER_ACCESSIBLE`
    /// set. Test-only helper; returns `false` if any level is not mapped or
    /// is missing the U bit. Hidden behind `cfg(feature = "test")` so the
    /// release kernel does not carry it.
    #[cfg(feature = "test")]
    pub fn user_bit_set_on_all_parents(&self, addr: VirtAddr) -> bool {
        use x86_64::registers::control::Cr3;

        let phys_offset = self.physical_memory_offset.as_u64();
        let (l4_frame, _) = Cr3::read();
        let mut table_phys = l4_frame.start_address().as_u64();

        for level in [
            PageTableLevel::Four,
            PageTableLevel::Three,
            PageTableLevel::Two,
            PageTableLevel::One,
        ] {
            let table_virt = phys_offset + table_phys;
            let table = unsafe { &*(table_virt as *const PageTable) };
            let idx = match level {
                PageTableLevel::Four => (addr.as_u64() >> 39) & 0x1FF,
                PageTableLevel::Three => (addr.as_u64() >> 30) & 0x1FF,
                PageTableLevel::Two => (addr.as_u64() >> 21) & 0x1FF,
                PageTableLevel::One => (addr.as_u64() >> 12) & 0x1FF,
            } as usize;
            let entry = &table[idx];
            if entry.is_unused() {
                return false;
            }
            if !entry.flags().contains(PageTableFlags::USER_ACCESSIBLE) {
                return false;
            }
            if matches!(level, PageTableLevel::One) {
                break;
            }
            table_phys = entry.addr().as_u64();
        }
        true
    }

    pub fn handle_page_fault(&mut self, addr: VirtAddr) -> Result<(), MapToError<Size4KiB>> {
        // Hot path: per-fault diagnostic logs are at trace level so the
        // default boot doesn't spend UART vmexits on routine demand-paging.
        // See plan U2 (docs/plans/2026-05-09-002-perf-frame-allocator-and-page-fault-hot-path-plan.md).
        // The opening `>>> PAGE FAULT at ...` line in the IDT handler stays
        // at info — that one line is what a debugger needs to see.
        debug_trace!("Handling page fault for address: {:?}", addr);

        let page = Page::containing_address(addr);

        // Don't check if page is already mapped - that check itself might cause a page fault!
        // Just try to map it and handle any errors

        // Check if this is a physical memory region access
        // Use the actual physical memory offset from our mapper
        let phys_mem_offset = self.physical_memory_offset.as_u64();
        let allocator_owned =
            !(addr.as_u64() >= phys_mem_offset && addr.as_u64() < phys_mem_offset + (1u64 << 40));
        let frame = if !allocator_owned {
            // For physical memory region, map to the corresponding physical frame
            let phys_addr = addr.as_u64() - phys_mem_offset;
            PhysFrame::containing_address(PhysAddr::new(phys_addr))
        } else {
            // For other regions (like heap), allocate a new frame
            self.frame_allocator
                .allocate_frame()
                .ok_or(MapToError::FrameAllocationFailed)?
        };

        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

        unsafe {
            match self
                .mapper
                .map_to(page, frame, flags, &mut self.frame_allocator)
            {
                Ok(flush) => {
                    flush.flush();
                    debug_trace!("Successfully mapped page {:?} to frame {:?}", page, frame);
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // Page is already mapped, that's fine
                    debug_trace!("Page {:?} was already mapped", page);
                    if allocator_owned {
                        let _ = self.frame_allocator.release_frame(frame);
                    }
                    return Ok(());
                }
                Err(e) => {
                    if allocator_owned {
                        let _ = self.frame_allocator.release_frame(frame);
                    }
                    debug_error!(
                        "Failed to map page {:?} to frame {:?}: {:?}",
                        page,
                        frame,
                        e
                    );
                    return Err(e);
                }
            }
        }

        Ok(())
    }
}

/// Reject ranges that are misaligned, empty, or that would land outside the
/// designated user VA range — including (defensively) any range that could
/// touch the kernel heap at `0x_4444_4444_0000` or process stacks at
/// `0x_5555_0000_0000`. The check is purely on virtual addresses; physical
/// frames are allocated only after this has returned `Ok`.
fn validate_user_range(virt_start: VirtAddr, num_pages: u64) -> Result<(), UserMapError> {
    if num_pages == 0 {
        return Err(UserMapError::VaOutOfRange);
    }
    if virt_start.as_u64() & 0xFFF != 0 {
        return Err(UserMapError::VaOutOfRange);
    }
    let end = virt_start
        .as_u64()
        .checked_add(
            num_pages
                .checked_mul(0x1000)
                .ok_or(UserMapError::VaOutOfRange)?,
        )
        .ok_or(UserMapError::VaOutOfRange)?;

    if virt_start.as_u64() < USER_VA_RANGE_START || end > USER_VA_RANGE_END {
        return Err(UserMapError::VaOutOfRange);
    }
    let first_slot = (virt_start.as_u64() >> 39) as usize;
    let last_slot = ((end - 1) >> 39) as usize;
    if (first_slot..=last_slot).any(is_kernel_reserved_slot) {
        return Err(UserMapError::VaOutOfRange);
    }
    Ok(())
}

fn page_indices(addr: VirtAddr) -> [usize; 4] {
    let value = addr.as_u64();
    [
        ((value >> 39) & 0x1ff) as usize,
        ((value >> 30) & 0x1ff) as usize,
        ((value >> 21) & 0x1ff) as usize,
        ((value >> 12) & 0x1ff) as usize,
    ]
}

unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}
