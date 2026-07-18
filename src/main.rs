#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]
// AgenticOS is a single binary crate whose subsystem APIs are also exercised by
// the feature-gated QEMU test harness. In a normal kernel build, rustc therefore
// sees many intentionally dormant API surfaces as dead code. Keep useful lints
// enabled while suppressing that structural noise and the intentional `lib.rs`
// module name used by the kernel's internal support library.
#![allow(dead_code, special_module_name)]

extern crate alloc;

mod bootloader_config;
mod kernel;
mod panic;

// Module structure
mod arch;
mod commands;
mod drivers;
mod fs;
mod graphics;
mod input;
mod lib;
mod mm;
mod net;
mod process;
mod stdlib;
mod terminal;
mod tests;
mod tools;
mod userland;
mod window;

use bootloader_api::{entry_point, BootInfo};
use bootloader_config::BOOTLOADER_CONFIG;

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    kernel::init(boot_info);

    #[cfg(feature = "test")]
    tests::run_tests();

    kernel::run();
}
