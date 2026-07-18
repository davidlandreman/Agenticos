use crate::{debug_debug, debug_info};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

#[cfg(feature = "test")]
pub fn exit_qemu_success() {
    debug_info!("Exiting QEMU with success status...");
    exit_qemu(QemuExitCode::Success);
}

#[cfg(feature = "test")]
pub fn exit_qemu_failed() {
    use crate::debug_error;
    debug_error!("Exiting QEMU with failure status...");
    exit_qemu(QemuExitCode::Failed);
}

pub trait Testable {
    /// Fully-qualified path of the test function, e.g.
    /// `"agenticos::tests::arc::test_weak_basic"`. Used for filtering and the
    /// per-test boot log. Default impl works for any `Fn()` test.
    fn name(&self) -> &'static str;
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn name(&self) -> &'static str {
        core::any::type_name::<T>()
    }
    fn run(&self) {
        debug_info!("{}...\t", self.name());
        self();
        debug_debug!("[ok]");
    }
}

#[cfg(feature = "test")]
#[cfg_attr(feature = "test", expect(dead_code, reason = "production-only API"))]
pub fn test_runner(tests: &[&dyn Testable]) {
    debug_info!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu_success();
}
