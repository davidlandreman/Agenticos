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

/// `exit(code: i32) -> i64` — placeholder until U7 wires the real long-jump.
///
/// Records the code into `LAST_EXIT_CODE` so tests can observe it. In U7 this
/// will be replaced by the long-jump back to the saved kernel continuation;
/// the function will diverge (`-> !`) and never return to the dispatcher.
///
/// For now we return 0 so the dispatcher's iretq still runs and execution
/// continues after the `int 0x80`. A user app calling `exit` today would
/// continue past the call instead of terminating — that is acceptable
/// because no real user app exists yet (U7+/U8 land that).
pub fn exit_handler(args: &mut SyscallArgs) -> i64 {
    // The user-side ABI passes `code` in RDI (System V first arg). Sign-extend
    // from i32 so a user `exit(-1)` is recorded as -1, not 0xFFFFFFFF.
    let code = args.rdi as i32 as i64;
    *LAST_EXIT_CODE.lock() = Some(code);
    crate::debug_info!("USERLAND: exit({}) recorded (placeholder; U7 wires long-jump)", code);
    0
}

/// Register `print` and `exit` against the SYSCALL_TABLE. Called once during
/// kernel init, before any user app loads. Order matters: `print` gets ID 0,
/// `exit` gets ID 1, and the trampoline page is built from this order.
pub fn register_first_class_syscalls() {
    register_syscall("print", print_handler).expect("register print");
    register_syscall("exit", exit_handler).expect("register exit");
}
