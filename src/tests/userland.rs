use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::test_utils::Testable;
use crate::mm::paging::{
    UserMapError, UserPerms, USER_LOAD_BASE, USER_VA_RANGE_END, USER_VA_RANGE_START,
};
use crate::tests::userland_fixtures as fix;
use crate::userland::abi::{
    self, nr, syscall_dispatch, validate_user_slice, UserVaBounds, EAGAIN, EBADF, EFAULT, EINVAL,
    ENOENT, ENOSYS, ENOTTY, EPERM, ERANGE, EROFS, LAST_EXIT_CODE,
};
use crate::userland::error::LoaderError;
use crate::userland::fdtable::{FdSlot, FdTable};
use crate::userland::loader::load_elf;
use crate::userland::path::{copy_user_cstr, normalize_path};
use alloc::vec;
use x86_64::VirtAddr;

// ---------- GDT / TSS sanity ----------

/// After `gdt::init()`, CS must read 0x08 and SS 0x10 — the two literals the
/// existing naked asm in `src/arch/x86_64/{preemption,context_switch}.rs`
/// hard-codes when constructing `iretq` frames.
fn test_gdt_kernel_selectors() {
    use x86_64::instructions::segmentation::{Segment, CS, SS};
    let cs = CS::get_reg();
    let ss = SS::get_reg();
    assert_eq!(cs.0, 0x08, "kernel CS must remain at GDT slot 1");
    assert_eq!(ss.0, 0x10, "kernel SS must remain at GDT slot 2");
}

/// The user selectors live at known offsets and carry RPL=3.
fn test_gdt_user_selectors() {
    use crate::arch::x86_64::gdt::selectors;
    use x86_64::PrivilegeLevel;
    let sel = selectors();
    assert_eq!(sel.user_data.0 & !0x3, 0x18, "user data at GDT slot 3");
    assert_eq!(sel.user_code.0 & !0x3, 0x20, "user code at GDT slot 4");
    assert_eq!(sel.user_data.rpl(), PrivilegeLevel::Ring3);
    assert_eq!(sel.user_code.rpl(), PrivilegeLevel::Ring3);
}

/// U6: SSE/SSE2 must be enabled in CR0/CR4 before any ring-3
/// transition can fire. musl + libstdc++ binaries (everything from
/// HELLOCPP through ZSH.ELF) emit SSE2 in `__init_tls` before reaching
/// `main`; if `enable_sse()` is removed from `kernel::init()` or
/// reordered after a ring-3-reachable path, the first SSE instruction
/// `#UD`s and the binary appears to hang under interactive load.
/// Catching this at test time (rather than as "zsh doesn't boot") is
/// the whole point of having the regression.
fn test_sse_enabled_before_ring3() {
    assert!(
        crate::arch::x86_64::fpu::sse_enabled(),
        "SSE/SSE2 not enabled — enable_sse() must run early in kernel::init() \
         before any path that could enter ring 3 (loader → enter_user_mode). \
         Without it, musl __init_tls #UDs on its first SSE2 instruction.",
    );
}

/// `ltr` must have run — TR is non-zero after `gdt::init()`.
fn test_tss_loaded() {
    let tr: u16;
    unsafe {
        core::arch::asm!("str {:x}", out(reg) tr, options(nomem, nostack, preserves_flags));
    }
    assert_ne!(
        tr, 0,
        "TR must be loaded with the TSS selector after gdt::init()"
    );
}

// ---------- mm: user-region mapper ----------

fn test_map_user_region_kernel_can_read() {
    let va = VirtAddr::new(USER_LOAD_BASE);
    let frames =
        crate::mm::memory::with_memory_mapper(|m| m.map_user_region(va, 1, UserPerms::ReadWrite))
            .expect("mapper")
            .expect("map");
    assert_eq!(frames.len(), 1);

    let mut sum: u64 = 0;
    unsafe {
        let p = va.as_u64() as *const u8;
        for i in 0..0x1000 {
            sum = sum.wrapping_add(*p.add(i) as u64);
        }
    }
    assert_eq!(sum, 0, "freshly mapped user page should be zero-filled");

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

fn test_map_user_region_propagates_user_bit() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x1000);
    crate::mm::memory::with_memory_mapper(|m| m.map_user_region(va, 1, UserPerms::ReadExecute))
        .unwrap()
        .unwrap();

    let ok = crate::mm::memory::with_memory_mapper(|m| m.user_bit_set_on_all_parents(va)).unwrap();
    assert!(ok, "USER bit must be set on every parent table entry");

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

fn test_unmap_user_region_returns_frames() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x2000);
    let mapped =
        crate::mm::memory::with_memory_mapper(|m| m.map_user_region(va, 2, UserPerms::ReadWrite))
            .unwrap()
            .unwrap();
    assert_eq!(mapped.len(), 2);

    let unmapped = crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 2))
        .unwrap()
        .unwrap();
    assert_eq!(unmapped.len(), 2);
    assert_eq!(unmapped[0], mapped[0]);
    assert_eq!(unmapped[1], mapped[1]);
}

fn test_map_user_region_rejects_double_map() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x4000);
    crate::mm::memory::with_memory_mapper(|m| m.map_user_region(va, 1, UserPerms::ReadWrite))
        .unwrap()
        .unwrap();

    let err =
        crate::mm::memory::with_memory_mapper(|m| m.map_user_region(va, 1, UserPerms::ReadWrite))
            .unwrap()
            .unwrap_err();
    assert_eq!(err, UserMapError::PageAlreadyMapped);

    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap();
}

fn test_map_user_region_rejects_out_of_range() {
    crate::mm::memory::with_memory_mapper(|m| {
        // Kernel heap address.
        let r = m.map_user_region(VirtAddr::new(0x_4444_4444_0000), 1, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Above the user range.
        let r = m.map_user_region(VirtAddr::new(USER_VA_RANGE_END), 1, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Misaligned start.
        let r = m.map_user_region(
            VirtAddr::new(USER_VA_RANGE_START + 1),
            1,
            UserPerms::ReadWrite,
        );
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // Zero pages.
        let r = m.map_user_region(VirtAddr::new(USER_LOAD_BASE), 0, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);

        // In-range start whose end exceeds USER_VA_RANGE_END.
        let last_page = VirtAddr::new(USER_VA_RANGE_END - 0x1000);
        let r = m.map_user_region(last_page, 2, UserPerms::ReadWrite);
        assert_eq!(r.unwrap_err(), UserMapError::VaOutOfRange);
    })
    .unwrap();
}

fn test_unmap_user_region_rejects_unmapped() {
    let va = VirtAddr::new(USER_LOAD_BASE + 0x6000);
    let err = crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(va, 1))
        .unwrap()
        .unwrap_err();
    assert_eq!(err, UserMapError::PageNotMapped);
}

// ---------- abi: numeric dispatcher + slice validation ----------

/// Out-of-range syscall numbers fall through to `-ENOSYS` without invoking
/// any handler.
fn test_dispatch_unregistered_returns_enosys() {
    let mut args = SyscallArgs::default();
    args.rax = 9999;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
}

/// Trace mode on: an unknown syscall returns `-ENOSYS` (same as off, in
/// the synthetic test path) and marks the per-nr "seen" bit so the next
/// occurrence demotes from info to trace.
fn test_unknown_syscall_trace_mode_returns_enosys_and_marks_seen() {
    use crate::userland::abi::{
        is_trace_mode, reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    let prior = is_trace_mode();
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    // Pick an unused-but-in-range nr (Linux x86-64 currently uses 0..335;
    // 411 is unused and < TRACE_NR_CAPACITY so the per-nr bookkeeping
    // applies). Using a number ≥ 512 would test the overflow path
    // instead — see test_unknown_syscall_trace_mode_capacity_overflow.
    let nr = 411;
    assert!(!unknown_syscall_was_seen(nr));
    let mut args = SyscallArgs::default();
    args.rax = nr;
    args.rdi = 0xdead_beef;
    args.rsi = 0xfeed_face;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(unknown_syscall_was_seen(nr));
    // Restore prior state so subsequent tests see the same dispatcher
    // behavior they were authored against.
    set_trace_mode(prior);
    reset_unknown_syscall_trace();
}

/// Trace mode on, same nr twice: the swap is itself the bookkeeping —
/// `unknown_syscall_was_seen` reports true after the first call and stays
/// true after subsequent calls.
fn test_unknown_syscall_trace_mode_marks_only_once() {
    use crate::userland::abi::{
        reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    let nr = 412;
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let _ = syscall_dispatch(&mut args);
    assert!(unknown_syscall_was_seen(nr));
    let _ = syscall_dispatch(&mut args);
    assert!(unknown_syscall_was_seen(nr));
    set_trace_mode(false);
    reset_unknown_syscall_trace();
}

/// Trace mode OFF: the synthetic-test dispatcher path returns `-ENOSYS`
/// (no active continuation to long-jump to) but does NOT mark the SEEN
/// bookkeeping — that's exclusive to trace mode.
fn test_unknown_syscall_trace_mode_off_does_not_mark() {
    use crate::userland::abi::{
        reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    set_trace_mode(false);
    reset_unknown_syscall_trace();
    let nr = 413;
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(!unknown_syscall_was_seen(nr));
}

/// Trace mode on, nr beyond TRACE_NR_CAPACITY (512): handler returns
/// ENOSYS without panicking and `unknown_syscall_was_seen` reports false
/// (those numbers are not tracked individually — they log every time).
fn test_unknown_syscall_trace_mode_capacity_overflow() {
    use crate::userland::abi::{
        reset_unknown_syscall_trace, set_trace_mode, unknown_syscall_was_seen,
    };
    set_trace_mode(true);
    reset_unknown_syscall_trace();
    let nr = 9999; // > 512
    let mut args = SyscallArgs::default();
    args.rax = nr;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOSYS);
    assert!(!unknown_syscall_was_seen(nr));
    set_trace_mode(false);
}

// ---------- U3: musl-init / zsh-startup syscalls ----------

/// `poll` reports only the direction that is currently usable: an empty
/// stdin is not readable, while stdout/stderr are writable.
fn test_dispatch_poll_streams_ready() {
    install_streams_for_dispatcher_test();
    // Three pollfd entries on stdin/stdout/stderr asking for read+write.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    let mut fds = [
        PollFd {
            fd: 0,
            events: 0x0001 | 0x0004,
            revents: 0,
        },
        PollFd {
            fd: 1,
            events: 0x0001 | 0x0004,
            revents: 0,
        },
        PollFd {
            fd: 2,
            events: 0x0001 | 0x0004,
            revents: 0,
        },
    ];
    let ptr = fds.as_mut_ptr() as u64;
    let bytes = (fds.len() * core::mem::size_of::<PollFd>()) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::POLL;
    args.rdi = ptr;
    args.rsi = fds.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 2);
    assert_eq!(fds[0].revents, 0);
    assert_eq!(fds[1].revents, 0x0004);
    assert_eq!(fds[2].revents, 0x0004);
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `select` uses the same descriptor readiness model as `poll`.
fn test_dispatch_select_streams_ready() {
    #[repr(C)]
    struct Timeval {
        seconds: i64,
        microseconds: i64,
    }

    install_streams_for_dispatcher_test();
    let mut read_mask = 1u64 << 0;
    let mut write_mask = (1u64 << 1) | (1u64 << 2);
    let timeout = Timeval {
        seconds: 0,
        microseconds: 0,
    };
    let read_ptr = &mut read_mask as *mut u64 as u64;
    let write_ptr = &mut write_mask as *mut u64 as u64;
    let timeout_ptr = &timeout as *const Timeval as u64;
    let start = core::cmp::min(read_ptr, core::cmp::min(write_ptr, timeout_ptr));
    let end = core::cmp::max(
        read_ptr + 8,
        core::cmp::max(
            write_ptr + 8,
            timeout_ptr + core::mem::size_of::<Timeval>() as u64,
        ),
    );
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut args = SyscallArgs::default();
    args.rax = nr::SELECT;
    args.rdi = 3;
    args.rsi = read_ptr;
    args.rdx = write_ptr;
    args.r8 = timeout_ptr;
    assert_eq!(syscall_dispatch(&mut args), 2);
    assert_eq!(read_mask, 0);
    assert_eq!(write_mask, (1u64 << 1) | (1u64 << 2));

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// Linux `select` rejects a requested descriptor that is not open.
fn test_dispatch_select_unknown_fd_returns_ebadf() {
    #[repr(C)]
    struct Timeval {
        seconds: i64,
        microseconds: i64,
    }

    install_streams_for_dispatcher_test();
    let read_mask = 1u64 << 3;
    let timeout = Timeval {
        seconds: 0,
        microseconds: 0,
    };
    let read_ptr = &read_mask as *const u64 as u64;
    let timeout_ptr = &timeout as *const Timeval as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(read_ptr, timeout_ptr),
        end: core::cmp::max(
            read_ptr + 8,
            timeout_ptr + core::mem::size_of::<Timeval>() as u64,
        ),
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::SELECT;
    args.rdi = 4;
    args.rsi = read_ptr;
    args.r8 = timeout_ptr;
    assert_eq!(syscall_dispatch(&mut args), EBADF);

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `poll` on an unknown fd reports POLLNVAL and counts toward the result.
fn test_dispatch_poll_unknown_fd_returns_pollnval() {
    install_streams_for_dispatcher_test();
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    let mut fds = [PollFd {
        fd: 999,
        events: 0x0001,
        revents: 0,
    }];
    let ptr = fds.as_mut_ptr() as u64;
    let bytes = (fds.len() * core::mem::size_of::<PollFd>()) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::POLL;
    args.rdi = ptr;
    args.rsi = fds.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 1);
    assert_eq!(fds[0].revents, 0x0020); // POLLNVAL
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// Linux ignores negative pollfd entries. musl reserves fd=-1 slots for DNS
/// TCP fallback while waiting on its UDP socket, so POLLNVAL here would turn
/// resolver waits into a busy loop.
fn test_dispatch_poll_negative_fd_is_ignored() {
    install_streams_for_dispatcher_test();
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    let mut fds = [
        PollFd {
            fd: -1,
            events: 0x0001 | 0x0004,
            revents: 0x7fff,
        },
        PollFd {
            fd: 1,
            events: 0x0004,
            revents: 0,
        },
    ];
    let ptr = fds.as_mut_ptr() as u64;
    let bytes = (fds.len() * core::mem::size_of::<PollFd>()) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::POLL;
    args.rdi = ptr;
    args.rsi = fds.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 1);
    assert_eq!(fds[0].revents, 0);
    assert_eq!(fds[1].revents, 0x0004);
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `poll` with nfds=0 returns 0 immediately without touching the buffer.
fn test_dispatch_poll_zero_nfds_returns_zero() {
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::POLL;
    args.rdi = 0; // null pointer is fine when nfds=0
    args.rsi = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
}

/// `poll` with nfds beyond the cap returns -EINVAL — defends against
/// integer-overflow attacks on `nfds * sizeof(pollfd)`.
fn test_dispatch_poll_nfds_over_cap_returns_einval() {
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::POLL;
    args.rdi = 0xdead_beef;
    args.rsi = 1024; // > POLL_MAX_NFDS = 64
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EINVAL);
}

/// `readlink("/proc/self/exe", ...)` returns the active binary's path
/// when one is set, EBADF/ENOENT otherwise.
fn test_dispatch_readlink_proc_self_exe() {
    use crate::userland::lifecycle::with_active_user;
    use alloc::string::String;
    with_active_user(|p| {
        p.exe_path = Some(String::from("/HOST/ZSH.ELF"));
    });
    let path = b"/proc/self/exe\0";
    let mut buf = [0u8; 64];
    let path_ptr = path.as_ptr() as u64;
    let path_len = path.len() as u64;
    let buf_ptr = buf.as_mut_ptr() as u64;
    let buf_len = buf.len() as u64;
    // VA bounds need to cover both the path string and the output buffer.
    let lo = core::cmp::min(path_ptr, buf_ptr);
    let hi = core::cmp::max(path_ptr + path_len, buf_ptr + buf_len);
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READLINK;
    args.rdi = path_ptr;
    args.rsi = buf_ptr;
    args.rdx = buf_len;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, b"/HOST/ZSH.ELF".len() as i64);
    assert_eq!(&buf[..ret as usize], b"/HOST/ZSH.ELF");
    crate::userland::abi::clear_user_va_bounds();
    with_active_user(|p| {
        p.exe_path = None;
    });
}

/// `readlink("/proc/self/fd/0", ...)` returns "/dev/tty" when stdin is
/// the standard stream slot.
fn test_dispatch_readlink_proc_self_fd_stdin() {
    install_streams_for_dispatcher_test();
    let path = b"/proc/self/fd/0\0";
    let mut buf = [0u8; 64];
    let path_ptr = path.as_ptr() as u64;
    let buf_ptr = buf.as_mut_ptr() as u64;
    let lo = core::cmp::min(path_ptr, buf_ptr);
    let hi = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + buf.len() as u64);
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READLINK;
    args.rdi = path_ptr;
    args.rsi = buf_ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, b"/dev/tty".len() as i64);
    assert_eq!(&buf[..ret as usize], b"/dev/tty");
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `readlink("/proc/self/fd/-1", ...)` returns -ENOENT — the bounded
/// integer parse rejects negative input.
fn test_dispatch_readlink_proc_self_fd_negative_rejected() {
    install_streams_for_dispatcher_test();
    let path = b"/proc/self/fd/-1\0";
    let mut buf = [0u8; 64];
    let path_ptr = path.as_ptr() as u64;
    let buf_ptr = buf.as_mut_ptr() as u64;
    let lo = core::cmp::min(path_ptr, buf_ptr);
    let hi = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + buf.len() as u64);
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READLINK;
    args.rdi = path_ptr;
    args.rsi = buf_ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOENT);
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `readlink("/proc/self/fd/99999999999999999999", ...)` returns -ENOENT
/// — the bounded u32 parse rejects overflow.
fn test_dispatch_readlink_proc_self_fd_overflow_rejected() {
    install_streams_for_dispatcher_test();
    let path = b"/proc/self/fd/99999999999999999999\0";
    let mut buf = [0u8; 64];
    let path_ptr = path.as_ptr() as u64;
    let buf_ptr = buf.as_mut_ptr() as u64;
    let lo = core::cmp::min(path_ptr, buf_ptr);
    let hi = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + buf.len() as u64);
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READLINK;
    args.rdi = path_ptr;
    args.rsi = buf_ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOENT);
    crate::userland::abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `readlink("/etc/nonexistent", ...)` returns -ENOENT — non-procfs
/// paths are not symlinks here.
fn test_dispatch_readlink_other_returns_enoent() {
    let path = b"/etc/nonexistent\0";
    let mut buf = [0u8; 64];
    let path_ptr = path.as_ptr() as u64;
    let buf_ptr = buf.as_mut_ptr() as u64;
    let lo = core::cmp::min(path_ptr, buf_ptr);
    let hi = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + buf.len() as u64);
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: lo,
        end: hi,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::READLINK;
    args.rdi = path_ptr;
    args.rsi = buf_ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, ENOENT);
    crate::userland::abi::clear_user_va_bounds();
}

/// `getrlimit(RLIMIT_NOFILE, &out)` writes RLIM_INFINITY into both
/// rlim_cur and rlim_max and returns 0.
fn test_dispatch_getrlimit_returns_infinity() {
    let mut out = [0u64; 2];
    let ptr = out.as_mut_ptr() as u64;
    let bytes = (out.len() * 8) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::GETRLIMIT;
    args.rdi = 7; // RLIMIT_NOFILE
    args.rsi = ptr;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
    assert_eq!(out[0], u64::MAX);
    assert_eq!(out[1], u64::MAX);
    crate::userland::abi::clear_user_va_bounds();
}

/// `prlimit64(0, RLIMIT_STACK, NULL, &old)` writes RLIM_INFINITY into
/// the old buffer; with NULL old it just returns 0.
fn test_dispatch_prlimit64_old_value_writes_infinity() {
    let mut out = [0u64; 2];
    let ptr = out.as_mut_ptr() as u64;
    let bytes = (out.len() * 8) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::PRLIMIT64;
    args.rdi = 0;
    args.rsi = 3; // RLIMIT_STACK
    args.rdx = 0; // new_limit NULL
    args.r10 = ptr; // old_limit
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
    assert_eq!(out[0], u64::MAX);
    assert_eq!(out[1], u64::MAX);
    crate::userland::abi::clear_user_va_bounds();
}

/// `prlimit64(..., NULL, NULL)` returns 0 without touching memory.
fn test_dispatch_prlimit64_null_old_returns_zero() {
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::PRLIMIT64;
    args.rdi = 0;
    args.rsi = 3;
    args.rdx = 0;
    args.r10 = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
}

/// `getrusage(RUSAGE_SELF, &usage)` zero-fills the 144-byte rusage struct
/// and returns 0; `getrusage(99, ...)` rejects an unknown `who` with EINVAL.
fn test_dispatch_getrusage_self_zero_fills_and_rejects_unknown_who() {
    let mut out = [0xffu8; 144];
    let ptr = out.as_mut_ptr() as u64;
    let bytes = out.len() as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: ptr,
        end: ptr + bytes,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::GETRUSAGE;
    args.rdi = 0; // RUSAGE_SELF
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert!(out.iter().all(|&b| b == 0));

    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::GETRUSAGE;
    args.rdi = 99; // not a valid `who`
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), crate::userland::abi::EINVAL);

    crate::userland::abi::clear_user_va_bounds();
}

/// ITIMER_REAL validates its input, rounds sub-tick values upward, returns the
/// prior timer, and disarms when passed a null new-value pointer.
fn test_dispatch_setitimer_real_arms_queries_and_validates() {
    let mut storage = [[0i64; 4]; 2];
    storage[0] = [0, 250_000, 0, 1];
    let start = storage.as_mut_ptr() as u64;
    let end = start + core::mem::size_of_val(&storage) as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds { start, end });
    crate::userland::lifecycle::with_current_process(|process| {
        process.real_timer = crate::userland::lifecycle::RealTimerState::disarmed();
    });

    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::SETITIMER;
    args.rdi = 0; // ITIMER_REAL
    args.rsi = storage[0].as_ptr() as u64;
    args.rdx = storage[1].as_mut_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(storage[1], [0; 4]);
    crate::userland::lifecycle::with_current_process(|process| {
        assert_eq!(process.real_timer.interval_ticks, 25);
        let deadline = process
            .real_timer
            .deadline_tick
            .expect("one-microsecond timer should arm");
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        assert!(deadline >= now && deadline <= now.saturating_add(1));
    });

    // Linux accepts a null new-value pointer as a disarm operation. The old
    // interval and remaining one-tick value are still returned.
    storage[1] = [0; 4];
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(&storage[1][..2], &[0, 250_000]);
    assert!(storage[1][2] == 0 && (storage[1][3] == 0 || storage[1][3] == 10_000));
    crate::userland::lifecycle::with_current_process(|process| {
        assert!(process.real_timer.deadline_tick.is_none());
    });

    storage[0] = [0, 0, -1, 0];
    args.rsi = storage[0].as_ptr() as u64;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), crate::userland::abi::EINVAL);

    args.rdi = 1; // ITIMER_VIRTUAL is outside the delivered scope.
    assert_eq!(syscall_dispatch(&mut args), crate::userland::abi::EINVAL);

    crate::userland::lifecycle::with_current_process(|process| {
        process.real_timer = crate::userland::lifecycle::RealTimerState::disarmed();
    });
    crate::userland::abi::clear_user_va_bounds();
}

/// `nanosleep` request validation and the non-blocking return paths.
/// The genuinely-blocking path diverges via
/// `block_current_ring3_and_yield`, so it can't run in this synchronous
/// dispatcher harness — validation, the synthetic-dispatch
/// short-circuit, and the elapsed-deadline restart machinery are
/// exercised instead.
fn test_dispatch_nanosleep_validation_and_synthetic_return() {
    // NULL req → EFAULT.
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::NANOSLEEP;
    args.rdi = 0;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), crate::userland::abi::EFAULT);

    // Invalid tv_nsec → EINVAL.
    let bad: [i64; 2] = [0, 1_000_000_000];
    let bad_ptr = bad.as_ptr() as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: bad_ptr,
        end: bad_ptr + 16,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::NANOSLEEP;
    args.rdi = bad_ptr;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), crate::userland::abi::EINVAL);
    crate::userland::abi::clear_user_va_bounds();

    // Valid 50 ms request from synthetic context → immediate 0 (no
    // scheduler context to yield from).
    let req: [i64; 2] = [0, 50_000_000];
    let req_ptr = req.as_ptr() as u64;
    crate::userland::abi::set_user_va_bounds(crate::userland::abi::UserVaBounds {
        start: req_ptr,
        end: req_ptr + 16,
    });
    let mut args = SyscallArgs::default();
    args.rax = crate::userland::abi::nr::NANOSLEEP;
    args.rdi = req_ptr;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);
    crate::userland::abi::clear_user_va_bounds();

    // Restart machinery: a re-fired sleep whose absolute deadline has
    // already elapsed observes `now >= sleep_deadline`, reports done,
    // and clears the per-process deadline.
    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    crate::userland::lifecycle::with_current_process(|process| {
        process.sleep_deadline = Some(now);
    });
    assert_eq!(crate::userland::lifecycle::nanosleep_deadline(5), None);
    crate::userland::lifecycle::with_current_process(|process| {
        assert!(process.sleep_deadline.is_none());
    });
}

/// `write(1, valid_ptr, len)` succeeds and returns `len`. The active
/// user-VA bounds bracket the kernel buffer for the duration of the call —
/// the dispatcher does not care where the bytes come from, only that the
/// slice lies within the declared bounds.
/// Pin stdin/stdout/stderr in the FD table so the dispatcher doesn't
/// reject `write(1, …)` with `-EBADF`. Phase 2 routed `write` through
/// the FD table; the older write-handler tests need the streams pinned
/// to keep their original meaning.
fn install_streams_for_dispatcher_test() {
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.fd_table.install_default_streams();
    });
}

fn clear_streams_after_dispatcher_test() {
    crate::userland::lifecycle::with_active_user(|au| au.fd_table.clear());
}

fn test_write_handler_valid_slice() {
    install_streams_for_dispatcher_test();
    let buf: [u8; 5] = [b'h', b'e', b'l', b'l', b'o'];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1; // stdout
    args.rsi = ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, buf.len() as i64);

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `write(99, ptr, len)` to an unsupported fd returns `-EBADF` without
/// touching the buffer.
fn test_write_handler_rejects_unknown_fd() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 99;
    args.rsi = 0xdead_beef;
    args.rdx = 16;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EBADF);
}

/// `write(1, kernel_ptr, 5)` is rejected by the slice validator without
/// dereferencing.
fn test_write_handler_rejects_kernel_pointer() {
    install_streams_for_dispatcher_test();
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xffff_8000_0000_0000;
    args.rdx = 5;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    clear_streams_after_dispatcher_test();
}

/// `write(1, ptr+4, 100)` with an 8-byte bounds window is rejected as the
/// span exceeds the upper bound.
fn test_write_handler_rejects_span_past_bounds() {
    install_streams_for_dispatcher_test();
    let buf: [u8; 8] = [0; 8];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 8,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = ptr + 4;
    args.rdx = 100;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// **Wraparound**: ptr + len overflowing u64 must be rejected even when
/// bounds are wide. checked_add is the defense.
fn test_write_handler_rejects_pointer_wraparound() {
    install_streams_for_dispatcher_test();
    abi::set_user_va_bounds(UserVaBounds {
        start: 0,
        end: u64::MAX,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xFFFF_FFFF_FFFF_FF00;
    args.rdx = 0x200;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, EFAULT);
    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `write(1, _, 0)` is a no-op, succeeds, and returns 0 even with no
/// active user-VA bounds.
fn test_write_handler_zero_len_succeeds() {
    install_streams_for_dispatcher_test();
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1;
    args.rsi = 0xdead_beef;
    args.rdx = 0;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 0);
    clear_streams_after_dispatcher_test();
}

/// A single `write()` far above the 4 KiB staging bound lands fully in a
/// tmpfs-backed file via the kernel-side chunk loop, byte-exact. musl's
/// `fwrite` issues large direct writes, so this is the shape a compiler
/// writing an object file or executable produces.
fn test_write_file_large_chunked() {
    setup_phase2_active_user();
    let path = b"/bigwrite.tmp\0";
    let path_ptr = path.as_ptr() as u64;

    let mut payload = alloc::vec![0u8; 64 * 1024];
    for (i, byte) in payload.iter_mut().enumerate() {
        // Period coprime with 4096 so chunk reordering or duplication
        // cannot reproduce the pattern.
        *byte = (i % 251) as u8;
    }
    let buf_ptr = payload.as_ptr() as u64;
    let start = core::cmp::min(path_ptr, buf_ptr);
    let end = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + payload.len() as u64);
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut open = SyscallArgs::default();
    open.rax = nr::OPEN;
    open.rdi = path_ptr;
    open.rsi = 0x241; // O_WRONLY|O_CREAT|O_TRUNC
    let fd = syscall_dispatch(&mut open);
    assert!(fd >= 0, "open for write failed: {}", fd);

    let mut write = SyscallArgs::default();
    write.rax = nr::WRITE;
    write.rdi = fd as u64;
    write.rsi = buf_ptr;
    write.rdx = payload.len() as u64;
    assert_eq!(syscall_dispatch(&mut write), payload.len() as i64);

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);

    let readback = crate::fs::File::open_read("/bigwrite.tmp")
        .expect("reopen written file")
        .read_to_vec()
        .expect("read back written file");
    assert_eq!(readback.len(), payload.len());
    assert!(readback == payload, "read-back bytes differ from payload");

    let mut unlink = SyscallArgs::default();
    unlink.rax = nr::UNLINK;
    unlink.rdi = path_ptr;
    assert_eq!(syscall_dispatch(&mut unlink), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `writev()` with iovs above the staging bound chunks each file iov and
/// returns the full total.
fn test_writev_file_large_iovs() {
    setup_phase2_active_user();
    let path = b"/bigwritev.tmp\0";
    let path_ptr = path.as_ptr() as u64;

    let mut payload = alloc::vec![0u8; 9000];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = (i % 239) as u8;
    }
    let buf_ptr = payload.as_ptr() as u64;
    // iov[0] = first 6000 bytes, iov[1] = remaining 3000.
    let iovecs: [u64; 4] = [buf_ptr, 6000, buf_ptr + 6000, 3000];
    let iov_ptr = iovecs.as_ptr() as u64;
    let start = [path_ptr, buf_ptr, iov_ptr].into_iter().min().unwrap();
    let end = [
        path_ptr + path.len() as u64,
        buf_ptr + payload.len() as u64,
        iov_ptr + 32,
    ]
    .into_iter()
    .max()
    .unwrap();
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut open = SyscallArgs::default();
    open.rax = nr::OPEN;
    open.rdi = path_ptr;
    open.rsi = 0x241; // O_WRONLY|O_CREAT|O_TRUNC
    let fd = syscall_dispatch(&mut open);
    assert!(fd >= 0, "open for writev failed: {}", fd);

    let mut writev = SyscallArgs::default();
    writev.rax = nr::WRITEV;
    writev.rdi = fd as u64;
    writev.rsi = iov_ptr;
    writev.rdx = 2;
    assert_eq!(syscall_dispatch(&mut writev), payload.len() as i64);

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);

    let readback = crate::fs::File::open_read("/bigwritev.tmp")
        .expect("reopen writev file")
        .read_to_vec()
        .expect("read back writev file");
    assert!(readback == payload, "writev read-back differs");

    let mut unlink = SyscallArgs::default();
    unlink.rax = nr::UNLINK;
    unlink.rdi = path_ptr;
    assert_eq!(syscall_dispatch(&mut unlink), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `pwrite64()` above the staging bound writes fully at the offset and
/// leaves the descriptor position untouched; `pread64()` above the bound
/// returns a POSIX short read of the staging size instead of -EFAULT.
fn test_pwrite_large_and_pread_short() {
    setup_phase2_active_user();
    let path = b"/bigpwrite.tmp\0";
    let path_ptr = path.as_ptr() as u64;

    let mut payload = alloc::vec![0u8; 12 * 1024];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = (i % 233) as u8;
    }
    let buf_ptr = payload.as_ptr() as u64;
    let mut readbuf = alloc::vec![0u8; 8192];
    let read_ptr = readbuf.as_mut_ptr() as u64;
    let start = [path_ptr, buf_ptr, read_ptr].into_iter().min().unwrap();
    let end = [
        path_ptr + path.len() as u64,
        buf_ptr + payload.len() as u64,
        read_ptr + readbuf.len() as u64,
    ]
    .into_iter()
    .max()
    .unwrap();
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut open = SyscallArgs::default();
    open.rax = nr::OPEN;
    open.rdi = path_ptr;
    open.rsi = 0x42; // O_RDWR|O_CREAT
    let fd = syscall_dispatch(&mut open);
    assert!(fd >= 0, "open for pwrite failed: {}", fd);

    let mut pwrite = SyscallArgs::default();
    pwrite.rax = nr::PWRITE64;
    pwrite.rdi = fd as u64;
    pwrite.rsi = buf_ptr;
    pwrite.rdx = payload.len() as u64;
    pwrite.r10 = 0;
    assert_eq!(syscall_dispatch(&mut pwrite), payload.len() as i64);

    // Position must still be 0: a sequential read sees the file start.
    let mut read = SyscallArgs::default();
    read.rax = nr::READ;
    read.rdi = fd as u64;
    read.rsi = read_ptr;
    read.rdx = 16;
    assert_eq!(syscall_dispatch(&mut read), 16);
    assert_eq!(&readbuf[..16], &payload[..16]);

    // pread64 with an 8 KiB request: short read of the 4 KiB staging
    // bound (previously -EFAULT), from the requested offset.
    let mut pread = SyscallArgs::default();
    pread.rax = nr::PREAD64;
    pread.rdi = fd as u64;
    pread.rsi = read_ptr;
    pread.rdx = readbuf.len() as u64;
    pread.r10 = 1000;
    assert_eq!(syscall_dispatch(&mut pread), 4096);
    assert_eq!(&readbuf[..4096], &payload[1000..1000 + 4096]);

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);

    let mut unlink = SyscallArgs::default();
    unlink.rax = nr::UNLINK;
    unlink.rdi = path_ptr;
    assert_eq!(syscall_dispatch(&mut unlink), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `chmod`/`fchmod` are validated success no-ops: FAT and tmpfs carry no
/// permission bits and execve performs no +x check. TinyCC chmods its
/// output executable after writing it.
fn test_dispatch_chmod_fchmod_noops() {
    setup_phase2_active_user();

    // chmod on an existing managed file succeeds.
    let path = b"/etc/zshrc\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::CHMOD;
    args.rdi = ptr;
    args.rsi = 0o755;
    assert_eq!(syscall_dispatch(&mut args), 0);

    // chmod on a missing path reports ENOENT.
    let missing = b"/no-such-file-for-chmod\0";
    let missing_ptr = missing.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: missing_ptr,
        end: missing_ptr + missing.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::CHMOD;
    args.rdi = missing_ptr;
    args.rsi = 0o755;
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    // chmod on the synthetic /bin namespace is rejected.
    let bin = b"/bin/ls\0";
    let bin_ptr = bin.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: bin_ptr,
        end: bin_ptr + bin.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::CHMOD;
    args.rdi = bin_ptr;
    args.rsi = 0o755;
    assert_eq!(syscall_dispatch(&mut args), EPERM);

    // fchmod: any valid descriptor succeeds, unknown fd is EBADF.
    let mut args = SyscallArgs::default();
    args.rax = nr::FCHMOD;
    args.rdi = 1; // stdout
    args.rsi = 0o644;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCHMOD;
    args.rdi = 99;
    args.rsi = 0o644;
    assert_eq!(syscall_dispatch(&mut args), EBADF);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// Unit coverage for the UTF-8 seam guard used by chunked terminal
/// writes: a chunk must never end partway through a multi-byte sequence.
fn test_utf8_safe_chunk_len_boundaries() {
    use crate::userland::syscalls::utf8_safe_chunk_len;

    // ASCII tail: full length.
    assert_eq!(utf8_safe_chunk_len(b"abcdef"), 6);
    // Complete 2-byte sequence at the end ("é" = C3 A9): full length.
    assert_eq!(utf8_safe_chunk_len(&[b'a', 0xC3, 0xA9]), 3);
    // Dangling 2-byte lead at the end: trim it.
    assert_eq!(utf8_safe_chunk_len(&[b'a', b'b', 0xC3]), 2);
    // First two bytes of a 3-byte sequence ("€" = E2 82 AC): trim both.
    assert_eq!(utf8_safe_chunk_len(&[b'a', 0xE2, 0x82]), 1);
    // Complete 3-byte sequence: full length.
    assert_eq!(utf8_safe_chunk_len(&[0xE2, 0x82, 0xAC]), 3);
    // First three bytes of a 4-byte sequence (F0 9F 92 96): trim all three.
    assert_eq!(utf8_safe_chunk_len(&[b'x', 0xF0, 0x9F, 0x92]), 1);
    // Complete 4-byte sequence: full length.
    assert_eq!(utf8_safe_chunk_len(&[0xF0, 0x9F, 0x92, 0x96]), 4);
    // Not valid UTF-8 anyway (all continuation bytes): progress guarantee.
    assert_eq!(utf8_safe_chunk_len(&[0x80, 0x80, 0x80, 0x80]), 4);
}

/// `exit_group(42)` records 42 in `LAST_EXIT_CODE` and returns through the
/// synthetic kernel-test fallback when no real ring-3 process is current.
fn test_exit_group_handler_records_code() {
    *LAST_EXIT_CODE.lock() = None;
    let prior_pid = crate::userland::lifecycle::current_user_pid();
    crate::userland::lifecycle::set_current_user_pid(Some(crate::userland::lifecycle::KERNEL_PID));
    let mut args = SyscallArgs::default();
    args.rax = nr::EXIT_GROUP;
    args.rdi = 42;
    let _ = syscall_dispatch(&mut args);
    crate::userland::lifecycle::set_current_user_pid(prior_pid);
    assert_eq!(*LAST_EXIT_CODE.lock(), Some(42));
}

/// `validate_user_slice(_, 0)` is OK regardless of bounds.
fn test_validate_user_slice_zero_len_ok() {
    abi::clear_user_va_bounds();
    assert!(validate_user_slice(0xdead_beef, 0).is_ok());
}

// ---------- loader ----------

fn test_loader_happy_path() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("load_elf happy");

    assert_eq!(image.entry.as_u64(), 0x40_0000);
    assert_eq!(image.stack_top.as_u64(), crate::mm::paging::USER_STACK_TOP);

    // U2: stack is no longer recorded in image.mappings — Process
    // owns stack teardown. The only mapping for this fixture is the
    // single PT_LOAD page (no TLS).
    assert_eq!(image.mapping_count(), 1);
    assert_eq!(image.total_pages(), 1);

    // U2: stack window is exposed on the image for U3 to install on Process.
    assert_eq!(
        image.stack_initial_bottom,
        crate::mm::paging::USER_STACK_TOP - crate::mm::paging::USER_STACK_INITIAL_PAGES * 0x1000
    );
    // For a one-page PT_LOAD at USER_LOAD_BASE, the global cap binds:
    // USER_STACK_TOP - 768*0x1000 = 0x50_0000 (3 MiB of stack room),
    // which is above the per-binary floor (0x40_1000 + 16*0x1000).
    assert_eq!(
        image.stack_max_growth_floor,
        crate::mm::paging::USER_STACK_TOP - crate::mm::paging::USER_STACK_MAX_GROWTH_PAGES * 0x1000
    );

    unsafe {
        let p = 0x40_0000u64 as *const u8;
        for i in 16..0x100 {
            assert_eq!(*p.add(i), 0, "bss tail not zeroed at +{}", i);
        }
        for i in 0..16u8 {
            assert_eq!(*p.add(i as usize), i);
        }
    }
    // Stack cleanup is handled by `UserImage::Drop` when `image` goes
    // out of scope — the loader recorded `stack_initial_bottom` and Drop
    // unmaps `[initial_bottom, USER_STACK_TOP)`.
}

fn test_loader_bad_magic() {
    let bytes: alloc::vec::Vec<u8> = alloc::vec::Vec::from(&b"XXXX"[..]);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::BadMagic);
}

fn test_loader_wrong_arch() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u16(&mut bytes, 18, fix::EM_AARCH64);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongArch);
}

fn test_loader_wrong_class() {
    let mut bytes = fix::happy_path_elf();
    bytes[4] = 1;
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongArch);
}

fn test_loader_wrong_type() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u16(&mut bytes, 16, fix::ET_REL);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::WrongType);
}

fn test_loader_truncated_phdrs() {
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![fix::PhdrSpec {
            p_type: fix::PT_LOAD,
            p_flags: fix::PF_R | fix::PF_X,
            p_offset: 0x1000,
            p_vaddr: 0x40_0000,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        }],
        payloads: vec![(0x1000u64, vec![1u8, 2, 3, 4])],
        truncate_to: None,
    }
    .build();
    let mut bytes = bytes;
    fix::write_u16(&mut bytes, 56, 4);
    bytes.truncate((fix::EHDR_SIZE + fix::PHDR_SIZE) as usize);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::Truncated);
}

fn test_loader_va_out_of_range() {
    let p_vaddr = 0x_4444_4444_0000u64;
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: vec![fix::PhdrSpec {
            p_type: fix::PT_LOAD,
            p_flags: fix::PF_R | fix::PF_X,
            p_offset,
            p_vaddr,
            p_filesz: 4,
            p_memsz: 4,
            p_align: 0x1000,
        }],
        payloads: vec![(p_offset, vec![1u8, 2, 3, 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::VaOutOfRange);
}

fn test_loader_overlapping_pt_load() {
    let p_offset_a = 0x1000u64;
    let p_offset_b = 0x2000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset: p_offset_a,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 0x100,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_W,
                p_offset: p_offset_b,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 0x100,
                p_align: 0x1000,
            },
        ],
        payloads: vec![(p_offset_a, vec![1u8; 4]), (p_offset_b, vec![2u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::OverlappingPtLoad
    );
}

fn test_loader_entry_not_mapped() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 24, 0x40_5000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::EntryNotMapped);
}

fn test_loader_alignment_bad() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 64 + 48, 0x2000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::AlignmentBad);
}

/// PT_TLS is now supported. The image loads, the TCB self-pointer is
/// initialized to USER_TCB_VA, and the FS_BASE accessor on UserImage
/// reflects the TCB address.
fn test_loader_pt_tls_loads() {
    use crate::mm::paging::{USER_TCB_VA, USER_TLS_IMAGE_VA};
    let bytes = fix::tls_smoke_elf();
    let image = load_elf(&bytes).expect("load_elf with PT_TLS");

    assert_eq!(image.tls_fs_base, Some(VirtAddr::new(USER_TCB_VA)));

    // tdata bytes (the four 0x55 bytes the fixture put at p_offset) landed
    // at the TLS image VA.
    unsafe {
        let p = USER_TLS_IMAGE_VA as *const u8;
        for i in 0..4 {
            assert_eq!(*p.add(i), 0x55, "tdata[{}] not copied", i);
        }
        // tbss is zero-filled by the fresh mapping.
        for i in 4..0x100 {
            assert_eq!(*p.add(i), 0, "tbss[{}] not zero", i);
        }
        // TCB self-pointer at offset 0.
        let tcb = USER_TCB_VA as *const u64;
        assert_eq!(core::ptr::read_unaligned(tcb), USER_TCB_VA);
        // dtv slot at offset 8 is zero.
        assert_eq!(core::ptr::read_unaligned(tcb.add(1)), 0);
    }

    drop(image);
}

/// Oversized PT_TLS (>4 KiB image) is rejected with `TlsUnsupported` so
/// the milestone's single-page TLS limit is honored.
fn test_loader_pt_tls_oversized_rejected() {
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_TLS,
                p_flags: fix::PF_R,
                p_offset,
                p_vaddr: 0,
                p_filesz: 4,
                // 5 KiB image — over the milestone cap.
                p_memsz: 0x1400,
                p_align: 0x10,
            },
        ],
        payloads: vec![(p_offset, vec![1u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::TlsUnsupported);
}

fn test_loader_pt_interp_rejected() {
    let p_offset = 0x1000u64;
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: 0x40_0000,
        phdrs: vec![
            fix::PhdrSpec {
                p_type: fix::PT_LOAD,
                p_flags: fix::PF_R | fix::PF_X,
                p_offset,
                p_vaddr: 0x40_0000,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 0x1000,
            },
            fix::PhdrSpec {
                p_type: fix::PT_INTERP,
                p_flags: fix::PF_R,
                p_offset,
                p_vaddr: 0,
                p_filesz: 4,
                p_memsz: 4,
                p_align: 1,
            },
        ],
        payloads: vec![(p_offset, vec![1u8; 4])],
        truncate_to: None,
    }
    .build();
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::InterpUnsupported
    );
}

fn test_loader_segment_overflow() {
    let mut bytes = fix::happy_path_elf();
    fix::write_u64(&mut bytes, 64 + 8, u64::MAX - 4);
    fix::write_u64(&mut bytes, 64 + 32, 100);
    let err = load_elf(&bytes).unwrap_err();
    assert!(
        matches!(
            err,
            LoaderError::SegmentOverflow | LoaderError::AlignmentBad
        ),
        "got {:?}",
        err
    );
}

fn test_loader_unsupported_reloc() {
    let bytes = fix::elf_with_one_reloc("anything", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert_eq!(
        load_elf(&bytes).unwrap_err(),
        LoaderError::UnsupportedReloc(fix::R_X86_64_TPOFF64)
    );
}

/// After the SYSCALL transition, any GLOB_DAT / JUMP_SLOT against an
/// undefined extern is rejected as `UnresolvedImport` — the kernel-side
/// name registry is gone, so the loader has no resolver to consult.
fn test_loader_glob_dat_unresolved() {
    let bytes = fix::elf_with_one_reloc("anything", fix::R_X86_64_GLOB_DAT, 0x40_1000);
    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::UnresolvedImport);
}

/// Static-no-pie ET_EXEC binaries from musl-cross-make typically emit no
/// relocations at all — the walker must accept that as a no-op.
fn test_loader_no_relocations_is_ok() {
    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("no-relocations load");
    drop(image);
}

/// On a relocation-phase failure, the partial `UserImage` is dropped and
/// every recorded mapping is unmapped. Verify by re-mapping the same VAs
/// after an `UnsupportedReloc` failure.
fn test_loader_rollback_unmaps_on_reloc_failure() {
    let bytes = fix::elf_with_one_reloc("anything", fix::R_X86_64_TPOFF64, 0x40_1000);
    assert!(load_elf(&bytes).is_err());

    let r1 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(0x40_0000), 1, UserPerms::ReadExecute)
    })
    .unwrap();
    assert!(
        r1.is_ok(),
        "PT_LOAD #1 was not unmapped on rollback: {:?}",
        r1.err()
    );

    let r2 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(0x40_1000), 1, UserPerms::ReadWrite)
    })
    .unwrap();
    assert!(
        r2.is_ok(),
        "PT_LOAD #2 was not unmapped on rollback: {:?}",
        r2.err()
    );

    let stack_bottom = crate::mm::paging::USER_STACK_TOP - 8 * 0x1000;
    let r3 = crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(stack_bottom), 8, UserPerms::ReadWrite)
    })
    .unwrap();
    assert!(
        r3.is_ok(),
        "user stack was not unmapped on rollback: {:?}",
        r3.err()
    );

    crate::mm::memory::with_memory_mapper(|m| {
        m.unmap_user_region(VirtAddr::new(0x40_0000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(0x40_1000), 1).unwrap();
        m.unmap_user_region(VirtAddr::new(stack_bottom), 8).unwrap();
    });
}

// ---------- enter_user_mode + lifecycle ----------

fn reset_active_user() {
    let _img = crate::userland::release_active_image();
    drop(_img);
    crate::userland::force_clear_active_for_test();
    // Scheduler unit tests use synthetic PIDs. A booted fixture must never
    // inherit one of those ready entries or the kernel main loop will attempt
    // to resume a nonexistent process before the fixture gets CPU time.
    while crate::userland::lifecycle::pop_next_ring3().is_some() {}
}

/// Mutations to the PID-0 compatibility sentinel are not a live ring-3
/// process and must not block a real launch. The scheduler-era lifecycle
/// tracks runnable processes by their nonzero PIDs rather than enforcing the
/// old singleton-image invariant.
fn test_enter_user_mode_ignores_kernel_sentinel_state() {
    reset_active_user();

    let dummy = crate::userland::image::UserImage::new(
        x86_64::VirtAddr::new(0x40_0000),
        x86_64::VirtAddr::new(0x80_0000),
        0x40_0000,
        0x80_0000,
    );
    crate::userland::lifecycle::with_active_user(|au| {
        au.image = Some(dummy);
    });

    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    assert!(matches!(
        result.0,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(result.1, 0);

    reset_active_user();
}

/// Fixture B — Linux initial-stack contract.
///
/// The binary walks the kernel-built argc/argv/envp/auxv frame and exits
/// with code 0 if every check passes, or 1..6 indicating which assertion
/// failed (argc / argv[0] / argv[1] NULL / envp[0] NULL / AT_RANDOM
/// missing / AT_RANDOM ptr null).
fn test_run_initial_stack_fixture_b() {
    reset_active_user();
    let bytes = fix::auxv_walker_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 0,
        "auxv walker exited with code {} — see fixture comments for the meaning",
        result.1
    );
}

/// Fixture D — live unknown-syscall fallback. Binary issues syscall 999,
/// verifies that ring 3 receives `-ENOSYS`, then exits normally. Compatibility
/// probes must not require trace mode to survive.
fn test_run_unknown_syscall_returns_enosys() {
    reset_active_user();
    let bytes = fix::unknown_syscall_enosys_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 0,
        "fixture did not observe -ENOSYS from syscall 999"
    );
}

/// Fixture A — SYSCALL fast-path smoke test. The smallest possible end-to-end
/// proof of the SYSCALL transition: a `syscall` with `RAX=NR_EXIT_GROUP,
/// RDI=42` records exit code 42 via cooperative_exit.
fn test_run_syscall_exit42_fixture_a() {
    reset_active_user();
    let bytes = fix::syscall_exit42_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 42);
}

fn test_run_happy_path_hello() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 0);
}

fn test_run_fault_ud() {
    reset_active_user();
    let bytes = fix::fault_ud_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 6),
        other => panic!("expected Abnormal(#UD), got {:?}", other),
    }
}

/// A ring-3 fault must return control to the scheduler, release the failed
/// image, and leave the kernel able to launch a fresh process. The deleted
/// setjmp continuation path used to require SS=0x10 here; scheduler-era
/// long-mode kernel execution also permits a null SS, so successful recovery
/// and relaunch is the relevant contract.
fn test_kernel_and_userland_resume_after_user_fault() {
    reset_active_user();
    let bytes = fix::fault_ud_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let fault = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();
    assert!(matches!(
        fault.0,
        crate::userland::lifecycle::ExitKind::Abnormal { vector: 6, .. }
    ));

    let image = load_elf(&fix::hello_exit0_elf()).expect("load follow-up ELF");
    let follow_up = crate::userland::enter_user_mode(image).expect("launch after fault");
    let _ = crate::userland::release_active_image();
    assert!(matches!(
        follow_up.0,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(follow_up.1, 0);
}

fn test_run_fault_pf() {
    reset_active_user();
    let bytes = fix::fault_pf_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 14),
        other => panic!("expected Abnormal(#PF), got {:?}", other),
    }
}

// ---------- U8: ZSH.ELF end-to-end ----------
//
// These tests exercise the committed static zsh through the same launcher and
// environment as a desktop terminal. Keep `-f` on parser/syscall smoke tests;
// the startup-config test intentionally enables interactive rc sourcing.

/// Drive `RunProcess::run` against `/host/ZSH.ELF` with the given argv.
/// Trace mode is enabled for detailed discovery logging; unsupported syscalls
/// return ENOSYS in normal operation too. Returns `Some(exit)` if the binary
/// was staged and the run completed; `None` (skip) if not staged.
#[cfg(feature = "test")]
fn drive_zsh(argv_after_path: &[&str]) -> Option<(crate::userland::lifecycle::ExitKind, i64)> {
    use crate::userland::lifecycle::with_active_user;
    use alloc::string::String;
    let path = crate::userland::process_service::ZSH_HOST_PATH;
    if !crate::fs::exists(path) {
        crate::debug_info!("[u8] {} not staged; skipping", path);
        return None;
    }
    // Enable trace mode for detailed once-per-number argument logs. It does
    // not alter the ENOSYS behavior observed by the binary.
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();

    let mut argv_owned: alloc::vec::Vec<String> = alloc::vec::Vec::new();
    argv_owned.push(String::from(path));
    for a in argv_after_path {
        argv_owned.push(String::from(*a));
    }
    let argv_borrows: alloc::vec::Vec<&str> = argv_owned.iter().map(|s| s.as_str()).collect();
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &argv_borrows,
        &crate::userland::process_service::DEFAULT_USER_ENV,
    )
    .expect("launch staged zsh");

    crate::userland::abi::set_trace_mode(prior_trace);
    let still_active = with_active_user(|au| au.image.is_some());
    assert!(
        !still_active,
        "active-user slot should be empty after zsh run() returns"
    );
    Some(result)
}

/// `zsh -f +m -c 'exit 0'` with the exact desktop-terminal environment —
/// proves the normal terminal launch profile loads, musl init runs, the
/// parser handles -c, and cooperative exit returns cleanly.
fn test_run_zsh_minimal_exit() {
    let Some((kind, code)) = drive_zsh(&["-f", "+m", "-c", "exit 0"]) else {
        return;
    };
    assert!(matches!(
        kind,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(code, 0);
}

/// `zsh -f +m -c 'echo hi'` — exercises the print/write path and
/// cooperative exit.
fn test_run_zsh_echo_command() {
    let Some((kind, code)) = drive_zsh(&["-f", "+m", "-c", "echo hi"]) else {
        return;
    };
    assert!(matches!(
        kind,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(code, 0);
}

/// `zsh -f +m -c 'pwd'` — proves getcwd, the pwd builtin, and the
/// envp-driven path resolution work.
fn test_run_zsh_pwd() {
    let Some((kind, code)) = drive_zsh(&["-f", "+m", "-c", "pwd"]) else {
        return;
    };
    assert!(matches!(
        kind,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(code, 0);
}

/// `zsh -c 'ls; :'` forks and execs BusyBox, then demand-pages its executable
/// while the parent waits. The trailing builtin prevents zsh's final-command
/// in-place-exec optimization, pinning the interactive parent wait path.
fn test_run_zsh_external_ls() {
    let Some((kind, code)) = drive_zsh(&["-f", "+m", "-c", "ls; :"]) else {
        return;
    };
    assert!(matches!(
        kind,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(code, 0);
}

/// Interactive zsh must source the staged global config and build an Agnoster
/// prompt in-process, without leaving upstream's per-redraw `$()` fork in PS1.
fn test_run_zsh_global_rc_agnoster_prompt() {
    let command = r#"build_prompt; [[ "$PROMPT" == *$'\ue0b0'* ]] || exit 42; [[ "$PROMPT" != *'$('* ]] || exit 43; [[ "$PROMPT" == *'%m'*'%~'* ]] || exit 44; [[ -n "${preexec_functions[(r)_agenticos_terminal_title_preexec]}" ]] || exit 45; exit 0"#;
    let Some((kind, code)) = drive_zsh(&["+m", "-ic", command]) else {
        return;
    };
    assert!(matches!(
        kind,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(code, 0, "agnoster startup/prompt command failed");
}

/// Launch the committed Links binary through the normal userland loader.
/// `-version` exits before opening the interactive event loop, making this a
/// deterministic smoke test for staging, static-musl startup, and the binary's
/// command-line path.
fn test_run_links_version() {
    let path = crate::userland::bin_namespace::LINKS_HOST_PATH;
    if !crate::fs::exists(path) {
        crate::debug_info!("[links2] {} not staged; skipping", path);
        return;
    }
    let prior_trace = crate::userland::abi::is_trace_mode();
    crate::userland::abi::set_trace_mode(true);
    crate::userland::abi::reset_unknown_syscall_trace();
    let result = crate::userland::launcher::launch_user_binary(
        path,
        &[path, "-version"],
        &crate::userland::process_service::DEFAULT_USER_ENV,
    )
    .expect("launch staged Links");
    crate::userland::abi::set_trace_mode(prior_trace);

    assert!(matches!(
        result.0,
        crate::userland::lifecycle::ExitKind::Cooperative
    ));
    assert_eq!(result.1, 0);
    let still_active = crate::userland::lifecycle::with_active_user(|au| au.image.is_some());
    assert!(
        !still_active,
        "active-user slot should be empty after Links exits"
    );
    assert!(
        crate::fs::exists("/root/.links"),
        "Links should create its configuration directory under HOME=/root"
    );
}

fn test_run_fault_gp() {
    reset_active_user();
    let bytes = fix::fault_gp_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    match result.0 {
        ExitKind::Abnormal { vector, .. } => assert_eq!(vector, 13),
        other => panic!("expected Abnormal(#GP), got {:?}", other),
    }
}

/// `write` with a kernel-range pointer returns EFAULT; the app then does
/// `exit_group(EFAULT)`. EFAULT = -14 (sign-extended through the i32 cast
/// in `exit_group_handler`).
fn test_run_bad_pointer_syscall() {
    reset_active_user();
    let bytes = fix::print_kernel_ptr_then_exit_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result = crate::userland::enter_user_mode(image).expect("enter_user_mode");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, -14);
}

fn test_run_leak_loop_happy() {
    for _ in 0..3 {
        reset_active_user();
        let bytes = fix::hello_exit0_elf();
        let image = load_elf(&bytes).expect("load_elf in leak loop");
        let result = crate::userland::enter_user_mode(image).expect("enter_user_mode in leak loop");
        let _ = crate::userland::release_active_image();
        use crate::userland::lifecycle::ExitKind;
        assert!(matches!(result.0, ExitKind::Cooperative));
    }
}

// ---------- Phase 1: user stdin queue + read(0) syscall ----------

/// `userland::stdin::pop_into` against an installed-but-empty queue
/// reports zero, and reports the requested bytes after a producer push.
fn test_user_stdin_install_push_pop() {
    crate::userland::stdin::clear();
    assert!(!crate::userland::stdin::is_active());

    crate::userland::stdin::install();
    assert!(crate::userland::stdin::is_active());

    let mut buf = [0u8; 16];
    assert_eq!(
        crate::userland::stdin::pop_into(&mut buf),
        0,
        "empty queue must return 0 (caller treats as block-needed)"
    );

    crate::userland::stdin::push_bytes(b"hi\n");
    assert_eq!(crate::userland::stdin::queued_len(), 3);

    let n = crate::userland::stdin::pop_into(&mut buf);
    assert_eq!(n, 3);
    assert_eq!(&buf[..n], b"hi\n");
    assert_eq!(crate::userland::stdin::queued_len(), 0);

    crate::userland::stdin::clear();
    assert!(!crate::userland::stdin::is_active());
}

/// `push_bytes` while no user is active is a silent no-op — the producer
/// (TerminalWindow) never has to gate the call itself.
fn test_user_stdin_push_when_inactive_is_noop() {
    crate::userland::stdin::clear();
    crate::userland::stdin::push_bytes(b"dropped");
    assert!(!crate::userland::stdin::is_active());
}

/// `read(0, ptr, len)` fast-path: when bytes are already queued, the
/// dispatcher returns them without entering the sti/hlt blocking loop.
/// Drives the dispatcher synthetically (no ring 3) so the test never
/// actually halts the CPU waiting for keyboard input.
fn test_dispatch_read_returns_queued_bytes() {
    install_streams_for_dispatcher_test();
    let buf = [0u8; 32];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 32,
    });

    let terminal_id = crate::window::WindowId::new();
    let prior_terminal = crate::userland::lifecycle::with_active_user(|p| {
        let prior = p.terminal_id;
        p.terminal_id = Some(terminal_id);
        prior
    });
    crate::userland::stdin::install_for_terminal(terminal_id);
    crate::userland::stdin::push_bytes_for_terminal(terminal_id, b"echo me\n");

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = 0;
    args.rsi = ptr;
    args.rdx = 16;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 8, "expected 8 bytes (\"echo me\\n\")");
    assert_eq!(&buf[..8], b"echo me\n");

    crate::userland::stdin::clear_for_terminal(terminal_id);
    crate::userland::lifecycle::with_active_user(|p| p.terminal_id = prior_terminal);
    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// `read(0)` with no active user-stdin queue reports 0 (EOF) rather than
/// hanging in the sti/hlt loop. Defensive: a production launch always
/// installs the queue before iretq, so this branch only triggers in the
/// in-kernel test path.
fn test_dispatch_read_no_active_user_returns_zero() {
    install_streams_for_dispatcher_test();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 16,
    });
    crate::userland::stdin::clear();

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = 0;
    args.rsi = ptr;
    args.rdx = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

/// Smoke test for `enter_user_mode_with`: a hello binary launched with a
/// real argv/envp still exits cleanly with code 0. We don't assert on the
/// frame layout here — the auxv walker fixture covers the
/// argv=1, envp=0 case; this just verifies the new entry point doesn't
/// regress the happy path when extra strings are emitted onto the stack.
fn test_enter_user_mode_with_argv_envp() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let argv = ["/host/HELLO.ELF", "alpha", "beta"];
    let envp = ["PATH=/host", "HOME=/", "TERM=dumb"];
    let result =
        crate::userland::enter_user_mode_with(image, &argv, &envp).expect("enter_user_mode_with");
    let _ = crate::userland::release_active_image();

    use crate::userland::lifecycle::ExitKind;
    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(result.1, 0);
}

// ---------- Phase 2: FD table ----------

fn test_fdtable_install_default_streams() {
    let mut t = FdTable::new();
    t.install_default_streams();
    assert!(matches!(t.get(0), Some(FdSlot::Stdin)));
    assert!(matches!(t.get(1), Some(FdSlot::Stdout)));
    assert!(matches!(t.get(2), Some(FdSlot::Stderr)));
    assert!(t.get(3).is_none());
}

fn test_fdtable_alloc_and_close() {
    let mut t = FdTable::new();
    t.install_default_streams();
    // Build a fake file slot for allocation. We can't easily fabricate a
    // real Arc<File> in unit tests, so we use the Stdin marker as a
    // stand-in — `alloc` accepts any FdSlot variant.
    let fd = t.alloc(FdSlot::Stdin).expect("first alloc");
    assert_eq!(fd, 3, "lowest-free-fd should start at 3");
    let fd2 = t.alloc(FdSlot::Stdin).expect("second alloc");
    assert_eq!(fd2, 4);
    assert!(t.close(fd).is_ok());
    let fd3 = t.alloc(FdSlot::Stdin).expect("third alloc reuses slot 3");
    assert_eq!(fd3, 3, "closed slot 3 must be reused");
    assert_eq!(t.close(99).err(), Some(EBADF));
}

fn test_fdtable_dup_and_dup2() {
    let mut t = FdTable::new();
    t.install_default_streams();
    let dup_fd = t.dup(1).expect("dup stdout");
    assert_eq!(dup_fd, 3);
    assert!(matches!(t.get(dup_fd), Some(FdSlot::Stdout)));

    let target = t.dup2(0, 7).expect("dup2 stdin to fd 7");
    assert_eq!(target, 7);
    assert!(matches!(t.get(7), Some(FdSlot::Stdin)));

    // dup2(fd, fd) is a no-op on a valid fd.
    assert_eq!(t.dup2(1, 1), Some(1));
    // dup2 from a closed fd returns None (-> EBADF at the syscall layer).
    assert_eq!(t.dup2(20, 5), None);
}

// ---------- Phase 2: path utilities ----------

fn test_normalize_path_absolute_keeps_path() {
    assert_eq!(normalize_path("/host", "/etc/passwd"), "/etc/passwd");
}

fn test_normalize_path_relative_anchors_at_cwd() {
    assert_eq!(normalize_path("/host", "foo.txt"), "/host/foo.txt");
    assert_eq!(normalize_path("/", "foo.txt"), "/foo.txt");
}

fn test_normalize_path_collapses_redundancy() {
    assert_eq!(normalize_path("/host", "./a/./b//c"), "/host/a/b/c");
    assert_eq!(normalize_path("/host", "a/../b"), "/host/b");
    assert_eq!(normalize_path("/host", "../.."), "/");
    assert_eq!(normalize_path("/", "../foo"), "/foo");
    assert_eq!(normalize_path("/host", "."), "/host");
}

fn test_copy_user_cstr_happy_path() {
    let bytes = b"hello\0world";
    let ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + bytes.len() as u64,
    });
    let s = copy_user_cstr(ptr).expect("copy hello");
    assert_eq!(s, "hello");
    abi::clear_user_va_bounds();
}

fn test_copy_user_cstr_unterminated_at_bound_returns_efault() {
    let bytes = b"abcdef";
    let ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + bytes.len() as u64,
    });
    // No NUL within bounds → -EFAULT
    assert_eq!(copy_user_cstr(ptr), Err(EFAULT));
    abi::clear_user_va_bounds();
}

// ---------- Phase 2: dispatcher tests ----------

/// Helper: install default streams and a fixed cwd for syscall tests
/// that don't go through `enter_user_mode_with`.
fn setup_phase2_active_user() {
    use alloc::string::String;
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.fd_table.install_default_streams();
        au.cwd = String::from("/host");
    });
}

fn teardown_phase2_active_user() {
    crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.clear();
        au.cwd.clear();
    });
}

fn test_dispatch_getcwd_returns_default() {
    setup_phase2_active_user();
    let buf = [0u8; 64];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETCWD;
    args.rdi = ptr;
    args.rsi = 64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(ret, 6, "expected '/host\\0' → 6 bytes");
    assert_eq!(&buf[..6], b"/host\0");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_getcwd_short_buffer_returns_erange() {
    setup_phase2_active_user();
    let buf = [0u8; 4];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 4,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETCWD;
    args.rdi = ptr;
    args.rsi = 4; // Need 6 (5 + NUL) — short.
    assert_eq!(syscall_dispatch(&mut args), ERANGE);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_chdir_root_succeeds() {
    setup_phase2_active_user();
    let path = b"/\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::CHDIR;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let cwd = crate::userland::lifecycle::with_active_user(|au| au.cwd.clone());
    assert_eq!(cwd, "/");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_chdir_nonexistent_returns_enoent() {
    setup_phase2_active_user();
    let path = b"/nonexistent_directory_xyz\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::CHDIR;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_open_nonexistent_returns_enoent() {
    setup_phase2_active_user();
    let path = b"/host/NEVER_EXISTS_XYZ.TXT\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0; // O_RDONLY
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_open_writable_flag_returns_erofs() {
    setup_phase2_active_user();
    let path = b"/host/HELLO.ELF\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 1; // O_WRONLY
    assert_eq!(syscall_dispatch(&mut args), EROFS);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_regular_file_fcntl_reports_write_access() {
    setup_phase2_active_user();
    let path = b"/fcntl-status.tmp\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut open = SyscallArgs::default();
    open.rax = nr::OPEN;
    open.rdi = ptr;
    open.rsi = 0o1101; // O_WRONLY|O_CREAT|O_TRUNC
    let fd = syscall_dispatch(&mut open);
    assert!(fd >= 0, "open for write failed: {}", fd);

    let mut fcntl = SyscallArgs::default();
    fcntl.rax = nr::FCNTL;
    fcntl.rdi = fd as u64;
    fcntl.rsi = 3; // F_GETFL
    assert_eq!(syscall_dispatch(&mut fcntl), 1, "expected O_WRONLY");

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);
    crate::fs::vfs::vfs_unlink("/fcntl-status.tmp").expect("unlink fcntl fixture");
    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_readv_regular_file_scatter() {
    setup_phase2_active_user();
    let fixture = crate::fs::File::create("/readv.tmp").expect("create readv fixture");
    assert_eq!(fixture.write(b"scatter").expect("write readv fixture"), 7);

    let path = b"/readv.tmp\0";
    let path_ptr = path.as_ptr() as u64;
    let mut first = [0u8; 3];
    let mut second = [0u8; 4];
    let iovecs = [
        first.as_mut_ptr() as u64,
        first.len() as u64,
        second.as_mut_ptr() as u64,
        second.len() as u64,
    ];
    let iov_ptr = iovecs.as_ptr() as u64;
    let start = [
        path_ptr,
        first.as_ptr() as u64,
        second.as_ptr() as u64,
        iov_ptr,
    ]
    .into_iter()
    .min()
    .unwrap();
    let end = [
        path_ptr + path.len() as u64,
        first.as_ptr() as u64 + first.len() as u64,
        second.as_ptr() as u64 + second.len() as u64,
        iov_ptr + 32,
    ]
    .into_iter()
    .max()
    .unwrap();
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut open = SyscallArgs::default();
    open.rax = nr::OPEN;
    open.rdi = path_ptr;
    let fd = syscall_dispatch(&mut open);
    assert!(fd >= 0, "open readv fixture failed: {}", fd);

    let mut readv = SyscallArgs::default();
    readv.rax = nr::READV;
    readv.rdi = fd as u64;
    readv.rsi = iov_ptr;
    readv.rdx = 2;
    assert_eq!(syscall_dispatch(&mut readv), 7);
    assert_eq!(&first, b"sca");
    assert_eq!(&second, b"tter");

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);
    crate::fs::vfs::vfs_unlink("/readv.tmp").expect("unlink readv fixture");
    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// The kernel-owned runtime `/etc/passwd` is directly visible to userland.
fn test_dispatch_open_runtime_etc_passwd() {
    setup_phase2_active_user();
    let path = b"/etc/passwd\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0; // O_RDONLY
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 0, "expected fd >= 0, got {}", fd);

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// The shipped zsh startup chain is imported into the managed runtime `/etc`.
fn test_dispatch_stat_runtime_zsh_config() {
    assert!(
        crate::fs::exists("/etc/zshrc"),
        "managed /etc must import the staged zshrc"
    );
    assert!(
        crate::fs::exists("/etc/zsh/functions/promptinit"),
        "managed /etc must import the staged zsh function library"
    );

    setup_phase2_active_user();
    for path in [
        b"/etc/zshrc\0".as_slice(),
        b"/etc/zsh/functions/promptinit\0".as_slice(),
    ] {
        let path_ptr = path.as_ptr() as u64;
        let mut statbuf = [0u8; 144];
        let buf_ptr = statbuf.as_mut_ptr() as u64;
        let start = core::cmp::min(path_ptr, buf_ptr);
        let end = core::cmp::max(path_ptr + path.len() as u64, buf_ptr + statbuf.len() as u64);
        abi::set_user_va_bounds(UserVaBounds { start, end });

        let mut args = SyscallArgs::default();
        args.rax = nr::STAT;
        args.rdi = path_ptr;
        args.rsi = buf_ptr;
        let result = syscall_dispatch(&mut args);
        assert_eq!(result, 0, "stat({:?}) failed", path);

        let mode = u32::from_ne_bytes([statbuf[24], statbuf[25], statbuf[26], statbuf[27]]);
        assert_eq!(mode & 0o170000, 0o100000, "expected regular file");
    }

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// Files not seeded into the kernel-owned runtime `/etc` remain absent.
fn test_dispatch_open_etc_unmanaged_file_returns_enoent() {
    setup_phase2_active_user();
    let path = b"/etc/shadow\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), ENOENT);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// Path normalization still resolves traversal back to the managed file.
fn test_dispatch_open_etc_traversal_collapses() {
    setup_phase2_active_user();
    let path = b"/etc/../etc/passwd\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 0, "expected fd >= 0 after traversal, got {}", fd);

    let mut close = SyscallArgs::default();
    close.rax = nr::CLOSE;
    close.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut close), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_open_runtime_etc_for_write_returns_erofs() {
    setup_phase2_active_user();
    let path = b"/etc/resolv.conf\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 1; // O_WRONLY
    assert_eq!(syscall_dispatch(&mut args), EROFS);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_unlink_runtime_etc_returns_eperm() {
    setup_phase2_active_user();
    let path = b"/etc/resolv.conf\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::UNLINK;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), EPERM);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_close_stream_is_noop() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::CLOSE;
    args.rdi = 1; // stdout
    assert_eq!(syscall_dispatch(&mut args), 0);

    let still_open =
        crate::userland::lifecycle::with_active_user(|au| au.fd_table.get(1).is_some());
    assert!(still_open, "stdout must remain open after a close attempt");
    teardown_phase2_active_user();
}

fn test_dispatch_dup_stdout() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::DUP;
    args.rdi = 1; // stdout
    let new_fd = syscall_dispatch(&mut args);
    assert_eq!(new_fd, 3);

    let is_stdout = crate::userland::lifecycle::with_active_user(|au| {
        matches!(au.fd_table.get(3), Some(FdSlot::Stdout))
    });
    assert!(is_stdout);
    teardown_phase2_active_user();
}

fn test_dispatch_lseek_on_stream_returns_espipe() {
    use crate::userland::abi::ESPIPE;
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::LSEEK;
    args.rdi = 1;
    args.rsi = 0;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), ESPIPE);
    teardown_phase2_active_user();
}

fn test_dispatch_clock_gettime_writes_timespec() {
    setup_phase2_active_user();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 16,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::CLOCK_GETTIME;
    args.rdi = 1; // CLOCK_MONOTONIC
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    // tv_nsec at offset 8 must be < 1e9
    let ns_bytes: [u8; 8] = buf[8..16].try_into().unwrap();
    let nsec = i64::from_ne_bytes(ns_bytes);
    assert!(
        (0..1_000_000_000).contains(&nsec),
        "nsec out of range: {}",
        nsec
    );

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_clock_gettime_invalid_clock_einval() {
    setup_phase2_active_user();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 16,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::CLOCK_GETTIME;
    args.rdi = 99;
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), EINVAL);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_clock_realtime_uses_rtc_epoch() {
    setup_phase2_active_user();
    let buf = [0u8; 16];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 16,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::CLOCK_GETTIME;
    args.rdi = 0; // CLOCK_REALTIME
    args.rsi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let sec_bytes: [u8; 8] = buf[0..8].try_into().unwrap();
    let seconds = i64::from_ne_bytes(sec_bytes);
    assert!(
        seconds > 1_500_000_000,
        "realtime did not use the RTC-backed Unix epoch: {}",
        seconds
    );

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_umask_roundtrip_and_masks_bits() {
    setup_phase2_active_user();
    crate::userland::lifecycle::with_active_user(|process| process.umask = 0o022);

    let mut args = SyscallArgs::default();
    args.rax = nr::UMASK;
    args.rdi = 0o1077;
    assert_eq!(syscall_dispatch(&mut args), 0o022);
    assert_eq!(
        crate::userland::lifecycle::with_active_user(|process| process.umask),
        0o077
    );

    args.rdi = 0o022;
    assert_eq!(syscall_dispatch(&mut args), 0o077);
    teardown_phase2_active_user();
}

fn test_dispatch_utimensat_values_now_omit_and_errors() {
    const AT_FDCWD: u64 = (-100i64) as u64;
    const UTIME_NOW: i64 = 0x3fff_ffff;
    const UTIME_OMIT: i64 = 0x3fff_fffe;

    setup_phase2_active_user();
    let fixture = crate::fs::File::create("/utimens.tmp").expect("create utimens fixture");
    assert_eq!(fixture.write(b"x").expect("write utimens fixture"), 1);
    let data_fixture =
        crate::fs::File::create("/data/utimens-data.tmp").expect("create ext2 utimens fixture");
    assert_eq!(
        data_fixture
            .write(b"x")
            .expect("write ext2 utimens fixture"),
        1
    );
    crate::fs::vfs::vfs_set_times("/utimens.tmp", Some(11), Some(22)).expect("seed timestamps");

    let path = b"/utimens.tmp\0";
    let data_path = b"/data/utimens-data.tmp\0";
    let missing = b"/utimens-missing.tmp\0";
    let host = b"/host/BB.ELF\0";
    let explicit = [123i64, 0, 456, 0];
    let sentinels = [0i64, UTIME_OMIT, 0, UTIME_NOW];
    let pointers = [
        path.as_ptr() as u64,
        data_path.as_ptr() as u64,
        missing.as_ptr() as u64,
        host.as_ptr() as u64,
        explicit.as_ptr() as u64,
        sentinels.as_ptr() as u64,
    ];
    let start = *pointers.iter().min().unwrap();
    let end = [
        path.as_ptr() as u64 + path.len() as u64,
        data_path.as_ptr() as u64 + data_path.len() as u64,
        missing.as_ptr() as u64 + missing.len() as u64,
        host.as_ptr() as u64 + host.len() as u64,
        explicit.as_ptr() as u64 + 32,
        sentinels.as_ptr() as u64 + 32,
    ]
    .into_iter()
    .max()
    .unwrap();
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut args = SyscallArgs::default();
    args.rax = nr::UTIMENSAT;
    args.rdi = AT_FDCWD;
    args.rsi = path.as_ptr() as u64;
    args.rdx = explicit.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let metadata = crate::fs::vfs::vfs_unix_metadata("/utimens.tmp").expect("explicit stat");
    assert_eq!(metadata.accessed, 123);
    assert_eq!(metadata.modified, 456);

    args.rsi = data_path.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let data_metadata =
        crate::fs::vfs::vfs_unix_metadata("/data/utimens-data.tmp").expect("ext2 explicit stat");
    assert_eq!(data_metadata.accessed, 123);
    assert_eq!(data_metadata.modified, 456);
    args.rsi = path.as_ptr() as u64;

    args.rdx = sentinels.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let metadata = crate::fs::vfs::vfs_unix_metadata("/utimens.tmp").expect("sentinel stat");
    assert_eq!(metadata.accessed, 123, "UTIME_OMIT must preserve atime");
    assert!(
        metadata.modified > 1_500_000_000,
        "UTIME_NOW must use realtime"
    );

    args.r10 = 1;
    assert_eq!(syscall_dispatch(&mut args), EINVAL);
    args.r10 = 0;
    args.rdx = u64::MAX - 15;
    assert_eq!(syscall_dispatch(&mut args), EFAULT);
    args.rdx = explicit.as_ptr() as u64;
    args.rsi = missing.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), ENOENT);
    args.rsi = host.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), EROFS);

    crate::fs::vfs::vfs_unlink("/utimens.tmp").expect("unlink utimens fixture");
    crate::fs::vfs::vfs_unlink("/data/utimens-data.tmp").expect("unlink ext2 utimens fixture");
    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_getrandom_fills_buffer() {
    setup_phase2_active_user();
    let buf = [0u8; 32];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + 32,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::GETRANDOM;
    args.rdi = ptr;
    args.rsi = 32;
    assert_eq!(syscall_dispatch(&mut args), 32);
    assert!(buf.iter().any(|&b| b != 0), "getrandom returned all zeros");
    let first = buf;
    assert_eq!(syscall_dispatch(&mut args), 32);
    assert_ne!(buf, first, "consecutive getrandom calls repeated");

    args.rdx = 0x4;
    assert_eq!(syscall_dispatch(&mut args), abi::EINVAL);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_dev_null_rdwr_read_eof_write_sink() {
    setup_phase2_active_user();
    let path = b"/dev/null\0";
    let path_ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: path_ptr,
        end: path_ptr + path.len() as u64,
    });
    // git's sanitize_stdfds opens /dev/null O_RDWR — the write-capable
    // open must succeed, unlike every other device node.
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = path_ptr;
    args.rsi = 2; // O_RDWR
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3, "open(/dev/null, O_RDWR) failed: {}", fd);

    let bytes = [0xa5u8; 16];
    let bytes_ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: bytes_ptr,
        end: bytes_ptr + bytes.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = fd as u64;
    args.rsi = bytes_ptr;
    args.rdx = bytes.len() as u64;
    assert_eq!(
        syscall_dispatch(&mut args),
        bytes.len() as i64,
        "write to /dev/null must report full length"
    );

    args.rax = nr::READ;
    assert_eq!(syscall_dispatch(&mut args), 0, "/dev/null reads are EOF");
    assert_eq!(bytes, [0xa5u8; 16], "read must not touch the buffer");

    args = SyscallArgs::default();
    args.rax = nr::LSEEK;
    args.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut args), 0, "/dev/null is seekable");

    let stat = [0u8; 144];
    let stat_ptr = stat.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: stat_ptr,
        end: stat_ptr + stat.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::FSTAT;
    args.rdi = fd as u64;
    args.rsi = stat_ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(
        u32::from_ne_bytes(stat[24..28].try_into().unwrap()) & 0o170000,
        0o020000,
        "/dev/null is a character device"
    );
    assert_eq!(u64::from_ne_bytes(stat[40..48].try_into().unwrap()), 0x103);

    abi::set_user_va_bounds(UserVaBounds {
        start: path_ptr,
        end: path_ptr + path.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::ACCESS;
    args.rdi = path_ptr;
    args.rsi = 2; // W_OK
    assert_eq!(syscall_dispatch(&mut args), 0, "/dev/null is writable");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_dev_urandom_read_stat_and_seek() {
    setup_phase2_active_user();
    let path = b"/dev/urandom\0";
    let path_ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: path_ptr,
        end: path_ptr + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = path_ptr;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3, "open(/dev/urandom) failed: {}", fd);

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }
    let mut pollfd = PollFd {
        fd: fd as i32,
        events: 0x0001 | 0x0004,
        revents: 0,
    };
    let pollfd_ptr = &mut pollfd as *mut PollFd as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: pollfd_ptr,
        end: pollfd_ptr + core::mem::size_of::<PollFd>() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::POLL;
    args.rdi = pollfd_ptr;
    args.rsi = 1;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), 1);
    assert_eq!(pollfd.revents, 0x0001, "/dev/urandom is read-only-ready");

    let bytes = [0u8; 32];
    let bytes_ptr = bytes.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: bytes_ptr,
        end: bytes_ptr + bytes.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = fd as u64;
    args.rsi = bytes_ptr;
    args.rdx = bytes.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), bytes.len() as i64);
    let first = bytes;
    assert_eq!(syscall_dispatch(&mut args), bytes.len() as i64);
    assert_ne!(bytes, first, "/dev/urandom repeated consecutive reads");

    args = SyscallArgs::default();
    args.rax = nr::LSEEK;
    args.rdi = fd as u64;
    assert_eq!(syscall_dispatch(&mut args), abi::ESPIPE);

    let stat = [0u8; 144];
    let stat_ptr = stat.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: stat_ptr,
        end: stat_ptr + stat.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::FSTAT;
    args.rdi = fd as u64;
    args.rsi = stat_ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(
        u32::from_ne_bytes(stat[24..28].try_into().unwrap()) & 0o170000,
        0o020000
    );
    assert_eq!(u64::from_ne_bytes(stat[40..48].try_into().unwrap()), 0x109);

    abi::set_user_va_bounds(UserVaBounds {
        start: path_ptr,
        end: path_ptr + path.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = path_ptr;
    args.rsi = 1; // O_WRONLY
    assert_eq!(syscall_dispatch(&mut args), abi::EACCES);
    args.rax = nr::ACCESS;
    args.rsi = 2; // W_OK
    assert_eq!(syscall_dispatch(&mut args), abi::EACCES);
    args.rsi = 4; // R_OK
    assert_eq!(syscall_dispatch(&mut args), 0);
    args.rax = nr::UNLINK;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), abi::EPERM);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_dev_directory_lists_urandom() {
    setup_phase2_active_user();
    let path = b"/dev\0";
    let path_ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: path_ptr,
        end: path_ptr + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = path_ptr;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3, "open(/dev) failed: {}", fd);

    let entries = [0u8; 256];
    let entries_ptr = entries.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: entries_ptr,
        end: entries_ptr + entries.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::GETDENTS64;
    args.rdi = fd as u64;
    args.rsi = entries_ptr;
    args.rdx = entries.len() as u64;
    let written = syscall_dispatch(&mut args);
    assert!(written > 0);
    assert!(
        entries[..written as usize]
            .windows(b"urandom\0".len())
            .any(|window| window == b"urandom\0"),
        "/dev listing omitted urandom"
    );

    // The exact synthetic classifier must not shadow `/dev/null`, which zsh
    // creates in the writable overlay for stderr redirection.
    let null_path = b"/dev/null\0";
    let null_ptr = null_path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: null_ptr,
        end: null_ptr + null_path.len() as u64,
    });
    args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = null_ptr;
    args.rsi = 1 | 0o100; // O_WRONLY | O_CREAT
    let null_fd = syscall_dispatch(&mut args);
    assert!(
        null_fd >= 3,
        "synthetic devfs shadowed /dev/null: {}",
        null_fd
    );

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_uname_writes_sysname_linux() {
    setup_phase2_active_user();
    let buf = [0u8; 6 * 65];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::UNAME;
    args.rdi = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(&buf[..5], b"Linux");
    // machine field at offset 4*65
    assert_eq!(&buf[4 * 65..4 * 65 + 6], b"x86_64");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 4 PR-B: per-process address spaces ----------

/// `AddressSpace::new` succeeds after boot, returns a frame distinct
/// from the kernel L4 (which `capture_kernel_l4` recorded), and leaves
/// the user-half (PML4[0]) empty while copying the kernel-half entry
/// covering the heap (PML4[136]).
fn test_address_space_new_kernel_half_shared() {
    use crate::userland::address_space::AddressSpace;
    use x86_64::structures::paging::PageTable;

    let kernel_frame = crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    let aspace = AddressSpace::new().expect("AddressSpace::new should succeed");
    assert_ne!(
        aspace.l4_frame(),
        kernel_frame,
        "process L4 must be a fresh frame, not the kernel L4 itself"
    );

    // Walk both L4s through the bootloader's offset mapping and
    // compare PML4 entries.
    let phys_offset = crate::mm::memory::get_physical_memory_offset().expect("phys offset");
    let kernel_va = phys_offset + kernel_frame.start_address().as_u64();
    let user_va = phys_offset + aspace.l4_frame().start_address().as_u64();
    let kernel_table = unsafe { &*(kernel_va as *const PageTable) };
    let user_table = unsafe { &*(user_va as *const PageTable) };

    // PML4[0] is per-process — empty in the fresh user L4.
    assert!(
        user_table[0].is_unused(),
        "PML4[0] of a fresh AddressSpace must start unused"
    );

    // PML4[136] hosts the kernel heap (0x4444_4444_0000). It must be
    // mirrored from the kernel L4 so heap demand-paging continues to
    // work while the user L4 is active.
    assert_eq!(
        user_table[136].addr(),
        kernel_table[136].addr(),
        "kernel-heap PML4 entry must be shared by reference"
    );
    assert_eq!(
        user_table[136].flags(),
        kernel_table[136].flags(),
        "kernel-heap PML4 flags must match"
    );
}

/// Activating an `AddressSpace`, then dropping it, leaves CR3 pointing
/// at the kernel L4. The Drop impl is the safety net for early-return
/// error paths in `RunProcess::run_path`.
fn test_address_space_drop_restores_kernel_cr3() {
    use crate::userland::address_space::AddressSpace;
    use x86_64::registers::control::Cr3;

    let kernel_frame = crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    {
        let aspace = AddressSpace::new().expect("AddressSpace::new");
        // SAFETY: kernel half copied from kernel L4 — the code after
        // the CR3 write is still mapped.
        unsafe {
            aspace.activate();
        }
        let (after, _) = Cr3::read();
        assert_eq!(after, aspace.l4_frame());
        // aspace dropped here.
    }

    let (final_cr3, _) = Cr3::read();
    assert_eq!(
        final_cr3, kernel_frame,
        "AddressSpace::Drop must revert CR3 to the kernel L4"
    );
}

fn test_address_space_drop_reclaims_leaf_and_all_table_levels() {
    use crate::mm::paging::UserPerms;
    use crate::userland::address_space::AddressSpace;
    use x86_64::VirtAddr;

    let before = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    {
        let aspace = AddressSpace::new().expect("AddressSpace::new");
        unsafe {
            aspace.activate();
        }
        // Separate PTs and PDPTs force teardown to visit/release L1, L2,
        // L3, leaf, and finally L4 ownership.
        for address in [0x400000, 0x600000, 0x4000_0000] {
            crate::mm::memory::with_memory_mapper(|m| {
                m.map_user_region(VirtAddr::new(address), 1, UserPerms::ReadWrite)
                    .expect("cross-level user map")
            })
            .expect("memory mapper");
        }
    }
    let after = crate::mm::memory::with_memory_mapper(|m| m.frame_stats()).unwrap();
    assert_eq!(
        after, before,
        "dropping an active address space must reclaim every owned frame"
    );
}

// ---------- Phase 5 PR-B: signals ----------

/// `rt_sigaction(SIGUSR1, &act, &oldact, 8)` round-trips: the
/// installed handler comes back via a follow-up "query only" call.
fn test_dispatch_rt_sigaction_round_trip() {
    use crate::userland::signal::{SigAction, SIGUSR1};
    setup_phase2_active_user();

    let new_action = SigAction {
        sa_handler: 0xCAFE_BABE_DEAD_BEEF,
        sa_flags: 0x4000_0000, // SA_RESTORER
        sa_restorer: 0x1234_5678_9ABC_DEF0,
        sa_mask: 0,
    };
    let act = new_action;
    let mut oldact = SigAction::default();

    let act_ptr = &act as *const SigAction as u64;
    let oldact_ptr = &mut oldact as *mut SigAction as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(act_ptr, oldact_ptr),
        end: core::cmp::max(act_ptr, oldact_ptr) + 32,
    });

    // First call: install new, retrieve previous (default).
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGUSR1 as u64;
    args.rsi = act_ptr;
    args.rdx = oldact_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldact.sa_handler, 0); // SIG_DFL initially

    // Second call: query-only. Now we should see the action we just set.
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGUSR1 as u64;
    args.rsi = 0; // act = NULL → query
    args.rdx = oldact_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldact.sa_handler, 0xCAFE_BABE_DEAD_BEEF);
    assert_eq!(oldact.sa_restorer, 0x1234_5678_9ABC_DEF0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `rt_sigaction(NSIG, ...)` (signal 64 = SIGRTMAX) must not panic.
/// The actions table is sized `NSIG + 1`; an earlier off-by-one sized
/// it `NSIG` so `table[64]` blew up the kernel when zsh installed
/// handlers across the full RT-signal range at startup.
fn test_dispatch_rt_sigaction_sigrtmax_does_not_panic() {
    use crate::userland::signal::{SigAction, NSIG};
    setup_phase2_active_user();

    let act = SigAction {
        sa_handler: 0xAABB_CCDD_EEFF_0011,
        ..SigAction::default()
    };
    let mut oldact = SigAction::default();
    let act_ptr = &act as *const SigAction as u64;
    let oldact_ptr = &mut oldact as *mut SigAction as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(act_ptr, oldact_ptr),
        end: core::cmp::max(act_ptr, oldact_ptr) + 32,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = NSIG as u64; // signal 64
    args.rsi = act_ptr;
    args.rdx = oldact_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// SIGKILL and SIGSTOP must not be settable (POSIX). The syscall
/// returns 0 (no error) but the action stays as default.
fn test_dispatch_rt_sigaction_rejects_sigkill_sigstop() {
    use crate::userland::signal::{SigAction, SIGKILL};
    setup_phase2_active_user();

    let act = SigAction {
        sa_handler: 0xDEAD,
        ..SigAction::default()
    };
    let act_ptr = &act as *const SigAction as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: act_ptr,
        end: act_ptr + 32,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGACTION;
    args.rdi = SIGKILL as u64;
    args.rsi = act_ptr;
    args.rdx = 0;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let installed = crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state
            .action(SIGKILL)
            .unwrap_or_default()
            .sa_handler
    });
    assert_eq!(installed, 0, "SIGKILL action must remain SIG_DFL");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `rt_sigprocmask(SIG_BLOCK, &set, &oldset)` ORs into the blocked
/// mask; SIGKILL/SIGSTOP cannot be blocked even if the set requests it.
fn test_dispatch_rt_sigprocmask_block_strips_kill_stop() {
    use crate::userland::signal::{SIGKILL, SIGSTOP, SIGUSR1, SIG_BLOCK};
    setup_phase2_active_user();

    let set: u64 = (1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)) | (1 << (SIGUSR1 - 1));
    let mut oldset: u64 = 0;
    let set_ptr = &set as *const u64 as u64;
    let oldset_ptr = &mut oldset as *mut u64 as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(set_ptr, oldset_ptr),
        end: core::cmp::max(set_ptr, oldset_ptr) + 8,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGPROCMASK;
    args.rdi = SIG_BLOCK as u64;
    args.rsi = set_ptr;
    args.rdx = oldset_ptr;
    args.r10 = 8;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(oldset, 0, "previous mask was empty");

    let blocked = crate::userland::lifecycle::with_current_process(|p| p.signal_state.blocked);
    assert_eq!(blocked & (1 << (SIGUSR1 - 1)), 1 << (SIGUSR1 - 1));
    assert_eq!(
        blocked & (1 << (SIGKILL - 1)),
        0,
        "SIGKILL must not be blockable"
    );
    assert_eq!(
        blocked & (1 << (SIGSTOP - 1)),
        0,
        "SIGSTOP must not be blockable"
    );

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `rt_sigsuspend(&mask, 8)` must return `-EINTR` without parking when
/// the temporary mask exposes an already-pending handled signal. It also
/// saves the original mask for restoration after that handler returns.
fn test_rt_sigsuspend_pending_signal_returns_eintr_and_saves_mask() {
    use crate::userland::signal::{SigAction, SIGCHLD, SIGKILL, SIGSTOP, SIGUSR1};
    setup_phase2_active_user();

    let old_mask = 1u64 << (SIGCHLD - 1);
    crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.blocked = old_mask;
        p.signal_state.set_action(
            SIGCHLD,
            SigAction {
                sa_handler: 0xDEAD_BEEF,
                ..SigAction::default()
            },
        );
        p.signal_state.raise(SIGCHLD);
    });

    // The temporary mask unblocks SIGCHLD and tries to block the two
    // unblockable signals, which the kernel must strip.
    let new_mask: u64 = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1)) | (1u64 << (SIGUSR1 - 1));
    let mask_ptr = &new_mask as *const u64 as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: mask_ptr,
        end: mask_ptr + 8,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGSUSPEND;
    args.rdi = mask_ptr;
    args.rsi = 8;
    // Call the handler directly so the test can inspect state before the
    // dispatcher tail diverges into the synthetic user handler address.
    assert_eq!(
        crate::userland::syscalls::rt_sigsuspend_handler(&mut args),
        abi::EINTR
    );

    crate::userland::lifecycle::with_current_process(|p| {
        assert_eq!(p.signal_state.blocked, 1u64 << (SIGUSR1 - 1));
        assert_eq!(p.signal_state.suspend_restore_mask, Some(old_mask));

        p.signal_state.set_action(SIGCHLD, SigAction::default());
        p.signal_state.blocked = 0;
        p.signal_state.pending = 0;
        p.signal_state.suspend_restore_mask = None;
    });

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `rt_sigsuspend(ptr, 7)` rejects a non-canonical sigset size.
fn test_dispatch_rt_sigsuspend_invalid_sigsetsize() {
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGSUSPEND;
    args.rdi = 0xdead_beef;
    args.rsi = 7;
    assert_eq!(syscall_dispatch(&mut args), abi::EINVAL);
}

/// `rt_sigsuspend(NULL, 8)` rejects a null mask pointer with -EFAULT.
fn test_dispatch_rt_sigsuspend_null_mask_efaults() {
    abi::clear_user_va_bounds();
    let mut args = SyscallArgs::default();
    args.rax = nr::RT_SIGSUSPEND;
    args.rdi = 0;
    args.rsi = 8;
    assert_eq!(syscall_dispatch(&mut args), abi::EFAULT);
}

/// `kill(self, SIGUSR1)` sets SIGUSR1 pending on the current process.
/// The signal is blocked first: an unblocked SIGUSR1 with no handler
/// now takes its fatal default action at the dispatcher tail, which
/// would tear the process down mid-test.
fn test_dispatch_kill_self_sets_pending() {
    use crate::userland::signal::SIGUSR1;
    reset_active_user();

    // Install a non-zero PID so kill_handler's "kill me" branch fires.
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
        .expect("enter_user_mode_with");
    // Don't release yet — we want the Process slot populated for the
    // signal-state inspection.

    crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.blocked |= 1u64 << (SIGUSR1 - 1);
    });

    let me = crate::userland::lifecycle::current_pid();
    let mut args = SyscallArgs::default();
    args.rax = nr::KILL;
    args.rdi = me as u64;
    args.rsi = SIGUSR1 as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let pending = crate::userland::lifecycle::with_current_process(|p| p.signal_state.pending);
    assert_eq!(pending & (1 << (SIGUSR1 - 1)), 1 << (SIGUSR1 - 1));

    let _ = crate::userland::release_active_image();
}

/// Forking and exiting a child sets SIGCHLD pending on the parent's
/// signal state. We exercise this with the existing fork fixture and
/// inspect the parent's pending mask after fork returns.
fn test_fork_child_exit_sets_sigchld_on_parent() {
    use crate::userland::lifecycle::ExitKind;
    use crate::userland::signal::SIGCHLD;
    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new().expect("AddressSpace::new");
    unsafe {
        aspace.activate();
    }

    let bytes = fix::fork_then_wait_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result =
        crate::userland::enter_user_mode_with_aspace(image, &["agenticos-app"], &[], Some(aspace))
            .expect("enter_user_mode_with_aspace");

    // Parent process is still installed at this point — the run
    // command hasn't called release_active_image yet (we drive
    // enter_user_mode directly from the test).
    let pending = crate::userland::lifecycle::with_current_process(|p| p.signal_state.pending);
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert!(
        pending & (1 << (SIGCHLD - 1)) != 0,
        "expected SIGCHLD pending after child exit, got mask {:#x}",
        pending,
    );
}

/// `notify_parent_of_exit` is the shared helper that cooperative exits and
/// fault cleanup route through. It must (a) file a zombie the parent's `wait4` can find
/// and (b) raise SIGCHLD on the stashed parent. Regression guard for
/// the bug where ring-3 faults skipped both, leaving zsh hung in
/// `rt_sigsuspend` after `ls` crashed.
fn test_notify_parent_of_exit_files_zombie_and_raises_sigchld() {
    use crate::userland::lifecycle::{
        insert_process, mark_ring3_blocked, notify_parent_of_exit, reap_zombie, remove_process,
        with_process, ExitKind, Process, Ring3BlockReason,
    };
    use crate::userland::signal::{SigAction, SIGCHLD};

    // U7: both parent and child live in PROCESS_TABLE simultaneously.
    // The old PARENT_STASH dance is gone; SIGCHLD is raised on the
    // parent's regular entry via `with_process`.
    let mut parent = Process {
        pid: 100,
        parent_pid: 0,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        umask: 0o022,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: None,
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        kernel_continuation: None,
        terminal_id: None,
    };
    parent.signal_state.set_action(
        SIGCHLD,
        SigAction {
            sa_handler: 0xDEAD_BEEF,
            ..SigAction::default()
        },
    );
    insert_process(parent);
    clear_ring3_queues();
    mark_ring3_blocked(100, Ring3BlockReason::WaitingForSignal);

    notify_parent_of_exit(101, 100, 139);

    // SIGCHLD pending on parent's signal_state.
    let pending = with_process(100, |p| p.signal_state.pending).unwrap();
    assert!(
        pending & (1 << (SIGCHLD - 1)) != 0,
        "expected SIGCHLD pending on parent, got mask {:#x}",
        pending,
    );
    assert_eq!(
        crate::userland::lifecycle::pop_next_ring3(),
        Some(100),
        "SIGCHLD should wake a parent suspended for signal delivery"
    );
    assert!(
        with_process(100, |p| p.pending_syscall_interrupt).unwrap(),
        "sigsuspend wake must interrupt the re-fired syscall"
    );

    // Zombie filed for pid 101 → parent 100, exit code 139.
    let reaped = reap_zombie(101, 100).expect("zombie filed for child 101");
    assert_eq!(reaped, (101, 139, None));

    // Cleanup.
    remove_process(100);
}

/// When `parent_pid == 0` (top-level kernel-launched binary, no
/// userland parent), `notify_parent_of_exit` must be a no-op: no
/// zombie filed, no signal raised. Guards against the helper
/// accidentally creating phantom zombies for the run-command launch.
fn test_notify_parent_of_exit_skips_when_no_parent() {
    use crate::userland::lifecycle::{notify_parent_of_exit, reap_zombie};

    notify_parent_of_exit(202, 0, 7);
    // Try to reap with any plausible reaper; should find nothing.
    assert!(reap_zombie(202, 0).is_none());
    assert!(reap_zombie(202, 1).is_none());
    assert!(reap_zombie(-1, 0).is_none());
}

/// Regression guard: after `fork()` returns to the parent, the
/// parent's `user_va_bounds` must be restored so syscalls that
/// validate user pointers (write, wait4 with non-null status, read,
/// etc.) work. The child's long-jump back goes through
/// `long_jump_to_run_or_halt`, which calls `clear_user_va_bounds`;
/// `fork_handler` must save and restore the parent's bounds across
/// that boundary.
///
/// The fixture's parent calls `wait4(child, &status, 0, NULL)` with
/// `&status` pointing into its own stack. If bounds aren't restored,
/// `validate_user_slice` returns `-EFAULT` and the parent exits with
/// sentinel 99 instead of `WEXITSTATUS(status) == 42`.
fn test_fork_post_resume_user_va_bounds_restored() {
    use crate::userland::lifecycle::ExitKind;
    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new().expect("AddressSpace::new");
    unsafe {
        aspace.activate();
    }

    let bytes = fix::fork_then_wait_with_status_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result =
        crate::userland::enter_user_mode_with_aspace(image, &["agenticos-app"], &[], Some(aspace))
            .expect("enter_user_mode_with_aspace");

    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 42,
        "expected parent to exit WEXITSTATUS=42 (child's exit code), got {} \
         (99 means wait4 returned -EFAULT — bounds not restored)",
        result.1,
    );
}

/// Phase 5 PR-B2: full signal-delivery round trip.
///
/// Pre-installs a SIGUSR1 handler pointing at the fixture's handler
/// body, then enters ring 3. The fixture calls `kill(getpid(),
/// SIGUSR1)`; the dispatcher's post-syscall hook detects the pending
/// signal, builds a signal frame on the user stack, and `iretq`s into
/// the handler. The handler exits with code 42. If signal delivery
/// were broken, the fixture would fall through and exit with 99.
fn test_signal_delivery_handler_runs() {
    use crate::userland::lifecycle::ExitKind;
    use crate::userland::signal::{SigAction, SIGUSR1};

    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new().expect("AddressSpace::new");
    unsafe {
        aspace.activate();
    }

    let (bytes, handler_offset) = fix::signal_delivery_handler_exits_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let entry = image.entry.as_u64();

    // Tests bypass the run command; install the process slot
    // ourselves *and* pre-set the signal action so the dispatcher's
    // post-syscall hook will find a handler when the fixture's
    // kill() returns.
    // Pre-install the SIGUSR1 action via the test hook — it'll be
    // applied after `enter_user_mode_with_aspace` builds the Process
    // slot but before the iretq into ring 3.
    crate::userland::test_hooks::set_pre_iretq_signal_action(
        SIGUSR1,
        SigAction {
            sa_handler: entry + handler_offset,
            sa_flags: 0,
            sa_restorer: 0, // handler exits before returning; no restorer needed
            sa_mask: 0,
        },
    );

    let result =
        crate::userland::enter_user_mode_with_aspace(image, &["agenticos-app"], &[], Some(aspace))
            .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 42,
        "expected handler to run and exit 42, got {} (99 means signal didn't deliver)",
        result.1,
    );
}

// ---------- Tier 3: libuv plumbing ----------

fn test_dispatch_eventfd_epoll_edge_round_trip() {
    setup_phase2_active_user();
    let mut value = 1u64;
    let mut readback = 0u64;
    let mut interest = [0u8; 12];
    interest[..4].copy_from_slice(&(0x001u32 | (1u32 << 31)).to_ne_bytes());
    interest[4..].copy_from_slice(&0xfeed_beef_cafe_babeu64.to_ne_bytes());
    let mut events = [0u8; 12];
    let pointers = [
        &mut value as *mut u64 as u64,
        &mut readback as *mut u64 as u64,
        interest.as_mut_ptr() as u64,
        events.as_mut_ptr() as u64,
    ];
    abi::set_user_va_bounds(UserVaBounds {
        start: *pointers.iter().min().unwrap(),
        end: pointers[0]
            .saturating_add(8)
            .max(pointers[1].saturating_add(8))
            .max(pointers[2].saturating_add(12))
            .max(pointers[3].saturating_add(12)),
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::EVENTFD2;
    args.rsi = 0x800; // EFD_NONBLOCK
    let event_fd = syscall_dispatch(&mut args);
    assert!(event_fd >= 3);

    let mut args = SyscallArgs::default();
    args.rax = nr::EPOLL_CREATE1;
    let epoll_fd = syscall_dispatch(&mut args);
    assert!(epoll_fd >= 3);

    let mut args = SyscallArgs::default();
    args.rax = nr::EPOLL_CTL;
    args.rdi = epoll_fd as u64;
    args.rsi = 1; // EPOLL_CTL_ADD
    args.rdx = event_fd as u64;
    args.r10 = interest.as_ptr() as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::EPOLL_WAIT;
    args.rdi = epoll_fd as u64;
    args.rsi = events.as_mut_ptr() as u64;
    args.rdx = 1;
    args.r10 = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = event_fd as u64;
    args.rsi = &value as *const u64 as u64;
    args.rdx = 8;
    assert_eq!(syscall_dispatch(&mut args), 8);

    let mut args = SyscallArgs::default();
    args.rax = nr::EPOLL_WAIT;
    args.rdi = epoll_fd as u64;
    args.rsi = events.as_mut_ptr() as u64;
    args.rdx = 1;
    args.r10 = 0;
    assert_eq!(syscall_dispatch(&mut args), 1);
    assert_eq!(u32::from_ne_bytes(events[..4].try_into().unwrap()), 0x001);
    assert_eq!(
        u64::from_ne_bytes(events[4..].try_into().unwrap()),
        0xfeed_beef_cafe_babe
    );

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = event_fd as u64;
    args.rsi = &mut readback as *mut u64 as u64;
    args.rdx = 8;
    assert_eq!(syscall_dispatch(&mut args), 8);
    assert_eq!(readback, 1);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_socketpair_full_duplex() {
    setup_phase2_active_user();
    let mut fds = [-1i32; 2];
    let payload = *b"libuv-pair";
    let mut output = [0u8; 10];
    let fds_pointer = fds.as_mut_ptr() as u64;
    let payload_pointer = payload.as_ptr() as u64;
    let output_pointer = output.as_mut_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: fds_pointer.min(payload_pointer).min(output_pointer),
        end: (fds_pointer + 8)
            .max(payload_pointer + payload.len() as u64)
            .max(output_pointer + output.len() as u64),
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::SOCKETPAIR;
    args.rdi = 1; // AF_UNIX
    args.rsi = 1 | 0x800; // SOCK_STREAM | SOCK_NONBLOCK
    args.r10 = fds_pointer;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert!(fds[0] >= 3 && fds[1] >= 3 && fds[0] != fds[1]);

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = fds[0] as u64;
    args.rsi = payload_pointer;
    args.rdx = payload.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), payload.len() as i64);

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = fds[1] as u64;
    args.rsi = output_pointer;
    args.rdx = output.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), output.len() as i64);
    assert_eq!(output, payload);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_sigaltstack_membarrier_and_yield_profile() {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct StackT {
        sp: u64,
        flags: i32,
        padding: u32,
        size: u64,
    }
    setup_phase2_active_user();
    crate::userland::lifecycle::with_current_group(|process| {
        process.signal_alt_stack = crate::userland::signal::SignalAltStack::default();
        process.membarrier_private_registered = false;
    });
    let mut stack = [0u8; 4096];
    let requested = StackT {
        sp: stack.as_mut_ptr() as u64,
        flags: 0,
        padding: 0,
        size: stack.len() as u64,
    };
    let mut observed = StackT::default();
    let requested_pointer = &requested as *const StackT as u64;
    let observed_pointer = &mut observed as *mut StackT as u64;
    let stack_pointer = stack.as_mut_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: requested_pointer.min(observed_pointer).min(stack_pointer),
        end: (requested_pointer + core::mem::size_of::<StackT>() as u64)
            .max(observed_pointer + core::mem::size_of::<StackT>() as u64)
            .max(stack_pointer + stack.len() as u64),
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::SIGALTSTACK;
    args.rdi = requested_pointer;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let mut args = SyscallArgs::default();
    args.rax = nr::SIGALTSTACK;
    args.rsi = observed_pointer;
    assert_eq!(syscall_dispatch(&mut args), 0);
    assert_eq!(observed.sp, stack_pointer);
    assert_eq!(observed.size, 4096);
    assert_eq!(observed.flags, 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::MEMBARRIER;
    assert_eq!(syscall_dispatch(&mut args), (1 << 3) | (1 << 4));
    args.rdi = 1 << 3;
    assert_eq!(syscall_dispatch(&mut args), EPERM);
    args.rdi = 1 << 4;
    assert_eq!(syscall_dispatch(&mut args), 0);
    args.rdi = 1 << 3;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::SCHED_YIELD;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 5 PR-A: pipes ----------

/// Unit test for the kernel-side `Pipe` ring buffer: write some bytes,
/// read them back, verify both halves see the same data.
fn test_pipe_basic_write_then_read() {
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    let pipe = Pipe::new();
    let writer = PipeWriteHandle::new(pipe.clone(), false);
    let reader = PipeReadHandle::new(pipe, false);

    let n = writer.pipe().write(b"hello pipe");
    assert_eq!(n, 10);
    assert_eq!(reader.pipe().writers(), 1);
    assert_eq!(reader.pipe().readers(), 1);

    let mut buf = [0u8; 16];
    let read = reader.pipe().read(&mut buf);
    assert_eq!(read, 10);
    assert_eq!(&buf[..10], b"hello pipe");
}

/// Cloning a writer handle bumps the writer count; dropping it
/// decrements. Same for readers. The pipe knows when no one is left
/// on either side via these counts.
fn test_pipe_handle_clone_drop_tracks_counts() {
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    let pipe = Pipe::new();
    let writer1 = PipeWriteHandle::new(pipe.clone(), false);
    let reader1 = PipeReadHandle::new(pipe.clone(), false);
    assert_eq!(writer1.pipe().writers(), 1);
    assert_eq!(reader1.pipe().readers(), 1);

    let writer2 = writer1.clone();
    let reader2 = reader1.clone();
    assert_eq!(writer2.pipe().writers(), 2);
    assert_eq!(reader2.pipe().readers(), 2);

    drop(writer1);
    drop(reader1);
    assert_eq!(writer2.pipe().writers(), 1);
    assert_eq!(reader2.pipe().readers(), 1);

    drop(writer2);
    drop(reader2);
    assert_eq!(pipe.writers(), 0);
    assert_eq!(pipe.readers(), 0);
}

/// Buffer is bounded: writing more than `PIPE_CAPACITY` returns a
/// short write. The writer is expected to retry; we validate the
/// kernel side returns the right count.
fn test_pipe_short_write_at_capacity() {
    use crate::userland::pipe::{Pipe, PipeWriteHandle, PIPE_CAPACITY};

    let pipe = Pipe::new();
    let writer = PipeWriteHandle::new(pipe, false);

    let big: alloc::vec::Vec<u8> = alloc::vec![0xABu8; PIPE_CAPACITY + 100];
    let n = writer.pipe().write(&big);
    assert_eq!(n, PIPE_CAPACITY);

    // Subsequent write rejects (buffer full).
    let m = writer.pipe().write(b"more");
    assert_eq!(m, 0);
}

/// `pipe2(fds, 0)` allocates two fds, writes them to the user int[2],
/// and the resulting read/write fds round-trip bytes through the
/// pipe.
fn test_dispatch_pipe2_round_trip() {
    setup_phase2_active_user();

    // User buffer for the (read_fd, write_fd) pair.
    let fds_buf = [0u8; 8];
    let fds_ptr = fds_buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: fds_ptr,
        end: fds_ptr + 16, // include the data buffer below in the same window
    });

    // pipe2(&fds, 0)
    let mut args = SyscallArgs::default();
    args.rax = nr::PIPE2;
    args.rdi = fds_ptr;
    args.rsi = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let read_fd = i32::from_ne_bytes(fds_buf[0..4].try_into().unwrap());
    let write_fd = i32::from_ne_bytes(fds_buf[4..8].try_into().unwrap());
    assert!(read_fd >= 3 && write_fd >= 3 && read_fd != write_fd);

    // Use a separate buffer for the actual byte payload. Bounds need
    // to cover both the fds and the data buffer.
    let data = b"hi pipe!";
    let data_ptr = data.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(fds_ptr, data_ptr),
        end: core::cmp::max(fds_ptr + 8, data_ptr + data.len() as u64),
    });

    // write(write_fd, data, len)
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = write_fd as u64;
    args.rsi = data_ptr;
    args.rdx = data.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), data.len() as i64);

    // read(read_fd, dst, 64)
    let dst = [0u8; 64];
    let dst_ptr = dst.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: dst_ptr,
        end: dst_ptr + 64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = dst_ptr;
    args.rdx = 64;
    let n = syscall_dispatch(&mut args);
    assert_eq!(n, data.len() as i64);
    assert_eq!(&dst[..data.len()], data);

    // Close write end, then read returns EOF (0) — no writers and
    // empty buffer.
    let mut args = SyscallArgs::default();
    args.rax = nr::CLOSE;
    args.rdi = write_fd as u64;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = dst_ptr;
    args.rdx = 64;
    assert_eq!(syscall_dispatch(&mut args), 0);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `pipe2(O_NONBLOCK)` returns `EAGAIN` for an empty live pipe and exposes
/// mutable status flags through `fcntl`. Duplicates share the read endpoint's
/// open-file status, while the write endpoint remains independent.
fn test_dispatch_pipe2_nonblocking_and_fcntl_status() {
    const O_NONBLOCK: u64 = 0o4000;
    const O_WRONLY: i64 = 1;
    const F_DUPFD: u64 = 0;
    const F_GETFL: u64 = 3;
    const F_SETFL: u64 = 4;

    setup_phase2_active_user();
    let mut fds_buf = [0u8; 8];
    let fds_ptr = fds_buf.as_mut_ptr() as u64;
    let mut byte = 0u8;
    let byte_ptr = &mut byte as *mut u8 as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: core::cmp::min(fds_ptr, byte_ptr),
        end: core::cmp::max(fds_ptr + 8, byte_ptr + 1),
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::PIPE2;
    args.rdi = fds_ptr;
    args.rsi = O_NONBLOCK;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let read_fd = i32::from_ne_bytes(fds_buf[0..4].try_into().unwrap());
    let write_fd = i32::from_ne_bytes(fds_buf[4..8].try_into().unwrap());

    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = byte_ptr;
    args.rdx = 1;
    assert_eq!(syscall_dispatch(&mut args), EAGAIN);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = read_fd as u64;
    args.rsi = F_GETFL;
    assert_eq!(syscall_dispatch(&mut args), O_NONBLOCK as i64);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = write_fd as u64;
    args.rsi = F_GETFL;
    assert_eq!(syscall_dispatch(&mut args), O_WRONLY | O_NONBLOCK as i64);

    let payload = alloc::vec![0xABu8; crate::userland::pipe::PIPE_CAPACITY];
    let payload_ptr = payload.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: payload_ptr,
        end: payload_ptr + payload.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = write_fd as u64;
    args.rsi = payload_ptr;
    args.rdx = payload.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), payload.len() as i64);

    abi::set_user_va_bounds(UserVaBounds {
        start: byte_ptr,
        end: byte_ptr + 1,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = write_fd as u64;
    args.rsi = byte_ptr;
    args.rdx = 1;
    assert_eq!(syscall_dispatch(&mut args), EAGAIN);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = read_fd as u64;
    args.rsi = F_DUPFD;
    let duplicate = syscall_dispatch(&mut args);
    assert!(duplicate >= 3);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = read_fd as u64;
    args.rsi = F_SETFL;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = duplicate as u64;
    args.rsi = F_GETFL;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = write_fd as u64;
    args.rsi = F_GETFL;
    assert_eq!(syscall_dispatch(&mut args), O_WRONLY | O_NONBLOCK as i64);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// Pipe readiness changes as bytes enter the buffer: the write end starts
/// writable, and the read end becomes readable after a write.
fn test_dispatch_select_pipe_readiness() {
    #[repr(C)]
    struct Timeval {
        seconds: i64,
        microseconds: i64,
    }

    setup_phase2_active_user();
    let mut fds_buf = [0u8; 8];
    let fds_ptr = fds_buf.as_mut_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: fds_ptr,
        end: fds_ptr + 8,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::PIPE2;
    args.rdi = fds_ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let read_fd = i32::from_ne_bytes(fds_buf[0..4].try_into().unwrap());
    let write_fd = i32::from_ne_bytes(fds_buf[4..8].try_into().unwrap());

    let mut read_mask = 1u64 << read_fd;
    let mut write_mask = 1u64 << write_fd;
    let timeout = Timeval {
        seconds: 0,
        microseconds: 0,
    };
    let read_ptr = &mut read_mask as *mut u64 as u64;
    let write_ptr = &mut write_mask as *mut u64 as u64;
    let timeout_ptr = &timeout as *const Timeval as u64;
    let start = core::cmp::min(read_ptr, core::cmp::min(write_ptr, timeout_ptr));
    let end = core::cmp::max(
        read_ptr + 8,
        core::cmp::max(
            write_ptr + 8,
            timeout_ptr + core::mem::size_of::<Timeval>() as u64,
        ),
    );
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut args = SyscallArgs::default();
    args.rax = nr::SELECT;
    args.rdi = (core::cmp::max(read_fd, write_fd) + 1) as u64;
    args.rsi = read_ptr;
    args.rdx = write_ptr;
    args.r8 = timeout_ptr;
    assert_eq!(syscall_dispatch(&mut args), 1);
    assert_eq!(read_mask, 0);
    assert_eq!(write_mask, 1u64 << write_fd);

    let byte = b"x";
    let byte_ptr = byte.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: byte_ptr,
        end: byte_ptr + 1,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = write_fd as u64;
    args.rsi = byte_ptr;
    args.rdx = 1;
    assert_eq!(syscall_dispatch(&mut args), 1);

    read_mask = 1u64 << read_fd;
    abi::set_user_va_bounds(UserVaBounds { start, end });
    let mut args = SyscallArgs::default();
    args.rax = nr::SELECT;
    args.rdi = (read_fd + 1) as u64;
    args.rsi = read_ptr;
    args.r8 = timeout_ptr;
    assert_eq!(syscall_dispatch(&mut args), 1);
    assert_eq!(read_mask, 1u64 << read_fd);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// Zsh's `echo` builtin flushes prompt command-substitution output with
/// `writev`. The pipe target must accept those vectors instead of returning
/// ENOSYS and printing "write error: function not implemented" into the TTY.
fn test_dispatch_writev_pipe_round_trip() {
    #[repr(C)]
    struct TestIovec {
        base: u64,
        len: u64,
    }

    setup_phase2_active_user();

    let fds_buf = [0u8; 8];
    let fds_ptr = fds_buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: fds_ptr,
        end: fds_ptr + fds_buf.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::PIPE2;
    args.rdi = fds_ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);
    let read_fd = i32::from_ne_bytes(fds_buf[0..4].try_into().unwrap());
    let write_fd = i32::from_ne_bytes(fds_buf[4..8].try_into().unwrap());

    let first = b"prompt_";
    let second = b"segment";
    let iovecs = [
        TestIovec {
            base: first.as_ptr() as u64,
            len: first.len() as u64,
        },
        TestIovec {
            base: second.as_ptr() as u64,
            len: second.len() as u64,
        },
    ];
    let iov_ptr = iovecs.as_ptr() as u64;
    let start = core::cmp::min(
        iov_ptr,
        core::cmp::min(first.as_ptr() as u64, second.as_ptr() as u64),
    );
    let end = core::cmp::max(
        iov_ptr + core::mem::size_of_val(&iovecs) as u64,
        core::cmp::max(
            first.as_ptr() as u64 + first.len() as u64,
            second.as_ptr() as u64 + second.len() as u64,
        ),
    );
    abi::set_user_va_bounds(UserVaBounds { start, end });

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITEV;
    args.rdi = write_fd as u64;
    args.rsi = iov_ptr;
    args.rdx = iovecs.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), 14);

    let dst = [0u8; 32];
    let dst_ptr = dst.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: dst_ptr,
        end: dst_ptr + dst.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = read_fd as u64;
    args.rsi = dst_ptr;
    args.rdx = dst.len() as u64;
    assert_eq!(syscall_dispatch(&mut args), 14);
    assert_eq!(&dst[..14], b"prompt_segment");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

// ---------- Phase 4 PR-D: execve negative path ----------

/// Parent forks; child execve's a non-existent path; execve returns
/// `-ENOENT`; child treats that as failure and exit_group(11); parent
/// wait4s and exits 11.
fn test_fork_execve_badpath_returns_to_parent() {
    use crate::userland::lifecycle::ExitKind;
    reset_active_user();

    let aspace = crate::userland::address_space::AddressSpace::new().expect("AddressSpace::new");
    unsafe {
        aspace.activate();
    }

    let bytes = fix::fork_execve_badpath_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result =
        crate::userland::enter_user_mode_with_aspace(image, &["agenticos-app"], &[], Some(aspace))
            .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 11,
        "parent's final exit code should be 11, got {}",
        result.1
    );
}

// ---------- Phase 4 PR-C2: fork + wait4 round trip ----------

/// End-to-end fork test: a hand-rolled binary forks, child exits with
/// code 42, parent wait4s the child and then exits with code 7.
///
/// Asserts:
/// - The top-level cooperative exit recorded code 7 (parent's exit).
/// - The child's 42 was parked in `LAST_EXIT_CODE` at some point but
///   then overwritten by the parent's final exit — we don't observe
///   42 here directly.
/// - No abnormal exit.
fn test_fork_then_wait_returns_to_parent() {
    use crate::userland::lifecycle::ExitKind;

    reset_active_user();

    // The fixture's binary lives in PML4[0] of the active L4. The test
    // drives `enter_user_mode_with` which today still routes through
    // the kernel L4 (no AddressSpace passed), but fork() needs a
    // real address space to clone from. Build one and activate it.
    let aspace = crate::userland::address_space::AddressSpace::new()
        .expect("AddressSpace::new for fork fixture");
    // SAFETY: kernel half copied — kernel code post-CR3-write mapped.
    unsafe {
        aspace.activate();
    }

    let bytes = fix::fork_then_wait_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let result =
        crate::userland::enter_user_mode_with_aspace(image, &["agenticos-app"], &[], Some(aspace))
            .expect("enter_user_mode_with_aspace");
    let _ = crate::userland::release_active_image();

    assert!(matches!(result.0, ExitKind::Cooperative));
    assert_eq!(
        result.1, 7,
        "parent's final exit code should be 7, got {}",
        result.1
    );
}

// ---------- Phase 4 PR-C: AddressSpace clone (foundation for fork) ----------

/// `AddressSpace::clone_for_child` shares resident leaves read-only and
/// marks writable mappings COW. Resolving the child's first write must
/// allocate private backing without changing the parent's contents.
///
/// Drives the clone path on the active user L4 — we activate a parent
/// address space, map a single page, write a magic byte pattern, then
/// clone. After switching to the child, the same VA reads the magic;
/// after switching back to parent and overwriting, switching to the
/// child again reads the original magic (independent backing).
fn test_address_space_clone_for_child_uses_cow() {
    use crate::mm::paging::{CowOutcome, UserPerms, USER_LOAD_BASE};
    use crate::userland::address_space::AddressSpace;
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::PageTableFlags;
    use x86_64::VirtAddr;

    let kernel_frame = crate::mm::paging::kernel_l4_frame().expect("kernel L4 captured at boot");

    let parent = AddressSpace::new().expect("parent AddressSpace::new");
    // SAFETY: kernel half copied — kernel code post-CR3-write is mapped.
    unsafe {
        parent.activate();
    }

    // Map one user page in the parent and write a magic value.
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(USER_LOAD_BASE), 1, UserPerms::ReadWrite)
            .expect("parent map");
    });
    let parent_va = USER_LOAD_BASE as *mut u32;
    unsafe {
        core::ptr::write_volatile(parent_va, 0xAABB_CCDD);
    }

    // Build the child by cloning the parent's L4. Stay on parent's L4
    // for the clone walk — `clone_for_child` reads parent's tables.
    let child = AddressSpace::clone_for_child(parent.l4_frame()).expect("clone_for_child");
    assert_ne!(child.l4_frame(), parent.l4_frame());
    assert_ne!(child.l4_frame(), kernel_frame);

    let (parent_frame, parent_flags) = crate::mm::memory::with_memory_mapper(|m| {
        m.leaf_info(parent.l4_frame(), VirtAddr::new(USER_LOAD_BASE))
            .expect("parent leaf")
    })
    .expect("memory mapper");
    let (child_frame, child_flags) = crate::mm::memory::with_memory_mapper(|m| {
        m.leaf_info(child.l4_frame(), VirtAddr::new(USER_LOAD_BASE))
            .expect("child leaf")
    })
    .expect("memory mapper");
    assert_eq!(
        parent_frame, child_frame,
        "fork must initially share backing"
    );
    assert!(parent_flags.contains(PageTableFlags::BIT_9));
    assert!(child_flags.contains(PageTableFlags::BIT_9));
    assert!(!parent_flags.contains(PageTableFlags::WRITABLE));
    assert!(!child_flags.contains(PageTableFlags::WRITABLE));
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| m.frame_refcount(parent_frame))
            .expect("memory mapper"),
        Some(2)
    );

    // Activate the child. The shared page should read the magic value.
    unsafe {
        child.activate();
    }
    let child_val = unsafe { core::ptr::read_volatile(parent_va) };
    assert_eq!(child_val, 0xAABB_CCDD, "child must inherit parent's data");

    // Simulate the child's write fault, then modify its now-private page.
    assert_eq!(
        crate::mm::memory::with_memory_mapper(|m| {
            m.resolve_cow(child.l4_frame(), VirtAddr::new(USER_LOAD_BASE))
        })
        .expect("memory mapper"),
        CowOutcome::Copied
    );
    unsafe {
        core::ptr::write_volatile(parent_va, 0x1111_2222);
    }
    unsafe {
        parent.activate();
    }
    let parent_val = unsafe { core::ptr::read_volatile(parent_va) };
    assert_eq!(
        parent_val, 0xAABB_CCDD,
        "parent must not see child's writes after COW resolution"
    );

    // Cleanup: drop both address spaces. Drop reverts CR3 to kernel L4.
    drop(child);
    drop(parent);
    let (final_cr3, _) = Cr3::read();
    assert_eq!(final_cr3, kernel_frame);
}

// ---------- Phase 4 PR-A: Process table ----------

/// `getpid()` returns the kernel sentinel (0) when no user process is
/// active and a real positive PID after a launch.
fn test_getpid_returns_real_pid() {
    reset_active_user();
    // No active process → PID is the kernel sentinel.
    let mut args = SyscallArgs::default();
    args.rax = nr::GETPID;
    let kernel_pid = syscall_dispatch(&mut args);
    assert_eq!(kernel_pid, 0, "no active process must report PID 0");

    // Launch a happy-path binary; getpid during its run is observed
    // through the lifecycle's `current_pid()` (the dispatch-during-
    // ring-3 path mirrors the kernel-side current_pid).
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _result = crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
        .expect("enter_user_mode_with");
    // The active-user slot still carries the PID until release.
    let pid_after = crate::userland::lifecycle::with_current_process(|p| p.pid);
    assert!(pid_after >= 1, "expected ≥1, got {}", pid_after);
    let _ = crate::userland::release_active_image();

    // After release, PID resets.
    let mut args = SyscallArgs::default();
    args.rax = nr::GETPID;
    assert_eq!(syscall_dispatch(&mut args), 0);
}

/// PIDs are monotonic — successive `enter_user_mode_with` calls
/// produce strictly increasing PIDs.
fn test_pid_allocation_is_monotonic() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();

    let image1 = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image1, &["agenticos-app"], &[])
        .expect("enter_user_mode_with #1");
    let pid1 = crate::userland::lifecycle::with_current_process(|p| p.pid);
    let _ = crate::userland::release_active_image();

    let image2 = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image2, &["agenticos-app"], &[])
        .expect("enter_user_mode_with #2");
    let pid2 = crate::userland::lifecycle::with_current_process(|p| p.pid);
    let _ = crate::userland::release_active_image();

    assert!(
        pid2 > pid1,
        "PIDs must be monotonic: pid1={} pid2={}",
        pid1,
        pid2
    );
}

/// `getppid()` returns 0 (kernel sentinel) for binaries launched by
/// the run command — they're "kernel-parented" until fork() lands.
fn test_getppid_returns_kernel_sentinel() {
    reset_active_user();
    let bytes = fix::hello_exit0_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let _ = crate::userland::enter_user_mode_with(image, &["agenticos-app"], &[])
        .expect("enter_user_mode_with");
    let parent_pid = crate::userland::lifecycle::with_current_process(|p| p.parent_pid);
    assert_eq!(parent_pid, 0);
    let _ = crate::userland::release_active_image();
}

// ---------- Phase 3: TTY ----------

fn test_termios_default_is_canonical_with_echo() {
    use crate::userland::tty::{self, ECHO, ICANON};
    tty::install_default();
    let t = tty::snapshot();
    assert_ne!(t.c_lflag & ICANON, 0, "default must be canonical mode");
    assert_ne!(t.c_lflag & ECHO, 0, "default must echo");
    assert!(tty::is_canonical());
    assert!(tty::is_echo());
}

fn test_dispatch_ioctl_tcgets_returns_termios() {
    setup_phase2_active_user();
    crate::userland::tty::install_default();

    let buf = [0u8; 36];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 0; // stdin
    args.rsi = 0x5401; // TCGETS
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    // c_lflag at offset 12 (LE u32). It should be non-zero — at minimum
    // ICANON | ECHO are set in default termios.
    let lflag_bytes: [u8; 4] = buf[12..16].try_into().unwrap();
    let lflag = u32::from_ne_bytes(lflag_bytes);
    assert_ne!(lflag & 0o000002, 0, "ICANON bit should be set");
    assert_ne!(lflag & 0o000010, 0, "ECHO bit should be set");

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_tcsets_updates_termios() {
    use crate::userland::tty::{self, ECHO, ICANON};
    setup_phase2_active_user();
    tty::install_default();

    // Build a raw-ish termios in user memory: clear ICANON|ECHO|ISIG.
    let mut buf = [0u8; 36];
    // c_iflag = 0
    // c_oflag = 0
    // c_cflag = 0
    // c_lflag = 0 (raw)
    // c_line + c_cc default 0 — VMIN=1 expected by zsh's raw mode but
    // the kernel doesn't honor it today.
    let _ = (&mut buf, 0u32);

    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 0;
    args.rsi = 0x5402; // TCSETS
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let t = tty::snapshot();
    assert_eq!(t.c_lflag & ICANON, 0, "ICANON should be cleared");
    assert_eq!(t.c_lflag & ECHO, 0, "ECHO should be cleared");
    assert!(!tty::is_canonical());
    assert!(!tty::is_echo());

    // Restore default so subsequent tests stay in canonical mode.
    tty::install_default();
    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_on_file_returns_enotty() {
    use crate::userland::abi::ENOTTY;
    setup_phase2_active_user();
    // Synthetic file slot — using stdin marker as a fake non-tty slot
    // wouldn't actually trigger ENOTTY (Stdin is treated as tty). Drop
    // a directory slot instead via open() on /host if available.
    if !crate::fs::exists("/host") {
        teardown_phase2_active_user();
        return;
    }
    let path = b"/host\0";
    let pp = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: pp,
        end: pp + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pp;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    let buf = [0u8; 36];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = fd as u64;
    args.rsi = 0x5401;
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), ENOTTY);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

fn test_dispatch_ioctl_tiocgwinsz_returns_80x24() {
    setup_phase2_active_user();
    let buf = [0u8; 8];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 1; // stdout
    args.rsi = 0x5413; // TIOCGWINSZ
    args.rdx = ptr;
    assert_eq!(syscall_dispatch(&mut args), 0);

    let row = u16::from_ne_bytes(buf[0..2].try_into().unwrap());
    let col = u16::from_ne_bytes(buf[2..4].try_into().unwrap());
    assert_eq!(row, 24);
    assert_eq!(col, 80);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// U5: `ioctl(stdin, TIOCGPGRP, ...)` returns -ENOTTY. Zsh's
/// `acquire_pgrp` reads this as "no controlling tty" and clears the
/// MONITOR option, suppressing every subsequent setpgid/tcsetpgrp call.
/// Critical: the arm sits inside the request match — the non-tty
/// short-circuit only fires on file fds, not on stdin.
fn test_dispatch_ioctl_tiocgpgrp_returns_enotty() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 0; // stdin
    args.rsi = 0x540F; // TIOCGPGRP
    args.rdx = 0; // arg ignored — we never touch user memory
    assert_eq!(syscall_dispatch(&mut args), ENOTTY);
    teardown_phase2_active_user();
}

/// U5: `ioctl(stdout, TIOCSPGRP, ...)` returns 0 (silent success).
/// Defensive — zsh shouldn't reach this path with MONITOR cleared,
/// but the stub avoids surprises if a build configuration somehow does.
fn test_dispatch_ioctl_tiocspgrp_returns_zero() {
    setup_phase2_active_user();
    let mut args = SyscallArgs::default();
    args.rax = nr::IOCTL;
    args.rdi = 1; // stdout
    args.rsi = 0x5410; // TIOCSPGRP
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);
    teardown_phase2_active_user();
}

// ---------- Phase 2 PR-4: directories + getdents64 ----------

/// `open("/host")` must succeed (host folder mount root) and produce a
/// directory fd, not an EISDIR refusal. Skipped when the host mount
/// isn't present (e.g. some test rigs run without `-fsdev`).
fn test_dispatch_open_host_directory_succeeds() {
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();
    let path = b"/host\0";
    let ptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + path.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = ptr;
    args.rsi = 0; // O_RDONLY
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3, "expected dir fd ≥ 3, got {}", fd);

    // It must report as a directory in the FD table.
    let is_dir = crate::userland::lifecycle::with_active_user(|au| {
        matches!(au.fd_table.get(fd as i32), Some(FdSlot::Directory { .. }))
    });
    assert!(is_dir);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `read(dirfd, …)` returns `-EISDIR`. Userland is supposed to call
/// `getdents64` instead.
fn test_dispatch_read_on_directory_returns_eisdir() {
    use crate::userland::abi::EISDIR;
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();

    let path = b"/host\0";
    let pptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: pptr,
        end: pptr + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pptr;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    let buf = [0u8; 64];
    let bptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: bptr,
        end: bptr + 64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::READ;
    args.rdi = fd as u64;
    args.rsi = bptr;
    args.rdx = 64;
    assert_eq!(syscall_dispatch(&mut args), EISDIR);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// `getdents64(dirfd, buf, count)` returns at least one record on
/// `/host` (which the host-mount integration places at least
/// `HELLOCPP.ELF` and `HELLO.ELF` into). Skipped without the mount.
fn test_dispatch_getdents64_emits_records() {
    if !crate::fs::exists("/host") {
        return;
    }
    setup_phase2_active_user();

    // open
    let path = b"/host\0";
    let pptr = path.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: pptr,
        end: pptr + path.len() as u64,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::OPEN;
    args.rdi = pptr;
    args.rsi = 0;
    let fd = syscall_dispatch(&mut args);
    assert!(fd >= 3);
    abi::clear_user_va_bounds();

    // getdents64
    let buf = [0u8; 1024];
    let bptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: bptr,
        end: bptr + 1024,
    });
    let mut args = SyscallArgs::default();
    args.rax = nr::GETDENTS64;
    args.rdi = fd as u64;
    args.rsi = bptr;
    args.rdx = 1024;
    let written = syscall_dispatch(&mut args);
    assert!(
        written > 0,
        "getdents64 should emit at least one record, got {}",
        written
    );

    // Walk the records: first u64 is d_ino (should be non-zero), bytes
    // 16/17 are reclen (LE u16), 18 is d_type, name starts at 19.
    let mut off = 0usize;
    let mut count = 0usize;
    while off < written as usize {
        let reclen_bytes: [u8; 2] = buf[off + 16..off + 18].try_into().unwrap();
        let reclen = u16::from_ne_bytes(reclen_bytes) as usize;
        assert!(reclen > 19 && reclen <= written as usize - off);
        assert_eq!(
            reclen % 8,
            0,
            "reclen must be 8-byte aligned, got {}",
            reclen
        );
        // d_ino non-zero
        let ino_bytes: [u8; 8] = buf[off..off + 8].try_into().unwrap();
        assert_ne!(u64::from_ne_bytes(ino_bytes), 0, "d_ino must be non-zero");
        count += 1;
        off += reclen;
    }
    assert!(count >= 1);

    abi::clear_user_va_bounds();
    teardown_phase2_active_user();
}

/// FAT subdirectory walk: `metadata("/host/HELLOCPP.ELF")` and a
/// nested-equivalent path should both resolve when both are present.
/// On the boot disk we know `/banner.bmp` exists at root; deeper
/// paths exist on the host mount when staged by `build.sh`.
fn test_fat_metadata_subdirectory_tolerated() {
    // Sanity: the root path must work (regression of the simple case).
    let _ = crate::fs::metadata("/banner.bmp");

    // A nested path that is known to exist on the dev host's
    // `host_share` directory. If the staging didn't put a nested file
    // there, just skip — we don't fail the test for missing fixtures.
    if crate::fs::exists("/host/HELLOCPP.ELF") {
        let m = crate::fs::metadata("/host/HELLOCPP.ELF").expect("HELLOCPP.ELF metadata");
        assert!(m.size > 0);
    }

    // Negative: a deeply-nested nonexistent path must report not-found,
    // not a panic from the new walker.
    assert!(crate::fs::metadata("/host/no/such/dir/file.txt").is_err());
}

/// Regression: `write(1, …)` must not silently drop a buffer that
/// contains non-UTF-8 bytes. The original implementation strict-decoded
/// the slice and dropped the entire call on any invalid byte, which
/// made `cat` of a binary file print nothing instead of replacement
/// characters. The handler now uses lossy decoding.
fn test_write_handler_non_utf8_returns_full_len() {
    install_streams_for_dispatcher_test();
    let buf: [u8; 8] = [0x7F, b'E', b'L', b'F', 0xFF, 0xFE, 0x00, 0x42];
    let ptr = buf.as_ptr() as u64;
    abi::set_user_va_bounds(UserVaBounds {
        start: ptr,
        end: ptr + buf.len() as u64,
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::WRITE;
    args.rdi = 1; // stdout
    args.rsi = ptr;
    args.rdx = buf.len() as u64;
    let ret = syscall_dispatch(&mut args);
    assert_eq!(
        ret,
        buf.len() as i64,
        "write must accept the full slice even with non-UTF-8 bytes"
    );

    abi::clear_user_va_bounds();
    clear_streams_after_dispatcher_test();
}

fn test_dispatch_fcntl_getfd_setfd_roundtrip() {
    setup_phase2_active_user();
    // Allocate a synthetic file slot so we have something to set FD_CLOEXEC on.
    let fd = crate::userland::lifecycle::with_active_user(|au| {
        au.fd_table.alloc(FdSlot::Stdin).unwrap() // marker — fcntl flags only affect File slots
    });

    let mut args = SyscallArgs::default();
    args.rax = nr::FCNTL;
    args.rdi = fd as u64;
    args.rsi = 1; // F_GETFD
    assert_eq!(syscall_dispatch(&mut args), 0);

    args.rsi = 2; // F_SETFD
    args.rdx = 1; // FD_CLOEXEC
    assert_eq!(syscall_dispatch(&mut args), 0);
    // Stream slots ignore the flag — F_GETFD still returns 0.
    args.rsi = 1;
    args.rdx = 0;
    assert_eq!(syscall_dispatch(&mut args), 0);

    teardown_phase2_active_user();
}

fn test_run_leak_loop_fault() {
    for _ in 0..3 {
        reset_active_user();
        let bytes = fix::fault_ud_elf();
        let image = load_elf(&bytes).expect("load_elf in fault leak loop");
        let result =
            crate::userland::enter_user_mode(image).expect("enter_user_mode in fault leak loop");
        let _ = crate::userland::release_active_image();
        assert!(matches!(
            result.0,
            crate::userland::lifecycle::ExitKind::Abnormal { .. }
        ));
    }
}

// --- CLAUDE.md "Deferred" #2: rt_sigreturn restores blocked mask ---

/// POSIX handler-mask formula: old | sa_mask | bit(signum), with
/// SIGKILL/SIGSTOP always stripped.
fn test_handler_blocked_mask_includes_sa_mask_and_signum() {
    use crate::userland::signal::{SIGUSR1, SIGUSR2};
    use crate::userland::syscalls::handler_blocked_mask;

    // Empty pre-mask, sa_mask = SIGUSR2, signum = SIGUSR1.
    // Expect both bits set.
    let m = handler_blocked_mask(0, 1u64 << (SIGUSR2 - 1), SIGUSR1);
    assert_ne!(m & (1u64 << (SIGUSR1 - 1)), 0, "signum bit must be added");
    assert_ne!(m & (1u64 << (SIGUSR2 - 1)), 0, "sa_mask bit must be added");
}

/// Pre-existing blocked bits are preserved (we OR, not replace).
fn test_handler_blocked_mask_preserves_old_bits() {
    use crate::userland::signal::{SIGHUP, SIGUSR1, SIGUSR2};
    use crate::userland::syscalls::handler_blocked_mask;

    let old = 1u64 << (SIGHUP - 1);
    let m = handler_blocked_mask(old, 1u64 << (SIGUSR2 - 1), SIGUSR1);
    assert_ne!(
        m & (1u64 << (SIGHUP - 1)),
        0,
        "pre-existing block must persist"
    );
    assert_ne!(m & (1u64 << (SIGUSR1 - 1)), 0);
    assert_ne!(m & (1u64 << (SIGUSR2 - 1)), 0);
}

/// SIGKILL and SIGSTOP are stripped even if a misbehaving sa_mask
/// includes them — POSIX guarantees they're never blocked.
fn test_handler_blocked_mask_strips_kill_and_stop() {
    use crate::userland::signal::{SIGKILL, SIGSTOP, SIGUSR1};
    use crate::userland::syscalls::handler_blocked_mask;

    let evil_sa_mask = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
    let m = handler_blocked_mask(0, evil_sa_mask, SIGUSR1);
    assert_eq!(
        m & (1u64 << (SIGKILL - 1)),
        0,
        "SIGKILL must not be blocked"
    );
    assert_eq!(
        m & (1u64 << (SIGSTOP - 1)),
        0,
        "SIGSTOP must not be blocked"
    );
    assert_ne!(m & (1u64 << (SIGUSR1 - 1)), 0, "signum bit still added");
}

/// Delivering SIGUSR1 from the maybe_deliver_signal preparation path
/// must atomically install the handler mask on Process. This is the
/// integrity check for the new lock-held mask-install in
/// maybe_deliver_signal — without it the saved_blocked passed to
/// deliver_signal would never get a chance to be different from the
/// current blocked, and rt_sigreturn would restore the same mask it
/// was about to install. The fix's whole point is the atomicity.
fn test_maybe_deliver_signal_installs_handler_mask() {
    use crate::userland::lifecycle::with_current_process;
    use crate::userland::signal::{SigAction, SIGUSR1, SIGUSR2};
    use crate::userland::syscalls::prepare_deliverable_signal;

    let snap = with_current_process(|p| {
        let snap = (
            p.signal_state.blocked,
            p.signal_state.pending,
            p.signal_state.suspend_restore_mask,
        );
        // Install a SIG_DFL-but-not-DFL action so consume_deliverable
        // returns it. We use sa_handler = 0xDEAD so consume_deliverable
        // sees it as a real handler. We never actually dispatch — the
        // test pulls the consume+install path apart by calling the
        // pieces directly.
        let action = SigAction {
            sa_handler: 0xDEAD_BEEF,
            sa_flags: 0,
            sa_restorer: 0xCAFE_BABE,
            sa_mask: 1u64 << (SIGUSR2 - 1),
        };
        p.signal_state.set_action(SIGUSR1, action);
        p.signal_state.blocked = 0;
        p.signal_state.raise(SIGUSR1);
        snap
    });

    let prepared = prepare_deliverable_signal();

    let (sig, action, old_blocked) = prepared.expect("a deliverable signal was queued");
    assert_eq!(sig, SIGUSR1);
    assert_eq!(action.sa_handler, 0xDEAD_BEEF);
    assert_eq!(
        old_blocked, 0,
        "saved blocked must be the pre-install value"
    );

    with_current_process(|p| {
        // Handler mask installed.
        assert_ne!(p.signal_state.blocked & (1u64 << (SIGUSR1 - 1)), 0);
        assert_ne!(p.signal_state.blocked & (1u64 << (SIGUSR2 - 1)), 0);
    });

    // Simulate the rt_sigreturn restore.
    with_current_process(|p| {
        p.signal_state.blocked = old_blocked;
    });
    with_current_process(|p| {
        assert_eq!(
            p.signal_state.blocked, 0,
            "restore returns to pre-delivery mask"
        );
    });

    // Cleanup: reset the action and Process state.
    with_current_process(|p| {
        p.signal_state.set_action(SIGUSR1, SigAction::default());
        p.signal_state.blocked = snap.0;
        p.signal_state.pending = snap.1;
        p.signal_state.suspend_restore_mask = snap.2;
    });
}

/// A signal that completes `rt_sigsuspend` runs under the temporary
/// suspension mask, but its signal frame must restore the original mask.
fn test_prepare_deliverable_signal_restores_pre_sigsuspend_mask() {
    use crate::userland::lifecycle::with_current_process;
    use crate::userland::signal::{SigAction, SIGCHLD, SIGUSR1, SIGUSR2};
    use crate::userland::syscalls::prepare_deliverable_signal;

    let original_mask = 1u64 << (SIGCHLD - 1);
    let temporary_mask = 1u64 << (SIGUSR2 - 1);
    with_current_process(|p| {
        p.signal_state.set_action(
            SIGUSR1,
            SigAction {
                sa_handler: 0xDEAD_BEEF,
                ..SigAction::default()
            },
        );
        p.signal_state.blocked = temporary_mask;
        p.signal_state.pending = 0;
        p.signal_state.raise(SIGUSR1);
        p.signal_state.suspend_restore_mask = Some(original_mask);
    });

    let (sig, _, restore_mask) =
        prepare_deliverable_signal().expect("suspended signal should be deliverable");
    assert_eq!(sig, SIGUSR1);
    assert_eq!(restore_mask, original_mask);
    with_current_process(|p| {
        assert_eq!(p.signal_state.suspend_restore_mask, None);
        assert_ne!(p.signal_state.blocked & temporary_mask, 0);
        assert_ne!(p.signal_state.blocked & (1u64 << (SIGUSR1 - 1)), 0);

        p.signal_state.set_action(SIGUSR1, SigAction::default());
        p.signal_state.blocked = 0;
        p.signal_state.pending = 0;
    });
}

// --- U4: try_grow_user_stack classification + mutation ---

/// Helper: drop into Process and stage a stack window. Returns the
/// snapshot so the caller can restore after the test.
fn stage_stack_window(top: u64, bottom: u64, floor: u64, budget: u64) -> (u64, u64, u64, u64, u64) {
    use crate::userland::lifecycle::with_current_process;
    with_current_process(|p| {
        let snap = (
            p.stack_top,
            p.stack_bottom,
            p.stack_mapped_bottom,
            p.stack_max_growth_floor,
            p.growth_faults_remaining,
        );
        p.stack_top = top;
        p.stack_bottom = bottom;
        p.stack_mapped_bottom = bottom;
        p.stack_max_growth_floor = floor;
        p.growth_faults_remaining = budget;
        snap
    })
}

fn restore_stack_window(snap: (u64, u64, u64, u64, u64)) {
    use crate::userland::lifecycle::with_current_process;
    with_current_process(|p| {
        p.stack_top = snap.0;
        p.stack_bottom = snap.1;
        p.stack_mapped_bottom = snap.2;
        p.stack_max_growth_floor = snap.3;
        p.growth_faults_remaining = snap.4;
    });
}

/// A fault one page below the current bottom, well above the floor and
/// with budget remaining, returns `Grew` and updates stack_bottom.
fn test_try_grow_user_stack_grew() {
    use crate::mm::paging::USER_STACK_TOP;
    use crate::userland::lifecycle::{try_grow_user_stack, with_current_process, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 64 * 0x1000;
    // Map the initial commit so the fault handler's lookup of an existing
    // mapping (when it'd grow into it) doesn't surprise us. Then test the
    // grow path on the page below it.
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(
            VirtAddr::new(bottom),
            8,
            crate::mm::paging::UserPerms::ReadWrite,
        )
    })
    .unwrap()
    .unwrap();

    let snap = stage_stack_window(top, bottom, floor, 100);

    let fault_addr = VirtAddr::new(bottom - 0x800); // mid-page below bottom
    let outcome = try_grow_user_stack(fault_addr);
    assert_eq!(outcome, GrowOutcome::Grew);

    with_current_process(|p| {
        let expected_new_page = (bottom - 0x800) & !0xFFF;
        assert_eq!(p.stack_bottom, expected_new_page);
        assert_eq!(p.stack_mapped_bottom, expected_new_page);
        assert_eq!(p.growth_faults_remaining, 99);
    });

    // Cleanup: unmap the grown stack.
    crate::userland::lifecycle::with_current_process(crate::userland::lifecycle::unmap_user_stack);
    restore_stack_window(snap);
}

/// A fault below the growth floor returns `Overflow` and does not
/// mutate stack_bottom or call the mapper.
fn test_try_grow_user_stack_overflow_below_floor() {
    use crate::mm::paging::USER_STACK_TOP;
    use crate::userland::lifecycle::{try_grow_user_stack, with_current_process, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 16 * 0x1000;
    let snap = stage_stack_window(top, bottom, floor, 100);

    let fault_addr = VirtAddr::new(floor - 0x100);
    assert_eq!(try_grow_user_stack(fault_addr), GrowOutcome::Overflow);

    // Bookkeeping unchanged.
    with_current_process(|p| {
        assert_eq!(p.stack_bottom, bottom);
        assert_eq!(p.stack_mapped_bottom, bottom);
        assert_eq!(p.growth_faults_remaining, 100);
    });

    restore_stack_window(snap);
}

/// A fault inside the window but with budget==0 returns
/// `BudgetExhausted` and does not call the mapper.
fn test_try_grow_user_stack_budget_exhausted() {
    use crate::mm::paging::USER_STACK_TOP;
    use crate::userland::lifecycle::{try_grow_user_stack, with_current_process, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 64 * 0x1000;
    let snap = stage_stack_window(top, bottom, floor, 0);

    let fault_addr = VirtAddr::new(bottom - 0x800);
    assert_eq!(
        try_grow_user_stack(fault_addr),
        GrowOutcome::BudgetExhausted
    );

    with_current_process(|p| {
        assert_eq!(p.stack_bottom, bottom);
        assert_eq!(p.growth_faults_remaining, 0);
    });

    restore_stack_window(snap);
}

/// An unmapped page inside the stack window may sit above the low-water mark.
/// This happens after inherited/partial mappings; filling the hole must not
/// lower the contiguous stack bookkeeping.
fn test_try_grow_user_stack_fills_gap_above_bottom() {
    use crate::mm::paging::UserPerms;
    use crate::mm::paging::USER_STACK_TOP;
    use crate::userland::lifecycle::{try_grow_user_stack, with_current_process, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 64 * 0x1000;
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(bottom), 8, UserPerms::ReadWrite)
    })
    .unwrap()
    .unwrap();
    let gap_page = bottom + 0x2000;
    crate::mm::memory::with_memory_mapper(|m| m.unmap_user_region(VirtAddr::new(gap_page), 1))
        .unwrap()
        .unwrap();
    let snap = stage_stack_window(top, bottom, floor, 100);

    assert_eq!(
        try_grow_user_stack(VirtAddr::new(gap_page + 0x100)),
        GrowOutcome::Grew,
    );
    with_current_process(|p| {
        assert_eq!(p.stack_bottom, bottom);
        assert_eq!(p.stack_mapped_bottom, bottom);
        assert_eq!(p.growth_faults_remaining, 99);
    });

    crate::userland::lifecycle::with_current_process(crate::userland::lifecycle::unmap_user_stack);
    restore_stack_window(snap);
}

/// Already-mapped in-window pages are protection faults, not growth, and
/// addresses above stack_top are outside the stack window entirely.
fn test_try_grow_user_stack_rejects_mapped_and_out_of_range_pages() {
    use crate::mm::paging::{UserPerms, USER_STACK_TOP};
    use crate::userland::lifecycle::{try_grow_user_stack, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 64 * 0x1000;
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(VirtAddr::new(bottom), 8, UserPerms::ReadWrite)
    })
    .unwrap()
    .unwrap();
    let snap = stage_stack_window(top, bottom, floor, 100);

    assert_eq!(
        try_grow_user_stack(VirtAddr::new(bottom + 0x100)),
        GrowOutcome::NotStackGrow,
    );
    assert_eq!(
        try_grow_user_stack(VirtAddr::new(top + 0x10000)),
        GrowOutcome::NotStackGrow,
    );

    crate::userland::lifecycle::with_current_process(crate::userland::lifecycle::unmap_user_stack);
    restore_stack_window(snap);
}

/// Sentinel slot (no active process): try_grow returns NotStackGrow so
/// the caller routes through its normal heap/stack path.
fn test_try_grow_user_stack_sentinel_is_not_stack_grow() {
    use crate::userland::lifecycle::{try_grow_user_stack, GrowOutcome};
    use x86_64::VirtAddr;

    // Sentinel state — all stack fields zero.
    let snap = stage_stack_window(0, 0, 0, 0);

    assert_eq!(
        try_grow_user_stack(VirtAddr::new(0x7F_0000)),
        GrowOutcome::NotStackGrow,
    );

    restore_stack_window(snap);
}

/// After a successful growth, set_user_va_bounds widens the start so
/// validate_user_slice accepts pointers into the freshly mapped page.
fn test_try_grow_user_stack_widens_validated_bounds() {
    use crate::mm::paging::USER_STACK_TOP;
    use crate::userland::abi::{
        clear_user_va_bounds, set_user_va_bounds, user_va_bounds, validate_user_slice, UserVaBounds,
    };
    use crate::userland::lifecycle::{try_grow_user_stack, with_current_process, GrowOutcome};
    use x86_64::VirtAddr;

    let top = USER_STACK_TOP;
    let bottom = top - 8 * 0x1000;
    let floor = top - 64 * 0x1000;
    // Map the initial commit so growth can lower below it cleanly.
    crate::mm::memory::with_memory_mapper(|m| {
        m.map_user_region(
            VirtAddr::new(bottom),
            8,
            crate::mm::paging::UserPerms::ReadWrite,
        )
    })
    .unwrap()
    .unwrap();
    let stack_snap = stage_stack_window(top, bottom, floor, 100);
    set_user_va_bounds(UserVaBounds {
        start: bottom,
        end: top,
    });

    let new_page = (bottom - 0x800) & !0xFFF;
    // Before grow: a pointer at new_page is rejected.
    assert!(validate_user_slice(new_page, 8).is_err());

    let outcome = try_grow_user_stack(VirtAddr::new(bottom - 0x800));
    assert_eq!(outcome, GrowOutcome::Grew);

    // After grow: bounds.start is new_page; the pointer passes.
    let b = user_va_bounds().expect("bounds set");
    assert_eq!(b.start, new_page);
    assert!(validate_user_slice(new_page, 8).is_ok());

    with_current_process(crate::userland::lifecycle::unmap_user_stack);
    restore_stack_window(stack_snap);
    clear_user_va_bounds();
}

// --- U3: install_new_process_opt populates Process stack-window ---

/// After install_new_process_opt the Process slot reports the loader's
/// stack window: top = USER_STACK_TOP, bottom == mapped_bottom ==
/// initial commit bottom, and a full growth budget.
fn test_install_new_process_populates_stack_window() {
    use crate::mm::paging::{
        USER_BRK_BASE, USER_MMAP_BASE, USER_STACK_INITIAL_PAGES, USER_STACK_MAX_GROWTH_PAGES,
        USER_STACK_TOP,
    };
    use crate::userland::lifecycle::{install_new_process_opt, with_current_process};

    reset_active_user();

    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("load_elf");
    let pid = install_new_process_opt(image, USER_BRK_BASE, USER_MMAP_BASE, None);

    let expected_bottom = USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000;
    with_current_process(|p| {
        assert_eq!(p.stack_top, USER_STACK_TOP);
        assert_eq!(p.stack_bottom, expected_bottom);
        assert_eq!(p.stack_mapped_bottom, expected_bottom);
        // happy_path_elf has a one-page PT_LOAD at USER_LOAD_BASE; the
        // global cap binds.
        assert_eq!(
            p.stack_max_growth_floor,
            USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES * 0x1000
        );
        assert_eq!(p.growth_faults_remaining, USER_STACK_MAX_GROWTH_PAGES);
    });

    // Teardown the mappings while this process's address space is current,
    // then remove the actual table entry. Rewriting `p.pid` would not change
    // the table key and would strand a synthetic process between tests.
    with_current_process(|p| {
        crate::userland::lifecycle::unmap_user_stack(p);
        // Take the image out so it doesn't try to unmap the (already
        // unmapped) stack on Drop. unmap_user_stack cleared the
        // image's stack_initial_bottom, so Drop will skip the stack
        // anyway — but explicitly take it so the PT_LOAD mapping it
        // does still own gets unmapped too.
        let _img = p.image.take();
        drop(_img);
    });
    drop(crate::userland::lifecycle::remove_process(pid));
    crate::userland::lifecycle::reset_sentinel();
}

// --- U2: loader growth-floor + bounds_start + per-binary floor ---

/// A sufficiently high PT_LOAD makes its guard-derived floor bind more
/// tightly than the global 64 MiB stack-growth cap.
fn test_loader_per_binary_floor_from_pt_load_end() {
    use crate::mm::paging::{USER_STACK_GUARD_PAGES, USER_STACK_MAX_GROWTH_PAGES, USER_STACK_TOP};

    let global_floor = USER_STACK_TOP - USER_STACK_MAX_GROWTH_PAGES * 0x1000;
    // Keep the segment below the initial stack while placing its guarded
    // end above the global floor.
    let p_vaddr = global_floor + 0x20_0000;
    let payload: alloc::vec::Vec<u8> = (0..16u8).collect();
    let p_offset = 0x1000u64;
    let phdr = fix::PhdrSpec {
        p_type: fix::PT_LOAD,
        p_flags: fix::PF_R | fix::PF_X,
        p_offset,
        p_vaddr,
        p_filesz: payload.len() as u64,
        p_memsz: 0x100,
        p_align: 0x1000,
    };
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: alloc::vec![phdr],
        payloads: alloc::vec![(p_offset, payload)],
        truncate_to: None,
    }
    .build();

    let image = load_elf(&bytes).expect("load_elf per-binary floor");
    let pt_load_end = p_vaddr + 0x1000;
    let per_binary = pt_load_end + USER_STACK_GUARD_PAGES * 0x1000;
    assert!(per_binary > global_floor, "test premise broken");
    assert_eq!(image.stack_max_growth_floor, per_binary);
}

/// Reject a binary whose PT_LOAD reaches so high that the per-binary
/// growth floor would land above the initial stack commit — i.e.,
/// there's no room left even for the small initial mapping. This is
/// also the only PT_LOAD-vs-stack overlap case we need to enforce, since
/// the per-binary floor formula keeps the deepest stack page above
/// every PT_LOAD's end by construction.
fn test_loader_rejects_binary_too_big_for_initial_stack() {
    use crate::mm::paging::{USER_STACK_GUARD_PAGES, USER_STACK_INITIAL_PAGES, USER_STACK_TOP};

    let initial_bottom = USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000;
    let p_vaddr = initial_bottom - USER_STACK_GUARD_PAGES * 0x1000;
    let projected_floor = p_vaddr + 0x1000 + USER_STACK_GUARD_PAGES * 0x1000;
    assert!(projected_floor > initial_bottom, "test premise broken");

    let payload: alloc::vec::Vec<u8> = (0..16u8).collect();
    let p_offset = 0x1000u64;
    let phdr = fix::PhdrSpec {
        p_type: fix::PT_LOAD,
        p_flags: fix::PF_R | fix::PF_X,
        p_offset,
        p_vaddr,
        p_filesz: payload.len() as u64,
        p_memsz: 0x100,
        p_align: 0x1000,
    };
    let bytes = fix::Fixture {
        e_type: fix::ET_EXEC,
        e_machine: fix::EM_X86_64,
        ei_class: fix::ELFCLASS64,
        ei_data: fix::ELFDATA2LSB,
        e_entry: p_vaddr,
        phdrs: alloc::vec![phdr],
        payloads: alloc::vec![(p_offset, payload)],
        truncate_to: None,
    }
    .build();

    assert_eq!(load_elf(&bytes).unwrap_err(), LoaderError::VaOutOfRange);
}

/// bounds_start reflects what's actually mapped — the lowest of the
/// PT_LOAD pages and the initial stack commit. It is NOT pushed down
/// to the growth floor; the fault handler widens bounds via
/// set_user_va_bounds on each successful growth so validate_user_slice
/// never accepts a pointer into a page that has never been mapped.
fn test_loader_bounds_start_at_initial_commit() {
    use crate::mm::paging::{USER_STACK_INITIAL_PAGES, USER_STACK_TOP};

    let bytes = fix::happy_path_elf();
    let image = load_elf(&bytes).expect("load_elf");

    let initial_bottom = USER_STACK_TOP - USER_STACK_INITIAL_PAGES * 0x1000;
    // bounds_start = min(PT_LOAD start, initial_stack_bottom, USER_BRK_BASE).
    // PT_LOAD wins for this fixture (0x40_0000 < 0x7F_8000 <
    // USER_BRK_BASE 0x200_0000).
    assert_eq!(image.bounds_start, 0x40_0000);

    // The pre-grown-stack invariant we care about: bounds_start is
    // NOT lowered all the way to `stack_max_growth_floor`. (If it
    // were, validate_user_slice would accept pointers into pages that
    // have never been mapped, and an in-kernel deref of such a pointer
    // would fault under the mapper lock — re-introducing the bug
    // the wide-bounds rejection avoids.) For this fixture the growth
    // floor is just above the PT_LOAD end + 16-page guard, but well
    // below initial_bottom; bounds_start must NOT match it.
    assert!(image.stack_max_growth_floor < initial_bottom);
    assert_ne!(image.bounds_start, image.stack_max_growth_floor);
}

// --- U1: stack-window fields + unmap_user_stack helper ---

fn test_set_stack_window_records_all_fields() {
    use crate::userland::lifecycle::with_current_process;

    with_current_process(|p| {
        // Snapshot and restore so a regression here doesn't strand the
        // sentinel slot with junk values for any test that runs after.
        let snap = (
            p.stack_top,
            p.stack_bottom,
            p.stack_mapped_bottom,
            p.stack_max_growth_floor,
            p.growth_faults_remaining,
        );

        p.set_stack_window(
            0x80_0000,
            0x80_0000 - 8 * 0x1000,
            0x80_0000 - 8 * 0x1000,
            0x80_0000 - 768 * 0x1000,
            768,
        );
        assert_eq!(p.stack_top, 0x80_0000);
        assert_eq!(p.stack_bottom, 0x80_0000 - 8 * 0x1000);
        assert_eq!(p.stack_mapped_bottom, 0x80_0000 - 8 * 0x1000);
        assert_eq!(p.stack_max_growth_floor, 0x80_0000 - 768 * 0x1000);
        assert_eq!(p.growth_faults_remaining, 768);

        p.stack_top = snap.0;
        p.stack_bottom = snap.1;
        p.stack_mapped_bottom = snap.2;
        p.stack_max_growth_floor = snap.3;
        p.growth_faults_remaining = snap.4;
    });
}

fn test_unmap_user_stack_sentinel_is_noop() {
    use crate::userland::lifecycle::{unmap_user_stack, with_current_process};

    // Sentinel slot (PID 0 between processes) has all stack fields = 0.
    // unmap_user_stack must not touch the mapper or panic.
    with_current_process(|p| {
        let snap = (
            p.stack_top,
            p.stack_bottom,
            p.stack_mapped_bottom,
            p.stack_max_growth_floor,
            p.growth_faults_remaining,
        );
        p.stack_top = 0;
        p.stack_bottom = 0;
        p.stack_mapped_bottom = 0;
        p.stack_max_growth_floor = 0;
        p.growth_faults_remaining = 0;
        unmap_user_stack(p);
        assert_eq!(p.stack_top, 0);
        assert_eq!(p.stack_mapped_bottom, 0);
        // Restore.
        p.stack_top = snap.0;
        p.stack_bottom = snap.1;
        p.stack_mapped_bottom = snap.2;
        p.stack_max_growth_floor = snap.3;
        p.growth_faults_remaining = snap.4;
    });
}

fn test_unmap_user_stack_releases_range() {
    use crate::mm::memory::with_memory_mapper;
    use crate::mm::paging::{UserPerms, USER_STACK_TOP};
    use crate::userland::lifecycle::{unmap_user_stack, with_current_process};
    use x86_64::VirtAddr;

    // Map a small fake stack so we can assert unmap_user_stack frees it.
    const N: u64 = 4;
    let bottom = USER_STACK_TOP - N * 0x1000;
    with_memory_mapper(|m| m.map_user_region(VirtAddr::new(bottom), N, UserPerms::ReadWrite))
        .unwrap()
        .unwrap();

    with_current_process(|p| {
        let snap = (
            p.stack_top,
            p.stack_bottom,
            p.stack_mapped_bottom,
            p.stack_max_growth_floor,
            p.growth_faults_remaining,
        );
        p.stack_top = USER_STACK_TOP;
        p.stack_bottom = bottom;
        p.stack_mapped_bottom = bottom;
        p.stack_max_growth_floor = bottom - 0x1000;
        p.growth_faults_remaining = 0;

        unmap_user_stack(p);

        assert_eq!(p.stack_top, 0);
        assert_eq!(p.stack_mapped_bottom, 0);

        // Re-mapping must succeed — the range is empty again.
        with_memory_mapper(|m| m.map_user_region(VirtAddr::new(bottom), N, UserPerms::ReadWrite))
            .unwrap()
            .unwrap();
        with_memory_mapper(|m| m.unmap_user_region(VirtAddr::new(bottom), N))
            .unwrap()
            .unwrap();

        p.stack_top = snap.0;
        p.stack_bottom = snap.1;
        p.stack_mapped_bottom = snap.2;
        p.stack_max_growth_floor = snap.3;
        p.growth_faults_remaining = snap.4;
    });
}

// ---------- U2: FPU/FS_BASE save/restore primitives ----------

/// `FpuState::default()` and `FpuState::fresh()` are 16-byte aligned.
/// `fxsave`/`fxrstor` would `#GP` if the buffer's effective address is
/// misaligned; the `repr(C, align(16))` attribute on the type is the
/// load-bearing guarantee.
fn test_fpu_state_is_16_aligned() {
    use crate::arch::x86_64::fpu::FpuState;
    let s = FpuState::default();
    assert_eq!((&s as *const FpuState as usize) & 0xF, 0);
    let f = FpuState::fresh();
    assert_eq!((&f as *const FpuState as usize) & 0xF, 0);
}

/// `FpuState::fresh()` carries the architectural reset MXCSR
/// (0x1F80 — all FP exceptions masked, round-to-nearest) at offset
/// 24..28. A fresh process inherits this via the install path so
/// musl's `__init_tls` sees the SDM-default FP environment.
fn test_fpu_state_fresh_has_default_mxcsr() {
    use crate::arch::x86_64::fpu::FpuState;
    let f = FpuState::fresh();
    let bytes = f.bytes();
    assert_eq!(bytes[24], 0x80);
    assert_eq!(bytes[25], 0x1F);
    assert_eq!(bytes[26], 0x00);
    assert_eq!(bytes[27], 0x00);
}

/// Round-trip: write a known 128-bit pattern into XMM0, capture FPU
/// state, scribble XMM0, restore FPU state, observe the pattern again.
/// This is the load-bearing proof for U4 — without it, ring-3 switches
/// would silently corrupt the XMM register file across processes.
fn test_fpu_save_restore_preserves_xmm0() {
    use crate::arch::x86_64::fpu::{restore_fpu, save_fpu, FpuState};

    // The kernel is built with `+soft-float` so Rust codegen never
    // touches XMM. We use inline asm with memory operands to stage and
    // observe register values — `movdqu` doesn't need the operand
    // aligned, which keeps the test self-contained.
    let pattern: [u8; 16] = [
        0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE,
        0xF0,
    ];
    let mut readback: [u8; 16] = [0; 16];

    // Stage: load pattern into XMM0.
    unsafe {
        core::arch::asm!(
            "movdqu xmm0, [{0}]",
            in(reg) pattern.as_ptr(),
            options(nostack, preserves_flags),
        );
    }

    // Save the FPU/SSE state — captures XMM0 = pattern.
    let mut saved = FpuState::default();
    save_fpu(&mut saved);

    // Scribble: write all-zeros into XMM0.
    let zero: [u8; 16] = [0; 16];
    unsafe {
        core::arch::asm!(
            "movdqu xmm0, [{0}]",
            in(reg) zero.as_ptr(),
            options(nostack, preserves_flags),
        );
    }

    // Restore — XMM0 should be `pattern` again.
    restore_fpu(&saved);

    // Read XMM0 back into a buffer.
    unsafe {
        core::arch::asm!(
            "movdqu [{0}], xmm0",
            in(reg) readback.as_mut_ptr(),
            options(nostack, preserves_flags),
        );
    }

    assert_eq!(readback, pattern);
}

/// `save_user_cpu_state` / `restore_user_cpu_state` orchestrators bundle
/// FS_BASE save + FPU save. Stages an FS_BASE value via the MSR
/// directly, saves into a Process, clobbers FS_BASE, restores, and
/// verifies the original value is back.
fn test_save_restore_user_cpu_state_roundtrips_fs_base() {
    use crate::userland::lifecycle::{
        restore_user_cpu_state, save_user_cpu_state, ExitKind, Process,
    };

    let saved_original = crate::arch::x86_64::msr::read_fs_base();

    let mut p = Process {
        pid: 9000,
        parent_pid: 0,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        umask: 0o022,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: None,
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        kernel_continuation: None,
        terminal_id: None,
    };

    // Stage a recognizable FS_BASE value. The kernel half of the
    // address space is mapped at 0xffff800000000000+, so any canonical
    // value works as a marker — we never dereference it.
    let staged_fs_base: u64 = 0x0000_7FFF_DEAD_0000;
    crate::arch::x86_64::msr::set_fs_base(staged_fs_base);

    save_user_cpu_state(&mut p);
    assert_eq!(p.fs_base, staged_fs_base);

    // Clobber.
    crate::arch::x86_64::msr::set_fs_base(0xDEAD_BEEF);

    restore_user_cpu_state(&p);
    assert_eq!(crate::arch::x86_64::msr::read_fs_base(), staged_fs_base);

    // Restore the original FS_BASE so subsequent tests aren't affected.
    crate::arch::x86_64::msr::set_fs_base(saved_original);
}

// ---------- U6: wait4 POSIX-correctness fixes ----------

/// has_children returns false when no process has parent_pid == me and
/// no zombie has parent_pid == me. Used by wait4 to distinguish
/// "no children → ECHILD" from "has children, none zombie → block".
fn test_has_children_returns_false_when_no_children() {
    assert!(!crate::userland::lifecycle::has_children(9999));
}

/// has_children returns true when a child Process is in the table.
fn test_has_children_sees_live_child() {
    use crate::userland::lifecycle::{insert_process, remove_process, ExitKind, Process};
    let child = Process {
        pid: 8001,
        parent_pid: 8000,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        umask: 0o022,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: None,
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        kernel_continuation: None,
        terminal_id: None,
    };
    insert_process(child);
    assert!(crate::userland::lifecycle::has_children(8000));
    assert!(!crate::userland::lifecycle::has_children(8002));
    drop(remove_process(8001));
}

/// has_children returns true when a zombie is filed for me. Cleanup
/// drains the zombie so subsequent tests aren't affected.
fn test_has_children_sees_zombie_child() {
    use crate::userland::lifecycle::{reap_zombie, record_zombie};
    record_zombie(8101, 8100, 0);
    assert!(crate::userland::lifecycle::has_children(8100));
    let _ = reap_zombie(8101, 8100);
    assert!(!crate::userland::lifecycle::has_children(8100));
}

// ---------- U3: ring-3 scheduling state ----------

/// Helper: clear the ring-3 ready/blocked queues so tests start from a
/// known state. Tests that rely on ordering can't trust the queues to
/// be empty just because PROCESS_TABLE looks empty — earlier tests
/// might have left entries.
fn clear_ring3_queues() {
    // Use the public APIs to drain the queues. pop_next_ring3 returns
    // None once empty; remove_process clears blocked entries too, but
    // the blocked map's keys aren't easy to enumerate publicly — for
    // tests we use a synthetic insert/remove cycle.
    while crate::userland::lifecycle::pop_next_ring3().is_some() {}
    // Blocked entries get cleaned by mark_ring3_ready (which removes
    // from blocked). Tests that create blocked entries clean them up
    // explicitly via the same.
}

/// mark_ring3_ready pushes onto the back; pop_next_ring3 pops from the
/// front. FIFO order matters because the U5 timer ISR relies on it
/// for round-robin between concurrent ring-3 processes.
fn test_ring3_ready_queue_is_fifo() {
    clear_ring3_queues();
    crate::userland::lifecycle::mark_ring3_ready(10);
    crate::userland::lifecycle::mark_ring3_ready(11);
    crate::userland::lifecycle::mark_ring3_ready(12);
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(10));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(11));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(12));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), None);
}

/// Calling mark_ring3_ready twice with the same PID is idempotent —
/// the PID appears once in the queue, not twice. Guards against
/// duplicate scheduling slots if a wake path races with a ready
/// path.
fn test_ring3_ready_queue_dedups() {
    clear_ring3_queues();
    crate::userland::lifecycle::mark_ring3_ready(20);
    crate::userland::lifecycle::mark_ring3_ready(20);
    crate::userland::lifecycle::mark_ring3_ready(20);
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(20));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), None);
}

/// mark_ring3_blocked removes from ready queue and records the reason.
/// peek_next_ring3 sees the queue without the blocked PID.
fn test_ring3_blocked_removes_from_ready() {
    use crate::userland::lifecycle::Ring3BlockReason;
    clear_ring3_queues();
    crate::userland::lifecycle::mark_ring3_ready(30);
    crate::userland::lifecycle::mark_ring3_ready(31);
    crate::userland::lifecycle::mark_ring3_blocked(
        30,
        Ring3BlockReason::WaitingForChild { target: -1 },
    );
    // 30 is no longer ready; 31 still is.
    assert_eq!(crate::userland::lifecycle::peek_next_ring3(), Some(31));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(31));
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), None);
    // Re-readying 30 puts it back.
    crate::userland::lifecycle::mark_ring3_ready(30);
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(30));
}

/// wake_ring3_blocked_on_child moves a parent waiting for any child
/// (target == -1) from blocked to ready.
fn test_wake_ring3_blocked_on_child_any() {
    use crate::userland::lifecycle::Ring3BlockReason;
    clear_ring3_queues();
    crate::userland::lifecycle::mark_ring3_blocked(
        40,
        Ring3BlockReason::WaitingForChild { target: -1 },
    );
    assert!(crate::userland::lifecycle::peek_next_ring3().is_none());
    crate::userland::lifecycle::wake_ring3_blocked_on_child(40, 41);
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(40));
}

/// wake_ring3_blocked_on_child wakes a parent waiting for the
/// specific child PID; doesn't wake for unrelated child exits.
fn test_wake_ring3_blocked_on_child_specific() {
    use crate::userland::lifecycle::Ring3BlockReason;
    clear_ring3_queues();
    crate::userland::lifecycle::mark_ring3_blocked(
        50,
        Ring3BlockReason::WaitingForChild { target: 55 },
    );
    // Unrelated child exit — parent stays blocked.
    crate::userland::lifecycle::wake_ring3_blocked_on_child(50, 99);
    assert!(crate::userland::lifecycle::peek_next_ring3().is_none());
    // Matching child exit — parent wakes.
    crate::userland::lifecycle::wake_ring3_blocked_on_child(50, 55);
    assert_eq!(crate::userland::lifecycle::pop_next_ring3(), Some(50));
}

/// remove_process cleans the ring-3 ready and blocked entries so a
/// removed PID can never resurface in a scheduling decision.
fn test_remove_process_cleans_ring3_queues() {
    use crate::userland::lifecycle::Ring3BlockReason;
    clear_ring3_queues();

    // Insert two processes into the table so remove_process can find them.
    let p1 = crate::userland::lifecycle::Process {
        pid: 60,
        parent_pid: 0,
        image: None,
        exit_kind: crate::userland::lifecycle::ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        umask: 0o022,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: None,
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        kernel_continuation: None,
        terminal_id: None,
    };
    let p2 = crate::userland::lifecycle::Process {
        pid: 61,
        parent_pid: 0,
        image: None,
        exit_kind: crate::userland::lifecycle::ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: crate::userland::fdtable::FdTable::new(),
        umask: 0o022,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: alloc::string::String::from("/"),
        address_space: None,
        signal_state: crate::userland::signal::SignalState::new(),
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: None,
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: 0,
        fpu_state: crate::arch::x86_64::fpu::FpuState::default(),
        saved_user_state: crate::userland::user_state::UserState::default(),
        kernel_continuation: None,
        terminal_id: None,
    };
    crate::userland::lifecycle::insert_process(p1);
    crate::userland::lifecycle::insert_process(p2);

    crate::userland::lifecycle::mark_ring3_ready(60);
    crate::userland::lifecycle::mark_ring3_blocked(
        61,
        Ring3BlockReason::WaitingForChild { target: -1 },
    );

    // Removing 60 clears its ready entry.
    drop(crate::userland::lifecycle::remove_process(60));
    assert!(crate::userland::lifecycle::peek_next_ring3().is_none());
    // Removing 61 clears its blocked entry — a subsequent wake call
    // does nothing.
    drop(crate::userland::lifecycle::remove_process(61));
    crate::userland::lifecycle::wake_ring3_blocked_on_child(61, 99);
    assert!(crate::userland::lifecycle::peek_next_ring3().is_none());
}

/// The compatibility decision view preserves the entity tag selected from the
/// same queue and falls back to idle when that queue is empty.
fn test_next_runnable_uses_unified_queue() {
    use crate::process::entity::EntityId;
    use crate::process::scheduler::{Runnable, Scheduler};
    let mut sched = Scheduler::new();
    sched.init();

    sched.register_user(70).unwrap();
    sched.make_ready(EntityId::UserProcess(70), None).unwrap();
    let pick = sched.next_runnable();
    assert_eq!(pick, Runnable::RingThree(70));

    // No more ready entities — fall back to the idle PCB.
    let pick = sched.next_runnable();
    assert!(matches!(pick, Runnable::KernelThread(_)));
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_fpu_state_is_16_aligned,
        &test_fpu_state_fresh_has_default_mxcsr,
        &test_fpu_save_restore_preserves_xmm0,
        &test_save_restore_user_cpu_state_roundtrips_fs_base,
        &test_ring3_ready_queue_is_fifo,
        &test_ring3_ready_queue_dedups,
        &test_ring3_blocked_removes_from_ready,
        &test_wake_ring3_blocked_on_child_any,
        &test_wake_ring3_blocked_on_child_specific,
        &test_remove_process_cleans_ring3_queues,
        &test_next_runnable_uses_unified_queue,
        &test_has_children_returns_false_when_no_children,
        &test_has_children_sees_live_child,
        &test_has_children_sees_zombie_child,
        &test_set_stack_window_records_all_fields,
        &test_unmap_user_stack_sentinel_is_noop,
        &test_unmap_user_stack_releases_range,
        &test_install_new_process_populates_stack_window,
        &test_try_grow_user_stack_grew,
        &test_try_grow_user_stack_overflow_below_floor,
        &test_try_grow_user_stack_budget_exhausted,
        &test_try_grow_user_stack_fills_gap_above_bottom,
        &test_try_grow_user_stack_rejects_mapped_and_out_of_range_pages,
        &test_try_grow_user_stack_sentinel_is_not_stack_grow,
        &test_try_grow_user_stack_widens_validated_bounds,
        &test_handler_blocked_mask_includes_sa_mask_and_signum,
        &test_handler_blocked_mask_preserves_old_bits,
        &test_handler_blocked_mask_strips_kill_and_stop,
        &test_maybe_deliver_signal_installs_handler_mask,
        &test_prepare_deliverable_signal_restores_pre_sigsuspend_mask,
        &test_loader_per_binary_floor_from_pt_load_end,
        &test_loader_rejects_binary_too_big_for_initial_stack,
        &test_loader_bounds_start_at_initial_commit,
        &test_gdt_kernel_selectors,
        &test_sse_enabled_before_ring3,
        &test_gdt_user_selectors,
        &test_tss_loaded,
        &test_map_user_region_kernel_can_read,
        &test_map_user_region_propagates_user_bit,
        &test_unmap_user_region_returns_frames,
        &test_map_user_region_rejects_double_map,
        &test_map_user_region_rejects_out_of_range,
        &test_unmap_user_region_rejects_unmapped,
        // ABI / dispatcher
        &test_dispatch_unregistered_returns_enosys,
        &test_unknown_syscall_trace_mode_returns_enosys_and_marks_seen,
        &test_unknown_syscall_trace_mode_marks_only_once,
        &test_unknown_syscall_trace_mode_off_does_not_mark,
        &test_unknown_syscall_trace_mode_capacity_overflow,
        // U3: musl-init / zsh-startup syscalls
        &test_dispatch_poll_streams_ready,
        &test_dispatch_poll_unknown_fd_returns_pollnval,
        &test_dispatch_poll_negative_fd_is_ignored,
        &test_dispatch_poll_zero_nfds_returns_zero,
        &test_dispatch_poll_nfds_over_cap_returns_einval,
        &test_dispatch_select_streams_ready,
        &test_dispatch_select_unknown_fd_returns_ebadf,
        &test_dispatch_readlink_proc_self_exe,
        &test_dispatch_readlink_proc_self_fd_stdin,
        &test_dispatch_readlink_proc_self_fd_negative_rejected,
        &test_dispatch_readlink_proc_self_fd_overflow_rejected,
        &test_dispatch_readlink_other_returns_enoent,
        &test_dispatch_getrlimit_returns_infinity,
        &test_dispatch_prlimit64_old_value_writes_infinity,
        &test_dispatch_prlimit64_null_old_returns_zero,
        &test_dispatch_getrusage_self_zero_fills_and_rejects_unknown_who,
        &test_dispatch_setitimer_real_arms_queries_and_validates,
        &test_dispatch_nanosleep_validation_and_synthetic_return,
        &test_write_handler_valid_slice,
        &test_write_handler_rejects_unknown_fd,
        &test_write_handler_rejects_kernel_pointer,
        &test_write_handler_rejects_span_past_bounds,
        &test_write_handler_rejects_pointer_wraparound,
        &test_write_handler_zero_len_succeeds,
        &test_write_file_large_chunked,
        &test_writev_file_large_iovs,
        &test_pwrite_large_and_pread_short,
        &test_dispatch_eventfd_epoll_edge_round_trip,
        &test_dispatch_socketpair_full_duplex,
        &test_dispatch_sigaltstack_membarrier_and_yield_profile,
        &test_dispatch_chmod_fchmod_noops,
        &test_utf8_safe_chunk_len_boundaries,
        &test_exit_group_handler_records_code,
        &test_validate_user_slice_zero_len_ok,
        // Loader
        &test_loader_happy_path,
        &test_loader_bad_magic,
        &test_loader_wrong_arch,
        &test_loader_wrong_class,
        &test_loader_wrong_type,
        &test_loader_truncated_phdrs,
        &test_loader_va_out_of_range,
        &test_loader_overlapping_pt_load,
        &test_loader_entry_not_mapped,
        &test_loader_alignment_bad,
        &test_loader_pt_tls_loads,
        &test_loader_pt_tls_oversized_rejected,
        &test_loader_pt_interp_rejected,
        &test_loader_segment_overflow,
        &test_loader_unsupported_reloc,
        &test_loader_glob_dat_unresolved,
        &test_loader_no_relocations_is_ok,
        &test_loader_rollback_unmaps_on_reloc_failure,
        // enter_user_mode lifecycle
        &test_enter_user_mode_ignores_kernel_sentinel_state,
        &test_run_initial_stack_fixture_b,
        &test_run_unknown_syscall_returns_enosys,
        &test_run_syscall_exit42_fixture_a,
        &test_run_happy_path_hello,
        &test_run_fault_ud,
        &test_kernel_and_userland_resume_after_user_fault,
        // U8: ZSH.ELF end-to-end tests.
        &test_run_zsh_minimal_exit,
        &test_run_zsh_echo_command,
        &test_run_zsh_pwd,
        &test_run_zsh_external_ls,
        &test_run_zsh_global_rc_agnoster_prompt,
        &test_run_links_version,
        &test_run_fault_pf,
        &test_run_fault_gp,
        &test_run_bad_pointer_syscall,
        &test_run_leak_loop_happy,
        &test_run_leak_loop_fault,
        // Phase 1: stdin + argv/envp
        &test_user_stdin_install_push_pop,
        &test_user_stdin_push_when_inactive_is_noop,
        &test_dispatch_read_returns_queued_bytes,
        &test_dispatch_read_no_active_user_returns_zero,
        &test_enter_user_mode_with_argv_envp,
        // Phase 2: FD table
        &test_fdtable_install_default_streams,
        &test_fdtable_alloc_and_close,
        &test_fdtable_dup_and_dup2,
        // Phase 2: path utilities
        &test_normalize_path_absolute_keeps_path,
        &test_normalize_path_relative_anchors_at_cwd,
        &test_normalize_path_collapses_redundancy,
        &test_copy_user_cstr_happy_path,
        &test_copy_user_cstr_unterminated_at_bound_returns_efault,
        // Phase 2: dispatcher coverage
        &test_dispatch_getcwd_returns_default,
        &test_dispatch_getcwd_short_buffer_returns_erange,
        &test_dispatch_chdir_root_succeeds,
        &test_dispatch_chdir_nonexistent_returns_enoent,
        &test_dispatch_open_nonexistent_returns_enoent,
        &test_dispatch_open_writable_flag_returns_erofs,
        &test_dispatch_regular_file_fcntl_reports_write_access,
        &test_dispatch_readv_regular_file_scatter,
        &test_dispatch_open_runtime_etc_passwd,
        &test_dispatch_stat_runtime_zsh_config,
        &test_dispatch_open_etc_unmanaged_file_returns_enoent,
        &test_dispatch_open_etc_traversal_collapses,
        &test_dispatch_open_runtime_etc_for_write_returns_erofs,
        &test_dispatch_unlink_runtime_etc_returns_eperm,
        &test_dispatch_close_stream_is_noop,
        &test_dispatch_dup_stdout,
        &test_dispatch_lseek_on_stream_returns_espipe,
        &test_dispatch_clock_gettime_writes_timespec,
        &test_dispatch_clock_gettime_invalid_clock_einval,
        &test_dispatch_clock_realtime_uses_rtc_epoch,
        &test_dispatch_umask_roundtrip_and_masks_bits,
        &test_dispatch_utimensat_values_now_omit_and_errors,
        &test_dispatch_getrandom_fills_buffer,
        &test_dispatch_dev_null_rdwr_read_eof_write_sink,
        &test_dispatch_dev_urandom_read_stat_and_seek,
        &test_dispatch_dev_directory_lists_urandom,
        &test_dispatch_uname_writes_sysname_linux,
        &test_dispatch_fcntl_getfd_setfd_roundtrip,
        &test_write_handler_non_utf8_returns_full_len,
        // Phase 2 PR-4: directories
        &test_dispatch_open_host_directory_succeeds,
        &test_dispatch_read_on_directory_returns_eisdir,
        &test_dispatch_getdents64_emits_records,
        &test_fat_metadata_subdirectory_tolerated,
        // Phase 3: TTY
        &test_termios_default_is_canonical_with_echo,
        &test_dispatch_ioctl_tcgets_returns_termios,
        &test_dispatch_ioctl_tcsets_updates_termios,
        &test_dispatch_ioctl_on_file_returns_enotty,
        &test_dispatch_ioctl_tiocgwinsz_returns_80x24,
        &test_dispatch_ioctl_tiocgpgrp_returns_enotty,
        &test_dispatch_ioctl_tiocspgrp_returns_zero,
        // Phase 4 PR-A: process table + real PIDs
        &test_getpid_returns_real_pid,
        &test_pid_allocation_is_monotonic,
        &test_getppid_returns_kernel_sentinel,
        // Phase 4 PR-B: per-process address spaces
        &test_address_space_new_kernel_half_shared,
        &test_address_space_drop_restores_kernel_cr3,
        &test_address_space_drop_reclaims_leaf_and_all_table_levels,
        // Phase 4 PR-C: clone for fork
        &test_address_space_clone_for_child_uses_cow,
        // Phase 4 PR-C2: fork + wait4
        &test_fork_then_wait_returns_to_parent,
        // Phase 4 PR-D: execve (negative path)
        &test_fork_execve_badpath_returns_to_parent,
        // Phase 5 PR-A: pipes
        &test_pipe_basic_write_then_read,
        &test_pipe_handle_clone_drop_tracks_counts,
        &test_pipe_short_write_at_capacity,
        &test_dispatch_pipe2_round_trip,
        &test_dispatch_pipe2_nonblocking_and_fcntl_status,
        &test_dispatch_select_pipe_readiness,
        &test_dispatch_writev_pipe_round_trip,
        // Phase 5 PR-B: signal foundation
        &test_dispatch_rt_sigaction_round_trip,
        &test_dispatch_rt_sigaction_sigrtmax_does_not_panic,
        &test_dispatch_rt_sigaction_rejects_sigkill_sigstop,
        &test_dispatch_rt_sigprocmask_block_strips_kill_stop,
        &test_rt_sigsuspend_pending_signal_returns_eintr_and_saves_mask,
        &test_dispatch_rt_sigsuspend_invalid_sigsetsize,
        &test_dispatch_rt_sigsuspend_null_mask_efaults,
        &test_dispatch_kill_self_sets_pending,
        &test_fork_child_exit_sets_sigchld_on_parent,
        &test_notify_parent_of_exit_files_zombie_and_raises_sigchld,
        &test_notify_parent_of_exit_skips_when_no_parent,
        &test_fork_post_resume_user_va_bounds_restored,
        // Phase 5 PR-B2: real signal delivery
        &test_signal_delivery_handler_runs,
    ]
}
