//! Linux x86-64 syscall handlers.
//!
//! The surface implements what musl + libstdc++ static `hello` actually
//! exercises during startup and the C++ iostream write path:
//!
//! - **Real**: `write`, `writev`, `read` (EOF stub on stdin), `mmap`
//!   (anonymous private only), `munmap`, `mprotect` (no-op), `brk`,
//!   `arch_prctl(ARCH_SET_FS|ARCH_GET_FS)`, `exit_group`, `ioctl(TCGETS)`
//!   (returns `-ENOTTY` so libstdc++ picks full buffering).
//! - **Stubbed**: `set_tid_address` (returns fixed tid), `set_robust_list`
//!   (returns 0), `getuid`/`getgid`/`getpid`/`getppid` (return 0/0/1/1).
//!
//! Stubs are documented in-line at the call site so adding real semantics
//! later is a one-spot change. Anything outside this surface returns
//! `-ENOSYS` from the dispatcher's default arm; U10 will replace that
//! with a clean per-process termination.
//!
//! ## Pointer validation
//!
//! Every handler that reads a user-supplied buffer routes through
//! `abi::validate_user_slice`. Pointer wraparound and bounds violations
//! return `-EFAULT` without touching the buffer.
//!
//! ## Why this runs with interrupts disabled
//!
//! The SYSCALL stub leaves `IF` cleared (FMASK includes `IF`) until
//! `IRETQ` restores user RFLAGS. Handlers must NOT panic — the panic
//! path acquires the serial lock, which a pending IRQ cannot preempt
//! off, so panic-in-syscall-context is a guaranteed deadlock. Use
//! `Result` / negative-errno returns instead.

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::mm::paging::{
    UserPerms, USER_BRK_BASE, USER_VA_RANGE_END,
};
use crate::userland::abi::{
    validate_user_slice, EACCES, EBADF, EFAULT, EINTR, EINVAL, EIO, EISDIR, EMFILE, ENOENT,
    ENOSYS, ENOTDIR, ENOTTY, ERANGE, EROFS, ESPIPE, LAST_EXIT_CODE,
};
use crate::userland::fdtable::{FdSlot, FdTable, FD_TABLE_SIZE};
use crate::userland::path::{apply_fs_rewrite, copy_user_cstr, normalize_path};
use alloc::string::String;
use alloc::vec;
use x86_64::VirtAddr;

/// Maximum bytes a single `write` call can emit.
const WRITE_MAX_LEN: usize = 4096;
/// Maximum iovec entries per `writev`. libstdc++'s underlying stdio
/// rarely emits more than 2-3 iovecs at a time; 16 is plenty.
const WRITEV_MAX_IOV: usize = 16;
/// Maximum total bytes per `writev` (sum of iov_len).
const WRITEV_MAX_TOTAL: u64 = 16 * 1024;
/// Maximum mmap allocation in bytes.
const MMAP_MAX_LEN: u64 = 8 * 1024 * 1024;
/// Maximum brk growth from the initial anchor in bytes. Bumped from
/// 8 MiB to 32 MiB in U3 to give static-musl zsh's mallocng comfortable
/// headroom for transient startup spikes (parsing rc files, building
/// the keymap table, command-line history if enabled).
const BRK_MAX_BYTES: u64 = 32 * 1024 * 1024;

// ---------- Linux constants ----------

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;

const MAP_PRIVATE: u64 = 0x02;
const MAP_ANONYMOUS: u64 = 0x20;

const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
const TCSETSW: u64 = 0x5403;
const TCSETSF: u64 = 0x5404;
const TIOCGPGRP: u64 = 0x540F;
const TIOCSPGRP: u64 = 0x5410;
const TIOCGWINSZ: u64 = 0x5413;

// ---------- write / writev / read ----------

/// `write(fd: i32, buf: *const u8, count: usize) -> isize`
///
/// Routes through the FD table: stdout/stderr go to `print!`; opened
/// files return `-EROFS` (the FAT mount is read-only).
pub fn write_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let ptr = args.rsi;
    let len = args.rdx;

    // Match the dispatcher's original ordering: classify the fd first
    // (so unknown-fd tests still see EBADF without exercising the slice
    // validator), then bounds-check the buffer.
    let slot = with_fd_slot(fd);
    let pipe_handle = match slot {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => None,
        Some(FdSlot::File { .. }) => return EROFS,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(FdSlot::PipeWrite(handle, _)) => Some(handle),
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Stdin) | None => return EBADF,
    };

    if len > WRITE_MAX_LEN as u64 {
        return EFAULT;
    }
    if let Err(e) = validate_user_slice(ptr, len) {
        return e;
    }
    if len == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };

    if let Some(handle) = pipe_handle {
        // Phase 5 PR-A: write to a pipe. Returns -EPIPE when the
        // pipe has no readers (POSIX would also raise SIGPIPE; we
        // skip the signal until Phase 5 PR-B).
        if handle.pipe().readers() == 0 {
            return crate::userland::abi::EPIPE;
        }
        return handle.pipe().write(slice) as i64;
    }

    // Lossy: invalid UTF-8 bytes become U+FFFD rather than dropping the
    // entire call. A strict `from_utf8` here would silently swallow any
    // write that mixes valid text with binary data — e.g. cat'ing a
    // partially-binary file — and report success without printing.
    let s = alloc::string::String::from_utf8_lossy(slice);
    crate::print!("{}", s);
    len as i64
}

/// `writev(fd: i32, iov: *const iovec, iovcnt: i32) -> isize`
///
/// `iovec { void *iov_base; size_t iov_len; }`. Each entry is two qwords.
/// musl's stdio uses this to flush its buffer plus any pending putback in
/// one call.
pub fn writev_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let iov_ptr = args.rsi;
    let iovcnt = args.rdx as i64;

    // Phase 5 PR-A: writev on pipes is rare in practice (libc uses
    // single-buffer write for pipe stdio); reject for now to keep
    // the pipe path simple. Stdout/stderr keep going to print!.
    match with_fd_slot(fd) {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => {}
        Some(FdSlot::File { .. }) => return EROFS,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(FdSlot::PipeWrite(_, _)) => return ENOSYS,
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Stdin) | None => return EBADF,
    }
    if iovcnt < 0 || iovcnt as usize > WRITEV_MAX_IOV {
        return EINVAL;
    }
    let iov_bytes = (iovcnt as u64) * 16;
    if let Err(e) = validate_user_slice(iov_ptr, iov_bytes) {
        return e;
    }

    // Validate every iov_base/iov_len pair before writing any of them, so
    // a bad later entry doesn't produce a partial write.
    let mut total: u64 = 0;
    for i in 0..iovcnt as u64 {
        let entry = iov_ptr + i * 16;
        let base = unsafe { core::ptr::read_unaligned(entry as *const u64) };
        let len = unsafe { core::ptr::read_unaligned((entry + 8) as *const u64) };
        if let Err(e) = validate_user_slice(base, len) {
            return e;
        }
        match total.checked_add(len) {
            Some(t) if t <= WRITEV_MAX_TOTAL => total = t,
            _ => return EINVAL,
        }
    }

    // Now emit every iov in order.
    let mut written: u64 = 0;
    for i in 0..iovcnt as u64 {
        let entry = iov_ptr + i * 16;
        let base = unsafe { core::ptr::read_unaligned(entry as *const u64) };
        let len = unsafe { core::ptr::read_unaligned((entry + 8) as *const u64) };
        if len == 0 {
            continue;
        }
        let slice = unsafe { core::slice::from_raw_parts(base as *const u8, len as usize) };
        let s = alloc::string::String::from_utf8_lossy(slice);
        crate::print!("{}", s);
        written += len;
    }
    written as i64
}

/// Maximum bytes a single `read` call can consume in one trip. Bounds the
/// kernel-side staging buffer; libc internally loops on short reads.
const READ_MAX_LEN: usize = 4096;

/// `read(fd: i32, buf: *mut u8, count: usize) -> isize`
///
/// Routes through the FD table:
/// - **stdin (slot 0)**: blocks until the per-process stdin queue
///   (populated by the focused `TerminalWindow` on Enter) has at least
///   one byte, then copies as many as fit. Blocking uses `sti; hlt`
///   so the keyboard ISR + main loop can populate the queue.
/// - **stdout/stderr (slots 1/2)**: `-EBADF` (write-only).
/// - **opened file**: stages bytes through a kernel buffer (capped at
///   `READ_MAX_LEN`) and copies to the user pointer; advances the
///   per-handle position.
pub fn read_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let ptr = args.rsi;
    let len = args.rdx;

    if len == 0 {
        return 0;
    }
    let cap = core::cmp::min(len, READ_MAX_LEN as u64);
    if let Err(e) = validate_user_slice(ptr, cap) {
        return e;
    }

    let slot = with_fd_slot(fd);
    match slot {
        Some(FdSlot::Stdin) => read_stdin_blocking(ptr, cap),
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => EBADF,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => EISDIR,
        Some(FdSlot::PipeRead(handle, _)) => {
            // Phase 5 PR-A: drain bytes from the pipe. EOF when empty
            // *and* no writers remain. EAGAIN when empty but writers
            // exist (no real blocking until a concurrent scheduler
            // lands; for now the user app is expected to read after
            // the writer has run to completion).
            let mut staging = vec![0u8; cap as usize];
            let n = handle.pipe().read(&mut staging);
            if n > 0 {
                unsafe {
                    core::ptr::copy_nonoverlapping(staging.as_ptr(), ptr as *mut u8, n);
                }
                return n as i64;
            }
            if handle.pipe().writers() == 0 {
                return 0; // EOF
            }
            crate::userland::abi::EAGAIN
        }
        Some(FdSlot::PipeWrite(_, _)) => EBADF,
        Some(FdSlot::File { handle, .. }) => {
            // Stage the read inside a kernel buffer so the FAT/IDE path
            // never sees a user pointer (which could be unmapped, span a
            // page boundary the FAT layer doesn't understand, etc.).
            let mut staging = vec![0u8; cap as usize];
            match handle.read(&mut staging) {
                Ok(n) => {
                    if n > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                staging.as_ptr(),
                                ptr as *mut u8,
                                n,
                            );
                        }
                    }
                    n as i64
                }
                Err(e) => map_file_err(&e),
            }
        }
        None => EBADF,
    }
}

fn read_stdin_blocking(ptr: u64, cap: u64) -> i64 {
    if !crate::userland::stdin::is_active() {
        return 0;
    }
    let dst = unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, cap as usize) };
    let n = crate::userland::stdin::pop_into(dst);
    if n > 0 {
        return n as i64;
    }
    loop {
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nostack, preserves_flags));
        }
        let n = crate::userland::stdin::pop_into(dst);
        if n > 0 {
            return n as i64;
        }
    }
}

// ---------- mmap / munmap / mprotect ----------

/// `mmap(addr, length, prot, flags, fd, offset) -> void *`
///
/// Anonymous private only: `MAP_PRIVATE | MAP_ANONYMOUS`, fd = -1, addr
/// is treated as a hint and ignored (the kernel's bump arena chooses the
/// address). Length is rounded up to page granularity. PROT bits map
/// onto `UserPerms` (RW always; +X if requested; the loader's
/// permission-profile invariant — never both writable and executable —
/// would technically be violated by `PROT_READ|WRITE|EXEC`, but
/// libstdc++ doesn't ask for X here, so we treat the X bit as a hint
/// and pick `ReadWrite` for any RW request).
pub fn mmap_handler(args: &mut SyscallArgs) -> i64 {
    let _addr_hint = args.rdi;
    let length = args.rsi;
    let prot = args.rdx;
    let flags = args.r10;
    let fd = args.r8 as i64;
    let _offset = args.r9;

    if length == 0 || length > MMAP_MAX_LEN {
        return EINVAL;
    }
    if (flags & MAP_PRIVATE) == 0 || (flags & MAP_ANONYMOUS) == 0 {
        return ENOSYS;
    }
    if fd != -1 {
        return ENOSYS; // file-backed mmap not yet supported
    }
    if prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return EINVAL;
    }

    let pages = length.div_ceil(0x1000);
    let perms = if prot & PROT_EXEC != 0 {
        UserPerms::ReadExecute
    } else if prot & PROT_WRITE != 0 {
        UserPerms::ReadWrite
    } else {
        UserPerms::ReadOnly
    };

    // Allocate from the per-process bump arena.
    let addr = crate::userland::lifecycle::with_active_user(|au| {
        let next = au.mmap_next;
        let end = next + pages * 0x1000;
        if end > USER_VA_RANGE_END {
            return None;
        }
        au.mmap_next = end;
        Some(next)
    });
    let addr = match addr {
        Some(a) => a,
        None => return -12, // ENOMEM
    };

    // Map and record on the active UserImage so Drop unmaps it.
    let map_result = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(addr), pages, perms)
    });
    match map_result {
        Some(Ok(_)) => {}
        Some(Err(_)) => return -12, // ENOMEM
        None => return -12,
    }
    crate::userland::lifecycle::with_active_user(|au| {
        if let Some(img) = au.image.as_mut() {
            img.record_mapping(VirtAddr::new(addr), pages);
        }
    });

    addr as i64
}

/// `munmap(addr, length) -> int`
///
/// Best-effort: unmaps the page-aligned range. The bump arena pointer
/// does not retract — `mmap` always returns fresh VA. For the milestone,
/// freeing a region returns the pages to the kernel's frame allocator
/// (insofar as it tracks them) but doesn't reclaim the user-VA region
/// for future `mmap` calls.
pub fn munmap_handler(args: &mut SyscallArgs) -> i64 {
    let addr = args.rdi;
    let length = args.rsi;
    if addr & 0xFFF != 0 || length == 0 {
        return EINVAL;
    }
    let pages = length.div_ceil(0x1000);
    let _ = crate::mm::memory::with_memory_mapper(|m| {
        m.unmap_user_region(VirtAddr::new(addr), pages)
    });
    // Always succeed — the unmap may fail for never-mapped ranges, which
    // POSIX considers an error, but for the milestone a forgiving
    // implementation is fine.
    0
}

/// `mprotect(addr, length, prot) -> int`
///
/// Stub: returns 0. musl uses this for thread-stack guard pages on
/// `pthread_create`, which single-threaded hello-world doesn't trigger.
/// libstdc++ doesn't issue mprotect during basic iostream use either.
/// Real perm changes will land when a binary actually requires them.
pub fn mprotect_handler(_args: &mut SyscallArgs) -> i64 {
    0
}

// ---------- brk ----------

/// `brk(addr) -> void *`
///
/// `addr == 0` returns the current brk. `addr >= current_brk` grows the
/// region by mapping new pages from `current_brk` (page-aligned) up to
/// `addr` (page-aligned). `addr < current_brk` is treated as a no-op:
/// the milestone does not shrink (no real reclaim path either).
pub fn brk_handler(args: &mut SyscallArgs) -> i64 {
    let new_brk = args.rdi;

    let cur = crate::userland::lifecycle::with_active_user(|au| au.brk_current);
    if new_brk == 0 || new_brk <= cur {
        return cur as i64;
    }
    if new_brk > USER_BRK_BASE + BRK_MAX_BYTES {
        return cur as i64;
    }

    // Pages to map: from the page above the current brk to the page
    // covering new_brk - 1.
    let cur_page_end = (cur + 0xFFF) & !0xFFF;
    let new_page_end = (new_brk + 0xFFF) & !0xFFF;
    if new_page_end > cur_page_end {
        let pages = (new_page_end - cur_page_end) / 0x1000;
        let map_result = crate::mm::memory::with_memory_mapper(|m| {
            m.map_user_region(
                VirtAddr::new(cur_page_end),
                pages,
                UserPerms::ReadWrite,
            )
        });
        match map_result {
            Some(Ok(_)) => {}
            _ => return cur as i64, // mapping failed; brk unchanged
        }
        crate::userland::lifecycle::with_active_user(|au| {
            if let Some(img) = au.image.as_mut() {
                img.record_mapping(VirtAddr::new(cur_page_end), pages);
            }
        });
    }

    crate::userland::lifecycle::with_active_user(|au| {
        au.brk_current = new_brk;
        au.brk_current as i64
    })
}

// ---------- arch_prctl ----------

/// `arch_prctl(code: i32, addr: ulong) -> int`
///
/// `ARCH_SET_FS` (0x1002): write `addr` into `IA32_FS_BASE`. musl's
/// `__init_tls` issues this before any TLS-using code runs.
/// `ARCH_GET_FS` (0x1003): read `IA32_FS_BASE` and store in `*addr`.
/// Other codes return `-EINVAL`.
pub fn arch_prctl_handler(args: &mut SyscallArgs) -> i64 {
    let code = args.rdi;
    let addr = args.rsi;
    match code {
        ARCH_SET_FS => {
            // Validate addr is canonical and lies within user VA bounds.
            // We don't dereference; we write it to the MSR.
            if VirtAddr::try_new(addr).is_err() {
                return EINVAL;
            }
            crate::arch::x86_64::msr::set_fs_base(addr);
            0
        }
        ARCH_GET_FS => {
            if let Err(e) = validate_user_slice(addr, 8) {
                return e;
            }
            // Read current FS_BASE via the typed wrapper. Since we set it
            // ourselves, we could mirror it on ActiveUser instead of
            // round-tripping through the MSR — but reading is cheap.
            use x86_64::registers::model_specific::FsBase;
            let cur = FsBase::read().as_u64();
            unsafe { core::ptr::write_unaligned(addr as *mut u64, cur); }
            0
        }
        _ => EINVAL,
    }
}

// ---------- ioctl ----------

/// `ioctl(fd: i32, request: u64, arg) -> int`
///
/// Phase 3 surface — terminal control:
/// - `TCGETS`: copy the active termios into the user buffer.
/// - `TCSETS`/`TCSETSW`/`TCSETSF`: copy a user termios into the active
///   slot. `W` (drain output before applying) and `F` (drain + flush
///   input) carry no extra meaning here — there's no hardware queue to
///   drain — so all three are equivalent.
/// - `TIOCGWINSZ`: copy the synthesized winsize (80x24) into the user
///   buffer. zsh's `zle` consults this to decide where to wrap.
/// - `TIOCGPGRP`: U5 — return `-ENOTTY`. zsh's `acquire_pgrp`
///   (`Src/init.c`) treats this as "no controlling tty" and clears
///   `opts[MONITOR]`, which disables the entire job-control surface
///   (setpgid/setsid/tcsetpgrp). This is the cleanest path to
///   no-job-control: no `+m` argv hack required, no build-time
///   `--without-tcsetpgrp` reliance. The `--without-tcsetpgrp`
///   configure flag is also passed by U1's Makefile as
///   belt-and-suspenders.
/// - `TIOCSPGRP`: U5 — return `0`. Defensive stub; zsh shouldn't
///   reach this path with MONITOR cleared, but a silent success
///   avoids surprises if a configuration somehow does.
///
/// Calls on non-tty fds (anything other than stdin/stdout/stderr)
/// return `-ENOTTY`; libc relies on this to detect "this fd is a file"
/// and disable line buffering.
///
/// Per the feasibility doc-review finding, the new TIOCGPGRP arm sits
/// inside the request match alongside TCGETS — NOT relying on the
/// non-tty fd short-circuit above. Today's tty-fd unknown-request
/// default is `-ENOSYS`; without an explicit arm, TIOCGPGRP on stdin
/// would return ENOSYS (not ENOTTY), and zsh's MONITOR-clearing path
/// is gated specifically on ENOTTY.
pub fn ioctl_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let request = args.rsi;
    let arg = args.rdx;

    let is_tty = matches!(
        with_fd_slot(fd),
        Some(FdSlot::Stdin) | Some(FdSlot::Stdout) | Some(FdSlot::Stderr)
    );
    if !is_tty {
        return ENOTTY;
    }

    match request {
        TCGETS => {
            let size = core::mem::size_of::<crate::userland::tty::Termios>() as u64;
            if let Err(e) = validate_user_slice(arg, size) {
                return e;
            }
            let t = crate::userland::tty::snapshot();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &t as *const _ as *const u8,
                    arg as *mut u8,
                    size as usize,
                );
            }
            0
        }
        TCSETS | TCSETSW | TCSETSF => {
            let size = core::mem::size_of::<crate::userland::tty::Termios>() as u64;
            if let Err(e) = validate_user_slice(arg, size) {
                return e;
            }
            let mut t = crate::userland::tty::snapshot();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    arg as *const u8,
                    &mut t as *mut _ as *mut u8,
                    size as usize,
                );
            }
            crate::userland::tty::set(t);
            0
        }
        TIOCGWINSZ => {
            let size = core::mem::size_of::<crate::userland::tty::Winsize>() as u64;
            if let Err(e) = validate_user_slice(arg, size) {
                return e;
            }
            let ws = crate::userland::tty::winsize();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &ws as *const _ as *const u8,
                    arg as *mut u8,
                    size as usize,
                );
            }
            0
        }
        TIOCGPGRP => ENOTTY,
        TIOCSPGRP => 0,
        _ => ENOSYS,
    }
}

// ---------- thread / signal stubs ----------

/// `set_tid_address(tidptr: *mut int) -> pid_t`
///
/// Records nothing; returns the fake tid `1`. musl calls this once during
/// pthread init even single-threaded.
pub fn set_tid_address_handler(_args: &mut SyscallArgs) -> i64 {
    1
}

/// `set_robust_list(head, len) -> int` — no-op stub.
pub fn set_robust_list_handler(_args: &mut SyscallArgs) -> i64 {
    0
}

/// `rt_sigaction(signum, act, oldact, sigsetsize) -> int`.
///
/// Phase 5 PR-B: stores the action on the per-process `SignalState`.
/// `act == NULL` means "just query"; `oldact == NULL` means "don't
/// return previous action." `sigsetsize` must equal 8 (we represent
/// sigset_t as a single u64).
pub fn rt_sigaction_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::signal::{SigAction, NSIG};
    let signum = args.rdi as i32;
    let act_ptr = args.rsi;
    let oldact_ptr = args.rdx;
    let sigsetsize = args.r10 as usize;

    if signum < 1 || (signum as usize) > NSIG {
        return EINVAL;
    }
    if sigsetsize != 8 {
        return EINVAL;
    }

    // Snapshot current action (for oldact return).
    let prev = crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.action(signum).unwrap_or_default()
    });

    if oldact_ptr != 0 {
        let size = core::mem::size_of::<SigAction>() as u64;
        if let Err(e) = validate_user_slice(oldact_ptr, size) {
            return e;
        }
        unsafe {
            core::ptr::write_unaligned(oldact_ptr as *mut SigAction, prev);
        }
    }

    if act_ptr != 0 {
        let size = core::mem::size_of::<SigAction>() as u64;
        if let Err(e) = validate_user_slice(act_ptr, size) {
            return e;
        }
        let new_action = unsafe { core::ptr::read_unaligned(act_ptr as *const SigAction) };
        crate::userland::lifecycle::with_current_process(|p| {
            p.signal_state.set_action(signum, new_action);
        });
    }

    0
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize) -> int`.
///
/// Phase 5 PR-B: real implementation backed by `SignalState.blocked`.
/// `set == NULL` means "just query"; `oldset == NULL` means "don't
/// return previous mask."
pub fn rt_sigprocmask_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::signal::{SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK, SIGKILL, SIGSTOP};
    let how = args.rdi as i32;
    let set_ptr = args.rsi;
    let oldset_ptr = args.rdx;
    let sigsetsize = args.r10 as usize;

    if sigsetsize != 8 {
        return EINVAL;
    }

    // Snapshot current mask.
    let prev = crate::userland::lifecycle::with_current_process(|p| p.signal_state.blocked);

    if oldset_ptr != 0 {
        if let Err(e) = validate_user_slice(oldset_ptr, 8) {
            return e;
        }
        unsafe { core::ptr::write_unaligned(oldset_ptr as *mut u64, prev); }
    }

    if set_ptr != 0 {
        if let Err(e) = validate_user_slice(set_ptr, 8) {
            return e;
        }
        let set = unsafe { core::ptr::read_unaligned(set_ptr as *const u64) };
        // POSIX: SIGKILL and SIGSTOP can never be blocked. Strip them.
        let kill_stop_mask = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
        let sanitized = set & !kill_stop_mask;
        crate::userland::lifecycle::with_current_process(|p| {
            p.signal_state.blocked = match how {
                SIG_BLOCK => p.signal_state.blocked | sanitized,
                SIG_UNBLOCK => p.signal_state.blocked & !sanitized,
                SIG_SETMASK => sanitized,
                _ => return,
            };
        });
        if how != SIG_BLOCK && how != SIG_UNBLOCK && how != SIG_SETMASK {
            return EINVAL;
        }
    }

    0
}

/// `rt_sigsuspend(*mask, sigsetsize) -> int` — always returns `-EINTR`.
///
/// POSIX: atomically replace the signal mask with `*mask`, suspend
/// until a deliverable signal arrives, run its handler, then return
/// with the original mask restored.
///
/// Our kernel can't truly suspend (no scheduler that blocks user
/// processes mid-syscall). Pragmatic implementation that works for
/// zsh's `waitjobs` loop: install the new mask on the current process,
/// return `-EINTR`. The dispatcher tail's `maybe_deliver_signal` then
/// finds any pending handler-installed signal that the new mask
/// unblocks (notably SIGCHLD, which our synchronous fork has already
/// raised by the time zsh enters sigsuspend) and `iretq`s into the
/// handler. The handler runs `waitpid`, reaps the zombie, returns via
/// `rt_sigreturn`, and zsh sees the syscall returned `-EINTR`.
///
/// Known gap: the original mask is not restored after the handler
/// returns. `rt_sigreturn` only restores `UserState`, not `blocked`.
/// Same gap exists for `sa_mask` on regular signal delivery. zsh
/// re-asserts its mask via `rt_sigprocmask` on every `waitjobs`
/// iteration, so this doesn't bite in practice. Real fix lands when we
/// teach `deliver_signal` / `rt_sigreturn` to save and restore the
/// blocked mask alongside `UserState`.
pub fn rt_sigsuspend_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::signal::{SIGKILL, SIGSTOP};
    let mask_ptr = args.rdi;
    let sigsetsize = args.rsi as usize;

    if sigsetsize != 8 {
        return EINVAL;
    }
    if let Err(e) = validate_user_slice(mask_ptr, 8) {
        return e;
    }
    let mask = unsafe { core::ptr::read_unaligned(mask_ptr as *const u64) };
    // POSIX: SIGKILL and SIGSTOP can never be blocked. Strip them so
    // the new mask doesn't accidentally swallow a pending KILL/STOP
    // bit during the (zero-duration) suspension window.
    let kill_stop_mask = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
    let sanitized = mask & !kill_stop_mask;
    crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.blocked = sanitized;
    });
    EINTR
}

// ---------- credentials ----------

pub fn getuid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn getgid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn geteuid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn getegid_handler(_: &mut SyscallArgs) -> i64 { 0 }

/// `getpid() -> pid_t`. Phase 4 PR-A returns the real per-process PID
/// instead of the previous fixed `1`. PIDs are allocated monotonically
/// starting at `1` by `enter_user_mode_with`, so each successive
/// `run /HOST/...ELF` sees a different number.
pub fn getpid_handler(_: &mut SyscallArgs) -> i64 {
    crate::userland::lifecycle::current_pid() as i64
}

/// `getppid() -> pid_t`. Returns the parent PID. For binaries launched
/// by the `run` shell command, the parent is the kernel itself
/// (PID 0). Fork-spawned children (PR-C) report their real parent.
pub fn getppid_handler(_: &mut SyscallArgs) -> i64 {
    crate::userland::lifecycle::with_current_process(|p| p.parent_pid as i64)
}

// ---------- Phase 4 PR-C2: process management ----------

/// `fork() -> pid_t`. Synchronous-child semantics: the parent is
/// suspended in this syscall while the child runs to completion (or
/// until execve, once that lands). On child exit, the parent's fork
/// returns the child's PID; the child's exit status is parked in the
/// zombie table for waitpid to reap.
///
/// This intentionally does not support concurrency between parent and
/// child — pipelines and `cmd &` need a real scheduler (Phase 5+).
pub fn fork_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::user_state::{capture_callee_saved, CalleeSavedSnapshot, UserState};
    use crate::userland::lifecycle::{
        parent_stashed, record_zombie, reap_zombie, stash_parent, swap_current_process,
        take_stashed_parent, with_current_process, alloc_pid, ExitKind,
    };

    // 1. Capture parent's user-mode callee-saved registers IMMEDIATELY.
    //    Done via a naked-asm helper so the Rust compiler hasn't had
    //    a chance to spill them yet. The CalleeSavedSnapshot's r12
    //    slot actually holds the user RSP (the syscall stub stashed
    //    it there before dispatch).
    let mut callee = CalleeSavedSnapshot::default();
    unsafe { capture_callee_saved(&mut callee as *mut _); }

    // 2. Read the user RIP, RFLAGS, and original user R12 from the
    //    SYSCALL stub's saved-state slots above SyscallArgs. Layout
    //    is fixed by the stub:
    //       args + 56 = rcx (user RIP)
    //       args + 64 = r11 (user RFLAGS)
    //       args + 72 = original user R12
    let user_rip;
    let user_rflags;
    let user_r12;
    unsafe {
        let p = args as *const SyscallArgs as *const u64;
        user_rip = core::ptr::read(p.add(7));
        user_rflags = core::ptr::read(p.add(8));
        user_r12 = core::ptr::read(p.add(9));
    }

    // 3. Build the child's full register snapshot: same as parent at
    //    fork()'s syscall instruction, except rax = 0 (the "I am the
    //    child" signal) and rcx/r11 are clobbered (intentional — the
    //    SYSCALL ABI documents them as undefined on return, so we
    //    don't need to restore them).
    let child_state = UserState {
        rax: 0, // child sees fork() return 0
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: callee.rbx,
        rbp: callee.rbp,
        rsp: callee.r12_register, // stub stashed user RSP into r12
        r12: user_r12,
        r13: callee.r13,
        r14: callee.r14,
        r15: callee.r15,
        rip: user_rip,
        rflags: user_rflags,
    };

    // 4. Refuse nested fork (PR-C2 supports depth = 1).
    if parent_stashed() {
        crate::debug_warn!("fork(): nested fork not yet supported");
        return EINVAL;
    }

    // 5. Allocate the child PID. Pull what we need from the parent's
    //    Process under one lock, build the child Process below.
    let child_pid = alloc_pid();
    let parent_l4_frame = match with_current_process(|p| {
        p.address_space.as_ref().map(|a| a.l4_frame())
    }) {
        Some(f) => f,
        None => {
            crate::debug_warn!("fork(): parent has no AddressSpace (test path?)");
            return ENOSYS;
        }
    };

    // 6. Eagerly clone the parent's address space (fresh L4 + copy of
    //    every leaf page in PML4[0]). Built on the parent's L4 — we
    //    haven't switched CR3 yet.
    let child_aspace = match crate::userland::address_space::AddressSpace::clone_for_child(
        parent_l4_frame,
    ) {
        Ok(a) => a,
        Err(e) => {
            crate::debug_error!("fork(): clone_for_child failed: {:?}", e);
            return -12; // ENOMEM
        }
    };

    // 7. Build the child Process. State pieces (FD table, cwd, brk,
    //    mmap) are cloned by value; address space ownership transfers.
    let child_process = with_current_process(|parent| crate::userland::lifecycle::Process {
        pid: child_pid,
        parent_pid: parent.pid,
        continuation: None,
        image: None, // child shares parent's image — kept implicitly
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: parent.brk_current,
        mmap_next: parent.mmap_next,
        fd_table: parent.fd_table.clone(),
        cwd: parent.cwd.clone(),
        address_space: Some(child_aspace),
        // Phase 5 PR-B: child inherits parent's signal dispositions
        // and blocked mask. Pending mask resets to empty (POSIX:
        // pending signals are not inherited across fork).
        signal_state: parent.signal_state.fork_clone(),
        // Phase 5 PR-C1: child gets its own freshly-allocated kernel
        // stack so its SYSCALL handlers don't share rsp0 with the
        // parent's suspended fork() handler.
        kernel_stack: Some(crate::userland::kernel_stack::KernelStack::new()),
        // U3: child shares parent's exe path (fork doesn't change the
        // running binary; execve replaces it).
        exe_path: parent.exe_path.clone(),
    });

    // 8. Stash the parent's Process and install the child as current.
    //    The parent's Process holds its AddressSpace — we leave it on
    //    the parent's L4 (CR3 is still pointing at parent's L4) so
    //    the kernel state we just ran is consistent.
    let parent_process = swap_current_process(child_process);
    stash_parent(parent_process);

    // 9. Activate the child's L4. From here until the child exits,
    //    CR3 references the child's page table.
    crate::userland::lifecycle::with_current_process(|child| {
        if let Some(a) = child.address_space.as_ref() {
            // SAFETY: AddressSpace::new copied the kernel half from
            // the kernel L4, so the kernel code that runs after this
            // CR3 write is still mapped.
            unsafe { a.activate(); }
        }
    });

    // 9a. Save the parent's pointer-validation bounds. The child's
    //     long-jump back (cooperative or abnormal) flows through
    //     `long_jump_to_run_or_halt`, which calls `clear_user_va_bounds`
    //     unconditionally. Without restoring here, the parent's next
    //     syscall with any user pointer would fail `-EFAULT` because
    //     `validate_user_slice` would see `None` bounds. Symptom: zsh
    //     hangs after a child crash because its post-fork sigsuspend /
    //     waitpid / write all reject their user pointers.
    let saved_user_va_bounds = crate::userland::abi::user_va_bounds();

    // 9b. Phase 5 PR-C1: point both TSS.rsp0 and the SYSCALL stub's
    //     `gs:[0]` slot at the *child's* freshly-allocated kernel
    //     stack so its syscalls don't share rsp0 with the parent's
    //     in-flight fork() frame. Save the parent's value to restore
    //     after the child long-jumps back.
    let saved_kernel_rsp_top: u64;
    unsafe {
        core::arch::asm!(
            "mov {0}, gs:[0]",
            out(reg) saved_kernel_rsp_top,
            options(nomem, preserves_flags, nostack),
        );
    }
    let child_top = crate::userland::lifecycle::with_current_process(|child| {
        child.kernel_stack.as_ref().expect("child kernel stack").top()
    });
    unsafe {
        crate::arch::x86_64::syscall::set_percpu_kernel_rsp_top(child_top.as_u64());
        crate::arch::x86_64::gdt::set_kernel_rsp0(child_top);
    }

    // 10. Dispatch the child via setjmp+iretq. The asm helper saves
    //     the kernel continuation (resume here on child exit), then
    //     iretqs into the child at the saved RIP/RSP/RFLAGS with
    //     restored GP regs. When the child eventually `exit_group`s,
    //     `cooperative_exit` long-jumps back; control resumes after
    //     the asm call below.
    let user_cs = crate::arch::x86_64::gdt::selectors().user_code.0 as u64;
    let user_ss = crate::arch::x86_64::gdt::selectors().user_data.0 as u64;
    unsafe {
        super::enter_user_mode_with_regs_asm(&child_state as *const _, user_cs, user_ss);
    }

    // 10b. Restore the parent's kernel rsp0 (its own per-process
    //      kernel stack from PR-C1).
    unsafe {
        crate::arch::x86_64::syscall::set_percpu_kernel_rsp_top(saved_kernel_rsp_top);
        crate::arch::x86_64::gdt::set_kernel_rsp0(VirtAddr::new(saved_kernel_rsp_top));
    }

    // 11. Child has exited and long-jumped back. Capture the recorded
    //     zombie info (the child's `exit_group` already filed it into
    //     ZOMBIES). Then dismantle the child slot and reinstall the
    //     parent.
    let (child_exit_code, child_pid_recorded) = with_current_process(|child| {
        (child.exit_code, child.pid)
    });
    debug_assert_eq!(child_pid_recorded, child_pid);

    // Drop the child Process. Its AddressSpace::Drop reverts CR3 to
    // the kernel L4 if needed (it will, since child's L4 is still
    // CR3 here).
    let parent = take_stashed_parent().expect("parent stash empty");
    let child_dropped = swap_current_process(parent);
    drop(child_dropped);

    // 12. Switch back to the parent's L4 so the rest of the parent's
    //     execution sees the parent's user mappings. AddressSpace
    //     activation is unsafe; we know the kernel half is consistent.
    with_current_process(|p| {
        if let Some(a) = p.address_space.as_ref() {
            unsafe { a.activate(); }
        }
    });

    // 12b. Restore the parent's pointer-validation bounds (paired with
    //      step 9a). If we somehow entered fork without bounds set
    //      (test path with no `enter_user_mode`), leave bounds cleared.
    if let Some(bounds) = saved_user_va_bounds {
        crate::userland::abi::set_user_va_bounds(bounds);
    }

    // 13. The zombie was already recorded by the child's exit_group.
    //     Sanity-check that we can find it (if not, _exit didn't run
    //     through the child path — bug).
    let _ = (child_exit_code, reap_zombie);

    crate::debug_info!(
        "fork(): child {} exited with {}; parent resumes",
        child_pid,
        child_exit_code,
    );

    // 14. Parent's fork() returns the child PID.
    child_pid as i64
}

pub fn vfork_handler(args: &mut SyscallArgs) -> i64 {
    // vfork in real Linux runs the child sharing parent's memory until
    // exec/exit. We don't support that; route to fork (full eager copy).
    fork_handler(args)
}

pub fn clone_handler(_args: &mut SyscallArgs) -> i64 {
    // glibc/musl wrap fork() as clone(SIGCHLD, 0, ...). For PR-C2 we
    // treat clone as ENOSYS so libc falls back to the explicit fork
    // syscall path. Full clone() with thread/CLONE_VM semantics is
    // Phase 5/6.
    ENOSYS
}

/// `execve(path, argv, envp)`. Replaces the current process's image
/// in place: drops user pages from the current address space, builds a
/// fresh L4, loads the new ELF into it, lays out a new initial stack
/// with the supplied argv/envp, and `iretq`s into the new entry point.
///
/// PID, parent_pid, FD table, cwd, stdin queue, and termios are all
/// retained — that's the contract of execve. The existing kernel
/// continuation (set when the process was first entered, or by `fork`
/// for a forked child) is preserved, so the new program's eventual
/// `_exit` flows back to the original caller.
///
/// On success: does not return (control flows to ring 3 of the new
/// program). On failure: returns `-errno`.
pub fn execve_handler(args: &mut SyscallArgs) -> i64 {
    use crate::mm::paging::{USER_BRK_BASE, USER_MMAP_BASE};
    use crate::userland::abi::{set_user_va_bounds, UserVaBounds};
    use crate::userland::path::copy_user_cstr_array;
    use crate::userland::user_state::UserState;
    use alloc::string::String;
    use alloc::vec::Vec;

    // 1. Pull path/argv/envp into kernel memory while the OLD address
    //    space is still active and the user pointers are valid.
    let path_ptr = args.rdi;
    let argv_ptr = args.rsi;
    let envp_ptr = args.rdx;

    let raw_path = match crate::userland::path::copy_user_cstr(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Normalize once; both the /bin namespace rewrite and the /etc
    // rewrite need the canonical form. The rewrites are mutually
    // exclusive in practice (one matches /bin/<applet>, the other
    // /etc/<name>) but the ordering still matters for security: `..`
    // segments MUST be collapsed before either prefix check.
    let normalized_path = crate::userland::lifecycle::with_current_process(|p| {
        crate::userland::path::normalize_path(&p.cwd, &raw_path)
    });
    // Virtual /bin namespace: rewrite the load path to BB.ELF AND
    // override argv[0] so BusyBox's multicall dispatcher selects the
    // requested applet. Linux preserves the caller's argv[0] verbatim;
    // we deviate here because a multicall binary needs argv[0] to
    // carry the applet name. Documented in src/userland/bin_namespace.rs.
    let bin_applet =
        crate::userland::bin_namespace::apply_bin_rewrite(&normalized_path).map(|(_, n)| n);
    let resolved_path = if bin_applet.is_some() {
        String::from(crate::userland::bin_namespace::BB_HOST_PATH)
    } else {
        // U4: same `/etc/...` rewrite that resolve_user_path applies — if
        // someone ever does `execve("/etc/something")` it lands at the
        // FAT-staged location. Cosmetic for execve in practice (zsh
        // execs binaries under /HOST/, not /etc/), but consistent.
        crate::userland::path::apply_fs_rewrite(&normalized_path)
    };
    let argv_strings: Vec<String> = match copy_user_cstr_array(argv_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let envp_strings: Vec<String> = match copy_user_cstr_array(envp_ptr) {
        Ok(v) => v,
        Err(e) => return e,
    };

    // 2. Read the new binary off the filesystem. Use the existing
    //    File API; this is the same path the `run` shell command
    //    takes for top-level launches.
    const MAX_USER_BINARY_BYTES: u64 = 16 * 1024 * 1024;
    let bytes = match crate::fs::file_handle::File::open_read(&resolved_path) {
        Ok(file) => {
            if file.size() > MAX_USER_BINARY_BYTES {
                return ENOSYS; // -E2BIG would be more accurate
            }
            match file.read_to_vec() {
                Ok(v) => v,
                Err(ref e) => return map_file_err(e),
            }
        }
        Err(ref e) => return map_file_err(e),
    };

    // 3. Build a fresh AddressSpace for the new image. If this fails,
    //    we haven't touched the old state yet — return cleanly.
    let new_aspace = match crate::userland::address_space::AddressSpace::new() {
        Ok(a) => a,
        Err(_) => return -12, // ENOMEM
    };

    // 4. Drop the OLD image while the OLD aspace is still active so
    //    the recorded mappings can be unmapped from the right L4.
    //    UserImage::Drop walks its mapping list and calls
    //    `unmap_user_region`, which targets the active CR3.
    let old_image = crate::userland::lifecycle::with_current_process(|p| p.image.take());
    drop(old_image);

    // 5. Take the OLD aspace out of the Process. Don't drop yet —
    //    we keep it alive in case load_elf below fails so we can
    //    roll back to it and return -errno from execve.
    let old_aspace =
        crate::userland::lifecycle::with_current_process(|p| p.address_space.take());

    // 6. Activate the new L4 and load the new ELF into it.
    // SAFETY: kernel half copied from kernel L4; the kernel code
    // post-CR3-write is still mapped.
    unsafe { new_aspace.activate(); }

    let image = match crate::userland::loader::load_elf(&bytes) {
        Ok(i) => i,
        Err(e) => {
            // Load failed. Roll back to the old aspace if we still
            // have it. The old image is already gone (step 4), so
            // there's no clean rollback for the user pages — but the
            // process can at least reach `cooperative_exit` cleanly.
            crate::debug_error!("execve(): load_elf failed: {:?}", e);
            if let Some(old) = old_aspace.as_ref() {
                unsafe { old.activate(); }
            }
            crate::userland::lifecycle::with_current_process(|p| {
                p.address_space = old_aspace;
            });
            return EINVAL;
        }
    };

    // 7. Commit: drop the old aspace (frame leaks under the bump
    //    allocator but that's pre-existing).
    drop(old_aspace);

    // 8. Extract image bits we need for the initial stack and the
    //    iretq frame, before moving image onto the Process.
    let entry = image.entry.as_u64();
    let stack_top = image.stack_top.as_u64();
    let bounds = UserVaBounds {
        start: image.bounds_start,
        end: image.bounds_end,
    };
    let phdr_bytes = image.phdr_bytes.clone();
    let e_phnum = image.e_phnum;

    // 9. Build the new initial stack with the supplied argv/envp.
    //    `argv[0]` is the program name; if the user passed an empty
    //    argv we synthesize one from the path so musl's
    //    `program_invocation_name` isn't NULL.
    let mut argv_refs: Vec<&str> = if argv_strings.is_empty() {
        alloc::vec![resolved_path.as_str()]
    } else {
        argv_strings.iter().map(|s| s.as_str()).collect()
    };
    // BusyBox multicall: argv[0] picks the applet, regardless of what
    // the caller passed. See bin_applet computation above.
    if let Some(applet) = bin_applet {
        argv_refs[0] = applet;
    }
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();
    let user_rsp = super::build_initial_stack(stack_top, &phdr_bytes, e_phnum, &argv_refs, &envp_refs);

    // 10. Move new image and aspace onto the Process; reset brk/mmap
    //     anchors and exit info. Retain PID, parent_pid, FD table,
    //     cwd, continuation.
    crate::userland::lifecycle::with_current_process(|p| {
        p.image = Some(image);
        p.address_space = Some(new_aspace);
        p.brk_current = USER_BRK_BASE;
        p.mmap_next = USER_MMAP_BASE;
        p.exit_kind = crate::userland::lifecycle::ExitKind::None;
        p.exit_code = 0;
        // U3: exec replaces the running binary, so /proc/self/exe now
        // points at the new program. argv[0] is the canonical name.
        p.exe_path = Some(String::from(argv_refs[0]));
        // Phase 5 PR-B: POSIX semantics — exec resets signal
        // dispositions but preserves the blocked mask. Pending
        // signals are also preserved across exec.
        let preserved_blocked = p.signal_state.blocked;
        let preserved_pending = p.signal_state.pending;
        p.signal_state = crate::userland::signal::SignalState::new();
        p.signal_state.blocked = preserved_blocked;
        p.signal_state.pending = preserved_pending;
    });
    set_user_va_bounds(bounds);
    // Phase 3 termios: a freshly exec'd process gets a default tty.
    crate::userland::tty::install_default();

    crate::debug_info!(
        "execve({}): entry={:#x}, rsp={:#x}, argv={:?}",
        resolved_path,
        entry,
        user_rsp,
        argv_refs,
    );

    // 11. iretq directly into the new entry point. Diverges — the
    //     existing syscall-stub frame is abandoned, the existing
    //     kernel continuation stays in place for the new program's
    //     eventual `_exit` to long-jump through.
    let user_cs = crate::arch::x86_64::gdt::selectors().user_code.0 as u64;
    let user_ss = crate::arch::x86_64::gdt::selectors().user_data.0 as u64;
    let state = UserState {
        rax: 0,
        rdi: 0, rsi: 0, rdx: 0, r10: 0, r8: 0, r9: 0,
        rbx: 0, rbp: 0, rsp: user_rsp,
        r12: 0, r13: 0, r14: 0, r15: 0,
        rip: entry,
        rflags: 0x202,
    };
    unsafe {
        super::iretq_to_user_with_regs(&state as *const _, user_cs, user_ss);
    }
}

// ---------- Phase 5 PR-B: signals ----------

/// `kill(pid, sig) -> int`. Sets `sig` pending on the target process.
///
/// Synchronous-fork model: the only addressable processes are
/// "self" (`pid == getpid()`) and "any parent that's currently
/// stashed in `PARENT_STASH`" — the latter being the only other
/// process that exists. Any other PID returns `-ESRCH`.
///
/// `sig == 0` is the "is the target alive?" probe; we return 0 for
/// addressable PIDs without setting any pending bit.
pub fn kill_handler(args: &mut SyscallArgs) -> i64 {
    let pid = args.rdi as i32;
    let sig = args.rsi as i32;
    if sig < 0 || (sig as usize) > crate::userland::signal::NSIG {
        return EINVAL;
    }
    let me = crate::userland::lifecycle::current_pid() as i32;
    if pid == me {
        if sig == 0 {
            return 0;
        }
        crate::userland::lifecycle::with_current_process(|p| p.signal_state.raise(sig));
        return 0;
    }
    // Try to deliver to the stashed parent (if any).
    let delivered = crate::userland::lifecycle::with_current_process(|p| p.parent_pid as i32) == pid;
    if delivered && pid != 0 {
        if sig == 0 {
            return 0;
        }
        crate::userland::lifecycle::raise_signal_on_stashed_parent(sig);
        return 0;
    }
    -3 // ESRCH
}

/// `tkill(tid, sig)` — single-threaded model: same as `kill(tid,
/// sig)`. We don't track per-thread IDs so PID == TID.
pub fn tkill_handler(args: &mut SyscallArgs) -> i64 {
    kill_handler(args)
}

/// `tgkill(tgid, tid, sig)` — three-arg variant. Reduce to kill by
/// taking the second arg as the target.
pub fn tgkill_handler(args: &mut SyscallArgs) -> i64 {
    let mut shimmed = SyscallArgs::default();
    shimmed.rdi = args.rsi; // tid → pid
    shimmed.rsi = args.rdx; // sig
    kill_handler(&mut shimmed)
}

/// `rt_sigreturn() -> noreturn`.
///
/// User signal handler returned. Its `ret` instruction popped the
/// `sa_restorer` address (placed at the top of the signal frame),
/// which executed `mov $15, eax; syscall` and landed us here. By
/// this point the user RSP — preserved across the syscall stub
/// stash via `r12` — points just past the popped restorer, i.e. at
/// the saved `UserState` we wrote when delivering the signal.
///
/// Read the frame, restore the user state, and `iretq` back to the
/// pre-signal RIP/regs.
pub fn rt_sigreturn_handler(_args: &mut SyscallArgs) -> i64 {
    use crate::userland::user_state::UserState;
    // The syscall stub stashed user RSP into r12 before calling the
    // dispatcher; r12 is callee-saved through Rust calls, so it
    // still holds user RSP here. Read it back via inline asm before
    // the compiler can clobber it.
    let user_rsp: u64;
    unsafe {
        core::arch::asm!("mov {0}, r12", out(reg) user_rsp, options(nomem, preserves_flags, nostack));
    }

    // The frame layout matches `deliver_signal` below: at user_rsp
    // we wrote the saved UserState, immediately following the (now
    // popped) sa_restorer pointer. signum follows after UserState
    // but we don't need it on the return path.
    let saved: UserState = unsafe { core::ptr::read_unaligned(user_rsp as *const UserState) };

    let user_cs = crate::arch::x86_64::gdt::selectors().user_code.0 as u64;
    let user_ss = crate::arch::x86_64::gdt::selectors().user_data.0 as u64;
    unsafe {
        super::iretq_to_user_with_regs(&saved as *const _, user_cs, user_ss);
    }
}

/// Build a signal frame on the user stack and `iretq` into the
/// handler. Diverges. Called from `syscall_dispatch` when a pending,
/// unblocked signal with a custom handler is detected after a syscall
/// returns.
///
/// Frame layout on user stack (low → high address):
/// ```text
///   user_rsp_at_handler_entry → [ sa_restorer        ]   8 bytes
///                                [ saved UserState   ]  128 bytes
///                                [ signum (i64)      ]   8 bytes
/// ```
/// Total 144 bytes, frame address aligned to 16.
///
/// SAFETY: `user_rsp_orig` must be a writable user-mapped address;
/// we write 144 bytes downward from there. The caller (the dispatcher)
/// reads it from the syscall stub's stashed `r12`, which is the
/// user's stack pointer at the point of the syscall — guaranteed
/// writable because the user just used it.
unsafe fn deliver_signal(
    signum: i32,
    action: crate::userland::signal::SigAction,
    callee: crate::userland::user_state::CalleeSavedSnapshot,
    args: &SyscallArgs,
    syscall_ret: i64,
) -> ! {
    use crate::userland::user_state::UserState;

    if action.sa_restorer == 0 {
        // No restorer means the handler can't return cleanly via the
        // standard rt_sigreturn trampoline. We still deliver — the
        // handler may simply not return (calls exit_group, longjmp,
        // etc.), which is what our delivery test relies on. If the
        // handler does try to `ret`, it'll pop 0 as the return
        // address and fault; user-mode bug, not kernel-mode bug.
        crate::debug_warn!(
            "deliver_signal: sig {} handler has no sa_restorer — handler must not `ret`",
            signum
        );
    }

    // 1. Snapshot the user state at the point of the interruption.
    let p = args as *const SyscallArgs as *const u64;
    let user_rip = core::ptr::read(p.add(7));
    let user_rflags = core::ptr::read(p.add(8));
    let user_r12_orig = core::ptr::read(p.add(9));
    let user_rsp = callee.r12_register;
    let saved = UserState {
        rax: syscall_ret as u64,
        rdi: args.rdi, rsi: args.rsi, rdx: args.rdx,
        r10: args.r10, r8: args.r8, r9: args.r9,
        rbx: callee.rbx, rbp: callee.rbp,
        rsp: user_rsp,
        r12: user_r12_orig,
        r13: callee.r13, r14: callee.r14, r15: callee.r15,
        rip: user_rip, rflags: user_rflags,
    };

    // 2. Allocate space on the user stack, 16-aligned. 144 bytes for
    //    [sa_restorer | UserState | signum]; round up to 160 for the
    //    next 16-byte boundary so the handler entry RSP is aligned.
    const FRAME_SIZE: u64 = 8 + 128 + 8;
    let frame_total = (FRAME_SIZE + 15) & !15; // 160
    let frame_addr = user_rsp - frame_total;

    // 3. Write the frame contents. We're running with CR3 = the user
    //    process's L4, so user-VA writes from kernel mode go to the
    //    right pages.
    *(frame_addr as *mut u64) = action.sa_restorer;
    core::ptr::write_unaligned((frame_addr + 8) as *mut UserState, saved);
    *((frame_addr + 8 + 128) as *mut u64) = signum as u64;

    crate::debug_info!(
        "deliver_signal: sig={} handler={:#x} restorer={:#x} frame={:#x}",
        signum,
        action.sa_handler,
        action.sa_restorer,
        frame_addr,
    );

    // 4. Build a fresh UserState for the handler invocation.
    let handler_state = UserState {
        rax: 0,
        rdi: signum as u64, // handler(int sig)
        rsi: 0,             // siginfo_t* (SA_SIGINFO not supported)
        rdx: 0,             // ucontext_t*
        r10: 0, r8: 0, r9: 0,
        rbx: 0, rbp: 0,
        rsp: frame_addr,
        r12: 0, r13: 0, r14: 0, r15: 0,
        rip: action.sa_handler,
        rflags: 0x202,
    };

    let user_cs = crate::arch::x86_64::gdt::selectors().user_code.0 as u64;
    let user_ss = crate::arch::x86_64::gdt::selectors().user_data.0 as u64;
    super::iretq_to_user_with_regs(&handler_state as *const _, user_cs, user_ss);
}

/// Public wrapper so the dispatcher in `abi.rs` can call into the
/// delivery path without exposing the internal asm dance.
pub fn maybe_deliver_signal(
    callee: crate::userland::user_state::CalleeSavedSnapshot,
    args: &SyscallArgs,
    syscall_ret: i64,
) -> Option<i64> {
    let candidate = crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.consume_deliverable()
    });
    if let Some((sig, action)) = candidate {
        unsafe { deliver_signal(sig, action, callee, args, syscall_ret); }
    }
    None
}

// ---------- Phase 5 PR-A: pipes ----------

/// `pipe(int pipefd[2]) -> int`. Equivalent to `pipe2(pipefd, 0)`.
pub fn pipe_handler(args: &mut SyscallArgs) -> i64 {
    pipe2_common(args.rdi, 0)
}

/// `pipe2(int pipefd[2], int flags) -> int`.
///
/// Allocates a kernel pipe object and two fds — `pipefd[0]` for
/// reading, `pipefd[1]` for writing. Both honor the `O_CLOEXEC` flag.
/// `O_NONBLOCK` is ignored (the synchronous-fork model doesn't need
/// blocking I/O semantics on pipes for short pipelines).
pub fn pipe2_handler(args: &mut SyscallArgs) -> i64 {
    pipe2_common(args.rdi, args.rsi as u32)
}

fn pipe2_common(fds_ptr: u64, flags: u32) -> i64 {
    use crate::userland::fdtable::FdSlot;
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    if fds_ptr == 0 {
        return EFAULT;
    }
    if let Err(e) = validate_user_slice(fds_ptr, 8) {
        return e;
    }
    let cloexec = (flags & O_CLOEXEC) != 0;

    let pipe = Pipe::new();
    let read_handle = PipeReadHandle::new(pipe.clone());
    let write_handle = PipeWriteHandle::new(pipe);

    // Allocate both fds atomically — if the second alloc fails, undo
    // the first by removing it before returning EMFILE. Without this,
    // a partially-installed pair would leak a slot.
    let read_fd = match with_fd_table_mut(|t| t.alloc(FdSlot::PipeRead(read_handle, cloexec))) {
        Some(fd) => fd,
        None => return EMFILE,
    };
    let write_fd = match with_fd_table_mut(|t| t.alloc(FdSlot::PipeWrite(write_handle, cloexec))) {
        Some(fd) => fd,
        None => {
            let _ = with_fd_table_mut(|t| t.close(read_fd));
            return EMFILE;
        }
    };

    // Write the fd pair into the user's int[2].
    unsafe {
        core::ptr::write_unaligned(fds_ptr as *mut i32, read_fd);
        core::ptr::write_unaligned((fds_ptr + 4) as *mut i32, write_fd);
    }
    0
}

/// `wait4(pid, status, options, rusage) -> pid`. Reaps a zombie child;
/// since fork is synchronous, by the time the parent reaches wait4
/// the zombie is already in the table. Writes a Linux-shaped status
/// word to `status` if non-NULL: `((exit_code & 0xFF) << 8)`.
pub fn wait4_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::abi::ECHILD;
    let target = args.rdi as i32;
    let status_ptr = args.rsi;
    let _options = args.rdx;
    let _rusage = args.r10;

    let me = crate::userland::lifecycle::current_pid();
    match crate::userland::lifecycle::reap_zombie(target, me) {
        Some((pid, code)) => {
            if status_ptr != 0 {
                if let Err(e) = validate_user_slice(status_ptr, 4) {
                    return e;
                }
                let status = ((code as u32) & 0xFF) << 8;
                unsafe { core::ptr::write(status_ptr as *mut u32, status); }
            }
            pid as i64
        }
        None => ECHILD,
    }
}

// ---------- exit ----------

/// `exit_group(status: i32) -> !` — terminate the user process by
/// long-jumping to the saved kernel continuation. For Phase 4 PR-C2,
/// if the dying process is a forked child, also record it as a zombie
/// so the parent's `wait4` can reap.
pub fn exit_group_handler(args: &mut SyscallArgs) -> i64 {
    let code = args.rdi as i32 as i64;
    *LAST_EXIT_CODE.lock() = Some(code);

    let (has_cont, pid, parent_pid) =
        crate::userland::lifecycle::with_active_user(|au| {
            (au.continuation.is_some(), au.pid, au.parent_pid)
        });
    if !has_cont {
        crate::debug_info!("USERLAND: exit_group({}) recorded (no active continuation)", code);
        return 0;
    }

    // Forked-child path: park a zombie so the parent's wait4 finds it
    // and raise SIGCHLD on the stashed parent. `notify_parent_of_exit`
    // is a no-op when parent_pid == 0 (top-level kernel-launched
    // binary). The abnormal-exit and unimplemented-syscall paths in
    // `lifecycle.rs` route through the same helper so all three exit
    // paths surface a consistent SIGCHLD + zombie to the parent.
    if parent_pid != 0 {
        crate::debug_info!(
            "USERLAND: child pid={} exit_group({}) — long-jumping to fork()",
            pid,
            code,
        );
    } else {
        crate::debug_info!("USERLAND: exit_group({}) — long-jumping to run command", code);
    }
    crate::userland::lifecycle::notify_parent_of_exit(pid, parent_pid, code);
    crate::userland::lifecycle::cooperative_exit(code);
}

// =====================================================================
// Phase 2: file syscalls, stat, cwd, time, random, uname
// =====================================================================

// ---------- Linux syscall constants ----------

/// `openat` first arg sentinel meaning "anchor relative paths at the
/// process cwd."
const AT_FDCWD: i32 = -100;

/// `O_CLOEXEC` — only flag we materially honor (record on the slot).
const O_CLOEXEC: u32 = 0o2000000;
/// Standard access modes — we only support `O_RDONLY` (0). Any non-zero
/// access bit (`O_WRONLY=1`, `O_RDWR=2`) returns `-EROFS`.
const O_ACCMODE: u32 = 0o3;
const O_RDONLY: u32 = 0;
/// Modify-the-world flags we reject as `-EROFS` since the FS is read-only.
const O_WRITE_BITS: u32 = 0o3 | 0o100 | 0o1000 | 0o2000; // RDWR|WRONLY|CREAT|TRUNC|APPEND

/// `lseek` whence values (Linux/POSIX).
const SEEK_SET: i32 = 0;
const SEEK_CUR: i32 = 1;
const SEEK_END: i32 = 2;

/// `fcntl` cmd values — only the small subset libc actually uses pre-exec.
const F_DUPFD: i32 = 0;
const F_GETFD: i32 = 1;
const F_SETFD: i32 = 2;
const F_GETFL: i32 = 3;
const F_SETFL: i32 = 4;
const F_DUPFD_CLOEXEC: i32 = 1030;
const FD_CLOEXEC: u64 = 1;

/// `access` mode bits. The kernel ignores read/write/exec specifics —
/// the FS is read-only and has no permission model — so we just check
/// existence (F_OK) and report success for any combination thereof.
const F_OK: u32 = 0;
const _R_OK: u32 = 4;
const _W_OK: u32 = 2;
const _X_OK: u32 = 1;

/// `clock_gettime` clock IDs we recognize. Both anchor at boot for now;
/// real wall-clock will arrive when an RTC or NTP source is wired up.
const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;

/// `linux_stat64` (x86-64) — 144 bytes laid out per `arch/x86/include/uapi/asm/stat.h`.
#[repr(C)]
#[derive(Default)]
struct LinuxStat {
    st_dev: u64,
    st_ino: u64,
    st_nlink: u64,
    st_mode: u32,
    st_uid: u32,
    st_gid: u32,
    __pad0: u32,
    st_rdev: u64,
    st_size: i64,
    st_blksize: i64,
    st_blocks: i64,
    st_atime: i64,
    st_atime_nsec: u64,
    st_mtime: i64,
    st_mtime_nsec: u64,
    st_ctime: i64,
    st_ctime_nsec: u64,
    __unused: [i64; 3],
}
const _STAT_SIZE_CHECK: () = assert!(core::mem::size_of::<LinuxStat>() == 144);

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const PERM_READ_ALL: u32 = 0o444;
const PERM_RX_ALL: u32 = 0o555;

// ---------- helpers ----------

/// Acquire a clone of the FD slot at `fd`. Releases the `ActiveUser`
/// mutex before returning so subsequent FS calls don't risk lock-order
/// inversion with the FAT layer.
fn with_fd_slot(fd: i32) -> Option<FdSlot> {
    if fd < 0 || (fd as usize) >= FD_TABLE_SIZE {
        return None;
    }
    crate::userland::lifecycle::with_active_user(|au| au.fd_table.get(fd).cloned())
}

/// Run `f` against the live FD table. `f` must not call into anything
/// that re-enters `with_active_user` (notably FS calls).
fn with_fd_table_mut<R>(f: impl FnOnce(&mut FdTable) -> R) -> R {
    crate::userland::lifecycle::with_active_user(|au| f(&mut au.fd_table))
}

fn with_cwd<R>(f: impl FnOnce(&str) -> R) -> R {
    crate::userland::lifecycle::with_active_user(|au| f(&au.cwd))
}

fn set_cwd(new: String) {
    crate::userland::lifecycle::with_active_user(|au| au.cwd = new);
}

/// Map `crate::fs::filesystem::FilesystemError` onto Linux `-errno`.
fn map_filesystem_err(err: &crate::fs::filesystem::FilesystemError) -> i64 {
    use crate::fs::filesystem::FilesystemError as FE;
    match err {
        FE::NotFound => ENOENT,
        FE::PermissionDenied => EACCES,
        FE::InvalidPath => ENOENT,
        FE::ReadOnly => EROFS,
        FE::IsADirectory => EISDIR,
        FE::NotADirectory => ENOTDIR,
        FE::AlreadyExists => crate::userland::abi::EEXIST,
        FE::BufferTooSmall => EINVAL,
        FE::UnsupportedOperation => ENOSYS,
        _ => EIO,
    }
}

/// Map `crate::fs::file_handle::FileError` onto Linux `-errno` values.
fn map_file_err(err: &crate::fs::file_handle::FileError) -> i64 {
    use crate::fs::file_handle::FileError as FE;
    match err {
        FE::NotFound => ENOENT,
        FE::AccessDenied => EACCES,
        FE::InvalidPath => ENOENT,
        FE::NotAFile => EISDIR,
        FE::NotADirectory => ENOTDIR,
        FE::HandleClosed => EBADF,
        FE::SeekOutOfBounds => EINVAL,
        FE::BufferTooSmall => EINVAL,
        FE::IoError => EIO,
        FE::FilesystemError(inner) => map_filesystem_err(inner),
    }
}

fn map_fs_err(err: &crate::fs::fs_manager::FsError) -> i64 {
    use crate::fs::fs_manager::FsError as E;
    match err {
        E::FileNotFound => ENOENT,
        E::AccessDenied => EACCES,
        E::InvalidPath => ENOENT,
        E::NotAFile => EISDIR,
        E::NotADirectory => ENOTDIR,
        E::BufferTooSmall => EINVAL,
        E::NotImplemented => ENOSYS,
        E::IoError => EIO,
    }
}

/// Resolve a user path string against the active CWD into a normalized
/// kernel-side string, then apply the U4 `/etc/...` rewrite so musl's
/// `getpwuid_r` and friends find files staged under `host_share/ETC/`.
/// The rewrite runs AFTER `normalize_path` per the security finding —
/// `..` segments must be collapsed before the prefix check or
/// `/etc/../etc/shadow` could bypass the allowlist.
fn resolve_user_path(ptr: u64) -> Result<String, i64> {
    let raw = copy_user_cstr(ptr)?;
    let normalized = with_cwd(|cwd| normalize_path(cwd, &raw));
    Ok(apply_fs_rewrite(&normalized))
}

/// Coarse monotonic clock — `timer_ticks * 10ms`. Wall-clock equivalence
/// is intentional: we have no RTC/NTP, so realtime starts at 0 too.
fn monotonic_ns() -> u64 {
    let ticks = crate::arch::x86_64::interrupts::get_timer_ticks();
    ticks.saturating_mul(10_000_000)
}

// ---------- open / openat / close ----------

/// `open(path, flags, mode) -> int`. Equivalent to `openat(AT_FDCWD, …)`.
pub fn open_handler(args: &mut SyscallArgs) -> i64 {
    open_common(AT_FDCWD, args.rdi, args.rsi as u32)
}

/// `openat(dirfd, path, flags, mode) -> int`. Only `AT_FDCWD` for dirfd
/// is supported in this milestone — opening a file *relative to a
/// directory fd* needs the FAT subdir walker (PR-4).
pub fn openat_handler(args: &mut SyscallArgs) -> i64 {
    open_common(args.rdi as i32, args.rsi, args.rdx as u32)
}

fn open_common(dirfd: i32, path_ptr: u64, flags: u32) -> i64 {
    if dirfd != AT_FDCWD {
        // openat with a real dirfd is rejected for now. zsh and basic libc
        // overwhelmingly use AT_FDCWD; `man 2 openat` documents this as
        // the common case.
        return ENOSYS;
    }
    if (flags & O_WRITE_BITS) != 0 {
        return EROFS;
    }
    if (flags & O_ACCMODE) != O_RDONLY {
        return EROFS;
    }
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cloexec = (flags & O_CLOEXEC) != 0;

    // Virtual /bin namespace: opening /bin returns a directory FD that
    // getdents64 unpacks into the applet list; opening /bin/<applet>
    // returns a regular File backed by BB.ELF (so tools that read or
    // mmap their argv[0] see the BusyBox binary).
    use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir, BB_HOST_PATH};
    if is_bin_dir(&path) {
        return with_fd_table_mut(|t| {
            t.alloc(FdSlot::VirtualBinDir { cursor: 0, cloexec })
        })
        .map(|fd| fd as i64)
        .unwrap_or(EMFILE);
    }
    if apply_bin_rewrite(&path).is_some() {
        let handle = match crate::fs::file_handle::File::open_read(BB_HOST_PATH) {
            Ok(h) => h,
            Err(ref e) => return map_file_err(e),
        };
        return with_fd_table_mut(|t| t.alloc(FdSlot::File { handle, cloexec }))
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE);
    }

    // Check whether the path is a directory before reaching for File::open
    // (which rejects directories). Directories get their own slot variant
    // so getdents64 can iterate them.
    use crate::fs::filesystem::FileType;
    let meta = match crate::fs::metadata(&path) {
        Ok(m) => m,
        Err(ref e) => return map_fs_err(e),
    };
    if meta.file_type == FileType::Directory {
        let dir = match crate::fs::file_handle::Directory::open(&path) {
            Ok(d) => d,
            Err(ref e) => return map_file_err(e),
        };
        return with_fd_table_mut(|t| {
            t.alloc(FdSlot::Directory {
                handle: dir,
                cursor: 0,
                cloexec,
            })
        })
        .map(|fd| fd as i64)
        .unwrap_or(EMFILE);
    }

    let handle = match crate::fs::file_handle::File::open_read(&path) {
        Ok(h) => h,
        Err(ref e) => return map_file_err(e),
    };
    with_fd_table_mut(|t| {
        t.alloc(FdSlot::File {
            handle,
            cloexec,
        })
    })
    .map(|fd| fd as i64)
    .unwrap_or(EMFILE)
}

/// `close(fd) -> int`. Drops the `Arc<File>` (which closes the underlying
/// handle if this was the last reference). Standard streams cannot be
/// closed in this milestone — closing them would orphan stdout/stderr
/// for the rest of the run, which complicates teardown.
pub fn close_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let slot = with_fd_slot(fd);
    if matches!(
        slot,
        Some(FdSlot::Stdin) | Some(FdSlot::Stdout) | Some(FdSlot::Stderr)
    ) {
        // POSIX permits closing stdin/stdout/stderr; we just no-op.
        return 0;
    }
    with_fd_table_mut(|t| t.close(fd)).err().unwrap_or(0)
}

// ---------- lseek ----------

/// `lseek(fd, offset, whence) -> off_t`. Stream slots return `-ESPIPE`.
/// `SEEK_END` is computed against the file's recorded size at open time
/// (the FAT layer treats files as size-stable for our read-only mount).
pub fn lseek_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let offset = args.rsi as i64;
    let whence = args.rdx as i32;

    let slot = with_fd_slot(fd);
    let handle = match slot {
        Some(FdSlot::File { handle, .. }) => handle,
        Some(FdSlot::Directory { .. }) => {
            // POSIX permits seeking on directories with SEEK_SET to
            // rewind. For now we only honor SEEK_SET 0 → reset cursor.
            if whence == SEEK_SET && offset == 0 {
                with_fd_table_mut(|t| {
                    if let Some(FdSlot::Directory { cursor, .. }) = t.get_mut(fd) {
                        *cursor = 0;
                    }
                });
                return 0;
            }
            return ESPIPE;
        }
        Some(FdSlot::PipeRead(_, _)) | Some(FdSlot::PipeWrite(_, _)) => return ESPIPE,
        Some(_) => return ESPIPE,
        None => return EBADF,
    };

    let new_pos: i64 = match whence {
        SEEK_SET => offset,
        SEEK_CUR => (handle.position() as i64).saturating_add(offset),
        SEEK_END => (handle.size() as i64).saturating_add(offset),
        _ => return EINVAL,
    };
    if new_pos < 0 {
        return EINVAL;
    }
    match handle.seek(new_pos as u64) {
        Ok(p) => p as i64,
        Err(ref e) => map_file_err(e),
    }
}

// ---------- dup / dup2 / fcntl ----------

pub fn dup_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    with_fd_table_mut(|t| t.dup(fd))
        .map(|n| n as i64)
        .unwrap_or(EBADF)
}

pub fn dup2_handler(args: &mut SyscallArgs) -> i64 {
    let oldfd = args.rdi as i32;
    let newfd = args.rsi as i32;
    with_fd_table_mut(|t| t.dup2(oldfd, newfd))
        .map(|n| n as i64)
        .unwrap_or(EBADF)
}

/// `fcntl(fd, cmd, arg) -> int`. Implements just enough of the cmd
/// surface for libc startup: F_DUPFD, F_DUPFD_CLOEXEC, F_GETFD,
/// F_SETFD, F_GETFL, F_SETFL (no-op).
pub fn fcntl_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let cmd = args.rsi as i32;
    let arg = args.rdx;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let slot = match with_fd_slot(fd) {
                Some(s) => s,
                None => return EBADF,
            };
            // F_DUPFD wants the lowest-fd-≥-arg variant; we approximate by
            // using the standard alloc (≥ 3) and ignoring `arg` — libc
            // just expects "some fresh fd," which any free slot satisfies.
            let _ = arg;
            let new = with_fd_table_mut(|t| t.alloc(slot)).unwrap_or(-1);
            if new < 0 {
                return EMFILE;
            }
            if cmd == F_DUPFD_CLOEXEC {
                let _ = with_fd_table_mut(|t| t.set_cloexec(new, true));
            }
            new as i64
        }
        F_GETFD => match with_fd_table_mut(|t| t.cloexec(fd)) {
            Ok(true) => FD_CLOEXEC as i64,
            Ok(false) => 0,
            Err(e) => e,
        },
        F_SETFD => {
            let cloexec = (arg & FD_CLOEXEC) != 0;
            match with_fd_table_mut(|t| t.set_cloexec(fd, cloexec)) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        F_GETFL => {
            // Always-RDONLY for files; stdin treats it as readable too.
            match with_fd_slot(fd) {
                Some(_) => O_RDONLY as i64,
                None => EBADF,
            }
        }
        F_SETFL => {
            // No-op: we don't track per-fd append/nonblock state.
            match with_fd_slot(fd) {
                Some(_) => 0,
                None => EBADF,
            }
        }
        _ => ENOSYS,
    }
}

// ---------- stat / access ----------

fn fill_stat(meta: &crate::fs::filesystem::DirectoryEntry, size_override: Option<u64>) -> LinuxStat {
    use crate::fs::filesystem::FileType;
    let is_dir = meta.file_type == FileType::Directory;
    let mut st = LinuxStat::default();
    st.st_mode = if is_dir { S_IFDIR | PERM_RX_ALL } else { S_IFREG | PERM_READ_ALL };
    st.st_nlink = if is_dir { 2 } else { 1 };
    st.st_uid = 0;
    st.st_gid = 0;
    st.st_size = size_override.unwrap_or(meta.size) as i64;
    st.st_blksize = 4096;
    st.st_blocks = (st.st_size + 511) / 512;
    st
}

fn write_stat(out_ptr: u64, st: &LinuxStat) -> i64 {
    let size = core::mem::size_of::<LinuxStat>() as u64;
    if let Err(e) = validate_user_slice(out_ptr, size) {
        return e;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            st as *const LinuxStat as *const u8,
            out_ptr as *mut u8,
            size as usize,
        );
    }
    0
}

pub fn stat_handler(args: &mut SyscallArgs) -> i64 {
    let path_ptr = args.rdi;
    let out_ptr = args.rsi;
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(st) = stat_virtual_bin(&path) {
        return write_stat(out_ptr, &st);
    }
    let meta = match crate::fs::metadata(&path) {
        Ok(m) => m,
        Err(ref e) => return map_fs_err(e),
    };
    let st = fill_stat(&meta, None);
    write_stat(out_ptr, &st)
}

/// Synthesize a `LinuxStat` for the virtual `/bin` namespace. Returns
/// `Some(st)` if `path` is `/bin` (a directory) or `/bin/<applet>` (a
/// regular file shadowing `BB.ELF`); `None` for any other path.
fn stat_virtual_bin(path: &str) -> Option<LinuxStat> {
    use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir, APPLETS, BB_HOST_PATH};
    if is_bin_dir(path) {
        let mut st = LinuxStat::default();
        st.st_mode = S_IFDIR | PERM_RX_ALL;
        // `.` and `..` plus one for each applet entry — Linux directory
        // st_nlink semantics. Coreutils tools that branch on st_nlink ==
        // 2 for "empty" expect the count to reflect subdirs; we have
        // none, so 2 is correct. We expose applets as regular files,
        // not subdirectories.
        st.st_nlink = 2;
        st.st_blksize = 4096;
        return Some(st);
    }
    if apply_bin_rewrite(path).is_some() {
        // Stat shadows BB.ELF. Pull its size off the FAT mount so tools
        // that mmap their argv[0] see a sensible length. If BB.ELF isn't
        // staged the kernel returns a zero-size record rather than
        // failing — applet PATH lookup still works (access() returns 0)
        // and execve() will report the real error when it fails to load.
        let size = crate::fs::metadata(BB_HOST_PATH).map(|m| m.size).unwrap_or(0);
        let mut st = LinuxStat::default();
        st.st_mode = S_IFREG | PERM_RX_ALL;
        st.st_nlink = APPLETS.len() as u64;
        st.st_size = size as i64;
        st.st_blksize = 4096;
        st.st_blocks = (st.st_size + 511) / 512;
        return Some(st);
    }
    None
}

pub fn fstat_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let out_ptr = args.rsi;

    let slot = with_fd_slot(fd);
    match slot {
        Some(FdSlot::Stdin) | Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => {
            // Streams report as character devices with size 0; libc just
            // wants a successful fstat to decide buffering.
            let mut st = LinuxStat::default();
            st.st_mode = 0o020000 | 0o666; // S_IFCHR | rw-rw-rw-
            st.st_blksize = 4096;
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::File { handle, .. }) => {
            // For files we use the recorded path to look up metadata,
            // overriding the size with the live handle's recorded size
            // (in case a future write path adjusts it).
            let path = handle.path();
            let meta = match crate::fs::metadata(&path) {
                Ok(m) => m,
                Err(ref e) => return map_fs_err(e),
            };
            let st = fill_stat(&meta, Some(handle.size()));
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::PipeRead(_, _)) | Some(FdSlot::PipeWrite(_, _)) => {
            // Pipes report as FIFOs in real Linux. We synthesize an
            // S_IFIFO record so isatty() / file-classification code
            // can distinguish.
            const S_IFIFO: u32 = 0o010000;
            let mut st = LinuxStat::default();
            st.st_mode = S_IFIFO | 0o600;
            st.st_blksize = 4096;
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::Directory { handle, .. }) => {
            let path = handle.path();
            // Synthesize directory stat: metadata("/") may fail because
            // mounts cover only sub-paths, so handle that case directly.
            let st = if path == "/" {
                let mut st = LinuxStat::default();
                st.st_mode = S_IFDIR | PERM_RX_ALL;
                st.st_nlink = 2;
                st.st_blksize = 4096;
                st
            } else {
                let meta = match crate::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(ref e) => return map_fs_err(e),
                };
                fill_stat(&meta, None)
            };
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::VirtualBinDir { .. }) => {
            // Synthesized /bin — same shape stat() reports for the path.
            let st = stat_virtual_bin("/bin").expect("/bin is always virtual");
            write_stat(out_ptr, &st)
        }
        None => EBADF,
    }
}

/// `newfstatat(dirfd, path, statbuf, flags)` — only `AT_FDCWD` is
/// supported for `dirfd`; `flags` (e.g. `AT_SYMLINK_NOFOLLOW`) are
/// ignored (the FS has no symlinks).
pub fn newfstatat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let path_ptr = args.rsi;
    let out_ptr = args.rdx;
    let _flags = args.r10;
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(st) = stat_virtual_bin(&path) {
        return write_stat(out_ptr, &st);
    }
    let meta = match crate::fs::metadata(&path) {
        Ok(m) => m,
        Err(ref e) => return map_fs_err(e),
    };
    let st = fill_stat(&meta, None);
    write_stat(out_ptr, &st)
}

pub fn access_handler(args: &mut SyscallArgs) -> i64 {
    let path_ptr = args.rdi;
    let _mode = args.rsi as u32;
    access_common(path_ptr)
}

pub fn faccessat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let path_ptr = args.rsi;
    let _mode = args.rdx as u32;
    access_common(path_ptr)
}

fn access_common(path_ptr: u64) -> i64 {
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Virtual /bin namespace. Both the directory itself and every known
    // applet entry are addressable for access(): X_OK on /bin/<applet>
    // is what zsh's PATH lookup probes.
    if crate::userland::bin_namespace::is_bin_dir(&path)
        || crate::userland::bin_namespace::apply_bin_rewrite(&path).is_some()
    {
        return 0;
    }
    if crate::fs::exists(&path) {
        0
    } else {
        ENOENT
    }
}

// ---------- cwd ----------

/// `getcwd(buf, size) -> int`. Returns the byte length on success
/// (including trailing NUL), `-ERANGE` if the buffer is too small,
/// `-EFAULT` on a bad pointer.
pub fn getcwd_handler(args: &mut SyscallArgs) -> i64 {
    let buf = args.rdi;
    let size = args.rsi;

    let cwd = with_cwd(|c| alloc::string::String::from(c));
    let needed = cwd.len() as u64 + 1; // NUL
    if size < needed {
        return ERANGE;
    }
    if let Err(e) = validate_user_slice(buf, needed) {
        return e;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(cwd.as_ptr(), buf as *mut u8, cwd.len());
        *((buf + cwd.len() as u64) as *mut u8) = 0;
    }
    needed as i64
}

pub fn chdir_handler(args: &mut SyscallArgs) -> i64 {
    let path_ptr = args.rdi;
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    chdir_to(path)
}

pub fn fchdir_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let slot = with_fd_slot(fd);
    let path = match slot {
        Some(FdSlot::File { handle, .. }) => handle.path(),
        _ => return EBADF,
    };
    chdir_to(path)
}

fn chdir_to(path: alloc::string::String) -> i64 {
    use crate::fs::filesystem::FileType;
    // Treat the root directory as always-valid even if the FS doesn't
    // surface a `metadata("/")` entry (mounts cover only sub-paths).
    if path == "/" {
        set_cwd(path);
        return 0;
    }
    let meta = match crate::fs::metadata(&path) {
        Ok(m) => m,
        Err(ref e) => return map_fs_err(e),
    };
    if meta.file_type != FileType::Directory {
        return ENOTDIR;
    }
    set_cwd(path);
    0
}

// ---------- time / random / uname ----------

#[repr(C)]
#[derive(Default)]
struct LinuxTimespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[repr(C)]
#[derive(Default)]
struct LinuxTimeval {
    tv_sec: i64,
    tv_usec: i64,
}

pub fn clock_gettime_handler(args: &mut SyscallArgs) -> i64 {
    let clk = args.rdi as i32;
    let ts_ptr = args.rsi;

    if clk != CLOCK_REALTIME && clk != CLOCK_MONOTONIC {
        return EINVAL;
    }
    if let Err(e) = validate_user_slice(ts_ptr, core::mem::size_of::<LinuxTimespec>() as u64) {
        return e;
    }
    let ns = monotonic_ns();
    let ts = LinuxTimespec {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            &ts as *const LinuxTimespec as *const u8,
            ts_ptr as *mut u8,
            core::mem::size_of::<LinuxTimespec>(),
        );
    }
    0
}

pub fn gettimeofday_handler(args: &mut SyscallArgs) -> i64 {
    let tv_ptr = args.rdi;
    let _tz_ptr = args.rsi; // legacy timezone arg, ignored
    if tv_ptr == 0 {
        return 0;
    }
    if let Err(e) = validate_user_slice(tv_ptr, core::mem::size_of::<LinuxTimeval>() as u64) {
        return e;
    }
    let ns = monotonic_ns();
    let tv = LinuxTimeval {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_usec: ((ns % 1_000_000_000) / 1_000) as i64,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            &tv as *const LinuxTimeval as *const u8,
            tv_ptr as *mut u8,
            core::mem::size_of::<LinuxTimeval>(),
        );
    }
    0
}

/// `getrandom(buf, len, flags) -> ssize_t`. Tiny xorshift64 seeded from
/// the timer; not cryptographically secure but gives libc a non-zero
/// answer. AT_RANDOM in auxv plays the same role for stack-canary
/// init — both paths converge here for any extra entropy zsh asks for.
pub fn getrandom_handler(args: &mut SyscallArgs) -> i64 {
    let buf = args.rdi;
    let len = args.rsi;
    let _flags = args.rdx;

    if len == 0 {
        return 0;
    }
    let cap = core::cmp::min(len, 4096);
    if let Err(e) = validate_user_slice(buf, cap) {
        return e;
    }
    // xorshift64* — small, fast, deterministic enough for libc seed needs.
    let mut state: u64 = monotonic_ns() ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 0xDEAD_BEEF_CAFE_BABE;
    }
    for i in 0..cap {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let byte = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8;
        unsafe {
            *((buf + i) as *mut u8) = byte;
        }
    }
    cap as i64
}

#[repr(C)]
struct LinuxUtsname {
    sysname: [u8; 65],
    nodename: [u8; 65],
    release: [u8; 65],
    version: [u8; 65],
    machine: [u8; 65],
    domainname: [u8; 65],
}

fn pack_utsname_field(field: &mut [u8; 65], s: &str) {
    let n = core::cmp::min(64, s.len());
    field[..n].copy_from_slice(&s.as_bytes()[..n]);
    // Remainder is already zero from the initializer.
}

// ---------- getdents64 ----------

/// `linux_dirent64` in-memory layout (per `include/uapi/linux/dirent.h`):
///
/// ```text
///   d_ino    : u64       (offset 0)
///   d_off    : u64       (offset 8)  — opaque cookie for the next call
///   d_reclen : u16       (offset 16)
///   d_type   : u8        (offset 18)
///   d_name   : [u8; …]   (offset 19, NUL-terminated)
///   pad      : enough zeros to make d_reclen 8-byte-aligned
/// ```
///
/// libc reads `d_reclen` to step from one record to the next, so we
/// must round each record up to an 8-byte boundary.
const DIRENT_HEADER_SIZE: usize = 19;

const DT_UNKNOWN: u8 = 0;
const DT_REG: u8 = 8;
const DT_DIR: u8 = 4;

#[inline]
fn align_up_8(n: usize) -> usize {
    (n + 7) & !7
}

/// FNV-1a 64-bit. Used to fabricate a non-zero `d_ino` from the entry
/// name + parent path — FAT has no real inodes, but glob/find walk
/// dirents and refuse zero `d_ino` in some libc paths.
fn fnv1a_64(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// `getdents64(fd, dirp, count) -> isize`. Walks the directory's
/// snapshotted entries from the per-fd cursor, emitting as many full
/// records as fit in `count` bytes. Returns the bytes written, or 0
/// when the cursor has reached the end (libc treats that as EOF).
/// `getdents64` dispatch for the synthesized `/bin` directory. Returns
/// `Some(bytes_written)` if `fd` is a `VirtualBinDir` slot (including
/// `Some(0)` at EOF); `None` to fall through to the FAT path.
///
/// Emits `.` and `..` once at cursor 0, then one record per applet in
/// the order they appear in `APPLETS`. Cursor encoding:
///   - 0           → not yet started; emit `.` and `..` then applets
///   - 1..=APPLETS.len() → next applet index to emit (1 = first applet)
///   - APPLETS.len() + 1 → EOF
fn getdents64_virtual_bin(fd: i32, dirp: u64, cap: usize) -> Option<i64> {
    use crate::userland::bin_namespace::APPLETS;

    let start = with_fd_table_mut(|t| match t.get(fd) {
        Some(FdSlot::VirtualBinDir { cursor, .. }) => Some(*cursor),
        _ => None,
    })?;
    let total_records = APPLETS.len() + 2; // ".", ".." + applets
    if start >= total_records {
        // EOF on this directory.
        return Some(0);
    }

    let mut staging: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(cap);
    let mut cursor = start;
    // Synthetic inode numbers — keep them deterministic and non-zero.
    let parent_seed = fnv1a_64(0xcbf2_9ce4_8422_2325, b"/bin");

    while cursor < total_records {
        let (name, d_type) = match cursor {
            0 => (".".as_bytes(), DT_DIR),
            1 => ("..".as_bytes(), DT_DIR),
            n => (APPLETS[n - 2].as_bytes(), DT_REG),
        };
        let reclen = align_up_8(DIRENT_HEADER_SIZE + name.len() + 1);
        if staging.len() + reclen > cap {
            break;
        }
        let d_ino = fnv1a_64(parent_seed, name);
        let next_cursor = (cursor + 1) as u64;
        staging.extend_from_slice(&d_ino.to_ne_bytes());
        staging.extend_from_slice(&next_cursor.to_ne_bytes());
        staging.extend_from_slice(&(reclen as u16).to_ne_bytes());
        staging.push(d_type);
        staging.extend_from_slice(name);
        staging.push(0);
        while staging.len() % 8 != 0 {
            staging.push(0);
        }
        cursor += 1;
    }

    if staging.is_empty() {
        // Buffer too small for even one record.
        return Some(EINVAL);
    }

    // Commit cursor + copy to user.
    with_fd_table_mut(|t| {
        if let Some(FdSlot::VirtualBinDir { cursor: c, .. }) = t.get_mut(fd) {
            *c = cursor;
        }
    });
    unsafe {
        core::ptr::copy_nonoverlapping(staging.as_ptr(), dirp as *mut u8, staging.len());
    }
    Some(staging.len() as i64)
}

pub fn getdents64_handler(args: &mut SyscallArgs) -> i64 {
    use crate::fs::filesystem::FileType;
    let fd = args.rdi as i32;
    let dirp = args.rsi;
    let count = args.rdx;

    if count == 0 {
        return EINVAL;
    }
    let cap = core::cmp::min(count, 64 * 1024);
    if let Err(e) = validate_user_slice(dirp, cap) {
        return e;
    }

    // Virtual /bin dispatches before the FAT directory path because its
    // FD slot is a different variant — no FAT entries to read.
    if let Some(written) = getdents64_virtual_bin(fd, dirp, cap as usize) {
        return written;
    }

    // Snapshot the entries + parent path under the active-user mutex.
    // We can't hold the mutex while walking the user buffer (`print!`
    // and friends could be called by other code paths), so collect
    // into a small kernel-side staging buffer first.
    let snapshot = with_fd_table_mut(|t| {
        let slot = t.get_mut(fd);
        match slot {
            Some(FdSlot::Directory { handle, cursor, .. }) => {
                let entries = handle.entries();
                let path = handle.path();
                let start = *cursor;
                Some((entries, path, start))
            }
            Some(_) => None,
            None => None,
        }
    });
    let (entries, parent_path, start_cursor) = match snapshot {
        Some(s) => s,
        None => {
            // Fd not present at all → EBADF; fd present but not a
            // directory → ENOTDIR. Disambiguate.
            return if with_fd_slot(fd).is_some() {
                ENOTDIR
            } else {
                EBADF
            };
        }
    };

    let parent_seed = fnv1a_64(0xcbf2_9ce4_8422_2325, parent_path.as_bytes());

    // Walk entries from the cursor, building records into a kernel
    // staging buffer, until either we run out of entries or the next
    // record won't fit.
    let mut staging: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(cap as usize);
    let mut consumed = 0usize;
    let mut cursor = start_cursor;
    while cursor < entries.len() {
        let entry = &entries[cursor];
        let name = &entry.name[..entry.name_len];
        let reclen = align_up_8(DIRENT_HEADER_SIZE + name.len() + 1);
        if staging.len() + reclen > cap as usize {
            break;
        }

        let d_ino = fnv1a_64(parent_seed, name);
        let d_type = match entry.file_type {
            FileType::File => DT_REG,
            FileType::Directory => DT_DIR,
            _ => DT_UNKNOWN,
        };
        // Header: u64 ino, u64 off, u16 reclen, u8 type
        staging.extend_from_slice(&d_ino.to_ne_bytes());
        // d_off semantics: opaque cookie pointing at the *next* record.
        // The simplest valid value is the cursor index after consuming
        // this entry — libc only uses it to seek back; we don't honor
        // that yet, but a non-zero value is required.
        let next_cursor = (cursor + 1) as u64;
        staging.extend_from_slice(&next_cursor.to_ne_bytes());
        staging.extend_from_slice(&(reclen as u16).to_ne_bytes());
        staging.push(d_type);
        // Name + NUL.
        staging.extend_from_slice(name);
        staging.push(0);
        // Pad to reclen.
        while staging.len() % 8 != 0 {
            staging.push(0);
        }
        debug_assert_eq!(staging.len() - consumed, reclen);
        consumed = staging.len();
        cursor += 1;
    }

    if staging.is_empty() {
        // Either at-EOF (returns 0) or the user buffer is too small for
        // even one record (Linux returns -EINVAL in that case).
        if start_cursor >= entries.len() {
            return 0;
        }
        return EINVAL;
    }

    // Commit cursor + copy to user.
    with_fd_table_mut(|t| {
        if let Some(FdSlot::Directory { cursor: c, .. }) = t.get_mut(fd) {
            *c = cursor;
        }
    });
    unsafe {
        core::ptr::copy_nonoverlapping(staging.as_ptr(), dirp as *mut u8, staging.len());
    }
    staging.len() as i64
}

pub fn uname_handler(args: &mut SyscallArgs) -> i64 {
    let out_ptr = args.rdi;
    let size = core::mem::size_of::<LinuxUtsname>() as u64;
    if let Err(e) = validate_user_slice(out_ptr, size) {
        return e;
    }
    let mut u = LinuxUtsname {
        sysname: [0; 65],
        nodename: [0; 65],
        release: [0; 65],
        version: [0; 65],
        machine: [0; 65],
        domainname: [0; 65],
    };
    pack_utsname_field(&mut u.sysname, "Linux");
    pack_utsname_field(&mut u.nodename, "agenticos");
    pack_utsname_field(&mut u.release, "6.0.0-agenticos");
    pack_utsname_field(&mut u.version, "AgenticOS phase-2");
    pack_utsname_field(&mut u.machine, "x86_64");
    pack_utsname_field(&mut u.domainname, "(none)");
    unsafe {
        core::ptr::copy_nonoverlapping(
            &u as *const LinuxUtsname as *const u8,
            out_ptr as *mut u8,
            size as usize,
        );
    }
    0
}

// ---------- U3: musl-init / zsh-startup syscalls ----------

// poll/ppoll constants. POLLNVAL is the only one we generate when an
// fd isn't valid; the others are just bit copies from `events` to
// `revents` for valid stream fds.
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLNVAL: i16 = 0x0020;

/// Linux `struct pollfd` — 8 bytes, packed naturally on x86-64.
#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

/// Maximum pollfd array length we'll process. zsh's ZLE polls one or
/// two fds; musl's `__init_libc` polls three (stdin/stdout/stderr).
/// Capping at 64 defends against integer-overflow attacks on
/// `nfds * sizeof(pollfd)` and against pathological user input
/// without restricting any realistic caller.
const POLL_MAX_NFDS: u64 = 64;

/// `poll(fds: *mut pollfd, nfds: nfds_t, timeout: int) -> int`
///
/// Real-shaped: validate the user pollfd array (with checked
/// multiplication of `nfds * size_of::<PollFd>()` to defeat overflow),
/// then for each entry mark `revents` according to the fd's class:
/// stdin/stdout/stderr report whatever events the caller asked for as
/// "ready" (we have no real I/O wait — the subsequent read/write call
/// is what blocks); valid open files and pipes report POLLIN/POLLOUT
/// likewise; unknown fds get POLLNVAL set. Returns the count of pollfd
/// entries with non-zero `revents`.
///
/// Timeout is ignored — every poll call returns immediately. zsh's ZLE
/// uses poll for keytimeout disambiguation; without a real timer the
/// best we can do is "always ready," which makes ZLE call read() and
/// block there.
pub fn poll_handler(args: &mut SyscallArgs) -> i64 {
    poll_common(args.rdi, args.rsi)
}

/// `ppoll(fds, nfds, *timeout, *sigmask, sigsetsize) -> int`
///
/// Linux-x86-64 ppoll. We ignore the timespec, sigmask, and sigsetsize;
/// shape is identical to `poll` for our purposes.
pub fn ppoll_handler(args: &mut SyscallArgs) -> i64 {
    poll_common(args.rdi, args.rsi)
}

fn poll_common(fds_ptr: u64, nfds: u64) -> i64 {
    if nfds == 0 {
        return 0;
    }
    if nfds > POLL_MAX_NFDS {
        return EINVAL;
    }
    // Checked multiplication — `nfds * sizeof(PollFd)` must not overflow.
    // A user passing nfds = u64::MAX would otherwise wrap to a small
    // length and `validate_user_slice` would happily approve a tiny
    // window while we read 8 * u64::MAX bytes. The cap above already
    // forecloses this in practice; the checked_mul is belt-and-suspenders.
    let bytes = match nfds.checked_mul(core::mem::size_of::<PollFd>() as u64) {
        Some(b) => b,
        None => return EINVAL,
    };
    if let Err(e) = validate_user_slice(fds_ptr, bytes) {
        return e;
    }
    let n = nfds as usize;
    let slice = unsafe {
        core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, n)
    };
    let mut ready = 0i64;
    for entry in slice.iter_mut() {
        let want = entry.events;
        let revents = match with_fd_slot(entry.fd) {
            Some(_) => want & (POLLIN | POLLOUT), // mirror requested bits as ready
            None => POLLNVAL,
        };
        entry.revents = revents;
        if revents != 0 {
            ready += 1;
        }
    }
    ready
}

/// `pselect6(nfds, *readfds, *writefds, *exceptfds, *timeout, *sigmask) -> int`
///
/// Stubbed `-ENOSYS` for now. The trace mode in U2 will surface a real
/// pselect6 call from zsh if its build calls it (most don't — `poll`
/// covers ZLE's needs in the common configuration).
pub fn pselect6_handler(_args: &mut SyscallArgs) -> i64 {
    ENOSYS
}

/// Maximum bytes we'll write into a user readlink buffer.
const READLINK_MAX_BUF: u64 = 4096;
/// Maximum cstring length we'll copy from user space when looking up
/// a readlink target.
const READLINK_MAX_PATH: usize = 256;

/// `readlink(path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t`
///
/// Inline procfs synthesis covers the two paths zsh actually opens:
///   - `/proc/self/exe` → the launch path of the current process
///     (set by `enter_user_mode_with_aspace` from argv[0]; updated by
///     execve). Used by zsh to resolve `$ZSH_ARGZERO`.
///   - `/proc/self/fd/<N>` → a synthetic name for fd N: `/dev/tty` for
///     the standard streams, the backing file path for opened files,
///     `pipe:[<n>]` for pipe ends. Used by `ttyname()`.
///
/// Other paths return `-ENOENT` (no real symlinks on the FAT mount).
/// The result is NOT null-terminated; we return the byte count written.
pub fn readlink_handler(args: &mut SyscallArgs) -> i64 {
    readlink_common(args.rdi, args.rsi, args.rdx)
}

/// `readlinkat(dirfd: i32, path: *const c_char, buf: *mut c_char, bufsiz: size_t) -> ssize_t`
///
/// Only supports `dirfd == AT_FDCWD`. Other dirfds return `-ENOSYS`.
/// musl prefers readlinkat over readlink in newer versions.
pub fn readlinkat_handler(args: &mut SyscallArgs) -> i64 {
    const AT_FDCWD: i32 = -100;
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    readlink_common(args.rsi, args.rdx, args.r10)
}

fn readlink_common(path_ptr: u64, buf_ptr: u64, bufsiz: u64) -> i64 {
    if bufsiz == 0 {
        return EINVAL;
    }
    if bufsiz > READLINK_MAX_BUF {
        return EINVAL;
    }
    if let Err(e) = validate_user_slice(buf_ptr, bufsiz) {
        return e;
    }
    let path = match copy_user_cstr(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    if path.len() > READLINK_MAX_PATH {
        return ERANGE;
    }
    let target = match resolve_proc_link(&path) {
        Some(t) => t,
        None => return ENOENT,
    };
    let bytes = target.as_bytes();
    let n = core::cmp::min(bytes.len(), bufsiz as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as i64
}

/// Inline minimal procfs: resolve `/proc/self/exe` and `/proc/self/fd/N`
/// to a synthetic target string, or `None` for any other path. Lives
/// here (not in a separate module) per scope-guardian: we only need
/// these two paths and putting them inline keeps the readlink handler
/// self-contained.
fn resolve_proc_link(path: &str) -> Option<String> {
    if path == "/proc/self/exe" {
        return crate::userland::lifecycle::with_active_user(|p| p.exe_path.clone());
    }
    let fd_prefix = "/proc/self/fd/";
    if let Some(rest) = path.strip_prefix(fd_prefix) {
        // Bounded integer parse — defends against `/proc/self/fd/-1`,
        // `/proc/self/fd/99999999999999999999`, leading zeros, trailing
        // garbage. `u32::from_str` rejects all of those.
        let fd: u32 = rest.parse().ok()?;
        return resolve_proc_self_fd(fd as i32);
    }
    None
}

fn resolve_proc_self_fd(fd: i32) -> Option<String> {
    let slot = with_fd_slot(fd)?;
    Some(match slot {
        FdSlot::Stdin | FdSlot::Stdout | FdSlot::Stderr => String::from("/dev/tty"),
        FdSlot::File { handle, .. } => handle.path(),
        FdSlot::Directory { handle, .. } => handle.path(),
        FdSlot::PipeRead(_, _) | FdSlot::PipeWrite(_, _) => String::from("pipe:[0]"),
        FdSlot::VirtualBinDir { .. } => String::from("/bin"),
    })
}

/// Linux `struct rlimit` (16 bytes on 64-bit).
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxRlimit {
    rlim_cur: u64,
    rlim_max: u64,
}

const RLIM_INFINITY: u64 = u64::MAX;

/// `getrlimit(resource: i32, *rlim: *mut rlimit) -> int`
///
/// Stub: every resource reports `RLIM_INFINITY` for both `cur` and `max`.
/// Sufficient for zsh's startup queries on `RLIMIT_STACK`, `RLIMIT_NOFILE`,
/// `RLIMIT_DATA`, `RLIMIT_AS`. Real per-process limits are out of scope.
pub fn getrlimit_handler(args: &mut SyscallArgs) -> i64 {
    let out_ptr = args.rsi;
    write_rlim_infinity(out_ptr)
}

/// `prlimit64(pid, resource, *new_limit, *old_limit) -> int`
///
/// Stub: ignore `pid` (we have one process from the user's perspective)
/// and `new_limit` (no enforcement); if `old_limit` is non-null, write
/// `RLIM_INFINITY` into it. Returns 0.
pub fn prlimit64_handler(args: &mut SyscallArgs) -> i64 {
    let old_ptr = args.r10;
    if old_ptr == 0 {
        return 0;
    }
    write_rlim_infinity(old_ptr)
}

fn write_rlim_infinity(out_ptr: u64) -> i64 {
    let size = core::mem::size_of::<LinuxRlimit>() as u64;
    if let Err(e) = validate_user_slice(out_ptr, size) {
        return e;
    }
    let r = LinuxRlimit { rlim_cur: RLIM_INFINITY, rlim_max: RLIM_INFINITY };
    unsafe {
        core::ptr::copy_nonoverlapping(
            &r as *const LinuxRlimit as *const u8,
            out_ptr as *mut u8,
            size as usize,
        );
    }
    0
}

/// Linux `struct rusage` layout (x86-64): two `timeval` pairs followed by
/// 14 `long` counters. 144 bytes total. zsh reads it at startup for the
/// `times` builtin / shell timing init.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxRusage {
    ru_utime_sec: i64,
    ru_utime_usec: i64,
    ru_stime_sec: i64,
    ru_stime_usec: i64,
    ru_maxrss: i64,
    ru_ixrss: i64,
    ru_idrss: i64,
    ru_isrss: i64,
    ru_minflt: i64,
    ru_majflt: i64,
    ru_nswap: i64,
    ru_inblock: i64,
    ru_oublock: i64,
    ru_msgsnd: i64,
    ru_msgrcv: i64,
    ru_nsignals: i64,
    ru_nvcsw: i64,
    ru_nivcsw: i64,
}

/// `getrusage(who: i32, *usage) -> int`
///
/// Stub: zero the `rusage` struct and return 0. We don't track per-process
/// CPU time or fault counters, so a zero report is the honest answer.
/// `who` is validated against the documented set (RUSAGE_SELF=0,
/// RUSAGE_CHILDREN=-1, RUSAGE_THREAD=1) — anything else returns -EINVAL,
/// matching Linux.
pub fn getrusage_handler(args: &mut SyscallArgs) -> i64 {
    const RUSAGE_CHILDREN: i32 = -1;
    const RUSAGE_SELF: i32 = 0;
    const RUSAGE_THREAD: i32 = 1;

    let who = args.rdi as i32;
    let out_ptr = args.rsi;

    if who != RUSAGE_SELF && who != RUSAGE_CHILDREN && who != RUSAGE_THREAD {
        return EINVAL;
    }
    let size = core::mem::size_of::<LinuxRusage>() as u64;
    if let Err(e) = validate_user_slice(out_ptr, size) {
        return e;
    }
    let zero = LinuxRusage::default();
    unsafe {
        core::ptr::copy_nonoverlapping(
            &zero as *const LinuxRusage as *const u8,
            out_ptr as *mut u8,
            size as usize,
        );
    }
    0
}

/// Linux `struct itimerval` (matches musl's layout: two `timeval` pairs,
/// each `{ tv_sec: i64, tv_usec: i64 }` on x86-64 = 32 bytes total).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxItimerval {
    it_interval_sec: i64,
    it_interval_usec: i64,
    it_value_sec: i64,
    it_value_usec: i64,
}

/// `setitimer(which: i32, *new_value, *old_value) -> int`
///
/// Stub: returns 0, no real timer wired. zsh installs SIGALRM handlers
/// for KEYTIMEOUT and TMOUT; without the timer firing those features are
/// effectively no-op (acceptable for `--no-rcs`-style usage). If
/// `old_value` is non-null, write zeroed itimerval (no prior timer was
/// active).
pub fn setitimer_handler(args: &mut SyscallArgs) -> i64 {
    let old_ptr = args.rdx;
    if old_ptr == 0 {
        return 0;
    }
    let size = core::mem::size_of::<LinuxItimerval>() as u64;
    if let Err(e) = validate_user_slice(old_ptr, size) {
        return e;
    }
    let zero = LinuxItimerval::default();
    unsafe {
        core::ptr::copy_nonoverlapping(
            &zero as *const LinuxItimerval as *const u8,
            old_ptr as *mut u8,
            size as usize,
        );
    }
    0
}

/// `nanosleep(*req: *const timespec, *rem: *mut timespec) -> int`
///
/// Stub: returns 0 immediately, ignoring the requested duration. zsh's
/// `sleep` builtin and `zselect` are the main consumers; neither is
/// exercised in our minimum interactive test path. If `rem` is non-null,
/// write a zeroed timespec (no remaining time, since we "slept" the full
/// duration in zero wall-clock).
pub fn nanosleep_handler(args: &mut SyscallArgs) -> i64 {
    let rem_ptr = args.rsi;
    if rem_ptr == 0 {
        return 0;
    }
    let size = core::mem::size_of::<LinuxTimespec>() as u64;
    if let Err(e) = validate_user_slice(rem_ptr, size) {
        return e;
    }
    let zero = LinuxTimespec::default();
    unsafe {
        core::ptr::copy_nonoverlapping(
            &zero as *const LinuxTimespec as *const u8,
            rem_ptr as *mut u8,
            size as usize,
        );
    }
    0
}
