use core::panic::PanicInfo;

#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    crate::diagnostics::crash::begin_panic(info)
}
