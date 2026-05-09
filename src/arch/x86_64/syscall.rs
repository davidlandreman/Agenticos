//! SYSCALL fast-path transport (Linux x86-64 ABI).
//!
//! Userland enters ring 0 via the `syscall` instruction. The CPU loads
//! `RIP <- IA32_LSTAR` (the address of `syscall_fastpath_entry` below),
//! stashes the return RIP in RCX and the return RFLAGS in R11, masks
//! RFLAGS via IA32_FMASK, and loads CS/SS from IA32_STAR. RSP is
//! **unchanged** — still pointing at the user stack. Our stub then:
//!
//! 1. `swapgs` so `gs` points at the kernel `PerCpu` struct.
//! 2. Stash user RSP at `gs:8`, load kernel RSP from `gs:0`.
//! 3. Save user state on the kernel stack: user R12 (used as a callee-saved
//!    scratch through the Rust dispatcher call), then user R11 / RCX, then
//!    the seven `SyscallArgs` fields.
//! 4. Move R10 -> RCX so the SysV C ABI lines up before the call (only
//!    matters if we ever call into Rust handlers that take args directly;
//!    the current dispatcher takes a single `*mut SyscallArgs`, so this
//!    is documentary).
//! 5. After dispatch, write the dispatcher's return into the saved RAX
//!    slot, restore the user GP regs, sanitize RFLAGS, build an IRETQ
//!    frame, restore user R12, and IRETQ to ring 3.
//!
//! ## Why IRETQ, not SYSRET
//!
//! The SYSRET path can fault in kernel mode if the user RIP we'd return to
//! is non-canonical (CVE-2012-0217 class). The kernel-side fault then
//! happens with kernel GS still active. IRETQ takes any RFLAGS / RIP / CS
//! / SS we push, so it's robust at the cost of a few extra cycles. We
//! additionally sanitize the return RFLAGS (mask `AC`, `RF`, `NT`, `IOPL`,
//! `VM`) before IRETQ so a buggy or hostile user value cannot leak into
//! kernel state.
//!
//! ## Why R12 as the through-call scratch
//!
//! The SysV C ABI preserves R12-R15, RBX, RBP across calls (callee-saved).
//! After `call {dispatch}`, R12 holds whatever it held before the call —
//! which is the value we put in it (user RSP loaded from gs:[8]). RCX and
//! R11 are caller-saved so the dispatcher can clobber them; we save those
//! on the stack before the call instead. RAX is freed up to receive the
//! dispatcher's return value.

use core::arch::naked_asm;
use core::ptr::addr_of;
use x86_64::VirtAddr;

use crate::arch::x86_64::gdt;
use crate::arch::x86_64::msr;

/// Snapshot of the user GP registers the dispatcher needs.
///
/// Built directly on the kernel stack by the SYSCALL entry stub. The
/// dispatcher receives `&mut SyscallArgs` so it can rewrite `rax` to set
/// the syscall return value.
///
/// Field order matches the order the stub pushes (low addr first), which
/// is the Linux x86-64 syscall ABI: number in RAX, then RDI/RSI/RDX/R10/
/// R8/R9 for args 1..6.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct SyscallArgs {
    pub rax: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
}

// ---------- per-CPU SYSCALL scratch ----------

/// Per-CPU scratch the SYSCALL entry stub uses to switch stacks.
///
/// Field offsets are load-bearing — the naked-asm stub references `gs:0`
/// for `kernel_rsp_top` and `gs:8` for `user_rsp_scratch`. Layout is
/// `repr(C)` to pin those offsets across compiler versions.
#[repr(C)]
struct PerCpu {
    /// Kernel stack top loaded into RSP after `swapgs` on SYSCALL entry.
    kernel_rsp_top: u64,
    /// Slot the SYSCALL stub writes the user RSP into before switching.
    user_rsp_scratch: u64,
}

/// Single per-CPU instance. Single-CPU kernel — see module comment.
#[repr(C, align(16))]
struct AlignedPerCpu(PerCpu);

static mut PERCPU: AlignedPerCpu = AlignedPerCpu(PerCpu {
    kernel_rsp_top: 0,
    user_rsp_scratch: 0,
});

/// Initialize the per-CPU struct and program GS_BASE / KERNEL_GS_BASE.
///
/// Must run after `gdt::init()` (we read `kernel_rsp0_top` from there) and
/// before `init_syscall_msrs` (which programs LSTAR to the entry stub).
pub fn init_percpu() {
    let kernel_rsp = gdt::kernel_rsp0_top().as_u64();
    // SAFETY: single-threaded boot sequence; no concurrent reader.
    unsafe {
        PERCPU.0.kernel_rsp_top = kernel_rsp;
        PERCPU.0.user_rsp_scratch = 0;
    }
    let percpu_addr = addr_of!(PERCPU) as u64;
    msr::init_gs_base(percpu_addr);
}

/// Program the SYSCALL fast-path MSRs.
///
/// Must run after `init_percpu()` and after `gdt::init()`. After this
/// returns, ring-3 code that issues `syscall` will land in
/// `syscall_fastpath_entry`. Idempotent.
pub fn init_syscall_msrs() {
    msr::enable_syscall_extensions();
    let sel = gdt::selectors();
    let lstar = VirtAddr::new(syscall_fastpath_entry as u64);
    msr::program_syscall_msrs(
        sel.kernel_code,
        sel.kernel_data,
        sel.user_code,
        sel.user_data,
        lstar,
    );
}

// ---------- entry stub ----------

/// LSTAR target — the kernel-side entry for `syscall`.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn syscall_fastpath_entry() {
    naked_asm!(
        // ---- Phase 1: stack switch ----
        "swapgs",
        "mov gs:[8], rsp",          // user_rsp_scratch <- user RSP
        "mov rsp, gs:[0]",          // RSP <- kernel_rsp_top

        // ---- Phase 2: save user state on kernel stack ----
        // Save user R12 first; we'll use R12 as a callee-saved scratch to
        // hold the user RSP across the Rust dispatcher call. The user
        // value is restored from this slot just before IRETQ.
        "push r12",
        "mov  r12, gs:[8]",         // R12 = user RSP (preserved by Rust)
        // Save user R11 (user RFLAGS) and RCX (user RIP) — caller-saved
        // across the dispatcher call.
        "push r11",
        "push rcx",
        // Build SyscallArgs (low addr first per repr(C) layout: rax, rdi,
        // rsi, rdx, r10, r8, r9). Push order is reverse: r9 first, rax last.
        "push r9",
        "push r8",
        "push r10",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rax",

        // Stack now (low to high addr from RSP):
        //   [rsp +  0] rax   (SyscallArgs.rax — syscall nr in / return out)
        //   [rsp +  8] rdi
        //   [rsp + 16] rsi
        //   [rsp + 24] rdx
        //   [rsp + 32] r10
        //   [rsp + 40] r8
        //   [rsp + 48] r9
        //   [rsp + 56] saved RCX (user RIP)
        //   [rsp + 64] saved R11 (user RFLAGS)
        //   [rsp + 72] saved user R12
        //   [rsp + 80] kernel_rsp_top (top of kernel stack)
        //
        // RSP = kernel_rsp_top - 80, which is 16-aligned (kernel_rsp_top
        // is 16-aligned; we pushed 10 qwords = 80 bytes). Inside the call
        // the call instruction pushes the return address, leaving RSP at
        // top - 88 ≡ 8 mod 16 — the SysV invariant.

        // ---- Phase 3: call dispatcher ----
        "mov rdi, rsp",             // &SyscallArgs
        "call {dispatch}",

        // RAX now holds the dispatcher's return value (i64). We want this
        // to flow out to user RAX through the IRETQ.

        // ---- Phase 4: restore user GP regs, sanitize RFLAGS ----
        "add rsp, 8",               // discard saved-rax slot (keep RAX)
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop r10",
        "pop r8",
        "pop r9",
        "pop rcx",                  // user RIP
        "pop r11",                  // user RFLAGS

        // Sanitize RFLAGS: clear AC (18), RF (16), NT (14), IOPL (12-13),
        // VM (17). Mask 0xFFF88FFF clears those six bits and is sign-
        // extended from imm32 to a 64-bit mask of 0xFFFFFFFFFFF88FFF.
        "and r11, 0xFFF88FFF",
        // Force IF (9) = 1 and reserved bit 1 = 1.
        "or  r11, 0x202",

        // ---- Phase 5: build IRETQ frame ----
        // Layout (low to high after pushes): RIP, CS, RFLAGS, RSP, SS.
        // Push reverse: SS first, RIP last.
        //
        // Selectors are taken from the GDT layout established in gdt.rs
        // (slot 3 = user_data 0x18, slot 4 = user_code 0x20). With RPL=3:
        //   SS = 0x18 | 3 = 0x1B
        //   CS = 0x20 | 3 = 0x23
        "push 0x1B",                // SS
        "push r12",                 // user RSP (held in r12 across dispatch)
        "push r11",                 // sanitized RFLAGS
        "push 0x23",                // CS
        "push rcx",                 // user RIP

        // ---- Phase 6: restore user R12, swapgs, IRETQ ----
        // The saved user R12 is at [rsp + 40] now (5 IRETQ qwords pushed
        // above the saved-r12 slot). Restore it before transferring back
        // to ring 3.
        "mov r12, [rsp + 40]",

        "swapgs",
        "iretq",

        dispatch = sym syscall_dispatch_entry,
    );
}

/// C-callable shim around the userland dispatcher.
///
/// The naked stub references this by `sym` rather than calling
/// `crate::userland::abi::syscall_dispatch` directly so the symbol
/// resolution stays local to this file.
#[no_mangle]
extern "C" fn syscall_dispatch_entry(args: *mut SyscallArgs) -> i64 {
    // SAFETY: the naked stub built `args` on the kernel stack and passes a
    // non-null pointer. We hold the pointer for the body of this function
    // only; the stub frees the slot via pops after we return.
    let args = unsafe { &mut *args };
    crate::userland::abi::syscall_dispatch(args)
}
