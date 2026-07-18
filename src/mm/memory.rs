use crate::{debug_debug, debug_info};
use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};
use x86_64::VirtAddr;

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

                debug_info!(
                    "Region {}: 0x{:016x} - 0x{:016x} ({} bytes, {})",
                    self.region_count - 1,
                    region.start,
                    region.end,
                    size,
                    kind_str
                );

                match region.kind {
                    MemoryRegionKind::Usable => self.stats.usable_memory += size,
                    MemoryRegionKind::Bootloader => self.stats.bootloader_memory += size,
                    _ => self.stats.reserved_memory += size,
                }
            }
        }

        debug_info!(
            "Memory manager initialized with {} regions",
            self.region_count
        );
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

                debug_debug!(
                    "Region {}: 0x{:016x} - 0x{:016x} ({} bytes, {} MB)",
                    i,
                    start,
                    end,
                    size,
                    size / (1024 * 1024)
                );

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
        debug_info!(
            "Total memory: {} MB ({} bytes)",
            self.stats.total_memory / (1024 * 1024),
            self.stats.total_memory
        );
        debug_info!(
            "Usable memory: {} MB ({} bytes)",
            self.stats.usable_memory / (1024 * 1024),
            self.stats.usable_memory
        );
        debug_info!(
            "Bootloader memory: {} MB ({} bytes)",
            self.stats.bootloader_memory / (1024 * 1024),
            self.stats.bootloader_memory
        );
        debug_info!(
            "Reserved memory: {} MB ({} bytes)",
            self.stats.reserved_memory / (1024 * 1024),
            self.stats.reserved_memory
        );

        if let Some(offset) = self.physical_memory_offset {
            debug_debug!("Physical memory offset: 0x{:016x}", offset);
        }
    }

    pub fn get_stats(&self) -> MemoryStats {
        self.stats
    }

    /// Get the physical memory offset used for virtual address translation
    pub fn get_physical_memory_offset(&self) -> Option<u64> {
        self.physical_memory_offset
    }

    /// Convert a physical address to a virtual address
    pub fn phys_to_virt(&self, phys_addr: u64) -> Option<u64> {
        self.physical_memory_offset.map(|offset| phys_addr + offset)
    }
}

// Global memory manager instance
static mut MEMORY_MANAGER: MemoryManager = MemoryManager::new();

// Static storage for memory regions to use with heap allocator
static mut STATIC_MEMORY_REGIONS: Option<&'static MemoryRegions> = None;

pub fn init(memory_regions: &'static MemoryRegions, phys_mem_offset: Option<u64>) {
    unsafe {
        (*&raw mut MEMORY_MANAGER).init(memory_regions, phys_mem_offset);
        STATIC_MEMORY_REGIONS = Some(memory_regions);
    }
}

// Static storage for the mapper
static mut STATIC_MAPPER: Option<crate::mm::paging::MemoryMapper> = None;

pub fn init_heap(phys_mem_offset: u64) {
    unsafe {
        let memory_regions = STATIC_MEMORY_REGIONS.expect("Memory regions not initialized");

        // Create mapper in static storage
        STATIC_MAPPER = Some(crate::mm::paging::MemoryMapper::new(
            VirtAddr::new(phys_mem_offset),
            memory_regions,
        ));

        // Store pointer globally for page fault handling
        if let Some(mapper) = &mut *(&raw mut STATIC_MAPPER) {
            crate::mm::paging::MAPPER = Some(mapper as *mut _);

            // Initialize heap
            crate::mm::heap::init_heap(mapper).expect("Failed to initialize heap");
        }

        // Phase 4 PR-B: capture the bootloader's CR3 as the kernel L4
        // so per-process address spaces can switch back to it on exit.
        crate::mm::paging::capture_kernel_l4();
    }
}

pub fn print_memory_info() {
    unsafe {
        (*&raw const MEMORY_MANAGER).print_memory_map();
    }
}

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn get_memory_stats() -> MemoryStats {
    unsafe { (*&raw const MEMORY_MANAGER).get_stats() }
}

/// Get the physical memory offset for virtual address translation
pub fn get_physical_memory_offset() -> Option<u64> {
    unsafe { (*&raw const MEMORY_MANAGER).get_physical_memory_offset() }
}

/// Convert a physical address to a virtual address using the bootloader's mapping
pub fn phys_to_virt(phys_addr: u64) -> Option<u64> {
    unsafe { (*&raw const MEMORY_MANAGER).phys_to_virt(phys_addr) }
}

/// Run a closure with mutable access to the global `MemoryMapper`. Returns
/// `None` if `init_heap` has not run yet. Used by the userland subsystem
/// (U6+) to wrap loader steps without each call re-resolving the global.
pub fn with_memory_mapper<R>(
    f: impl FnOnce(&mut crate::mm::paging::MemoryMapper) -> R,
) -> Option<R> {
    unsafe { crate::mm::paging::get_mapper().map(f) }
}
