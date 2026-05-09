//! Linux x86-64 ABI surface.
//!
//! Userland enters the kernel via the `syscall` instruction (programmed in
//! `arch::x86_64::syscall::init_syscall_msrs`). Syscall numbers and the
//! argument-register convention follow Linux x86-64:
//!
//! ```text
//!   RAX = syscall number  (return value on exit)
//!   RDI = arg1
//!   RSI = arg2
//!   RDX = arg3
//!   R10 = arg4   (System V uses RCX; the syscall instruction overwrites it
//!                with the user RIP, so the kernel ABI moves arg4 to R10)
//!   R8  = arg5
//!   R9  = arg6
//! ```
//!
//! Errors are returned as `-errno` in RAX. Negative-errno-style values
//! follow the Linux convention so unmodified static musl/glibc binaries
//! interpret them correctly without translation.
//!
//! This module owns the dispatcher and the small set of utilities every
//! handler shares: pointer-slice validation against the active user-VA
//! window, and the `LAST_EXIT_CODE` mirror that lets in-kernel tests
//! observe `exit_group(code)` without setting up a full ring-3 process.

use spin::Mutex;
use x86_64::VirtAddr;

use crate::arch::x86_64::syscall::SyscallArgs;

/// Linux negative-errno sentinels surfaced by handlers in this kernel.
pub const ENOSYS: i64 = -38;
pub const EFAULT: i64 = -14;
pub const EBADF: i64 = -9;
pub const EINVAL: i64 = -22;
pub const ENOTTY: i64 = -25;

/// Active user-VA bounds (inclusive lower, exclusive upper). Populated by
/// `enter_user_mode` before `iretq`-to-ring-3, cleared on exit. Pointer
/// validation in user-buffer-touching syscalls (e.g., `write`) consumes
/// this — when `None`, all user pointers are rejected (no active user
/// process means no valid user pointers).
///
/// Tests drive this directly via `set_user_va_bounds` / `clear_user_va_bounds`
/// to exercise the dispatcher without spinning up a full user process.
#[derive(Debug, Clone, Copy)]
pub struct UserVaBounds {
    pub start: u64,
    pub end: u64,
}

static USER_VA_BOUNDS: Mutex<Option<UserVaBounds>> = Mutex::new(None);

/// Last `exit_group` exit code — visible to in-kernel tests so they can
/// assert `exit_group(42)` recorded `42`. Real ring-3 entry routes through
/// `lifecycle::cooperative_exit` which long-jumps; the test path runs the
/// dispatcher directly without an active continuation, falls back to
/// recording here, and returns to the caller.
pub static LAST_EXIT_CODE: Mutex<Option<i64>> = Mutex::new(None);

pub fn set_user_va_bounds(bounds: UserVaBounds) {
    *USER_VA_BOUNDS.lock() = Some(bounds);
}

pub fn clear_user_va_bounds() {
    *USER_VA_BOUNDS.lock() = None;
}

pub fn user_va_bounds() -> Option<UserVaBounds> {
    *USER_VA_BOUNDS.lock()
}

/// Validate that a user-supplied `(ptr, len)` slice lies entirely within
/// the active user-VA bounds. `ptr + len` is computed with `checked_add`
/// to defeat integer wraparound near the top of the address space. A
/// `len` of 0 is valid and returns `Ok(())` regardless of `ptr`.
pub fn validate_user_slice(ptr: u64, len: u64) -> Result<(), i64> {
    if len == 0 {
        return Ok(());
    }
    let bounds = user_va_bounds().ok_or(EFAULT)?;
    let end = ptr.checked_add(len).ok_or(EFAULT)?;
    if ptr < bounds.start || end > bounds.end {
        return Err(EFAULT);
    }
    if VirtAddr::try_new(ptr).is_err() || VirtAddr::try_new(end).is_err() {
        return Err(EFAULT);
    }
    Ok(())
}

/// Linux x86-64 syscall numbers this kernel handles. The full surface lives
/// downstream in `syscalls.rs`; the dispatcher below routes by these.
pub mod nr {
    pub const WRITE: u64 = 1;
    pub const EXIT_GROUP: u64 = 231;
}

/// Central syscall dispatcher. Called from the naked SYSCALL entry stub in
/// `arch::x86_64::syscall` (via `syscall_dispatch_entry`). Routes by the
/// syscall number in `args.rax`. Unhandled numbers return `-ENOSYS`; U10
/// will replace this with a clean per-process termination via the existing
/// fault-cleanup path.
pub fn syscall_dispatch(args: &mut SyscallArgs) -> i64 {
    use crate::userland::syscalls;
    match args.rax {
        nr::WRITE => syscalls::write_handler(args),
        nr::EXIT_GROUP => syscalls::exit_group_handler(args),
        _ => ENOSYS,
    }
}
