use bootloader_api::info::MemoryRegions;
use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PhysFrame, Size4KiB,
        PageTableFlags, mapper::MapToError, Translate,
    },
    VirtAddr, PhysAddr,
};
use crate::{debug_info, debug_error};
use super::frame_allocator::BootInfoFrameAllocator;

// Global mapper for page fault handling
pub static mut MAPPER: Option<*mut MemoryMapper> = None;

pub unsafe fn get_mapper() -> Option<&'static mut MemoryMapper> {
    MAPPER.map(|ptr| &mut *ptr)
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
        self.mapper.translate_addr(addr)
    }
    
    pub fn handle_page_fault(&mut self, addr: VirtAddr) -> Result<(), MapToError<Size4KiB>> {
        debug_info!("Handling page fault for address: {:?}", addr);
        
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
                    debug_info!("Successfully mapped page {:?} to frame {:?}", page, frame);
                }
                Err(MapToError::PageAlreadyMapped(_)) => {
                    // Page is already mapped, that's fine
                    debug_info!("Page {:?} was already mapped", page);
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

unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}