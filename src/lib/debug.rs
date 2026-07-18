#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DebugLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

use core::fmt;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::arch::x86_64::interrupt_guard::InterruptMutex;

static DEBUG_LEVEL: AtomicU8 = AtomicU8::new(DebugLevel::Info as u8);
static SERIAL_OUTPUT: InterruptMutex<()> = InterruptMutex::new(());

pub fn set_debug_level(level: DebugLevel) {
    DEBUG_LEVEL.store(level as u8, Ordering::Release);
}

pub fn get_debug_level() -> DebugLevel {
    match DEBUG_LEVEL.load(Ordering::Acquire) {
        0 => DebugLevel::Error,
        1 => DebugLevel::Warn,
        2 => DebugLevel::Info,
        3 => DebugLevel::Debug,
        _ => DebugLevel::Trace,
    }
}

pub fn write_line(prefix: &str, args: fmt::Arguments<'_>) {
    let _guard = SERIAL_OUTPUT.lock();
    qemu_print::qemu_print!("{}", prefix);
    qemu_print::qemu_println!("{}", args);
}

/// Panic-safe serial output. If another CPU died while owning the lock, emit
/// anyway instead of deadlocking the panic path.
pub fn write_panic_line(prefix: &str, args: fmt::Arguments<'_>) {
    if let Some(_guard) = SERIAL_OUTPUT.try_lock() {
        qemu_print::qemu_print!("{}", prefix);
        qemu_print::qemu_println!("{}", args);
    } else {
        qemu_print::qemu_print!("{}", prefix);
        qemu_print::qemu_println!("{}", args);
    }
}

#[macro_export]
macro_rules! debug_error {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Error {
            $crate::lib::debug::write_line("[ERROR] ", format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! debug_warn {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Warn {
            $crate::lib::debug::write_line("[WARN ] ", format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! debug_info {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Info {
            $crate::lib::debug::write_line("[INFO ] ", format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! debug_debug {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Debug {
            $crate::lib::debug::write_line("[DEBUG] ", format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! debug_trace {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Trace {
            $crate::lib::debug::write_line("[TRACE] ", format_args!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        qemu_print::qemu_print!($($arg)*);
    };
}

#[macro_export]
macro_rules! debug_println {
    ($($arg:tt)*) => {
        qemu_print::qemu_println!($($arg)*);
    };
}

pub fn init() {
    set_debug_level(DebugLevel::Info);
    debug_info!("Debug subsystem initialized");
}
