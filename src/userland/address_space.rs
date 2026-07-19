//! Per-process L4 (PML4) page table.
//!
//! Each ring-3 process gets its own lower-canonical page-table trees.
//! Upper-half entries and the two explicitly reserved lower-half kernel
//! slots are copied from the kernel L4; every other lower-half slot is
//! private and initially empty.
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
/// Owns the root and every user page-table subtree reachable from it.
pub struct AddressSpace {
    l4_frame: PhysFrame<Size4KiB>,
    vmas: crate::userland::vm::VmaSet,
    vma_generation: u64,
    shadow_generation: u64,
}

impl AddressSpace {
    /// Allocate a fresh L4 and copy only shared kernel-owned slots.
    pub fn new() -> Result<Self, AddressSpaceError> {
        let kernel_frame =
            crate::mm::paging::kernel_l4_frame().ok_or(AddressSpaceError::KernelL4Missing)?;

        let result = with_memory_mapper(|mapper| {
            let phys_offset = mapper.physical_memory_offset().as_u64();
            let frame = mapper
                .allocate_root_frame()
                .ok_or(AddressSpaceError::OutOfFrames)?;
            mapper.zero_frame(frame);

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
                    if i >= 256 || crate::mm::paging::is_kernel_reserved_slot(i) {
                        new_table[i] = kernel_table[i].clone();
                    } else {
                        new_table[i].set_unused();
                    }
                }
            }

            let shadow_generation =
                crate::diagnostics::shadow::address_space::allocate(frame.start_address().as_u64());
            Ok(AddressSpace {
                l4_frame: frame,
                vmas: crate::userland::vm::VmaSet::new(),
                vma_generation: 1,
                shadow_generation,
            })
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

    pub fn vmas(&self) -> &crate::userland::vm::VmaSet {
        &self.vmas
    }

    pub fn vmas_mut(&mut self) -> &mut crate::userland::vm::VmaSet {
        self.vma_generation = self.vma_generation.wrapping_add(1).max(1);
        crate::diagnostics::shadow::address_space::update_vma_generation(
            self.shadow_generation,
            self.vma_generation,
        );
        &mut self.vmas
    }

    pub fn vma_generation(&self) -> u64 {
        self.vma_generation
    }

    pub fn shadow_generation(&self) -> u64 {
        self.shadow_generation
    }

    pub fn publish_owner(&mut self, tgid: u32) {
        crate::diagnostics::shadow::address_space::publish_owner(
            self.shadow_generation,
            tgid,
            self.vma_generation,
        );
        let _ = with_memory_mapper(|mapper| mapper.audit_user_address_space(self.l4_frame));
    }

    pub fn initialize_vmas_from_image(
        &mut self,
        image: &crate::userland::image::UserImage,
    ) -> Result<(), crate::userland::vm::VmError> {
        use crate::mm::paging::UserPerms;
        use crate::userland::vm::{VmProt, Vma, VmaBacking};

        self.vmas = crate::userland::vm::VmaSet::new();
        self.vma_generation = self.vma_generation.wrapping_add(1).max(1);
        crate::diagnostics::shadow::address_space::update_vma_generation(
            self.shadow_generation,
            self.vma_generation,
        );
        for mapping in image.mappings() {
            let prot = match mapping.perms {
                UserPerms::ReadExecute => VmProt::READ.union(VmProt::EXEC),
                UserPerms::ReadOnly => VmProt::READ,
                UserPerms::ReadWrite => VmProt::READ.union(VmProt::WRITE),
            };
            let start = mapping.virt_start.as_u64();
            let end = start + mapping.page_count * 0x1000;
            let backing = if let Some(elf) = image.elf_backing(start) {
                VmaBacking::Elf {
                    file: elf.file.clone(),
                    file_offset: elf.file_offset,
                    file_len: elf.file_len,
                    zero_tail: elf.zero_tail,
                }
            } else if image
                .tls_fs_base
                .is_some_and(|tcb| start == tcb.as_u64() || end == tcb.as_u64())
            {
                VmaBacking::Tls
            } else {
                VmaBacking::ElfResident
            };
            self.vmas.insert(Vma::new(start, end, prot, backing)?)?;
        }

        self.vmas.insert(Vma::new(
            image.stack_max_growth_floor,
            image.stack_top.as_u64(),
            VmProt::READ.union(VmProt::WRITE),
            VmaBacking::Stack {
                floor: image.stack_max_growth_floor,
                guard_bytes: crate::mm::paging::USER_STACK_GUARD_PAGES * 0x1000,
            },
        )?)?;
        Ok(())
    }

    /// Clone page-table structure while sharing resident leaves. Writable
    /// private leaves become read-only COW mappings in both processes;
    /// nonresident VMAs allocate no leaves.
    pub fn clone_for_child(
        parent_l4_frame: PhysFrame<Size4KiB>,
    ) -> Result<Self, AddressSpaceError> {
        // Build the child like a fresh AddressSpace — kernel half copied
        // from the kernel L4, PML4[0] empty.
        let child = Self::new()?;

        // Clone every user-owned lower-half subtree using shared leaves.
        let result = with_memory_mapper(|mapper| {
            let phys_offset = mapper.physical_memory_offset().as_u64();
            let parent_l4_va = phys_offset + parent_l4_frame.start_address().as_u64();
            let child_l4_va = phys_offset + child.l4_frame.start_address().as_u64();
            let parent_generation = crate::diagnostics::shadow::address_space::generation_for_l4(
                parent_l4_frame.start_address().as_u64(),
            );
            let child_generation = child.shadow_generation;

            // SAFETY: both L4s are kernel-mapped through the
            // bootloader's offset region. The parent's L4 is a live
            // address space (might be the active CR3 for the parent
            // process); we only read its PML4[0] subtree here, no
            // mutation. The child's L4 is freshly allocated; we own
            // the only reference and mutate freely.
            unsafe {
                let parent_table = &mut *(parent_l4_va as *mut PageTable);
                let child_table = &mut *(child_l4_va as *mut PageTable);
                for slot in 0..256 {
                    if crate::mm::paging::is_kernel_reserved_slot(slot)
                        || parent_table[slot].is_unused()
                    {
                        continue;
                    }
                    let parent_pdpt_pa = parent_table[slot].addr().as_u64();
                    let parent_flags = parent_table[slot].flags();
                    let child_pdpt_frame = mapper
                        .allocate_page_table_frame()
                        .ok_or(AddressSpaceError::OutOfFrames)?;
                    mapper.zero_frame(child_pdpt_frame);
                    child_table[slot].set_addr(child_pdpt_frame.start_address(), parent_flags);
                    clone_pdpt(
                        mapper,
                        phys_offset,
                        parent_pdpt_pa,
                        child_pdpt_frame.start_address().as_u64(),
                        parent_generation,
                        child_generation,
                        (slot as u64) << 39,
                    )?;
                }
                x86_64::instructions::tlb::flush_all();
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
        crate::diagnostics::shadow::address_space::activate(
            self.shadow_generation,
            self.l4_frame.start_address().as_u64(),
        );
    }
}

/// Recursively clone page-table structure while sharing leaf frames.
unsafe fn clone_pdpt(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
    parent_generation: u64,
    child_generation: u64,
    virtual_base: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &mut *((phys_offset + parent_pa) as *mut PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let p_pa = parent[i].addr().as_u64();
        let flags = parent[i].flags();
        let new_frame = mapper
            .allocate_page_table_frame()
            .ok_or(AddressSpaceError::OutOfFrames)?;
        mapper.zero_frame(new_frame);
        child[i].set_addr(new_frame.start_address(), flags);
        clone_pd(
            mapper,
            phys_offset,
            p_pa,
            new_frame.start_address().as_u64(),
            parent_generation,
            child_generation,
            virtual_base | ((i as u64) << 30),
        )?;
    }
    Ok(())
}

unsafe fn clone_pd(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
    parent_generation: u64,
    child_generation: u64,
    virtual_base: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &mut *((phys_offset + parent_pa) as *mut PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let p_pa = parent[i].addr().as_u64();
        let flags = parent[i].flags();
        let new_frame = mapper
            .allocate_page_table_frame()
            .ok_or(AddressSpaceError::OutOfFrames)?;
        mapper.zero_frame(new_frame);
        child[i].set_addr(new_frame.start_address(), flags);
        clone_pt(
            mapper,
            phys_offset,
            p_pa,
            new_frame.start_address().as_u64(),
            parent_generation,
            child_generation,
            virtual_base | ((i as u64) << 21),
        )?;
    }
    Ok(())
}

unsafe fn clone_pt(
    mapper: &mut crate::mm::paging::MemoryMapper,
    phys_offset: u64,
    parent_pa: u64,
    child_pa: u64,
    parent_generation: u64,
    child_generation: u64,
    virtual_base: u64,
) -> Result<(), AddressSpaceError> {
    let parent = &mut *((phys_offset + parent_pa) as *mut PageTable);
    let child = &mut *((phys_offset + child_pa) as *mut PageTable);
    for i in 0..512 {
        if parent[i].is_unused() {
            continue;
        }
        let frame = PhysFrame::containing_address(parent[i].addr());
        if !mapper.retain_leaf_frame(frame) {
            return Err(AddressSpaceError::OutOfFrames);
        }
        let mut flags = parent[i].flags();
        if flags.contains(PageTableFlags::WRITABLE) {
            flags.remove(PageTableFlags::WRITABLE);
            flags.insert(PageTableFlags::BIT_9);
            parent[i].set_flags(flags);
        }
        child[i].set_addr(frame.start_address(), flags);
        let virtual_page = virtual_base | ((i as u64) << 12);
        crate::diagnostics::shadow::memory::update_leaf_flags(
            parent_generation,
            virtual_page,
            flags.bits(),
        );
        if let Some((frame_index, _)) = mapper.shadow_frame_identity(frame) {
            crate::diagnostics::shadow::memory::map_leaf(
                child_generation,
                virtual_page,
                frame.start_address().as_u64(),
                frame_index,
                flags.bits(),
            );
        }
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
            unsafe {
                crate::mm::paging::activate_kernel_l4();
            }
        }
        crate::diagnostics::shadow::address_space::begin_destroy(self.shadow_generation);
        let destroyed = with_memory_mapper(|mapper| {
            let _ = mapper.audit_user_address_space(self.l4_frame);
            mapper.destroy_user_address_space(self.l4_frame);
        });
        debug_assert!(
            destroyed.is_some(),
            "memory mapper unavailable during address-space drop"
        );
        crate::diagnostics::shadow::address_space::release(self.shadow_generation);
    }
}
