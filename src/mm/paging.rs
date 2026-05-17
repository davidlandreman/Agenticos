use alloc::vec::Vec;
use bootloader_api::info::MemoryRegions;
use x86_64::{
    structures::paging::{
        page_table::PageTableLevel,
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PhysFrame, Size4KiB,
        PageTableFlags, mapper::MapToError, mapper::UnmapError, Translate,
    },
    VirtAddr, PhysAddr,
};
use crate::{debug_info, debug_error, debug_trace};
use super::frame_allocator::BootInfoFrameAllocator;

/// Base virtual address where a static non-PIE user binary is loaded.
pub const USER_LOAD_BASE: u64 = 0x0000_0000_0040_0000;

/// Top of the user stack (exclusive). The stack grows down from here.
pub const USER_STACK_TOP: u64 = 0x0000_0000_0080_0000;

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

/// Demand-grown stack — hard per-process cap on total mapped pages.
/// Once a process has grown its stack by this many pages, any further
/// fault into the growth window is treated as overflow and the process
/// is terminated.
///
/// 768 pages / 3 MiB leaves ~1 MiB of the 4 MiB user-VA slice
/// (`USER_LOAD_BASE..USER_STACK_TOP`) for the binary's text/data/bss
/// plus the 64 KiB code-vs-stack guard. The plan calls for measuring
/// real stack peaks on zsh / BusyBox / future ports before treating
/// this constant as settled; revisit if any real workload approaches
/// the cap.
pub const USER_STACK_MAX_GROWTH_PAGES: u64 = 768;

/// Demand-grown stack — guard region between the highest mapped PT_LOAD
/// page and the deepest the stack may ever grow into. A Stack-Clash-
/// style write that steps past the current stack bottom must land in
/// unmapped territory and #PF, not silently corrupt a PT_LOAD page.
///
/// 16 pages / 64 KiB is the minimum-viable bar — Linux post-CVE-
/// 2017-1000364 defaults to a 1 MiB `STACK_GUARD_GAP`, but our 4 MiB
/// user-VA slice can't accommodate that today. Revisit when untrusted
/// binaries become a real concern, or when a VA repartition gives the
/// stack more headroom.
pub const USER_STACK_GUARD_PAGES: u64 = 16;

/// Base virtual address of the per-process TLS region.
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
/// Sits above the user stack (top 0x80_0000) and well below USER_VA_RANGE_END
/// at 0x4000_0000, leaving room above for future brk and mmap arenas.
pub const USER_TLS_IMAGE_VA: u64 = 0x0000_0000_0100_0000;
pub const USER_TCB_VA: u64 = 0x0000_0000_0100_1000;

/// Initial brk anchor. `brk(0)` returns this; subsequent `brk(addr)` calls
/// grow up to `addr`, mapping pages on demand. Sized so musl's mallocng
/// initial heap fits without colliding with the mmap arena above.
pub const USER_BRK_BASE: u64 = 0x0000_0000_0200_0000; // 32 MiB

/// Base of the per-process mmap arena. Anonymous-only `mmap` calls bump
/// upward from this address, allocating `len` rounded up to page granularity
/// per call. Reaches `USER_VA_RANGE_END` at 1 GiB; the gap between the brk
/// arena and here is ~16 MiB which is plenty for the milestone heap.
pub const USER_MMAP_BASE: u64 = 0x0000_0000_0300_0000; // 48 MiB

/// Inclusive lower / exclusive upper bounds of the user-VA range. Anything
/// outside is reserved for the kernel and `map_user_region` rejects it.
///
/// Sized to host a multi-MiB libstdc++ static binary plus its TLS block,
/// brk arena, mmap arena, and stack. The 1 GiB ceiling at 0x4000_0000 is
/// the U4+U5 expansion from the original ~9 MiB window — well below any
/// kernel-side VA region. Sub-region constants for the TLS block, brk
/// anchor, and mmap arena are introduced by U7 / U9 as those features
/// land; for now only the load base and stack top are pinned.
pub const USER_VA_RANGE_START: u64 = USER_LOAD_BASE;
pub const USER_VA_RANGE_END: u64 = 0x0000_0000_4000_0000;

/// Permission profile applied to a user-mode mapping. Values are explicit
/// rather than packed flags so the loader cannot accidentally hand `WRITABLE`
/// to a `.text` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPerms {
    /// `.text` and the trampoline: present, executable, USER, no write.
    /// `NO_EXECUTE` is left clear here regardless of the segment permissions
    /// of `.rodata` etc., per D11 — `EFER.NXE` is not yet enabled, so the
    /// bit is documentary today.
    ReadExecute,
    /// `.rodata`: present, USER, no write, NX.
    ReadOnly,
    /// `.data`, `.bss`, stack, GOT: present, writable, USER, NX.
    ReadWrite,
}

impl UserPerms {
    fn leaf_flags(self) -> PageTableFlags {
        let base = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        match self {
            UserPerms::ReadExecute => base,
            UserPerms::ReadOnly => base | PageTableFlags::NO_EXECUTE,
            UserPerms::ReadWrite => {
                base | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE
            }
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

pub unsafe fn get_mapper() -> Option<&'static mut MemoryMapper> {
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
    unsafe { KERNEL_L4_FRAME = Some(frame); }
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
pub unsafe fn active_offset_page_table(
    phys_offset: VirtAddr,
) -> OffsetPageTable<'static> {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    let l4_va = phys_offset.as_u64() + frame.start_address().as_u64();
    let l4: &'static mut PageTable = &mut *(l4_va as *mut PageTable);
    OffsetPageTable::new(l4, phys_offset)
}

/// Translate a virtual address to a physical address
/// Returns None if the address is not mapped
pub fn translate_virt_to_phys(virt_addr: u64) -> Option<u64> {
    unsafe {
        get_mapper().and_then(|mapper| {
            mapper.translate_addr(VirtAddr::new(virt_addr))
                .map(|phys| phys.as_u64())
        })
    }
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
        debug_info!("Creating memory mapper with offset: {:?}", physical_memory_offset);
        
        let level_4_table = active_level_4_table(physical_memory_offset);
        let mapper = OffsetPageTable::new(level_4_table, physical_memory_offset);
        let frame_allocator = BootInfoFrameAllocator::init(memory_map);
        
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

    /// Read-only accessor for the bootloader's physical-memory offset.
    /// Used when mapping a freshly-allocated frame through the
    /// kernel-visible alias to zero or copy into it.
    pub fn physical_memory_offset(&self) -> VirtAddr {
        self.physical_memory_offset
    }

    /// Test-only: allocate one physical frame from the live frame
    /// allocator. Used by `tests::memory::test_live_frame_allocator_throughput`
    /// to confirm the U1 cursor's O(1) claim against the real memory map.
    /// Each call permanently consumes a frame for the rest of the test run.
    #[cfg(feature = "test")]
    pub fn allocate_test_frame(&mut self) -> Option<PhysFrame> {
        use x86_64::structures::paging::FrameAllocator;
        self.frame_allocator.allocate_frame()
    }

    /// Test-only: total frames issued by the live frame allocator.
    #[cfg(feature = "test")]
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
        let parent_flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE;

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

            let frame = self
                .frame_allocator
                .allocate_frame()
                .ok_or(UserMapError::OutOfFrames)?;

            // Zero the freshly allocated frame so user code never observes
            // stale data. The bootloader's offset mapping lets us reach
            // physical memory directly.
            unsafe {
                let virt = phys_offset.as_u64() + frame.start_address().as_u64();
                core::ptr::write_bytes(virt as *mut u8, 0u8, 0x1000);
            }

            unsafe {
                active_mapper
                    .map_to_with_table_flags(
                        page,
                        frame,
                        leaf_flags,
                        parent_flags,
                        &mut self.frame_allocator,
                    )
                    .map_err(UserMapError::from)?
                    .flush();
            }

            frames.push(frame);
        }

        debug_info!(
            "map_user_region: {} pages at {:?} ({:?})",
            num_pages, virt_start, perms
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

        let phys_offset = self.physical_memory_offset;
        let mut frames: Vec<PhysFrame> = Vec::with_capacity(num_pages as usize);

        // Phase 4 PR-B: same dance as `map_user_region` — operate on
        // whatever L4 is currently active, which is the per-process
        // address space during ring-3 lifetime.
        let mut active_mapper = unsafe { active_offset_page_table(phys_offset) };

        for i in 0..num_pages {
            let page_addr = VirtAddr::new(virt_start.as_u64() + i * 0x1000);
            let page = Page::<Size4KiB>::containing_address(page_addr);

            match active_mapper.unmap(page) {
                Ok((frame, flush)) => {
                    flush.flush();
                    frames.push(frame);
                }
                Err(UnmapError::PageNotMapped) => return Err(UserMapError::PageNotMapped),
                Err(e) => {
                    debug_error!("unmap_user_region: unexpected error {:?}", e);
                    return Err(UserMapError::PageNotMapped);
                }
            }
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
        let frame = if addr.as_u64() >= phys_mem_offset && addr.as_u64() < phys_mem_offset + (1u64 << 40) {
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
            match self.mapper.map_to(page, frame, flags, &mut self.frame_allocator) {
                Ok(flush) => {
                    flush.flush();
                    debug_trace!("Successfully mapped page {:?} to frame {:?}", page, frame);
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // Page is already mapped, that's fine
                    debug_trace!("Page {:?} was already mapped", page);
                    return Ok(());
                }
                Err(e) => {
                    debug_error!("Failed to map page {:?} to frame {:?}: {:?}", page, frame, e);
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
        .checked_add(num_pages.checked_mul(0x1000).ok_or(UserMapError::VaOutOfRange)?)
        .ok_or(UserMapError::VaOutOfRange)?;

    if virt_start.as_u64() < USER_VA_RANGE_START || end > USER_VA_RANGE_END {
        return Err(UserMapError::VaOutOfRange);
    }
    Ok(())
}

unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}