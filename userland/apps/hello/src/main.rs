//! HELLO.ELF — the first user app.
//!
//! Static, non-PIE, ET_EXEC, x86_64. Linked at base `0x40_0000` via the
//! sibling `linker.ld`. Calls the runtime's `print` then `exit(0)`.
//!
//! Built by `cargo build --release --manifest-path userland/Cargo.toml`
//! and staged by `build.sh` / `test.sh` to `host_share/HELLO.ELF`.

#![no_std]
#![no_main]

use runtime::{exit, print};

#[no_mangle]
pub unsafe extern "C" fn _start() -> ! {
    let msg: &[u8] = b"hello\n";
    let _ = print(msg.as_ptr(), msg.len());
    exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // A panic in user code becomes a clean exit with a non-zero code.
    // We deliberately do not try to print the panic message — that would
    // require a write-to-buffer + syscall path, which is more failure
    // surface than is justified for the first app.
    unsafe { exit(1) }
}
