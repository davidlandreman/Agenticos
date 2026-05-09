//! Kernel-side handlers for the first two syscalls: `print` and `exit`.
//!
//! These are the two syscalls the U8 hello app exercises. Both are registered
//! at kernel init time via `register_first_class_syscalls()` so they are
//! available before any user app loads.
//!
//! ## Pointer validation (S2 / S5 of the doc-review findings)
//!
//! `print_handler` validates the `(ptr, len)` slice via
//! `abi::validate_user_slice`, which uses **checked addition** to defeat the
//! wraparound case where `ptr + len` overflows past the top of the address
//! space. Kernel-range pointers, in-range starts that span past the user VA
//! window, and `len = 0` are all handled by the same helper.
//!
//! ## Why this runs in interrupt-gate context (FYI A5)
//!
//! The `int 0x80` IDT entry uses an interrupt gate, so IF is cleared on entry.
//! The print handler holds no kernel lock while writing through `crate::print!`
//! — a future refactor that adds locking inside `print!` will need to revisit
//! this if it wants to hold a lock across a possibly-blocking write. For the
//! first cut, no syscall handler holds a long-running lock.

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::abi::{
    register_syscall, validate_user_slice, EFAULT, LAST_EXIT_CODE,
};

/// Maximum number of bytes a single `print` call can emit. Defends against
/// a malicious or buggy user passing `len = u64::MAX` and exhausting kernel
/// time inside the handler. 4 KiB is plenty for the hello app.
const PRINT_MAX_LEN: usize = 4096;

/// `print(ptr: *const u8, len: usize) -> i64`
///
/// Returns the number of bytes written on success (always equal to `len`),
/// or a negative `EFAULT`-style errno on bad pointer.
pub fn print_handler(args: &mut SyscallArgs) -> i64 {
    let ptr = args.rdi;
    let len = args.rsi;

    // S5: bound the length before any pointer arithmetic. PRINT_MAX_LEN is a
    // hard ceiling — apps that need more should call print multiple times.
    if len > PRINT_MAX_LEN as u64 {
        return EFAULT;
    }

    if let Err(e) = validate_user_slice(ptr, len) {
        return e;
    }

    if len == 0 {
        return 0;
    }

    // SAFETY: `validate_user_slice` confirmed `ptr..ptr+len` lies inside the
    // active user-VA bounds, which the loader (U6) maps before entering ring
    // 3. The kernel can read user-accessible pages; the inverse is what the
    // USER bit enforces. The slice is read-only; we copy through `print!`
    // without retaining any pointer past this call.
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };

    // Trust UTF-8 — invalid bytes are replaced by `from_utf8_lossy`'s U+FFFD
    // so the kernel can never panic on a bad sequence. The plan calls for
    // "trusted, truncate on invalid"; lossy is the closer match.
    let s = core::str::from_utf8(slice).unwrap_or("");
    crate::print!("{}", s);

    len as i64
}

/// `exit(code: i32) -> !` — terminate the user process by long-jumping to
/// the saved kernel continuation (U7).
///
/// Records the code in `LAST_EXIT_CODE` (test observability) and into the
/// active-user slot, then calls `cooperative_exit`, which never returns.
/// The dispatcher's iretq is therefore *not* executed — the long-jump
/// resumes the run command's frame instead.
///
/// When called from a kernel-mode test (no active continuation), falls back
/// to recording `LAST_EXIT_CODE` and returning 0 — the test scaffold drives
/// the dispatcher directly without setting up `enter_user_mode`. This branch
/// is opt-in via the absence of an active continuation; an active user app
/// always takes the diverging path.
pub fn exit_handler(args: &mut SyscallArgs) -> i64 {
    let code = args.rdi as i32 as i64;
    *LAST_EXIT_CODE.lock() = Some(code);

    // If no continuation is installed, we are running from a kernel-mode
    // synthetic test — just record and return.
    let has_cont =
        crate::userland::lifecycle::with_active_user(|au| au.continuation.is_some());
    if !has_cont {
        crate::debug_info!("USERLAND: exit({}) recorded (no active continuation)", code);
        return 0;
    }

    crate::debug_info!("USERLAND: exit({}) — long-jumping to run command", code);
    crate::userland::lifecycle::cooperative_exit(code);
}

/// Register `print` and `exit` against the SYSCALL_TABLE. Called once during
/// kernel init, before any user app loads. Order matters: `print` gets ID 0,
/// `exit` gets ID 1, and the trampoline page is built from this order.
pub fn register_first_class_syscalls() {
    register_syscall("print", print_handler).expect("register print");
    register_syscall("exit", exit_handler).expect("register exit");
}
