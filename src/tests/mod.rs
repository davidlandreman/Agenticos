#[cfg(feature = "test")]
pub mod basic;
#[cfg(feature = "test")]
pub mod memory;
#[cfg(feature = "test")]
pub mod display;
#[cfg(feature = "test")]
pub mod interrupts;
#[cfg(feature = "test")]
pub mod heap;
#[cfg(feature = "test")]
pub mod arc;
#[cfg(feature = "test")]
pub mod filesystem;

#[cfg(feature = "test")]
pub fn run_tests() {
    use crate::{debug_info};
    use crate::lib::test_utils::{Testable, exit_qemu_success};
    
    debug_info!("=== Running Kernel Tests ===");
    
    // Collect all tests from modules
    let basic_tests = basic::get_tests();
    let memory_tests = memory::get_tests();
    let display_tests = display::get_tests();
    let interrupts_tests = interrupts::get_tests();
    let heap_tests = heap::get_tests();
    let arc_tests = arc::get_tests();
    let filesystem_tests = filesystem::get_tests();
    
    let mut total_tests = 0;
    
    // Run basic tests
    debug_info!("\n[Basic Tests]");
    debug_info!("Running {} tests", basic_tests.len());
    for test in basic_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run memory tests
    debug_info!("\n[Memory Tests]");
    debug_info!("Running {} tests", memory_tests.len());
    for test in memory_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run display tests
    debug_info!("\n[Display Tests]");
    debug_info!("Running {} tests", display_tests.len());
    for test in display_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run interrupt tests
    debug_info!("\n[Interrupt Tests]");
    debug_info!("Running {} tests", interrupts_tests.len());
    for test in interrupts_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run heap tests
    debug_info!("\n[Heap Tests]");
    debug_info!("Running {} tests", heap_tests.len());
    for test in heap_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run Arc tests
    debug_info!("\n[Arc Tests]");
    debug_info!("Running {} tests", arc_tests.len());
    for test in arc_tests {
        test.run();
        total_tests += 1;
    }
    
    // Run Filesystem tests
    debug_info!("\n[Filesystem Tests]");
    debug_info!("Running {} tests", filesystem_tests.len());
    for test in filesystem_tests {
        test.run();
        total_tests += 1;
    }
    
    debug_info!("\n=== All {} tests passed! ===", total_tests);
    exit_qemu_success();
}