//! x87/SSE feature enable for the kernel and ring-3 user processes.
//!
//! The kernel itself is built with `-mmx,-sse,+soft-float`, so the boot
//! path never touches XMM registers. But ring-3 user binaries — anything
//! linked against musl + libstdc++, including the C++ hello-world target —
//! emit SSE2 instructions in their startup (`__init_tls` issues
//! `movq xmm0, rbx` and `punpcklqdq` before reaching `main`). Those traps
//! `#UD` (vector 6) when the CPU enters ring 3 with `CR0.EM = 1` or
//! `CR4.OSFXSR = 0`.
//!
//! `enable_sse` flips the four bits the SDM (§13.1.4) requires for SSE/SSE2
//! to execute without faulting:
//!
//! - `CR0.EM = 0` — SSE/MMX instructions are NOT emulated; execute natively.
//! - `CR0.MP = 1` — pair `WAIT/FWAIT` with x87 task-switched state per the
//!   SDM-recommended pairing (the kernel has no plans to context-switch the
//!   x87 state today, but the bit is required for compliance and is harmless
//!   when `CR0.TS = 0`).
//! - `CR4.OSFXSR = 1` — the OS uses `FXSAVE`/`FXRSTOR` for FP state and the
//!   SSE register file is exposed.
//! - `CR4.OSXMMEXCPT = 1` — SIMD floating-point exceptions raise `#XM`
//!   (vector 19) instead of `#UD`, so a misbehaving user app gets a routable
//!   SIMD fault rather than an opaque invalid-opcode trap.
//!
//! No FP context-switching is implemented — the kernel never touches the
//! XMM register file, so user-process state is preserved across kernel
//! transitions for free (the SYSCALL fast path does not save/restore XMM).
//! This becomes load-bearing if the kernel ever uses SSE itself or if more
//! than one ring-3 process is concurrent.

use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};

/// Configure CR0/CR4 so SSE/SSE2 instructions execute in both ring 0 and
/// ring 3. Must run after the bootloader hands off and before any ring-3
/// transition; in practice it's invoked once early in `kernel::init`.
///
/// Idempotent: re-calling is a no-op (the flag updates are bit-set/clear,
/// not toggles).
pub fn enable_sse() {
    unsafe {
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);

        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR);
        cr4.insert(Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);
    }
}

/// True when CR0/CR4 are configured for ring-3 SSE/SSE2 execution —
/// `CR0.EM = 0`, `CR0.MP = 1`, `CR4.OSFXSR = 1`, `CR4.OSXMMEXCPT = 1`.
/// Used by the in-kernel test that proves `enable_sse()` ran before
/// any ring-3 transition could fire (a regression here makes
/// musl/libstdc++ binaries `#UD` silently inside `__init_tls`).
pub fn sse_enabled() -> bool {
    let cr0 = Cr0::read();
    let cr4 = Cr4::read();
    !cr0.contains(Cr0Flags::EMULATE_COPROCESSOR)
        && cr0.contains(Cr0Flags::MONITOR_COPROCESSOR)
        && cr4.contains(Cr4Flags::OSFXSR)
        && cr4.contains(Cr4Flags::OSXMMEXCPT_ENABLE)
}
