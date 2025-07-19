#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod kernel;
mod panic;

// Module structure
mod arch;
mod drivers;
mod graphics;
mod lib;
mod mm;
mod tests;

use bootloader_api::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::init(boot_info);
    
    #[cfg(feature = "test")]
    tests::run_tests();
    
    kernel::run();
}