//! Per-process L4 (PML4) page table.
//!
//! Phase 4 PR-B. Each ring-3 user process gets its own L4 frame. The
//! kernel half (every PML4 entry except [0]) is shared with the kernel
//! L4 by *copying the entries themselves* — the underlying L3/L2/L1
//! tables they point at are physically shared, so kernel heap pages
//! installed by the page-fault handler while one process is active are
//! visible to every other process and to the kernel itself.
//!
//! PML4[0] (the user-VA window 0..512 GiB, of which we use only the
//! first 1 GiB at `USER_VA_RANGE_START..USER_VA_RANGE_END`) is
//! exclusively per-process — that's the whole point.
//!
//! Lifecycle: `RunProcess::run_path` constructs an `AddressSpace`,
//! activates it (writes CR3) before calling `load_elf`, and switches
//! back to the kernel L4 after the user process exits.

use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::{PageTable, PageTableFlags, PhysFrame, Size4KiB};

use crate::mm::memory::with_memory_mapper;

/// Errors from `AddressSpace::new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    /// The frame allocator is out of physical frames.
    OutOfFrames,
    /// The mm subsystem isn't initialized (boot ordering bug).
    MapperUnavailable,
    /// `capture_kernel_l4` never ran (boot ordering bug).
    KernelL4Missing,
}

/// A per-process page-table root.
///
/// Owns one L4 frame. Dropping the value does not free the frame today
/// — the boot-time frame allocator is forward-only and matches the
/// existing leak behavior of `UserImage`. A future bitmap allocator
/// will plug this.
pub struct AddressSpace {
    l4_frame: PhysFrame<Size4KiB>,
}

impl AddressSpace {
    /// Allocate a fresh L4 frame and populate it with the kernel-shared
    /// PML4 entries (every entry except [0]). PML4[0] is left empty so
    /// the loader can install user mappings into it without colliding
    /// with anything from a previous process.
    pub fn new() -> Result<Self, AddressSpaceError> {
        let kernel_frame = crate::mm::paging::kernel_l4_frame()
            .ok_or(AddressSpaceError::KernelL4Missing)?;

        let result = with_memory_mapper(|mapper| {
            let phys_offset = mapper.physical_memory_offset().as_u64();
            let frame = mapper
                .allocate_one_frame()
                .ok_or(AddressSpaceError::OutOfFrames)?;

            let new_l4_va = phys_offset + frame.start_address().as_u64();
            let kernel_l4_va = phys_offset + kernel_frame.start_address().as_u64();

            // SAFETY: both frames are kernel-mapped through the
            // bootloader's offset region; the mapper just allocated the
            // new one so we hold the only reference; the kernel L4 is
            // live (we're running on it) but we only read 4 KiB out of
            // it as plain data and don't tear or reorder its entries.
            unsafe {
                let new_table = &mut *(new_l4_va as *mut PageTable);
                let kernel_table = &*(kernel_l4_va as *const PageTable);
                for i in 0..512 {
                    if i == 0 {
                        // PML4[0] = user half. Per-process; start empty.
                        new_table[i].set_unused();
                    } else {
                        // Share kernel-half entries by reference. The
                        // L3/L2/L1 tables they point at are physically
                        // shared with every other L4, so heap pages
                        // mapped by any process show up everywhere.
                        new_table[i] = kernel_table[i].clone();
                    }
                }
            }

            Ok(AddressSpace { l4_frame: frame })
        });

        match result {
            Some(Ok(aspace)) => Ok(aspace),
            Some(Err(e)) => Err(e),
            None => Err(AddressSpaceError::MapperUnavailable),
        }
    }

    /// Physical frame holding this address space's L4.
    pub fn l4_frame(&self) -> PhysFrame<Size4KiB> {
        self.l4_frame
    }

    /// Phase 4 PR-C: build a child address space that copies the
    /// parent's user-half (PML4[0]) by **eager copy** of every leaf
    /// page. Kernel-half PML4 entries are still shared by reference,
    /// so kernel heap/stack mappings remain visible to the child.
    ///
    /// Eager copy gives independent backing for the child's user pages,
    /// which is what fork() requires (writes by either side don't
    /// affect the other). A future copy-on-write optimization would
    /// share frames and mark them read-only until first write.
    ///
    /// Returns a fresh `AddressSpace` whose L4 has the child's own
    /// PML4[0] subtree. The parent's L4 is untouched.
    pub fn clone_for_child(parent_l4_frame: PhysFrame<Size4KiB>) -> Result<Self, AddressSpaceError> {
        // Build the child like a fresh AddressSpace — kernel half copied
        // from the kernel L4, PML4[0] empty.
        let child = Self::new()?;

        // Now eagerly copy the parent's PML4[0] subtree into the child.
        let result = with_memory_mapper(|mapper| {
            let phys_offset = mapper.physical_memory_offset().as_u64();
            let parent_l4_va = phys_offset + parent_l4_frame.start_address().as_u64();
            let child_l4_va = phys_offset + child.l4_frame.start_address().as_u64();

            // SAFETY: both L4s are kernel-mapped through the
            // bootloader's offset region. The parent's L4 is a live
            // address space (might be the active CR3 for the parent
            // process); we only read its PML4[0] subtree here, no
            // mutation. The child's L4 is freshly allocated; we own
            // the only reference and mutate freely.
            unsafe {
                let parent_table = &*(parent_l4_va as *const PageTable);
                let child_table = &mut *(child_l4_va as *mut PageTable);
                if !parent_table[0].is_unused() {
                    let parent_pdpt_pa = parent_table[0].addr().as_u64();
                    let parent_flags = parent_table[0].flags();
                    let child_pdpt_frame = mapper
                        .allocate_one_frame()
                        .ok_or(AddressSpaceError::OutOfFrames)?;
                    clone_pdpt(
                        mapper,
                        phys_offset,
                        parent_pdpt_pa,
                        child_pdpt_frame.start_address().as_u64(),
                    )?;
                    child_table[0].set_addr(
                        x86_64::PhysAddr::new(child_pdpt_frame.start_address().as_u64()),
                        parent_flags,
                    );
                }
            }
            Ok(())
        });

        match result {
            Some(Ok(())) => Ok(child),
            Some(Err(e)) => Err(e),
            None => Err(AddressSpaceError::MapperUnavailable),
        }
    }

    /// Switch CR3 to this address space. After this returns, the
    /// kernel is still fully mapped (via the shared upper PML4 entries)
    /// but PML4[0] now reflects this process's user half.
    ///
    /// SAFETY: the caller must guarantee that the code path that
    /// resumes after the CR3 write is itself reachable in this L4 —
    /// i.e., it lives in a kernel-half mapping that we copied from the
    /// kernel L4. Every code page in the kernel binary, the heap, and
    /// any kernel stack satisfies that.
    pub unsafe fn activate(&self) {
        Cr3::write(self.l4_frame, Cr3Flags::empty());
    }
}

/// Recursively clone a PDPT (L3) subtree from parent to child. Allocates
/// fresh frames for L2/L1 tables along the way and copies leaf data
/// pages so the child's writes don't aliasing the parent's memory.
unsafe fn clone_pdpt(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &*((phys_offset + parent_pa) as *const PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let p_pa = parent[i].addr().as_u64();
        let flags = parent[i].flags();
        let new_frame = mapper
            .allocate_one_frame()
            .ok_or(AddressSpaceError::OutOfFrames)?;
        clone_pd(mapper, phys_offset, p_pa, new_frame.start_address().as_u64())?;
        child[i].set_addr(x86_64::PhysAddr::new(new_frame.start_address().as_u64()), flags);
    }
    Ok(())
}

unsafe fn clone_pd(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &*((phys_offset + parent_pa) as *const PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let p_pa = parent[i].addr().as_u64();
        let flags = parent[i].flags();
        let new_frame = mapper
            .allocate_one_frame()
            .ok_or(AddressSpaceError::OutOfFrames)?;
        clone_pt(mapper, phys_offset, p_pa, new_frame.start_address().as_u64())?;
        child[i].set_addr(x86_64::PhysAddr::new(new_frame.start_address().as_u64()), flags);
    }
    Ok(())
}

unsafe fn clone_pt(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &*((phys_offset + parent_pa) as *const PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let p_pa = parent[i].addr().as_u64();
        let flags = parent[i].flags();
        // Allocate a fresh frame for the child's leaf and copy 4 KiB
        // of parent data. This is the eager-fork hot path; a future
        // copy-on-write swap would share the frame here and mark both
        // sides read-only.
        let new_frame = mapper
            .allocate_one_frame()
            .ok_or(AddressSpaceError::OutOfFrames)?;
        let src = (phys_offset + p_pa) as *const u8;
        let dst = (phys_offset + new_frame.start_address().as_u64()) as *mut u8;
        core::ptr::copy_nonoverlapping(src, dst, 0x1000);

        // Preserve the parent's leaf flags exactly. PRESENT, USER,
        // WRITABLE, NX all carry over so the child sees the same
        // permissions on each user page as the parent did.
        let _ = PageTableFlags::PRESENT;
        child[i].set_addr(x86_64::PhysAddr::new(new_frame.start_address().as_u64()), flags);
    }
    Ok(())
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // Safety net: if this AddressSpace is the currently active L4,
        // switch CR3 back to the kernel L4 before freeing/leaking the
        // frame. Otherwise an early-return error path could leave the
        // CPU running with CR3 pointing at a stale frame.
        let (current, _) = Cr3::read();
        if current == self.l4_frame {
            // SAFETY: every L4 we built shares the kernel half by
            // copying the kernel L4's PML4 entries, so the code after
            // this write is still mapped.
            unsafe { crate::mm::paging::activate_kernel_l4(); }
        }
        // Today the boot-time frame allocator is forward-only — the L4
        // frame leaks. A future bitmap-backed allocator will reclaim
        // it here, along with any L3/L2/L1 tables under PML4[0].
    }
}
