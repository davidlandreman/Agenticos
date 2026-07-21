//! Dispatcher-level tests for `fcntl(F_GETLK/F_SETLK/F_SETLKW)`.
//!
//! These drive `syscall_dispatch` synthetically (no current ring-3 process, so
//! the sentinel PID-0 fd table backs opens — the same harness the procfs tests
//! use) to exercise the `struct flock` ABI: whence/len resolution, the
//! `F_GETLK` reply marshaling, and the non-lockable-fd `EINVAL` guard. The
//! byte-range algebra itself is covered by `record_lock::record_lock_tests`;
//! cross-process contention and the blocking `F_SETLKW` wake are validated
//! end-to-end during GNU Make bring-up (single-owner synthetic dispatch cannot
//! model two distinct TGIDs).

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::userland::abi::{syscall_dispatch, EBADF, EINVAL};
use crate::userland::record_lock::{self, LockKind, LockRange};

const F_GETLK: i32 = 5;
const F_SETLK: i32 = 6;
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;
const SEEK_SET: i16 = 0;

/// Mirror of the kernel `LinuxFlock` layout (32 bytes, x86-64).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Flock {
    l_type: i16,
    l_whence: i16,
    _pad0: u32,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
    _pad1: u32,
}

fn set_bounds(ptr: u64, len: u64) {
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + len,
    });
}

fn dispatch_open(path: &[u8], flags: u64) -> i64 {
    set_bounds(path.as_ptr() as u64, path.len() as u64);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::OPEN;
    args.rdi = path.as_ptr() as u64;
    args.rsi = flags;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    ret
}

fn dispatch_close(fd: i64) -> i64 {
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::CLOSE;
    args.rdi = fd as u64;
    syscall_dispatch(&mut args)
}

fn dispatch_fcntl(fd: i64, cmd: i32, flock: &mut Flock) -> i64 {
    let ptr = flock as *mut Flock as u64;
    set_bounds(ptr, core::mem::size_of::<Flock>() as u64);
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::FCNTL;
    args.rdi = fd as u64;
    args.rsi = cmd as u64;
    args.rdx = ptr;
    let ret = syscall_dispatch(&mut args);
    crate::userland::abi::clear_user_va_bounds();
    ret
}

/// O_RDWR | O_CREAT | O_TRUNC.
const CREATE_FLAGS: u64 = 0o2 | 0o100 | 0o1000;

fn open_scratch(path: &[u8]) -> i64 {
    let fd = dispatch_open(path, CREATE_FLAGS);
    assert!(fd >= 0, "open {:?} failed: {}", path, fd);
    fd
}

fn whole_file(l_type: i16) -> Flock {
    Flock {
        l_type,
        l_whence: SEEK_SET,
        l_start: 0,
        l_len: 0,
        ..Flock::default()
    }
}

/// A whole-file write lock and its release both succeed through the ABI.
fn test_fcntl_setlk_roundtrip() {
    let fd = open_scratch(b"/work/rl-roundtrip\0");
    let mut lock = whole_file(F_WRLCK);
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut lock), 0);
    let mut unlock = whole_file(F_UNLCK);
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut unlock), 0);
    assert_eq!(dispatch_close(fd), 0);
}

/// `F_GETLK` marshals a conflicting holder's type/range/pid back into the
/// caller's `flock`. Seed the conflictor as a distinct owner directly, since
/// synthetic dispatch has only one TGID.
fn test_fcntl_getlk_reports_conflict() {
    const OTHER: u32 = 0x00AB_CDEF;
    let path = "/work/rl-getlk-conflict";
    let fd = open_scratch(b"/work/rl-getlk-conflict\0");
    record_lock::release_owner(OTHER);
    assert_eq!(
        record_lock::set(
            path,
            LockRange {
                start: 5,
                end: 15,
            },
            LockKind::Write,
            OTHER,
        ),
        Ok(())
    );
    let mut probe = whole_file(F_RDLCK);
    assert_eq!(dispatch_fcntl(fd, F_GETLK, &mut probe), 0);
    assert_eq!(probe.l_type, F_WRLCK);
    assert_eq!(probe.l_whence, SEEK_SET);
    assert_eq!(probe.l_start, 5);
    assert_eq!(probe.l_len, 11); // inclusive [5,15] -> len 11
    assert_eq!(probe.l_pid, OTHER as i32);
    record_lock::release_owner(OTHER);
    assert_eq!(dispatch_close(fd), 0);
}

/// `F_GETLK` on a free range reports `F_UNLCK`.
fn test_fcntl_getlk_free() {
    let fd = open_scratch(b"/work/rl-getlk-free\0");
    let mut probe = whole_file(F_WRLCK);
    assert_eq!(dispatch_fcntl(fd, F_GETLK, &mut probe), 0);
    assert_eq!(probe.l_type, F_UNLCK);
    assert_eq!(dispatch_close(fd), 0);
}

/// Locking a non-regular-file descriptor is rejected, and a bad fd is EBADF.
fn test_fcntl_lock_non_file_and_badfd() {
    // A synthetic `/proc` file is a `VirtualFile` slot — not lockable.
    let fd = dispatch_open(b"/proc/uptime\0", 0);
    assert!(fd >= 0, "open /proc/uptime failed: {}", fd);
    let mut lock = whole_file(F_WRLCK);
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut lock), EINVAL);
    assert_eq!(dispatch_close(fd), 0);

    let mut lock2 = whole_file(F_WRLCK);
    assert_eq!(dispatch_fcntl(999, F_SETLK, &mut lock2), EBADF);
}

/// Bad `l_whence`, bad `l_type`, and an `F_UNLCK` probe are all `EINVAL`.
fn test_fcntl_lock_validation() {
    let fd = open_scratch(b"/work/rl-validate\0");

    let mut bad_whence = Flock {
        l_type: F_WRLCK,
        l_whence: 99,
        ..Flock::default()
    };
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut bad_whence), EINVAL);

    let mut bad_type = Flock {
        l_type: 99,
        l_whence: SEEK_SET,
        ..Flock::default()
    };
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut bad_type), EINVAL);

    // F_UNLCK is not a valid F_GETLK probe type.
    let mut unlck_probe = whole_file(F_UNLCK);
    assert_eq!(dispatch_fcntl(fd, F_GETLK, &mut unlck_probe), EINVAL);

    assert_eq!(dispatch_close(fd), 0);
}

/// Closing a descriptor drops the process's locks on that file (POSIX).
fn test_fcntl_close_releases_locks() {
    let path = "/work/rl-close-release";
    let fd = open_scratch(b"/work/rl-close-release\0");
    let mut lock = whole_file(F_WRLCK);
    assert_eq!(dispatch_fcntl(fd, F_SETLK, &mut lock), 0);
    // A distinct owner is blocked while the sentinel holds the whole-file lock.
    const OTHER: u32 = 0x0055_00AA;
    record_lock::release_owner(OTHER);
    assert!(record_lock::set(
        path,
        LockRange {
            start: 0,
            end: u64::MAX,
        },
        LockKind::Write,
        OTHER,
    )
    .is_err());
    // Closing the sentinel's fd releases its lock; the other owner can proceed.
    assert_eq!(dispatch_close(fd), 0);
    assert_eq!(
        record_lock::set(
            path,
            LockRange {
                start: 0,
                end: u64::MAX,
            },
            LockKind::Write,
            OTHER,
        ),
        Ok(())
    );
    record_lock::release_owner(OTHER);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_fcntl_setlk_roundtrip,
        &test_fcntl_getlk_reports_conflict,
        &test_fcntl_getlk_free,
        &test_fcntl_lock_non_file_and_badfd,
        &test_fcntl_lock_validation,
        &test_fcntl_close_releases_locks,
    ]
}
