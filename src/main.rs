#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

mod kernel;
mod panic;
mod bootloader_config;

// Module structure
mod arch;
mod drivers;
mod fs;
mod graphics;
mod lib;
mod mm;
mod process;
mod tests;

use bootloader_api::{entry_point, BootInfo};
use bootloader_config::BOOTLOADER_CONFIG;

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::init(boot_info);
    
    #[cfg(feature = "test")]
    tests::run_tests();
    
    kernel::run();
}