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

use bootloader_api::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::init(boot_info);
    kernel::run();
}