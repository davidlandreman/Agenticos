//! Userland runtime: minimal Linux x86-64 ABI shims for ring-3 apps.
//!
//! Issues the `syscall` instruction directly with Linux numbers. The kernel
//! (in `src/arch/x86_64/syscall.rs::syscall_fastpath_entry`) handles the
//! ABI: arguments in RDI/RSI/RDX/R10/R8/R9, syscall number in RAX, return
//! value in RAX, errors as `-errno` in RAX, RCX/R11 clobbered by the
//! `syscall` instruction itself.
//!
//! Two stubs are exposed:
//!
//! - `print(ptr, len)` — wraps `write(1, ptr, len)`. Returns the byte
//!   count on success, a negative errno on failure.
//! - `exit(code)` — wraps `exit_group(code)`. Diverges; never returns.
//!
//! Linux syscall numbers are stable: `write = 1`, `exit_group = 231`. The
//! kernel side mirrors these constants in `src/userland/abi.rs::nr`.

#![no_std]

const NR_WRITE: u64 = 1;
const NR_EXIT_GROUP: u64 = 231;

const STDOUT_FD: i64 = 1;

/// Print `len` bytes starting at `ptr` to stdout (the active terminal).
///
/// Returns `len` on success or a negative `-errno` value on failure
/// (e.g., `-14` / `EFAULT` for a bad pointer, `-9` / `EBADF` for a
/// closed fd — though stdout is always considered open).
///
/// # Safety
///
/// `ptr` must point to at least `len` valid, mapped, user-accessible bytes.
/// A kernel-range pointer is rejected by the kernel without dereferencing
/// it.
#[inline]
pub unsafe fn print(ptr: *const u8, len: usize) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inout("rax") NR_WRITE => ret,
        in("rdi") STDOUT_FD,
        in("rsi") ptr,
        in("rdx") len,
        // The `syscall` instruction always clobbers RCX (return RIP) and
        // R11 (return RFLAGS); the kernel additionally treats them as
        // caller-saved.
        out("rcx") _,
        out("r11") _,
        options(nostack, preserves_flags),
    );
    ret
}

/// Terminate the current user process with `code`. Does not return.
///
/// The kernel records the exit code on the active user PCB and long-jumps
/// back to the saved `run`-command continuation, tearing down the process.
#[inline]
pub unsafe fn exit(code: i64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") NR_EXIT_GROUP,
        in("rdi") code,
        // Doesn't return, but list the syscall-clobbered regs anyway for
        // documentation; `noreturn` makes the listed-clobber set advisory.
        options(nostack, noreturn),
    );
}
