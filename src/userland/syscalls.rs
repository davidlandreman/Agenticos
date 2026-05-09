//! Linux x86-64 syscall handlers — the milestone-1 surface.
//!
//! `write(fd, buf, len)` and `exit_group(code)` are the two syscalls the
//! migrated rust runtime issues; they're enough to take the existing rust
//! `HELLO.ELF` end-to-end on the new ABI. The full set the C++ iostream
//! milestone needs (mmap/brk/arch_prctl/etc.) lands in U9.
//!
//! ## Pointer validation
//!
//! `write_handler` validates the `(ptr, len)` slice via
//! `abi::validate_user_slice`, which uses checked addition to defeat the
//! wraparound case where `ptr + len` overflows past the top of the address
//! space. Kernel-range pointers, in-range starts that span past the user VA
//! window, and `len = 0` are all handled by the same helper.
//!
//! ## Why this runs with interrupts disabled
//!
//! The SYSCALL stub leaves `IF` cleared (FMASK includes `IF`) until the
//! handler returns and `IRETQ` restores user RFLAGS. Handlers must NOT
//! panic — the panic path acquires the serial lock, which a pending IRQ
//! cannot preempt off, so panic-in-syscall-context is a guaranteed
//! deadlock. Use `Result` / negative-errno returns instead.

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::abi::{validate_user_slice, EBADF, EFAULT, LAST_EXIT_CODE};

/// Maximum bytes a single `write` call can emit. Defends against a
/// malicious or buggy user passing `len = u64::MAX` and exhausting kernel
/// time inside the handler. 4 KiB is plenty for a milestone-1 hello.
const WRITE_MAX_LEN: usize = 4096;

/// `write(fd: i32, buf: *const u8, count: usize) -> isize`
///
/// fd 1 (stdout) and fd 2 (stderr) route to the kernel serial/text path.
/// Other file descriptors return `-EBADF` until U9 lands real fd routing.
/// Returns the number of bytes written on success (always equal to `count`),
/// or a negative errno on failure.
pub fn write_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i64;
    let ptr = args.rsi;
    let len = args.rdx;

    if !(fd == 1 || fd == 2) {
        return EBADF;
    }
    if len > WRITE_MAX_LEN as u64 {
        return EFAULT;
    }
    if let Err(e) = validate_user_slice(ptr, len) {
        return e;
    }
    if len == 0 {
        return 0;
    }

    // SAFETY: validate_user_slice confirmed [ptr, ptr+len) lies inside the
    // active user-VA bounds, which the loader maps before entering ring 3.
    // Kernel-mode reads are unaffected by the USER bit.
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let s = core::str::from_utf8(slice).unwrap_or("");
    crate::print!("{}", s);

    len as i64
}

/// `exit_group(status: i32) -> !` — terminate the user process by
/// long-jumping to the saved kernel continuation.
///
/// Records the code in `LAST_EXIT_CODE` (test observability) and into the
/// active-user slot, then calls `cooperative_exit`, which never returns.
/// The dispatcher's IRETQ is therefore *not* executed — the long-jump
/// resumes the run command's frame instead.
///
/// When called from a kernel-mode test (no active continuation), falls back
/// to recording `LAST_EXIT_CODE` and returning 0 — the test scaffold drives
/// the dispatcher directly without setting up `enter_user_mode`.
pub fn exit_group_handler(args: &mut SyscallArgs) -> i64 {
    let code = args.rdi as i32 as i64;
    *LAST_EXIT_CODE.lock() = Some(code);

    let has_cont =
        crate::userland::lifecycle::with_active_user(|au| au.continuation.is_some());
    if !has_cont {
        crate::debug_info!("USERLAND: exit_group({}) recorded (no active continuation)", code);
        return 0;
    }

    crate::debug_info!("USERLAND: exit_group({}) — long-jumping to run command", code);
    crate::userland::lifecycle::cooperative_exit(code);
}
