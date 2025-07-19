use bootloader_api::info::{MemoryRegions, MemoryRegion, MemoryRegionKind};
use x86_64::VirtAddr;
use crate::{debug_info, debug_debug};

#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub total_memory: u64,
    pub usable_memory: u64,
    pub bootloader_memory: u64,
    pub reserved_memory: u64,
}

pub struct MemoryManager {
    regions: [Option<MemoryRegion>; 32], // Static array to store regions
    region_count: usize,
    stats: MemoryStats,
    physical_memory_offset: Option<u64>,
}

impl MemoryManager {
    pub const fn new() -> Self {
        Self {
            regions: [None; 32],
            region_count: 0,
            stats: MemoryStats {
                total_memory: 0,
                usable_memory: 0,
                bootloader_memory: 0,
                reserved_memory: 0,
            },
            physical_memory_offset: None,
        }
    }

    pub fn init(&mut self, memory_regions: &MemoryRegions, phys_mem_offset: Option<u64>) {
        debug_info!("=== Initializing Memory Manager ===");
        
        self.physical_memory_offset = phys_mem_offset;
        self.region_count = 0;
        
        // Store regions and calculate statistics
        for region in memory_regions.iter() {
            if self.region_count < self.regions.len() {
                self.regions[self.region_count] = Some(region.clone());
                self.region_count += 1;
                
                let size = region.end - region.start;
                self.stats.total_memory += size;
                
                // Print each region for debugging
                let kind_str = match region.kind {
                    MemoryRegionKind::Usable => "Usable",
                    MemoryRegionKind::Bootloader => "Bootloader",
                    MemoryRegionKind::UnknownBios(_) => "Unknown BIOS",
                    MemoryRegionKind::UnknownUefi(_) => "Unknown UEFI",
                    _ => "Unknown",
                };
                
                debug_info!("Region {}: 0x{:016x} - 0x{:016x} ({} bytes, {})",
                    self.region_count - 1, region.start, region.end, size, kind_str);
                
                match region.kind {
                    MemoryRegionKind::Usable => self.stats.usable_memory += size,
                    MemoryRegionKind::Bootloader => self.stats.bootloader_memory += size,
                    _ => self.stats.reserved_memory += size,
                }
            }
        }
        
        debug_info!("Memory manager initialized with {} regions", self.region_count);
        debug_info!("Physical memory offset: {:?}", phys_mem_offset);
    }

    pub fn print_memory_map(&self) {
        debug_info!("=== Memory Map ===");
        debug_info!("Total memory regions: {}", self.region_count);
        
        for i in 0..self.region_count {
            if let Some(region) = &self.regions[i] {
                let start = region.start;
                let end = region.end;
                let size = end - start;
                
                debug_debug!("Region {}: 0x{:016x} - 0x{:016x} ({} bytes, {} MB)",
                    i, start, end, size, size / (1024 * 1024));
                
                let kind_str = match region.kind {
                    MemoryRegionKind::Usable => "Usable",
                    MemoryRegionKind::Bootloader => "Bootloader",
                    MemoryRegionKind::UnknownBios(_) => "Unknown BIOS",
                    MemoryRegionKind::UnknownUefi(_) => "Unknown UEFI",
                    _ => "Unknown",
                };
                debug_debug!("  Type: {}", kind_str);
            }
        }
        
        self.print_summary();
    }

    pub fn print_summary(&self) {
        debug_info!("=== Memory Summary ===");
        debug_info!("Total memory: {} MB ({} bytes)", 
            self.stats.total_memory / (1024 * 1024), self.stats.total_memory);
        debug_info!("Usable memory: {} MB ({} bytes)", 
            self.stats.usable_memory / (1024 * 1024), self.stats.usable_memory);
        debug_info!("Bootloader memory: {} MB ({} bytes)", 
            self.stats.bootloader_memory / (1024 * 1024), self.stats.bootloader_memory);
        debug_info!("Reserved memory: {} MB ({} bytes)", 
            self.stats.reserved_memory / (1024 * 1024), self.stats.reserved_memory);
        
        if let Some(offset) = self.physical_memory_offset {
            debug_debug!("Physical memory offset: 0x{:016x}", offset);
        }
    }

    pub fn get_stats(&self) -> MemoryStats {
        self.stats
    }

    pub fn get_usable_memory(&self) -> u64 {
        self.stats.usable_memory
    }

    pub fn get_largest_usable_region(&self) -> Option<(u64, u64)> {
        let mut largest_size = 0u64;
        let mut largest_region = None;
        
        for i in 0..self.region_count {
            if let Some(region) = &self.regions[i] {
                if matches!(region.kind, MemoryRegionKind::Usable) {
                    let size = region.end - region.start;
                    if size > largest_size {
                        largest_size = size;
                        largest_region = Some((region.start, region.end));
                    }
                }
            }
        }
        
        largest_region
    }
}

// Global memory manager instance
static mut MEMORY_MANAGER: MemoryManager = MemoryManager::new();

// Static storage for memory regions to use with heap allocator
static mut STATIC_MEMORY_REGIONS: Option<&'static MemoryRegions> = None;

pub fn init(memory_regions: &'static MemoryRegions, phys_mem_offset: Option<u64>) {
    unsafe {
        MEMORY_MANAGER.init(memory_regions, phys_mem_offset);
        STATIC_MEMORY_REGIONS = Some(memory_regions);
    }
}

// Static storage for the mapper
static mut STATIC_MAPPER: Option<crate::mm::paging::MemoryMapper> = None;

pub fn init_heap(phys_mem_offset: u64) {
    unsafe {
        let memory_regions = STATIC_MEMORY_REGIONS
            .expect("Memory regions not initialized");
            
        // Create mapper in static storage
        STATIC_MAPPER = Some(crate::mm::paging::MemoryMapper::new(
            VirtAddr::new(phys_mem_offset),
            memory_regions
        ));
        
        // Store pointer globally for page fault handling
        if let Some(mapper) = &mut STATIC_MAPPER {
            crate::mm::paging::MAPPER = Some(mapper as *mut _);
            
            // Initialize heap
            crate::mm::heap::init_heap(mapper)
                .expect("Failed to initialize heap");
        }
    }
}

pub fn print_memory_info() {
    unsafe {
        MEMORY_MANAGER.print_memory_map();
    }
}

pub fn get_memory_stats() -> MemoryStats {
    unsafe {
        MEMORY_MANAGER.get_stats()
    }
}