use bootloader_api::config::{BootloaderConfig, Mapping};

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    
    // Request dynamic mapping - the bootloader will set up recursive paging
    // This allows us to modify page tables without needing all physical memory mapped
    config.mappings.dynamic_range_start = Some(0xFFFF_8000_0000_0000);
    config.mappings.dynamic_range_end = Some(0xFFFF_FFFF_FFFF_FFFF);
    
    // Map more physical memory by default to reduce page faults
    // This helps with accessing page table structures
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    
    config
};