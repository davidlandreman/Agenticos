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
    use crate::debug_info;
    debug_info!("Exiting QEMU with success status...");
    exit_qemu(QemuExitCode::Success);
}

#[cfg(feature = "test")]
pub fn exit_qemu_failed() {
    use crate::debug_error;
    debug_error!("Exiting QEMU with failure status...");
    exit_qemu(QemuExitCode::Failed);
}