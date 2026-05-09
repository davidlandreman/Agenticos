//! COM2 (0x2F8) serial port driver.
//!
//! COM1 is owned by the third-party `qemu_print` crate and used by the kernel's
//! debug-log macros. COM2 is unclaimed and is the transport for the
//! kernel ↔ host MCP bridge.

use spin::{Mutex, Once};
use uart_16550::SerialPort;

const COM2_BASE: u16 = 0x2F8;

pub struct Com2 {
    port: Mutex<SerialPort>,
}

impl Com2 {
    fn new() -> Self {
        // SAFETY: COM2's I/O port range (0x2F8..0x2FF) is owned exclusively by
        // this driver. `qemu_print` owns COM1 (0x3F8) and never touches 0x2F8.
        let mut port = unsafe { SerialPort::new(COM2_BASE) };
        port.init();
        Com2 {
            port: Mutex::new(port),
        }
    }

    /// Non-blocking read. Returns `None` if no byte is currently available.
    pub fn read_byte(&self) -> Option<u8> {
        let mut port = self.port.lock();
        // Bit 0 of the Line Status Register is "Data Ready". The uart_16550
        // 0.2 API exposes a non-blocking probe via `line_sts()` checked
        // against `LineStsFlags::INPUT_FULL`, but those types are private to
        // older versions. We do our own LSR read via the underlying I/O port:
        // 0x2FD = COM2 base + 5.
        use x86_64::instructions::port::Port;
        let mut lsr: Port<u8> = Port::new(COM2_BASE + 5);
        // SAFETY: 0x2FD is COM2's LSR; reading it has no side effects.
        let status = unsafe { lsr.read() };
        if status & 0x01 == 0 {
            None
        } else {
            Some(port.receive())
        }
    }

    pub fn write_byte(&self, b: u8) {
        self.port.lock().send(b);
    }

    pub fn write_all(&self, bytes: &[u8]) {
        let mut port = self.port.lock();
        for &b in bytes {
            port.send(b);
        }
    }
}

static COM2: Once<Com2> = Once::new();

/// Initialize the COM2 driver. Call once during kernel boot.
pub fn init() {
    COM2.call_once(Com2::new);
}

/// Get the global COM2 handle. Returns `None` if `init()` has not been called.
pub fn com2() -> Option<&'static Com2> {
    COM2.get()
}
