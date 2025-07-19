use bootloader_api::info::{MemoryRegions, MemoryRegionKind};
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;
use crate::{debug_info, debug_debug};

pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryRegions,
    next: usize,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static MemoryRegions) -> Self {
        debug_info!("Initializing frame allocator");
        BootInfoFrameAllocator {
            memory_map,
            next: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        let regions = self.memory_map.iter();
        let usable_regions = regions
            .filter(|r| r.kind == MemoryRegionKind::Usable);
        let addr_ranges = usable_regions
            .map(|r| {
                debug_debug!("Usable region: 0x{:x} - 0x{:x}", r.start, r.end);
                r.start..r.end
            });
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        // Skip the zero frame as it's not valid
        frame_addresses
            .filter(|&addr| addr != 0)
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        
        if let Some(frame) = frame {
            debug_debug!("Allocated frame at {:?}", frame.start_address());
        }
        
        frame
    }
}