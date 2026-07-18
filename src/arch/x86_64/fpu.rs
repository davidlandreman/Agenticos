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
//! ## Per-process FPU state (U2)
//!
//! Originally the SYSCALL fast path didn't save/restore XMM because the
//! kernel never touches the register file — single ring-3 process state
//! survived for free. U2 (the multi-ring-3 scheduling refactor) adds the
//! [`FpuState`] buffer plus [`save_fpu`] / [`restore_fpu`] primitives so
//! the U4 ring-3 switch can swap FPU state between processes on
//! context switch. The SYSCALL fast path is still cheap — saves happen
//! only on real switches, not on every syscall return.
//!
//! Invariant the kernel relies on: kernel code never executes SSE/MMX
//! instructions (target spec carries `+soft-float`). If that changes,
//! save_fpu must run on every syscall entry rather than only on switch.

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
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
pub fn sse_enabled() -> bool {
    let cr0 = Cr0::read();
    let cr4 = Cr4::read();
    !cr0.contains(Cr0Flags::EMULATE_COPROCESSOR)
        && cr0.contains(Cr0Flags::MONITOR_COPROCESSOR)
        && cr4.contains(Cr4Flags::OSFXSR)
        && cr4.contains(Cr4Flags::OSXMMEXCPT_ENABLE)
}

/// FXSAVE-area buffer. The CPU writes 512 bytes here; the area MUST be
/// 16-byte aligned per Intel SDM §10.5.1. `repr(C, align(16))`
/// propagates the alignment requirement out to any enclosing type
/// (e.g., the per-process `Process` struct embeds one of these).
#[repr(C, align(16))]
#[derive(Clone)]
pub struct FpuState {
    bytes: [u8; 512],
}

impl Default for FpuState {
    /// Architectural reset state for FPU/SSE. `fninit` + zeroed XMM
    /// registers corresponds to MXCSR=0x1F80 and all-zero data.
    /// Returning a zeroed buffer here is acceptable because the kernel
    /// always calls `fxrstor` on this buffer before the first user
    /// instruction; the reset bytes (header + zero registers) match
    /// what fresh processes expect. New processes that observe their
    /// own FPU state via, e.g., `fstmxcsr`, will see the FXSAVE area's
    /// `mxcsr` field initialized via [`Self::with_default_mxcsr`].
    fn default() -> Self {
        Self { bytes: [0u8; 512] }
    }
}

impl FpuState {
    /// FXSAVE area shape: bytes [24..28] hold MXCSR. Initialize to the
    /// architectural reset value (0x1F80 — all FP exceptions masked,
    /// round-to-nearest) so a freshly-installed process sees the same
    /// FPU configuration musl expects on startup.
    pub fn fresh() -> Self {
        let mut s = Self::default();
        s.bytes[24] = 0x80;
        s.bytes[25] = 0x1F;
        s.bytes[26] = 0x00;
        s.bytes[27] = 0x00;
        s
    }

    /// Raw byte view — used by the test-only roundtrip check that
    /// asserts XMM register state survives a save/restore pair.
    #[cfg(feature = "test")]
    pub fn bytes(&self) -> &[u8; 512] {
        &self.bytes
    }
}

/// Capture the current FPU/SSE register state into `buf`. Wraps the
/// `fxsave` instruction. Must be called from CPL=0; the buffer must be
/// 16-byte aligned (`FpuState` enforces this via `repr(align(16))`).
///
/// Used by the U4 ring-3 switch primitive on switch-out — after this
/// returns, the live XMM state can be clobbered without losing the
/// process's data.
#[inline]
pub fn save_fpu(buf: &mut FpuState) {
    unsafe {
        core::arch::asm!(
            "fxsave [{0}]",
            in(reg) buf.bytes.as_mut_ptr(),
            options(nostack, preserves_flags),
        );
    }
}

/// Reload FPU/SSE register state from `buf`. Wraps the `fxrstor`
/// instruction. Must be called from CPL=0; `buf` must be 16-byte
/// aligned. After this returns, the CPU's FPU/SSE state matches what
/// was in `buf` at the time of the matching `save_fpu`.
///
/// Used by the U4 ring-3 switch primitive on switch-in — restores the
/// resumed process's XMM registers, x87 state, and MXCSR before the
/// `iretq` to ring 3.
#[inline]
pub fn restore_fpu(buf: &FpuState) {
    unsafe {
        core::arch::asm!(
            "fxrstor [{0}]",
            in(reg) buf.bytes.as_ptr(),
            options(nostack, preserves_flags),
        );
    }
}
