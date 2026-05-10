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

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;
use x86_64::VirtAddr;

use crate::arch::x86_64::syscall::SyscallArgs;

/// Linux negative-errno sentinels surfaced by handlers in this kernel.
pub const ENOSYS: i64 = -38;
pub const EFAULT: i64 = -14;
pub const EBADF: i64 = -9;
pub const EINVAL: i64 = -22;
pub const ENOTTY: i64 = -25;
pub const ENOENT: i64 = -2;
pub const EIO: i64 = -5;
pub const EACCES: i64 = -13;
pub const EEXIST: i64 = -17;
pub const ENOTDIR: i64 = -20;
pub const EISDIR: i64 = -21;
pub const EMFILE: i64 = -24;
pub const ESPIPE: i64 = -29;
pub const EROFS: i64 = -30;
pub const ERANGE: i64 = -34;
pub const ENAMETOOLONG: i64 = -36;
pub const ENOTSUP: i64 = -95;
pub const ECHILD: i64 = -10;
pub const EAGAIN: i64 = -11;
pub const EPIPE: i64 = -32;

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

/// When true, `unhandled_syscall` returns `-ENOSYS` to the caller and
/// logs the first occurrence of each syscall number at info, subsequent
/// occurrences at trace, instead of terminating the process. Off by
/// default in production builds; tests and discovery boots flip it on
/// via `set_trace_mode` so a static-musl binary can survive past
/// unimplemented syscalls and reveal its full surface.
static TRACE_MODE: AtomicBool = AtomicBool::new(false);

/// Once-per-syscall-number bookkeeping for trace mode. `SEEN_NRS[n]`
/// flips false → true on the first unhandled occurrence of nr `n`. Linux
/// x86-64 has ~330 numbers; 512 is a safe ceiling that keeps the
/// indexing branchless and avoids a heap-backed map on the dispatcher
/// hot path. Numbers ≥ 512 are not tracked individually but still log +
/// return `-ENOSYS` (with the first-occurrence fast-path effectively
/// inverted: every call logs at info because we never set a sticky bit).
const TRACE_NR_CAPACITY: usize = 512;
static SEEN_NRS: [AtomicBool; TRACE_NR_CAPACITY] = {
    const INIT: AtomicBool = AtomicBool::new(false);
    [INIT; TRACE_NR_CAPACITY]
};

pub fn set_trace_mode(enabled: bool) {
    TRACE_MODE.store(enabled, Ordering::Relaxed);
}

pub fn is_trace_mode() -> bool {
    TRACE_MODE.load(Ordering::Relaxed)
}

/// Clear the per-syscall-number "already logged" bookkeeping. Called by
/// the run command before launching a user binary so each launch starts
/// the discovery loop fresh.
pub fn reset_unknown_syscall_trace() {
    for slot in SEEN_NRS.iter() {
        slot.store(false, Ordering::Relaxed);
    }
}

/// Test-visible probe: did the trace machinery record a first-occurrence
/// log for this number since the last `reset_unknown_syscall_trace`?
/// Returns false for numbers ≥ `TRACE_NR_CAPACITY` (those are logged on
/// every occurrence rather than tracked individually).
pub fn unknown_syscall_was_seen(nr: u64) -> bool {
    let idx = nr as usize;
    if idx >= TRACE_NR_CAPACITY {
        return false;
    }
    SEEN_NRS[idx].load(Ordering::Relaxed)
}

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

/// Linux x86-64 syscall numbers this kernel handles. The full surface
/// lives in `syscalls.rs`; the dispatcher below routes by these. Numbers
/// taken from `arch/x86/entry/syscalls/syscall_64.tbl` in the Linux
/// kernel.
pub mod nr {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const OPEN: u64 = 2;
    pub const CLOSE: u64 = 3;
    pub const STAT: u64 = 4;
    pub const FSTAT: u64 = 5;
    pub const LSTAT: u64 = 6;
    pub const POLL: u64 = 7;
    pub const LSEEK: u64 = 8;
    pub const MMAP: u64 = 9;
    pub const MPROTECT: u64 = 10;
    pub const MUNMAP: u64 = 11;
    pub const BRK: u64 = 12;
    pub const RT_SIGACTION: u64 = 13;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const IOCTL: u64 = 16;
    pub const WRITEV: u64 = 20;
    pub const NANOSLEEP: u64 = 35;
    pub const SETITIMER: u64 = 38;
    pub const ACCESS: u64 = 21;
    pub const DUP: u64 = 32;
    pub const DUP2: u64 = 33;
    pub const GETPID: u64 = 39;
    pub const EXIT: u64 = 60;
    pub const UNAME: u64 = 63;
    pub const FCNTL: u64 = 72;
    pub const GETCWD: u64 = 79;
    pub const CHDIR: u64 = 80;
    pub const FCHDIR: u64 = 81;
    pub const ARCH_PRCTL: u64 = 158;
    pub const GETUID: u64 = 102;
    pub const GETGID: u64 = 104;
    pub const GETEUID: u64 = 107;
    pub const GETEGID: u64 = 108;
    pub const GETPPID: u64 = 110;
    pub const GETTIMEOFDAY: u64 = 96;
    pub const GETRLIMIT: u64 = 97;
    pub const READLINK: u64 = 89;
    pub const SET_TID_ADDRESS: u64 = 218;
    pub const CLOCK_GETTIME: u64 = 228;
    pub const EXIT_GROUP: u64 = 231;
    pub const OPENAT: u64 = 257;
    pub const NEWFSTATAT: u64 = 262;
    pub const READLINKAT: u64 = 267;
    pub const FACCESSAT: u64 = 269;
    pub const PSELECT6: u64 = 270;
    pub const PPOLL: u64 = 271;
    pub const SET_ROBUST_LIST: u64 = 273;
    pub const PRLIMIT64: u64 = 302;
    pub const GETDENTS64: u64 = 217;
    pub const GETRANDOM: u64 = 318;
    // Phase 4 PR-C: process management
    pub const FORK: u64 = 57;
    pub const VFORK: u64 = 58;
    pub const EXECVE: u64 = 59;
    pub const WAIT4: u64 = 61;
    pub const CLONE: u64 = 56;
    pub const PIPE: u64 = 22;
    pub const PIPE2: u64 = 293;
    // Phase 5 PR-B: signals
    pub const KILL: u64 = 62;
    pub const TKILL: u64 = 200;
    pub const TGKILL: u64 = 234;
    pub const RT_SIGRETURN: u64 = 15;
}

/// Central syscall dispatcher. Called from the naked SYSCALL entry stub in
/// `arch::x86_64::syscall` (via `syscall_dispatch_entry`). Routes by the
/// syscall number in `args.rax`. Unhandled numbers return `-ENOSYS`; U10
/// will replace the default arm with a clean per-process termination via
/// the existing fault-cleanup path.
///
/// Phase 5 PR-B2: at dispatcher entry we capture user callee-saved
/// registers (rbx/rbp/r12-r15) — they still hold user values here
/// because the syscall stub didn't touch them and the dispatcher is
/// the first Rust function called. After the syscall handler returns,
/// we check whether a signal handler should run; if so, we build a
/// signal frame on the user stack and `iretq` straight into the
/// handler instead of returning normally.
pub fn syscall_dispatch(args: &mut SyscallArgs) -> i64 {
    use crate::userland::syscalls;
    use crate::userland::user_state::{capture_callee_saved, CalleeSavedSnapshot};

    // Capture callee-saved registers BEFORE any other code can clobber
    // them. The naked-asm helper does `mov [rdi + N], reg` for each;
    // `r12` here is the user RSP that the SYSCALL stub stashed before
    // calling us. The original user R12 lives on the kernel stack at
    // `args + 72`.
    let mut callee = CalleeSavedSnapshot::default();
    unsafe { capture_callee_saved(&mut callee as *mut _); }

    let result = match args.rax {
        // Phase 1: streams + memory + signal stubs
        nr::READ => syscalls::read_handler(args),
        nr::WRITE => syscalls::write_handler(args),
        nr::WRITEV => syscalls::writev_handler(args),
        nr::MMAP => syscalls::mmap_handler(args),
        nr::MPROTECT => syscalls::mprotect_handler(args),
        nr::MUNMAP => syscalls::munmap_handler(args),
        nr::BRK => syscalls::brk_handler(args),
        nr::RT_SIGACTION => syscalls::rt_sigaction_handler(args),
        nr::RT_SIGPROCMASK => syscalls::rt_sigprocmask_handler(args),
        nr::IOCTL => syscalls::ioctl_handler(args),
        // U3: musl-init / zsh-startup surface
        nr::POLL => syscalls::poll_handler(args),
        nr::PPOLL => syscalls::ppoll_handler(args),
        nr::PSELECT6 => syscalls::pselect6_handler(args),
        nr::READLINK => syscalls::readlink_handler(args),
        nr::READLINKAT => syscalls::readlinkat_handler(args),
        nr::GETRLIMIT => syscalls::getrlimit_handler(args),
        nr::PRLIMIT64 => syscalls::prlimit64_handler(args),
        nr::SETITIMER => syscalls::setitimer_handler(args),
        nr::NANOSLEEP => syscalls::nanosleep_handler(args),
        nr::ARCH_PRCTL => syscalls::arch_prctl_handler(args),
        nr::SET_TID_ADDRESS => syscalls::set_tid_address_handler(args),
        nr::SET_ROBUST_LIST => syscalls::set_robust_list_handler(args),
        // Phase 2: files
        nr::OPEN => syscalls::open_handler(args),
        nr::OPENAT => syscalls::openat_handler(args),
        nr::CLOSE => syscalls::close_handler(args),
        nr::LSEEK => syscalls::lseek_handler(args),
        nr::DUP => syscalls::dup_handler(args),
        nr::DUP2 => syscalls::dup2_handler(args),
        nr::FCNTL => syscalls::fcntl_handler(args),
        // Phase 2: stat / access
        nr::STAT => syscalls::stat_handler(args),
        nr::LSTAT => syscalls::stat_handler(args),
        nr::FSTAT => syscalls::fstat_handler(args),
        nr::NEWFSTATAT => syscalls::newfstatat_handler(args),
        nr::ACCESS => syscalls::access_handler(args),
        nr::FACCESSAT => syscalls::faccessat_handler(args),
        nr::GETDENTS64 => syscalls::getdents64_handler(args),
        // Phase 2: cwd
        nr::GETCWD => syscalls::getcwd_handler(args),
        nr::CHDIR => syscalls::chdir_handler(args),
        nr::FCHDIR => syscalls::fchdir_handler(args),
        // Phase 2: time / random / uname
        nr::CLOCK_GETTIME => syscalls::clock_gettime_handler(args),
        nr::GETTIMEOFDAY => syscalls::gettimeofday_handler(args),
        nr::GETRANDOM => syscalls::getrandom_handler(args),
        nr::UNAME => syscalls::uname_handler(args),
        // Credentials and exits
        nr::GETPID => syscalls::getpid_handler(args),
        nr::GETUID => syscalls::getuid_handler(args),
        nr::GETGID => syscalls::getgid_handler(args),
        nr::GETEUID => syscalls::geteuid_handler(args),
        nr::GETEGID => syscalls::getegid_handler(args),
        nr::GETPPID => syscalls::getppid_handler(args),
        // exit (60) is the single-threaded variant; route to exit_group
        // so the kernel-side cleanup is identical.
        nr::EXIT => syscalls::exit_group_handler(args),
        nr::EXIT_GROUP => syscalls::exit_group_handler(args),
        // Phase 4 PR-C: process management. Stubs return -ENOSYS for
        // fork/vfork/clone/execve; wait4 returns -ECHILD so libc's
        // "no children to wait for" branch fires cleanly.
        nr::FORK => syscalls::fork_handler(args),
        nr::VFORK => syscalls::vfork_handler(args),
        nr::CLONE => syscalls::clone_handler(args),
        nr::EXECVE => syscalls::execve_handler(args),
        nr::WAIT4 => syscalls::wait4_handler(args),
        nr::PIPE => syscalls::pipe_handler(args),
        nr::PIPE2 => syscalls::pipe2_handler(args),
        nr::KILL => syscalls::kill_handler(args),
        nr::TKILL => syscalls::tkill_handler(args),
        nr::TGKILL => syscalls::tgkill_handler(args),
        nr::RT_SIGRETURN => syscalls::rt_sigreturn_handler(args),
        _ => unhandled_syscall(args),
    };

    // Phase 5 PR-B2: deliver a pending signal if one is queued. This
    // diverges (iretq into the handler) — control never returns
    // here. If no signal is pending, return the syscall result
    // normally and let the SYSCALL stub iretq back to the caller.
    let _ = syscalls::maybe_deliver_signal(callee, args, result);
    result
}

/// Default arm for `syscall_dispatch`. Two behavior modes:
///
/// **Trace mode off** (default): routes to a clean per-process termination
/// via the kernel-continuation long-jump when an active user process
/// issued the syscall; falls back to returning `-ENOSYS` for the
/// in-kernel test path that exercises the dispatcher without an
/// `enter_user_mode`.
///
/// **Trace mode on** (set via `set_trace_mode(true)`, intended for
/// discovery boots and tests): logs the first occurrence of each
/// syscall number at info with a full arg dump, subsequent occurrences
/// at trace, and returns `-ENOSYS` to the caller. This lets a binary
/// continue past unhandled syscalls so we can see the full surface it
/// touches in one boot rather than one syscall at a time.
fn unhandled_syscall(args: &SyscallArgs) -> i64 {
    let nr = args.rax;
    if is_trace_mode() {
        // Per-nr "already logged" check is a swap so the test is itself
        // the bookkeeping update. Numbers ≥ TRACE_NR_CAPACITY can't be
        // tracked individually — log them every time at info to avoid
        // silently swallowing them.
        let already_seen = if (nr as usize) < TRACE_NR_CAPACITY {
            SEEN_NRS[nr as usize].swap(true, Ordering::Relaxed)
        } else {
            false
        };
        if !already_seen {
            crate::debug_info!(
                "[strace] first unknown nr={} args=({:#x},{:#x},{:#x},{:#x},{:#x},{:#x})",
                nr, args.rdi, args.rsi, args.rdx, args.r10, args.r8, args.r9
            );
        } else {
            crate::debug_trace!("[strace] nr={}", nr);
        }
        return ENOSYS;
    }
    let has_cont = crate::userland::lifecycle::with_active_user(|au| au.continuation.is_some());
    if has_cont {
        crate::userland::lifecycle::unimplemented_syscall_exit(nr);
    }
    // Test path: the dispatcher is being driven from kernel mode
    // synthetically. Returning -ENOSYS keeps unit tests deterministic.
    crate::debug_warn!("syscall_dispatch: unimplemented nr={} (no active continuation; returning -ENOSYS)", nr);
    ENOSYS
}
