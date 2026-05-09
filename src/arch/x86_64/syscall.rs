//! `int 0x80` syscall transport.
//!
//! D1 of the userland-app-platform plan: the first-cut syscall transport is a
//! plain `int 0x80` software interrupt. The IDT vector has DPL=3 so ring-3
//! code can issue it; the entry uses an **interrupt gate** which auto-clears
//! IF on entry. That is the safer default for a first cut — handlers run
//! with interrupts off until they iretq back, so the dispatcher cannot be
//! preempted while it touches kernel state. Switching to a trap gate later
//! is a one-line change if syscall handlers grow long enough that pending
//! interrupts (timer, keyboard) need to land while one is in flight.
//!
//! ## Why a two-piece naked-asm + Rust dispatcher
//!
//! `extern "x86-interrupt"` does not expose general-purpose registers to the
//! Rust handler — only the CPU-pushed `InterruptStackFrame`. RAX (the syscall
//! ID), RDI/RSI/RDX/R10/R8/R9 (the System V argument-passing registers, with
//! R10 substituting for RCX since `int` does not preserve RCX) all live in
//! the live register file and are clobberable by the prologue Rust generates
//! before the handler body runs. The naked stub below saves the live GP regs
//! into a `SyscallArgs` struct on the stack, calls the regular Rust
//! `syscall_dispatch`, then writes the dispatcher's return value back into
//! the saved RAX slot before restoring registers and `iretq`-ing.
//!
//! This mirrors the pattern in `preemption.rs::timer_interrupt_handler_preemptive`.

use core::arch::naked_asm;

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
