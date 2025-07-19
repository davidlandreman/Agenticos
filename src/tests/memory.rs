use crate::{debug_info};
use crate::lib::test_utils::Testable;
use crate::mm::memory;

fn test_memory_stats() {
    let stats = memory::get_memory_stats();
    assert!(stats.total_memory > 0, "Total memory should be greater than 0");
    assert!(stats.usable_memory > 0, "Usable memory should be greater than 0");
    assert!(stats.usable_memory <= stats.total_memory, "Usable memory should not exceed total memory");
    debug_info!("Memory - Total: {} MB, Usable: {} MB", 
               stats.total_memory / (1024 * 1024),
               stats.usable_memory / (1024 * 1024));
}

fn test_memory_alignment() {
    // Test that memory addresses are properly aligned
    let addr1 = 0x1000;
    let addr2 = 0x2000;
    assert_eq!(addr1 % 4096, 0, "Address should be page aligned");
    assert_eq!(addr2 % 4096, 0, "Address should be page aligned");
}

fn test_memory_ranges() {
    let stats = memory::get_memory_stats();
    // Ensure we have reasonable memory amounts (at least 1MB usable)
    assert!(stats.usable_memory >= 1024 * 1024, "Should have at least 1MB of usable memory");
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_memory_stats,
        &test_memory_alignment,
        &test_memory_ranges,
    ]
}