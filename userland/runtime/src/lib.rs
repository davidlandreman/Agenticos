//! Userland runtime: minimal shims for ring-3 apps.
//!
//! Ships the two syscall stubs (`print`, `exit`) as concrete bodies that
//! issue `int 0x80` directly with the kernel-assigned syscall IDs. This is
//! the **numeric-stub** path described as the U8 fallback in plan
//! `2026-05-08-004` — chosen as the actual primary because a static `-no-pie`
//! link in `lld` will not emit `R_X86_64_GLOB_DAT` / `R_X86_64_JUMP_SLOT`
//! relocations against undefined externals; those become hard link errors.
//!
//! The U6 loader's relocation walk is happy with this — it simply finds no
//! relocations to resolve. The kernel-mapped user-trampoline page (U5) still
//! exists at `0x0090_0000` and is safe to leave unused; future apps that DO
//! want symbol-keyed resolution (e.g., dynamically linked or built with a
//! different toolchain) can rely on it.
//!
//! Syscall IDs come from the kernel's `crate::userland::abi` registry order:
//!   id 0 -> `print(ptr, len) -> i64`
//!   id 1 -> `exit(code) -> !`
//!
//! Calling convention (mirrors the kernel's `SyscallArgs`):
//!   - rax = syscall id
//!   - rdi = arg0
//!   - rsi = arg1
//!   - return value lands in rax

#![no_std]

/// Print `len` bytes starting at `ptr` to the active terminal.
///
/// Returns 0 on success or a negative errno-style value (e.g., `-14` /
/// `EFAULT`) on failure. Callers must include any trailing newline; the
/// kernel does not append one.
///
/// # Safety
///
/// `ptr` must point to at least `len` valid, mapped, user-accessible bytes.
/// Passing a kernel-range pointer is rejected by the kernel without
/// dereferencing it.
#[inline]
pub unsafe fn print(ptr: *const u8, len: usize) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "int 0x80",
        inout("rax") 0u64 => ret,
        in("rdi") ptr,
        in("rsi") len,
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
        "int 0x80",
        in("rax") 1u64,
        in("rdi") code,
        options(nostack, noreturn),
    );
}
