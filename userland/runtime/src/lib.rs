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

/// AgenticOS-internal `gui_launch(name_ptr, name_len)` syscall. Returns
/// 0 on success or a negative errno on failure. Kernel-side number is
/// pinned in `src/userland/abi.rs::nr::GUI_LAUNCH`; must match.
///
/// # Safety
///
/// `ptr` must point to at least `len` valid, mapped, user-accessible bytes
/// containing a UTF-8 applet name (≤ 32 bytes).
const NR_GUI_LAUNCH: u64 = 5000;
#[inline]
pub unsafe fn gui_launch(ptr: *const u8, len: usize) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inout("rax") NR_GUI_LAUNCH => ret,
        in("rdi") ptr,
        in("rsi") len,
        out("rcx") _,
        out("r11") _,
        options(nostack, preserves_flags),
    );
    ret
}

/// Walk the initial stack laid out by the kernel and return `argv[0]` as
/// a `(ptr, len)` byte slice (NUL-terminator excluded). Returns
/// `(ptr::null(), 0)` if argc is 0 or argv[0] is NULL.
///
/// Layout at process entry (RSP at `_start`):
/// ```text
///   [rsp + 0]            argc (u64)
///   [rsp + 8]            argv[0]      <- *const u8
///   [rsp + 16]           argv[1]
///   ...
///   [rsp + 8*(argc+1)]   NULL
///   ...                  envp[0]...NULL, auxv...
/// ```
///
/// # Safety
///
/// `stack_top` must be the kernel-supplied initial RSP, otherwise the
/// derived pointers are undefined.
#[inline]
pub unsafe fn argv0_from_stack(stack_top: *const u64) -> (*const u8, usize) {
    let argc = core::ptr::read(stack_top);
    if argc == 0 {
        return (core::ptr::null(), 0);
    }
    let argv0_ptr = core::ptr::read(stack_top.add(1)) as *const u8;
    if argv0_ptr.is_null() {
        return (core::ptr::null(), 0);
    }
    // Compute strlen — cap at 256 so a corrupt argv can't run forever.
    let mut len = 0;
    while len < 256 {
        if core::ptr::read_volatile(argv0_ptr.add(len)) == 0 {
            break;
        }
        len += 1;
    }
    (argv0_ptr, len)
}
