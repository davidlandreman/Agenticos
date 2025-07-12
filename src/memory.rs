use bootloader_api::info::{MemoryRegions, MemoryRegion, MemoryRegionKind};
use qemu_print::qemu_println;

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
        qemu_println!("\n=== Initializing Memory Manager ===");
        
        self.physical_memory_offset = phys_mem_offset;
        self.region_count = 0;
        
        // Store regions and calculate statistics
        for region in memory_regions.iter() {
            if self.region_count < self.regions.len() {
                self.regions[self.region_count] = Some(region.clone());
                self.region_count += 1;
                
                let size = region.end - region.start;
                self.stats.total_memory += size;
                
                match region.kind {
                    MemoryRegionKind::Usable => self.stats.usable_memory += size,
                    MemoryRegionKind::Bootloader => self.stats.bootloader_memory += size,
                    _ => self.stats.reserved_memory += size,
                }
            }
        }
        
        qemu_println!("Memory manager initialized with {} regions", self.region_count);
    }

    pub fn print_memory_map(&self) {
        qemu_println!("\n=== Memory Map ===");
        qemu_println!("Total memory regions: {}", self.region_count);
        
        for i in 0..self.region_count {
            if let Some(region) = &self.regions[i] {
                let start = region.start;
                let end = region.end;
                let size = end - start;
                
                qemu_println!("Region {}: 0x{:016x} - 0x{:016x} ({} bytes, {} MB)",
                    i, start, end, size, size / (1024 * 1024));
                
                let kind_str = match region.kind {
                    MemoryRegionKind::Usable => "Usable",
                    MemoryRegionKind::Bootloader => "Bootloader",
                    MemoryRegionKind::UnknownBios(_) => "Unknown BIOS",
                    MemoryRegionKind::UnknownUefi(_) => "Unknown UEFI",
                    _ => "Unknown",
                };
                qemu_println!("  Type: {}", kind_str);
            }
        }
        
        self.print_summary();
    }

    pub fn print_summary(&self) {
        qemu_println!("\n=== Memory Summary ===");
        qemu_println!("Total memory: {} MB ({} bytes)", 
            self.stats.total_memory / (1024 * 1024), self.stats.total_memory);
        qemu_println!("Usable memory: {} MB ({} bytes)", 
            self.stats.usable_memory / (1024 * 1024), self.stats.usable_memory);
        qemu_println!("Bootloader memory: {} MB ({} bytes)", 
            self.stats.bootloader_memory / (1024 * 1024), self.stats.bootloader_memory);
        qemu_println!("Reserved memory: {} MB ({} bytes)", 
            self.stats.reserved_memory / (1024 * 1024), self.stats.reserved_memory);
        
        if let Some(offset) = self.physical_memory_offset {
            qemu_println!("\nPhysical memory offset: 0x{:016x}", offset);
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

pub fn init(memory_regions: &MemoryRegions, phys_mem_offset: Option<u64>) {
    unsafe {
        MEMORY_MANAGER.init(memory_regions, phys_mem_offset);
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