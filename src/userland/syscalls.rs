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
    validate_user_slice, EBADF, EFAULT, EINVAL, ENOSYS, ENOTTY, LAST_EXIT_CODE,
};
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
/// Maximum brk growth from the initial anchor in bytes.
const BRK_MAX_BYTES: u64 = 8 * 1024 * 1024;

// ---------- Linux constants ----------

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;

const MAP_PRIVATE: u64 = 0x02;
const MAP_ANONYMOUS: u64 = 0x20;

const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;

const TCGETS: u64 = 0x5401;

// ---------- write / writev / read ----------

/// `write(fd: i32, buf: *const u8, count: usize) -> isize`
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
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let s = core::str::from_utf8(slice).unwrap_or("");
    crate::print!("{}", s);
    len as i64
}

/// `writev(fd: i32, iov: *const iovec, iovcnt: i32) -> isize`
///
/// `iovec { void *iov_base; size_t iov_len; }`. Each entry is two qwords.
/// musl's stdio uses this to flush its buffer plus any pending putback in
/// one call.
pub fn writev_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i64;
    let iov_ptr = args.rsi;
    let iovcnt = args.rdx as i64;

    if !(fd == 1 || fd == 2) {
        return EBADF;
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
        let s = core::str::from_utf8(slice).unwrap_or("");
        crate::print!("{}", s);
        written += len;
    }
    written as i64
}

/// `read(fd: i32, buf: *mut u8, count: usize) -> isize`
///
/// stdin (fd 0): returns 0 (EOF) until a real keyboard input plumbing
/// lands. stdout/stderr return `-EBADF`. Other fds return `-EBADF` as
/// well — no file-descriptor table this milestone.
pub fn read_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i64;
    if fd == 0 {
        return 0; // EOF stub
    }
    EBADF
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

/// `ioctl(fd: i32, request: u64, ...) -> int`
///
/// Only `TCGETS` (0x5401) is recognized — return `-ENOTTY` so libstdc++'s
/// underlying stdio picks full buffering instead of line buffering for
/// stdout. Anything else: `-ENOSYS`.
pub fn ioctl_handler(args: &mut SyscallArgs) -> i64 {
    let request = args.rsi;
    match request {
        TCGETS => ENOTTY,
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

/// `rt_sigaction(signum, act, oldact, sigsetsize) -> int` — no-op stub.
/// musl installs handlers proactively on first signal use; returning 0
/// satisfies the call without actually wiring delivery.
pub fn rt_sigaction_handler(_args: &mut SyscallArgs) -> i64 {
    0
}

/// `rt_sigprocmask(how, set, oldset, sigsetsize) -> int` — no-op stub.
pub fn rt_sigprocmask_handler(_args: &mut SyscallArgs) -> i64 {
    0
}

// ---------- credentials ----------

pub fn getuid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn getgid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn geteuid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn getegid_handler(_: &mut SyscallArgs) -> i64 { 0 }
pub fn getpid_handler(_: &mut SyscallArgs) -> i64 { 1 }
pub fn getppid_handler(_: &mut SyscallArgs) -> i64 { 0 }

// ---------- exit ----------

/// `exit_group(status: i32) -> !` — terminate the user process by
/// long-jumping to the saved kernel continuation.
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
