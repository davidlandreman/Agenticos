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
pub const ECHILD: i64 = -10;
pub const EAGAIN: i64 = -11;
pub const EPIPE: i64 = -32;
pub const EINTR: i64 = -4;
pub const EPERM: i64 = -1;
pub const ESRCH: i64 = -3;
pub const ENOSPC: i64 = -28;
pub const EBUSY: i64 = -16;
pub const EXDEV: i64 = -18;
pub const EFBIG: i64 = -27;
pub const ENOTEMPTY: i64 = -39;
pub const ENOMEM: i64 = -12;
pub const ENFILE: i64 = -23;
pub const ENOLCK: i64 = -37;
pub const EAFNOSUPPORT: i64 = -97;
pub const EPROTONOSUPPORT: i64 = -93;
pub const EOPNOTSUPP: i64 = -95;
pub const ENOTCONN: i64 = -107;
pub const ENOTSUP: i64 = -95;
pub const EISCONN: i64 = -106;
pub const EINPROGRESS: i64 = -115;
pub const EALREADY: i64 = -114;
pub const ECONNREFUSED: i64 = -111;
pub const EADDRINUSE: i64 = -98;
pub const EADDRNOTAVAIL: i64 = -99;
pub const ETIMEDOUT: i64 = -110;
pub const ENOBUFS: i64 = -105;
pub const EMSGSIZE: i64 = -90;
pub const EDESTADDRREQ: i64 = -89;
pub const ENOPROTOOPT: i64 = -92;
pub const ENETDOWN: i64 = -100;
pub const ENETUNREACH: i64 = -101;

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
/// assert `exit_group(42)` recorded `42`. A synthetic dispatch with no real
/// ring-3 process (no PID or the PID-0 kernel sentinel) records here and
/// returns to the caller; real ring-3 entry continues through lifecycle
/// teardown.
pub static LAST_EXIT_CODE: Mutex<Option<i64>> = Mutex::new(None);

/// When true, `unhandled_syscall` logs the first occurrence of each syscall
/// number at info and subsequent occurrences at trace. Unknown syscalls return
/// `-ENOSYS` in every mode; trace mode changes diagnostics only.
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

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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
#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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

#[cfg_attr(not(feature = "test"), expect(dead_code, reason = "QEMU test API"))]
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
    let end = ptr.checked_add(len).ok_or(EFAULT)?;
    if VirtAddr::try_new(ptr).is_err() || VirtAddr::try_new(end).is_err() {
        return Err(EFAULT);
    }
    let vma_result = crate::userland::lifecycle::with_current_group(|process| {
        process.address_space.as_ref().map(|space| {
            space
                .vmas()
                .covers(ptr, len, crate::userland::vm::VmProt::READ)
        })
    });
    if let Some(covered) = vma_result {
        if !covered {
            return Err(EFAULT);
        }
        return crate::userland::usercopy::ensure_user_range(ptr, len, false);
    }
    let bounds = user_va_bounds().ok_or(EFAULT)?;
    if ptr < bounds.start || end > bounds.end {
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
    pub const READV: u64 = 19;
    pub const WRITEV: u64 = 20;
    pub const SELECT: u64 = 23;
    pub const SCHED_YIELD: u64 = 24;
    pub const MREMAP: u64 = 25;
    pub const MADVISE: u64 = 28;
    pub const NANOSLEEP: u64 = 35;
    pub const SETITIMER: u64 = 38;
    pub const SOCKET: u64 = 41;
    pub const CONNECT: u64 = 42;
    pub const ACCEPT: u64 = 43;
    pub const SENDTO: u64 = 44;
    pub const RECVFROM: u64 = 45;
    pub const SENDMSG: u64 = 46;
    pub const RECVMSG: u64 = 47;
    pub const SHUTDOWN: u64 = 48;
    pub const BIND: u64 = 49;
    pub const LISTEN: u64 = 50;
    pub const GETSOCKNAME: u64 = 51;
    pub const GETPEERNAME: u64 = 52;
    pub const SOCKETPAIR: u64 = 53;
    pub const SETSOCKOPT: u64 = 54;
    pub const GETSOCKOPT: u64 = 55;
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
    pub const GETTID: u64 = 186;
    pub const FUTEX: u64 = 202;
    pub const SCHED_GETAFFINITY: u64 = 204;
    pub const GETUID: u64 = 102;
    pub const GETGID: u64 = 104;
    pub const GETEUID: u64 = 107;
    pub const GETEGID: u64 = 108;
    pub const GETPPID: u64 = 110;
    pub const GETTIMEOFDAY: u64 = 96;
    pub const UMASK: u64 = 95;
    pub const GETRLIMIT: u64 = 97;
    pub const GETRUSAGE: u64 = 98;
    pub const SYSINFO: u64 = 99;
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
    pub const UTIMENSAT: u64 = 280;
    pub const ACCEPT4: u64 = 288;
    pub const PRLIMIT64: u64 = 302;
    pub const GETDENTS64: u64 = 217;
    pub const GETRANDOM: u64 = 318;
    // Phase B (FS writes): mutations on the now-writable namespace.
    pub const TRUNCATE: u64 = 76;
    pub const FTRUNCATE: u64 = 77;
    pub const RENAME: u64 = 82;
    pub const MKDIR: u64 = 83;
    pub const RMDIR: u64 = 84;
    pub const CREAT: u64 = 85;
    pub const UNLINK: u64 = 87;
    pub const LINK: u64 = 86;
    pub const SYMLINK: u64 = 88;
    pub const CHMOD: u64 = 90;
    pub const FCHMOD: u64 = 91;
    pub const FSYNC: u64 = 74;
    pub const FDATASYNC: u64 = 75;
    pub const SYNC: u64 = 162;
    pub const PREAD64: u64 = 17;
    pub const PWRITE64: u64 = 18;
    pub const SENDFILE: u64 = 40;
    pub const MKDIRAT: u64 = 258;
    pub const UNLINKAT: u64 = 263;
    pub const RENAMEAT: u64 = 264;
    pub const LINKAT: u64 = 265;
    pub const SYMLINKAT: u64 = 266;
    pub const SYNCFS: u64 = 306;
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
    pub const RT_SIGSUSPEND: u64 = 130;
    pub const SIGALTSTACK: u64 = 131;
    pub const EPOLL_CREATE: u64 = 213;
    pub const EPOLL_WAIT: u64 = 232;
    pub const EPOLL_CTL: u64 = 233;
    pub const EPOLL_PWAIT: u64 = 281;
    pub const EVENTFD: u64 = 284;
    pub const EVENTFD2: u64 = 290;
    pub const EPOLL_CREATE1: u64 = 291;
    pub const MEMBARRIER: u64 = 324;

    // AgenticOS-internal syscalls. Numbers picked well above the Linux
    // x86-64 range (currently ~450, growing) so a future Linux number
    // never collides with ours. 5000+ is reserved for AgenticOS.
    /// `sys_gui_launch(name_ptr: *const u8, name_len: usize) -> 0 | -errno`.
    /// Looks `name` up in the kernel-side GUI applet table
    /// (`src/commands/gui_launch_table.rs`) and spawns the matching
    /// kernel-side GUI process. Called by `GLAUNCH.ELF` (ring 3) when
    /// the user types e.g. `painting` in zsh.
    pub const GUI_LAUNCH: u64 = 5000;
    pub const GUI_WIN_CREATE: u64 = 5001;
    pub const GUI_WIN_PRESENT: u64 = 5002;
    pub const GUI_NEXT_EVENT: u64 = 5003;
    pub const GUI_WIN_DESTROY: u64 = 5004;
    pub const GUI_WIN_SET_TITLE: u64 = 5005;
    pub const GUI_GL_CONTEXT_CREATE: u64 = 5006;
    pub const GUI_GL_SUBMIT_FRAME: u64 = 5007;
    pub const GUI_GL_GET_INFO: u64 = 5008;
    pub const GUI_GL_CONTEXT_DESTROY: u64 = 5009;
    pub const SYSTEM_CONTROL: u64 = 5010;
    pub const GUI_EVENT_OPEN: u64 = 5011;
    pub const CLIPBOARD: u64 = 5012;
    /// Open the master end of a pty for the caller's GUI window
    /// (`TERMINAL.ELF`). Returns a `FdSlot::PtyMaster` descriptor.
    pub const PTY_OPEN: u64 = 5013;
    /// Update a pty master's winsize and raise SIGWINCH on the child.
    pub const PTY_SET_WINSIZE: u64 = 5014;
    /// Desktop-shell protocol (`DESKTOP.ELF` only, gated by
    /// `gui::register_desktop_shell`).
    pub const GUI_SHELL_REGISTER: u64 = 5015;
    pub const GUI_SHELL_LIST_WINDOWS: u64 = 5016;
    pub const GUI_SHELL_WINDOW_ACTION: u64 = 5017;
}

/// Central syscall dispatcher. Called from the naked SYSCALL entry stub in
/// `arch::x86_64::syscall` (via `syscall_dispatch_entry`). Routes by the
/// syscall number in `args.rax`. Unhandled numbers return `-ENOSYS` so libc
/// feature probes and build tools can select their fallback paths.
///
/// After the syscall handler returns, we check whether a signal
/// handler should run; if so, we build a signal frame on the user
/// stack and `iretq` straight into the handler instead of returning
/// normally. User callee-saved registers needed for the signal frame
/// are read from the explicit kernel-stack slots the SYSCALL stub
/// pushed (see [`crate::userland::user_state::read_user_callee_saved`]).
pub fn syscall_dispatch(args: &mut SyscallArgs) -> i64 {
    use crate::userland::syscalls;

    // A blocking syscall resumes by re-executing its SYSCALL instruction.
    // If an asynchronous signal woke it, deliver the handler before calling
    // the syscall implementation again; otherwise recv/read would simply
    // park a second time and the pending signal could never run.
    if crate::userland::lifecycle::take_pending_syscall_interrupt() {
        let _ = syscalls::maybe_deliver_signal(args, EINTR);
    }

    crate::userland::lifecycle::clear_stale_network_wait(args.rax);

    let result = match args.rax {
        // Phase 1: streams + memory + signal stubs
        nr::READ => syscalls::read_handler(args),
        nr::WRITE => syscalls::write_handler(args),
        nr::READV => syscalls::readv_handler(args),
        nr::WRITEV => syscalls::writev_handler(args),
        nr::MMAP => syscalls::mmap_handler(args),
        nr::MPROTECT => syscalls::mprotect_handler(args),
        nr::MUNMAP => syscalls::munmap_handler(args),
        nr::BRK => syscalls::brk_handler(args),
        nr::RT_SIGACTION => syscalls::rt_sigaction_handler(args),
        nr::RT_SIGPROCMASK => syscalls::rt_sigprocmask_handler(args),
        nr::IOCTL => syscalls::ioctl_handler(args),
        nr::SOCKET => crate::userland::network_syscalls::socket_handler(args),
        nr::CONNECT => crate::userland::network_syscalls::connect_handler(args),
        nr::ACCEPT => crate::userland::network_syscalls::accept_handler(args),
        nr::ACCEPT4 => crate::userland::network_syscalls::accept4_handler(args),
        nr::SENDTO => crate::userland::network_syscalls::sendto_handler(args),
        nr::RECVFROM => crate::userland::network_syscalls::recvfrom_handler(args),
        nr::SENDMSG => crate::userland::network_syscalls::sendmsg_handler(args),
        nr::RECVMSG => crate::userland::network_syscalls::recvmsg_handler(args),
        nr::SHUTDOWN => crate::userland::network_syscalls::shutdown_handler(args),
        nr::BIND => crate::userland::network_syscalls::bind_handler(args),
        nr::LISTEN => crate::userland::network_syscalls::listen_handler(args),
        nr::GETSOCKNAME => crate::userland::network_syscalls::getsockname_handler(args),
        nr::GETPEERNAME => crate::userland::network_syscalls::getpeername_handler(args),
        nr::SETSOCKOPT => crate::userland::network_syscalls::setsockopt_handler(args),
        nr::GETSOCKOPT => crate::userland::network_syscalls::getsockopt_handler(args),
        nr::SOCKETPAIR => crate::userland::local_stream::socketpair_handler(args),
        // U3: musl-init / zsh-startup surface
        nr::POLL => syscalls::poll_handler(args),
        nr::SELECT => syscalls::select_handler(args),
        nr::PPOLL => syscalls::ppoll_handler(args),
        nr::PSELECT6 => syscalls::pselect6_handler(args),
        nr::EPOLL_CREATE => crate::userland::epoll::epoll_create_handler(args),
        nr::EPOLL_CREATE1 => crate::userland::epoll::epoll_create1_handler(args),
        nr::EPOLL_CTL => crate::userland::epoll::epoll_ctl_handler(args),
        nr::EPOLL_WAIT => crate::userland::epoll::epoll_wait_handler(args),
        nr::EPOLL_PWAIT => crate::userland::epoll::epoll_pwait_handler(args),
        nr::EVENTFD => crate::userland::eventfd::eventfd_handler(args),
        nr::EVENTFD2 => crate::userland::eventfd::eventfd2_handler(args),
        nr::READLINK => syscalls::readlink_handler(args),
        nr::READLINKAT => syscalls::readlinkat_handler(args),
        nr::GETRLIMIT => syscalls::getrlimit_handler(args),
        nr::GETRUSAGE => syscalls::getrusage_handler(args),
        nr::SYSINFO => syscalls::sysinfo_handler(args),
        nr::PRLIMIT64 => syscalls::prlimit64_handler(args),
        nr::SETITIMER => syscalls::setitimer_handler(args),
        nr::NANOSLEEP => syscalls::nanosleep_handler(args),
        nr::ARCH_PRCTL => syscalls::arch_prctl_handler(args),
        nr::SET_TID_ADDRESS => syscalls::set_tid_address_handler(args),
        nr::SET_ROBUST_LIST => syscalls::set_robust_list_handler(args),
        nr::GETTID => syscalls::gettid_handler(args),
        nr::SCHED_YIELD => syscalls::sched_yield_handler(args),
        nr::SCHED_GETAFFINITY => syscalls::sched_getaffinity_handler(args),
        nr::SIGALTSTACK => syscalls::sigaltstack_handler(args),
        nr::MEMBARRIER => syscalls::membarrier_handler(args),
        nr::FUTEX => crate::userland::futex::handler(args),
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
        nr::LSTAT => syscalls::lstat_handler(args),
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
        nr::UMASK => syscalls::umask_handler(args),
        nr::UTIMENSAT => syscalls::utimensat_handler(args),
        nr::GETRANDOM => syscalls::getrandom_handler(args),
        nr::UNAME => syscalls::uname_handler(args),
        // Credentials and exits
        nr::GETPID => syscalls::getpid_handler(args),
        nr::GETUID => syscalls::getuid_handler(args),
        nr::GETGID => syscalls::getgid_handler(args),
        nr::GETEUID => syscalls::geteuid_handler(args),
        nr::GETEGID => syscalls::getegid_handler(args),
        nr::GETPPID => syscalls::getppid_handler(args),
        nr::EXIT => syscalls::exit_thread_handler(args),
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
        nr::RT_SIGSUSPEND => syscalls::rt_sigsuspend_handler(args),
        nr::GUI_LAUNCH => syscalls::gui_launch_handler(args),
        nr::GUI_WIN_CREATE => crate::userland::gui_syscalls::gui_win_create_handler(args),
        nr::GUI_WIN_PRESENT => crate::userland::gui_syscalls::gui_win_present_handler(args),
        nr::GUI_NEXT_EVENT => crate::userland::gui_syscalls::gui_next_event_handler(args),
        nr::GUI_WIN_DESTROY => crate::userland::gui_syscalls::gui_win_destroy_handler(args),
        nr::GUI_WIN_SET_TITLE => crate::userland::gui_syscalls::gui_win_set_title_handler(args),
        nr::GUI_GL_CONTEXT_CREATE => crate::userland::gui_gl::context_create_handler(args),
        nr::GUI_GL_SUBMIT_FRAME => crate::userland::gui_gl::submit_frame_handler(args),
        nr::GUI_GL_GET_INFO => crate::userland::gui_gl::get_info_handler(args),
        nr::GUI_GL_CONTEXT_DESTROY => crate::userland::gui_gl::context_destroy_handler(args),
        nr::SYSTEM_CONTROL => crate::system_control::syscall_handler(args),
        nr::GUI_EVENT_OPEN => crate::userland::gui_syscalls::gui_event_open_handler(args),
        nr::PTY_OPEN => crate::userland::pty_syscalls::pty_open_handler(args),
        nr::PTY_SET_WINSIZE => crate::userland::pty_syscalls::pty_set_winsize_handler(args),
        nr::CLIPBOARD => crate::clipboard::syscall_handler(args),
        nr::GUI_SHELL_REGISTER => crate::userland::gui_syscalls::gui_shell_register_handler(args),
        nr::GUI_SHELL_LIST_WINDOWS => {
            crate::userland::gui_syscalls::gui_shell_list_windows_handler(args)
        }
        nr::GUI_SHELL_WINDOW_ACTION => {
            crate::userland::gui_syscalls::gui_shell_window_action_handler(args)
        }
        // Phase B: namespace mutations
        nr::MKDIR => syscalls::mkdir_handler(args),
        nr::MKDIRAT => syscalls::mkdirat_handler(args),
        nr::RMDIR => syscalls::rmdir_handler(args),
        nr::UNLINK => syscalls::unlink_handler(args),
        nr::CHMOD => syscalls::chmod_handler(args),
        nr::FCHMOD => syscalls::fchmod_handler(args),
        nr::UNLINKAT => syscalls::unlinkat_handler(args),
        nr::RENAME => syscalls::rename_handler(args),
        nr::RENAMEAT => syscalls::renameat_handler(args),
        nr::LINK => syscalls::link_handler(args),
        nr::LINKAT => syscalls::linkat_handler(args),
        nr::SYMLINK => syscalls::symlink_handler(args),
        nr::SYMLINKAT => syscalls::symlinkat_handler(args),
        nr::CREAT => syscalls::creat_handler(args),
        nr::FTRUNCATE => syscalls::ftruncate_handler(args),
        nr::TRUNCATE => syscalls::truncate_handler(args),
        nr::FSYNC => syscalls::fsync_handler(args),
        nr::FDATASYNC => syscalls::fdatasync_handler(args),
        nr::SYNC => syscalls::sync_handler(args),
        nr::SYNCFS => syscalls::syncfs_handler(args),
        nr::PREAD64 => syscalls::pread64_handler(args),
        nr::PWRITE64 => syscalls::pwrite64_handler(args),
        nr::SENDFILE => syscalls::sendfile_handler(args),
        nr::MADVISE => syscalls::madvise_handler(args),
        nr::MREMAP => syscalls::mremap_handler(args),
        _ => unhandled_syscall(args),
    };

    // Phase 5 PR-B2: deliver a pending signal if one is queued. This
    // diverges (iretq into the handler) — control never returns
    // here. If no signal is pending, return the syscall result
    // normally and let the SYSCALL stub iretq back to the caller.
    let _ = syscalls::maybe_deliver_signal(args, result);
    result
}

/// Default arm for `syscall_dispatch`. Every unknown number returns
/// `-ENOSYS`. Trace mode adds once-per-number argument logging for discovery;
/// it never changes user-visible control flow.
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
                nr,
                args.rdi,
                args.rsi,
                args.rdx,
                args.r10,
                args.r8,
                args.r9
            );
        } else {
            crate::debug_trace!("[strace] nr={}", nr);
        }
        return ENOSYS;
    }
    crate::debug_warn!(
        "syscall_dispatch: unimplemented nr={}; returning -ENOSYS",
        nr
    );
    ENOSYS
}
