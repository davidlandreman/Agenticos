//! Syscall transport: `int 0x80` today, SYSCALL fast path soon.
//!
//! D1 of the userland-app-platform plan installed a plain `int 0x80`
//! interrupt gate (DPL=3, IF auto-cleared on entry). The Linux ABI cutover
//! replaces that vector with the `syscall` instruction; this file is the
//! seam.
//!
//! The per-CPU struct below is the foundation U3 will use: the SYSCALL
//! entry stub stashes the user RSP at `gs:8` and loads the kernel RSP from
//! `gs:0`. We allocate the struct as a single `static mut` and point both
//! `IA32_GS_BASE` and `IA32_KERNEL_GS_BASE` at it on boot so the first
//! `swapgs` is a no-op regardless of which MSR currently mirrors which
//! value. Single-CPU only — SMP would require a per-AP allocation and
//! per-AP MSR programming.

use core::arch::naked_asm;
use core::ptr::addr_of;

/// Per-CPU scratch the SYSCALL entry stub uses to switch stacks.
///
/// Field offsets are load-bearing: U3's naked-asm stub references `gs:0`
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
///
/// `repr(C)` + `align(16)` on the wrapping struct keeps the GS-relative
/// loads cheap and avoids the cross-cache-line splits an unaligned access
/// would risk on the SYSCALL hot path.
#[repr(C, align(16))]
struct AlignedPerCpu(PerCpu);

static mut PERCPU: AlignedPerCpu = AlignedPerCpu(PerCpu {
    kernel_rsp_top: 0,
    user_rsp_scratch: 0,
});

/// Initialize per-CPU state and program GS_BASE / KERNEL_GS_BASE.
///
/// Must run after `gdt::init()` (we read `kernel_rsp0_top` from there) and
/// before any SYSCALL stub installation that would consume GS. Currently
/// also runs before any user app exists, so there's no race against
/// `swapgs`.
pub fn init_percpu() {
    let kernel_rsp = crate::arch::x86_64::gdt::kernel_rsp0_top().as_u64();
    // SAFETY: single-threaded boot sequence; no concurrent reader.
    unsafe {
        PERCPU.0.kernel_rsp_top = kernel_rsp;
        PERCPU.0.user_rsp_scratch = 0;
    }
    let percpu_addr = addr_of!(PERCPU) as u64;
    crate::arch::x86_64::msr::init_gs_base(percpu_addr);
}

/// Snapshot of the user GP registers the dispatcher needs.
///
/// Layout matches the order pushed by the naked stub below: the struct is
/// built directly on the kernel stack (in the rsp0 stack the CPU switched to
/// on the ring-3 -> ring-0 transition). The dispatcher receives `&mut
/// SyscallArgs` so it can rewrite `rax` to set the syscall return value.
///
/// `r10` is the System V "syscall fourth argument" register — `syscall`
/// instruction semantics (which we are not using yet) clobber RCX, so the
/// userland ABI uses R10 for the fourth argument. We mirror that here even
/// though `int 0x80` does not clobber RCX, to keep the convention stable
/// across a future `syscall`/`sysret` migration.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct SyscallArgs {
    /// Syscall ID. Also where the return value is written.
    pub rax: u64,
    /// Argument 1.
    pub rdi: u64,
    /// Argument 2.
    pub rsi: u64,
    /// Argument 3.
    pub rdx: u64,
    /// Argument 4 (System V uses R10 for syscall, not RCX).
    pub r10: u64,
    /// Argument 5.
    pub r8: u64,
    /// Argument 6.
    pub r9: u64,
}

/// Syscall vector. Linux convention; chosen to coexist with PIC-remapped
/// hardware vectors at 32..47 and the rest of the IDT we have not used.
pub const SYSCALL_VECTOR: u8 = 0x80;

/// Naked entry stub installed at IDT vector `SYSCALL_VECTOR`.
///
/// The CPU has already pushed (low to high addr): RIP, CS, RFLAGS, RSP, SS
/// onto the kernel rsp0 stack. We then push the seven syscall registers in
/// the SyscallArgs order (last to first so the struct laid out at `rsp` after
/// the pushes matches `SyscallArgs`'s field order on a stack-grows-down
/// system). RDI is loaded with `rsp` so the Rust dispatcher receives a
/// `*mut SyscallArgs`. After the call, RAX is reloaded from the `[rsp+0]`
/// slot (which the dispatcher may have rewritten) so the value flows back to
/// userland in RAX through `iretq`.
#[unsafe(naked)]
#[no_mangle]
pub unsafe extern "C" fn syscall_interrupt_handler() {
    naked_asm!(
        // Build SyscallArgs on the stack. Field order in the struct (low addr
        // first): rax, rdi, rsi, rdx, r10, r8, r9. Stack grows down, so push
        // in reverse — r9 first, rax last.
        "push r9",
        "push r8",
        "push r10",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rax",

        // Pass &mut SyscallArgs in RDI (System V first-arg register).
        "mov rdi, rsp",

        // Call the Rust dispatcher. It returns its i64 result in RAX, which
        // we ignore here — the dispatcher is responsible for writing the
        // return value into args.rax via the &mut pointer it received. That
        // way the same struct slot is the source of truth for "what RAX
        // ends up with on iretq."
        "call {dispatch}",

        // Restore the saved GP registers. RAX is loaded last so the
        // dispatcher's rewrite of args.rax flows out to userland.
        "pop rax",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop r10",
        "pop r8",
        "pop r9",

        "iretq",

        dispatch = sym crate::arch::x86_64::syscall::syscall_dispatch_entry,
    );
}

/// C-callable shim around the userland dispatcher. Splitting this out from
/// `crate::userland::abi::syscall_dispatch` keeps the naked stub's `sym`
/// reference inside this file (the stub cannot reference cross-crate symbols
/// with arbitrary mangling, and a free fn here gives us a stable name).
#[no_mangle]
extern "C" fn syscall_dispatch_entry(args: *mut SyscallArgs) {
    // Safety: the naked stub built `args` directly on the kernel stack and
    // passes a non-null pointer. We hold the pointer for the body of this
    // function only; the stub frees the slot via `pop`s after we return.
    let args = unsafe { &mut *args };
    let ret = crate::userland::abi::syscall_dispatch(args);
    args.rax = ret as u64;
}
