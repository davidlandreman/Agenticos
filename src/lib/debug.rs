
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DebugLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
    Trace = 4,
}

static mut DEBUG_LEVEL: DebugLevel = DebugLevel::Info;

pub fn set_debug_level(level: DebugLevel) {
    unsafe { DEBUG_LEVEL = level };
}

pub fn get_debug_level() -> DebugLevel {
    unsafe { DEBUG_LEVEL }
}

#[macro_export]
macro_rules! debug_error {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Error {
            qemu_print::qemu_print!("[ERROR] ");
            qemu_print::qemu_println!($($arg)*);
        }
    };
}

#[macro_export]
macro_rules! debug_warn {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Warn {
            qemu_print::qemu_print!("[WARN ] ");
            qemu_print::qemu_println!($($arg)*);
        }
    };
}

#[macro_export]
macro_rules! debug_info {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Info {
            qemu_print::qemu_print!("[INFO ] ");
            qemu_print::qemu_println!($($arg)*);
        }
    };
}

#[macro_export]
macro_rules! debug_debug {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Debug {
            qemu_print::qemu_print!("[DEBUG] ");
            qemu_print::qemu_println!($($arg)*);
        }
    };
}

#[macro_export]
macro_rules! debug_trace {
    ($($arg:tt)*) => {
        if $crate::lib::debug::get_debug_level() >= $crate::lib::debug::DebugLevel::Trace {
            qemu_print::qemu_print!("[TRACE] ");
            qemu_print::qemu_println!($($arg)*);
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