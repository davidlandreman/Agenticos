#[cfg(feature = "test")]
use crate::lib::test_utils::Testable;
use crate::debug_info;

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_breakpoint_interrupt,
        // Note: We can only safely test the breakpoint interrupt
        // Other interrupts that panic would terminate the test runner
    ]
}

fn test_breakpoint_interrupt() {
    debug_info!("Testing breakpoint interrupt...");
    
    // The breakpoint handler in interrupts.rs doesn't panic, so we can test it directly
    // Trigger breakpoint interrupt using int3 instruction
    unsafe {
        core::arch::asm!("int3");
    }
    
    // If we reach here without panicking, the test passes
    debug_info!("Breakpoint interrupt handled successfully");
}