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
use crate::userland::abi::{
    validate_user_slice, EACCES, EBADF, EBUSY, EEXIST, EFAULT, EFBIG, EINTR, EINVAL, EIO, EISDIR,
    EMFILE, ENOENT, ENOMEM, ENOSPC, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTTY, EPERM, ERANGE, EROFS,
    ESPIPE, EXDEV, LAST_EXIT_CODE,
};
use crate::userland::fdtable::{FdSlot, FdTable, FD_TABLE_SIZE};
use crate::userland::path::{copy_user_cstr, normalize_path};
use alloc::string::String;
use alloc::vec;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

/// Maximum bytes a single `write` call can emit.
const WRITE_MAX_LEN: usize = 4096;
/// Maximum iovec entries per `writev`. libstdc++'s underlying stdio
/// rarely emits more than 2-3 iovecs at a time; 16 is plenty.
const WRITEV_MAX_IOV: usize = 16;
/// Maximum total bytes per `writev` (sum of iov_len).
const WRITEV_MAX_TOTAL: u64 = 16 * 1024;
/// Maximum mmap allocation in bytes.
const MMAP_MAX_LEN: u64 = 512 * 1024 * 1024;
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
const MAP_FIXED: u64 = 0x10;
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
    enum Target {
        StdoutErr,
        Pipe(crate::userland::pipe::PipeWriteHandle),
        File(crate::lib::arc::Arc<crate::fs::file_handle::File>),
        Socket(u64),
    }
    let slot = with_fd_slot(fd);
    let target = match slot {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => Target::StdoutErr,
        Some(FdSlot::File { handle, .. }) => Target::File(handle),
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(FdSlot::PipeWrite(handle, _)) => Target::Pipe(handle),
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Socket { handle, .. }) => Target::Socket(handle.id()),
        Some(FdSlot::Stdin) | None => return EBADF,
    };

    if len > WRITE_MAX_LEN as u64 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }
    let mut staging = alloc::vec![0u8; len as usize];
    if let Err(e) = crate::userland::usercopy::copy_from_user(&mut staging, ptr) {
        return e;
    }
    let slice = staging.as_slice();

    match target {
        Target::Pipe(handle) => {
            // Write to a pipe. Return `-EPIPE` when no readers remain
            // (POSIX would also raise SIGPIPE; we skip the signal
            // until a later milestone). When the ring buffer has
            // room, take what fits and return. When it's full but
            // readers still exist, block via the ring-3 scheduler —
            // `Pipe::read` and the last `PipeReadHandle::Drop` wake
            // `WaitingForPipeWrite` blockers, so the resumed SYSCALL
            // re-enters this handler.
            if handle.pipe().readers() == 0 {
                return crate::userland::abi::EPIPE;
            }
            let n = handle.pipe().write(slice);
            if n > 0 {
                return n as i64;
            }
            // n == 0 means the buffer was full. Re-check readers in
            // case they vanished between the readers() probe and the
            // write (reader process exited while we were preparing).
            if handle.pipe().readers() == 0 {
                return crate::userland::abi::EPIPE;
            }
            unsafe {
                crate::userland::switch::block_current_ring3_and_yield(
                    args,
                    crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeWrite,
                )
            }
        }
        Target::File(handle) => match handle.write(slice) {
            Ok(n) => n as i64,
            Err(ref e) => map_file_err(e),
        },
        Target::Socket(id) => crate::userland::network_syscalls::write_connected(args, id, slice),
        Target::StdoutErr => {
            // Lossy: invalid UTF-8 bytes become U+FFFD rather than
            // dropping the entire call. A strict `from_utf8` here
            // would silently swallow writes that mix valid text with
            // binary data (e.g. cat'ing a partially-binary file).
            let s = alloc::string::String::from_utf8_lossy(slice);
            // U8/bugfix: route to THIS process's terminal_id, not the
            // global CURRENT_OUTPUT_TERMINAL. With multiple ring-3
            // processes (one per terminal), the global is wrong —
            // last-launcher wins, so zsh1's writes would land in
            // terminal 2's window.
            let dest_terminal = crate::userland::lifecycle::with_current_process(|p| p.terminal_id);
            match dest_terminal {
                Some(tid) => crate::window::terminal::write_to_terminal_id(tid, &s),
                None => crate::print!("{}", s),
            }
            len as i64
        }
    }
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

    enum Target {
        StdoutErr,
        Pipe(crate::userland::pipe::PipeWriteHandle),
        File(crate::lib::arc::Arc<crate::fs::file_handle::File>),
        Socket(u64),
    }
    let target = match with_fd_slot(fd) {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => Target::StdoutErr,
        Some(FdSlot::File { handle, .. }) => Target::File(handle),
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(FdSlot::PipeWrite(handle, _)) => Target::Pipe(handle),
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Socket { handle, .. }) => Target::Socket(handle.id()),
        Some(FdSlot::Stdin) | None => return EBADF,
    };
    if iovcnt < 0 || iovcnt as usize > WRITEV_MAX_IOV {
        return EINVAL;
    }
    // Validate every iov_base/iov_len pair before writing any of them, so
    // a bad later entry doesn't produce a partial write.
    let mut total: u64 = 0;
    let mut iovecs = alloc::vec::Vec::with_capacity(iovcnt as usize);
    for i in 0..iovcnt as u64 {
        let entry = iov_ptr + i * 16;
        let base = match crate::userland::usercopy::read_unaligned::<u64>(entry) {
            Ok(value) => value,
            Err(e) => return e,
        };
        let len = match crate::userland::usercopy::read_unaligned::<u64>(entry + 8) {
            Ok(value) => value,
            Err(e) => return e,
        };
        if let Err(e) = crate::userland::usercopy::ensure_user_range(base, len, false) {
            return e;
        }
        match total.checked_add(len) {
            Some(t) if t <= WRITEV_MAX_TOTAL => total = t,
            _ => return EINVAL,
        }
        iovecs.push((base, len));
    }

    // U8/bugfix: route the StdoutErr fast path to the writing
    // process's terminal_id (same reasoning as write_handler). Look
    // up once outside the loop; only relevant when target is
    // StdoutErr but cheap enough to always compute.
    let dest_terminal = if matches!(target, Target::StdoutErr) {
        crate::userland::lifecycle::with_current_process(|p| p.terminal_id)
    } else {
        None
    };

    // Now emit every iov in order. Short writes break the loop so
    // POSIX writev's "stop at the first short write" semantics hold.
    let mut written: u64 = 0;
    for (base, len) in iovecs {
        if len == 0 {
            continue;
        }
        let mut bytes = alloc::vec![0u8; len as usize];
        if let Err(e) = crate::userland::usercopy::copy_from_user(&mut bytes, base) {
            return if written > 0 { written as i64 } else { e };
        }
        let slice = bytes.as_slice();
        match &target {
            Target::StdoutErr => {
                let s = alloc::string::String::from_utf8_lossy(slice);
                match dest_terminal {
                    Some(tid) => crate::window::terminal::write_to_terminal_id(tid, &s),
                    None => crate::print!("{}", s),
                }
                written += len;
            }
            Target::Pipe(handle) => {
                if handle.pipe().readers() == 0 {
                    return if written > 0 {
                        written as i64
                    } else {
                        crate::userland::abi::EPIPE
                    };
                }
                let n = handle.pipe().write(slice);
                if n > 0 {
                    written += n as u64;
                    if (n as u64) < len {
                        break;
                    }
                    continue;
                }
                if handle.pipe().readers() == 0 {
                    return if written > 0 {
                        written as i64
                    } else {
                        crate::userland::abi::EPIPE
                    };
                }
                if written > 0 {
                    return written as i64;
                }
                unsafe {
                    crate::userland::switch::block_current_ring3_and_yield(
                        args,
                        crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeWrite,
                    )
                }
            }
            Target::File(handle) => match handle.write(slice) {
                Ok(n) => {
                    written += n as u64;
                    if (n as u64) < len {
                        break;
                    }
                }
                Err(ref e) => {
                    if written > 0 {
                        return written as i64;
                    }
                    return map_file_err(e);
                }
            },
            Target::Socket(id) => {
                let result = crate::userland::network_syscalls::write_connected(args, *id, slice);
                if result < 0 {
                    return if written > 0 { written as i64 } else { result };
                }
                written += result as u64;
                if result as u64 != len {
                    break;
                }
            }
        }
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
    let slot = with_fd_slot(fd);
    match slot {
        Some(FdSlot::Stdin) => read_stdin_blocking(args, ptr, cap),
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => EBADF,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => EISDIR,
        Some(FdSlot::PipeRead(handle, _)) => {
            // Drain bytes from the pipe. EOF when empty *and* no
            // writers remain. When empty but writers exist, block via
            // the ring-3 scheduler — `Pipe::write` and the last
            // `PipeWriteHandle::Drop` both wake `WaitingForPipeRead`
            // blockers, so the resumed SYSCALL re-enters this handler
            // and either drains bytes or observes EOF. zsh's
            // entersubsh_ret self-pipe (and any other POSIX-blocking
            // pipe reader) depends on this.
            let mut staging = vec![0u8; cap as usize];
            let n = handle.pipe().read(&mut staging);
            if n > 0 {
                if let Err(e) = crate::userland::usercopy::copy_to_user(ptr, &staging[..n]) {
                    return e;
                }
                return n as i64;
            }
            if handle.pipe().writers() == 0 {
                return 0; // EOF
            }
            unsafe {
                crate::userland::switch::block_current_ring3_and_yield(
                    args,
                    crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeRead,
                );
            }
        }
        Some(FdSlot::PipeWrite(_, _)) => EBADF,
        Some(FdSlot::Socket { handle, .. }) => {
            crate::userland::network_syscalls::read_connected(args, handle.id(), ptr, cap as usize)
        }
        Some(FdSlot::File { handle, .. }) => {
            // Stage the read inside a kernel buffer so the FAT/IDE path
            // never sees a user pointer (which could be unmapped, span a
            // page boundary the FAT layer doesn't understand, etc.).
            let mut staging = vec![0u8; cap as usize];
            match handle.read(&mut staging) {
                Ok(n) => {
                    if n > 0 {
                        if let Err(e) = crate::userland::usercopy::copy_to_user(ptr, &staging[..n])
                        {
                            return e;
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

fn read_stdin_blocking(args: &SyscallArgs, ptr: u64, cap: u64) -> i64 {
    if !crate::userland::stdin::is_active_for_current_process() {
        return 0;
    }
    let mut staging = alloc::vec![0u8; cap as usize];
    let n = crate::userland::stdin::pop_into_for_current_process(&mut staging);
    if n > 0 {
        if let Err(e) = crate::userland::usercopy::copy_to_user(ptr, &staging[..n]) {
            return e;
        }
        return n as i64;
    }

    // U8: no input available. Block via the ring-3 scheduler instead
    // of spinning on `sti;hlt;cli` (which monopolized the CPU and
    // blocked other ring-3 processes from being scheduled). Yields
    // to the next runnable ring-3 process, or back to the kernel
    // main loop if none. When input arrives, the input ISR's stdin
    // push path calls `wake_ring3_blocked_on_input`, moving us to
    // `ring3_ready`. The kernel main loop's `save_kernel_and_resume_ring3`
    // (or another ring-3 yielding) picks us up; our SYSCALL re-fires
    // and this handler re-runs from the top — re-checks the queue,
    // pops the now-present bytes, returns.
    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForInput,
        );
    }
}

// ---------- mmap / munmap / mprotect ----------

/// `mmap(addr, length, prot, flags, fd, offset) -> void *`
///
/// Creates a metadata-only anonymous-private or file-private VMA. A valid
/// free hint is honored; otherwise a top-down reusable gap is selected.
/// Resident pages are allocated only when touched.
pub fn mmap_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::vm::{VmProt, Vma, VmaBacking};

    let addr_hint = args.rdi;
    let length = args.rsi;
    let prot = args.rdx;
    let flags = args.r10;
    let fd = args.r8 as i64;
    let offset = args.r9;

    if length == 0 || length > MMAP_MAX_LEN {
        return EINVAL;
    }
    if (flags & MAP_PRIVATE) == 0 || flags & MAP_FIXED != 0 {
        return ENOSYS;
    }
    if prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return EINVAL;
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return EACCES;
    }
    if offset & 0xfff != 0 {
        return EINVAL;
    }

    let len = length.div_ceil(0x1000) * 0x1000;
    let mut vm_prot = VmProt::NONE;
    if prot & PROT_READ != 0 {
        vm_prot = vm_prot.union(VmProt::READ);
    }
    if prot & PROT_WRITE != 0 {
        vm_prot = vm_prot.union(VmProt::WRITE);
    }
    if prot & PROT_EXEC != 0 {
        vm_prot = vm_prot.union(VmProt::EXEC);
    }

    let file = if flags & MAP_ANONYMOUS == 0 {
        if fd < 0 {
            return EBADF;
        }
        crate::userland::lifecycle::with_current_process(|process| {
            match process.fd_table.get(fd as i32) {
                Some(FdSlot::File { handle, .. }) => Some(handle.clone()),
                _ => None,
            }
        })
    } else {
        if fd != -1 {
            return EINVAL;
        }
        None
    };
    if flags & MAP_ANONYMOUS == 0 && file.is_none() {
        return EBADF;
    }

    crate::userland::lifecycle::with_current_process(|process| {
        let Some(space) = process.address_space.as_mut() else {
            return ENOMEM;
        };
        let stack_floor = space
            .vmas()
            .as_slice()
            .iter()
            .find_map(|vma| matches!(vma.backing, VmaBacking::Stack { .. }).then_some(vma.start))
            .unwrap_or(crate::mm::paging::USER_STACK_TOP);
        let hinted_end = addr_hint.checked_add(len);
        let addr = if addr_hint & 0xfff == 0
            && hinted_end.is_some_and(|end| space.vmas().is_free(addr_hint, end))
        {
            addr_hint
        } else {
            match space
                .vmas()
                .find_gap_top_down(len, stack_floor.saturating_sub(1024 * 1024))
            {
                Ok(address) => address,
                Err(_) => return ENOMEM,
            }
        };
        let backing = match file {
            Some(ref handle) => VmaBacking::FilePrivate {
                file: handle.clone(),
                file_offset: offset,
                file_size: handle.size(),
            },
            None => VmaBacking::Anonymous,
        };
        let Ok(vma) = Vma::new(addr, addr + len, vm_prot, backing) else {
            return ENOMEM;
        };
        if space.vmas_mut().insert(vma).is_err() {
            return ENOMEM;
        }
        addr as i64
    })
}

/// `munmap(addr, length) -> int`
///
/// Splits/trims intersecting VMAs, tolerates holes, and releases every
/// resident leaf in the range. Subsequent mmap gap search can reuse it.
pub fn munmap_handler(args: &mut SyscallArgs) -> i64 {
    let addr = args.rdi;
    let length = args.rsi;
    if addr & 0xFFF != 0 || length == 0 {
        return EINVAL;
    }
    let end = match addr.checked_add(length.div_ceil(0x1000) * 0x1000) {
        Some(end) => end,
        None => return EINVAL,
    };
    let l4 = crate::userland::lifecycle::with_current_process(|process| {
        let Some(space) = process.address_space.as_mut() else {
            return None;
        };
        if space.vmas_mut().remove(addr, end).is_err() {
            return None;
        }
        Some(space.l4_frame())
    });
    let Some(l4) = l4 else {
        return EINVAL;
    };
    crate::mm::memory::with_memory_mapper(|mapper| {
        let mut page = addr;
        while page < end {
            if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
                let _ = mapper.unmap_page_from(l4, VirtAddr::new(page));
            }
            page += 0x1000;
        }
    });
    0
}

/// `mprotect(addr, length, prot) -> int`
///
/// Updates logical VMA protections and hardware flags on resident pages.
/// COW software state is preserved and writable+executable is rejected.
pub fn mprotect_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::vm::VmProt;
    let addr = args.rdi;
    let length = args.rsi;
    let prot = args.rdx;
    if addr & 0xfff != 0 || length == 0 || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return EINVAL;
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return EACCES;
    }
    let Some(end) = addr.checked_add(length.div_ceil(0x1000) * 0x1000) else {
        return EINVAL;
    };
    let mut vm_prot = VmProt::NONE;
    if prot & PROT_READ != 0 {
        vm_prot = vm_prot.union(VmProt::READ);
    }
    if prot & PROT_WRITE != 0 {
        vm_prot = vm_prot.union(VmProt::WRITE);
    }
    if prot & PROT_EXEC != 0 {
        vm_prot = vm_prot.union(VmProt::EXEC);
    }
    let l4 = crate::userland::lifecycle::with_current_process(|process| {
        let space = process.address_space.as_mut()?;
        space.vmas_mut().protect(addr, end, vm_prot).ok()?;
        Some(space.l4_frame())
    });
    let Some(l4) = l4 else {
        return ENOMEM;
    };
    crate::mm::memory::with_memory_mapper(|mapper| {
        let mut page = addr;
        while page < end {
            if let Some((frame, mut flags)) = mapper.leaf_info(l4, VirtAddr::new(page)) {
                flags.remove(PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE);
                if vm_prot != VmProt::NONE {
                    flags.insert(PageTableFlags::USER_ACCESSIBLE);
                }
                if vm_prot.contains(VmProt::EXEC) {
                    flags.remove(PageTableFlags::NO_EXECUTE);
                } else {
                    flags.insert(PageTableFlags::NO_EXECUTE);
                }
                if vm_prot.contains(VmProt::WRITE) {
                    if flags.contains(PageTableFlags::BIT_9)
                        || mapper.frame_refcount(frame).is_some_and(|count| count > 1)
                    {
                        flags.insert(PageTableFlags::BIT_9);
                    } else {
                        flags.insert(PageTableFlags::WRITABLE);
                    }
                }
                let _ = mapper.set_leaf_flags(l4, VirtAddr::new(page), flags);
            }
            page += 0x1000;
        }
    });
    0
}

// ---------- brk ----------

/// `brk(addr) -> void *`
///
/// `addr == 0` returns the current break. Growth changes only heap VMA
/// metadata; shrink releases complete pages and clears the retained
/// boundary-page tail so regrowth cannot reveal stale bytes.
pub fn brk_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::vm::{VmProt, Vma, VmaBacking};
    let new_brk = args.rdi;

    let cur = crate::userland::lifecycle::with_active_user(|au| au.brk_current);
    if new_brk == 0 {
        return cur as i64;
    }
    let base = crate::userland::lifecycle::with_current_process(|p| p.brk_base);
    if new_brk < base || new_brk > base + BRK_MAX_BYTES {
        return cur as i64;
    }
    let old_page_end = (cur + 0xFFF) & !0xFFF;
    let new_page_end = (new_brk + 0xFFF) & !0xFFF;
    // Linux keeps the partially used boundary page resident. Clear the
    // truncated bytes now so a later byte-granular regrowth cannot expose
    // stale heap contents from that page.
    if new_brk < cur {
        let zero_end = cur.min(new_page_end);
        if zero_end > new_brk {
            let zeros = [0u8; 0x1000];
            if crate::userland::usercopy::copy_to_user(
                new_brk,
                &zeros[..(zero_end - new_brk) as usize],
            )
            .is_err()
            {
                return cur as i64;
            }
        }
    }
    let l4 = crate::userland::lifecycle::with_current_process(|process| {
        let Some(space) = process.address_space.as_mut() else {
            return None;
        };
        let original = space.vmas().clone();
        if old_page_end > base {
            let _ = space.vmas_mut().remove(base, old_page_end);
        }
        if new_page_end > base {
            let vma = Vma::new(
                base,
                new_page_end,
                VmProt::READ.union(VmProt::WRITE),
                VmaBacking::Heap,
            )
            .ok()?;
            if space.vmas_mut().insert(vma).is_err() {
                *space.vmas_mut() = original;
                return None;
            }
        }
        process.brk_current = new_brk;
        Some(space.l4_frame())
    });
    let Some(l4) = l4 else {
        return cur as i64;
    };
    if new_page_end < old_page_end {
        crate::mm::memory::with_memory_mapper(|mapper| {
            let mut page = new_page_end;
            while page < old_page_end {
                if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
                    let _ = mapper.unmap_page_from(l4, VirtAddr::new(page));
                }
                page += 0x1000;
            }
        });
    }
    new_brk as i64
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
            // Validate addr is canonical AND in user VA range. Without
            // the user-VA check, a buggy/hostile user could set FS_BASE
            // to a kernel address, leak kernel data via `mov fs:[X]`,
            // and trigger CPL=3 page faults at kernel pages. Pre-fix,
            // we accepted any canonical address — which surfaced as
            // weird ring-3 faults at kernel-heap addresses when zsh
            // ran with a stale FS_BASE.
            if VirtAddr::try_new(addr).is_err() {
                return EINVAL;
            }
            if !(crate::mm::paging::USER_VA_RANGE_START..crate::mm::paging::USER_VA_RANGE_END)
                .contains(&addr)
            {
                crate::debug_error!(
                    "arch_prctl(SET_FS, {:#x}): rejected, not in user VA range",
                    addr
                );
                return EINVAL;
            }
            crate::debug_info!("arch_prctl(SET_FS, {:#x}) accepted", addr);
            crate::arch::x86_64::msr::set_fs_base(addr);
            // U8/bugfix: keep Process.fs_base in sync with the MSR so
            // resume_ring3 restores the right value after a block/wake
            // round-trip. Pre-fix, only save_user_cpu_state updated
            // Process.fs_base — meaning a child forked between
            // arch_prctl and the first preempt would inherit a stale
            // fs_base (0).
            crate::userland::lifecycle::with_current_process(|p| {
                p.fs_base = addr;
            });
            0
        }
        ARCH_GET_FS => {
            // Read current FS_BASE via the typed wrapper. Since we set it
            // ourselves, we could mirror it on ActiveUser instead of
            // round-tripping through the MSR — but reading is cheap.
            use x86_64::registers::model_specific::FsBase;
            let cur = FsBase::read().as_u64();
            crate::userland::usercopy::write_unaligned(addr, &cur).map_or_else(|e| e, |_| 0)
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
            let t = crate::userland::tty::snapshot();
            crate::userland::usercopy::write_unaligned(arg, &t).map_or_else(|e| e, |_| 0)
        }
        TCSETS | TCSETSW | TCSETSF => {
            let t = match crate::userland::usercopy::read_unaligned(arg) {
                Ok(value) => value,
                Err(e) => return e,
            };
            crate::userland::tty::set(t);
            0
        }
        TIOCGWINSZ => {
            let ws = crate::userland::tty::winsize();
            crate::userland::usercopy::write_unaligned(arg, &ws).map_or_else(|e| e, |_| 0)
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
        if let Err(e) = crate::userland::usercopy::write_unaligned(oldact_ptr, &prev) {
            return e;
        }
    }

    if act_ptr != 0 {
        let new_action = match crate::userland::usercopy::read_unaligned::<SigAction>(act_ptr) {
            Ok(value) => value,
            Err(e) => return e,
        };
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
    use crate::userland::signal::{SIGKILL, SIGSTOP, SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK};
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
        if let Err(e) = crate::userland::usercopy::write_unaligned(oldset_ptr, &prev) {
            return e;
        }
    }

    if set_ptr != 0 {
        let set = match crate::userland::usercopy::read_unaligned::<u64>(set_ptr) {
            Ok(value) => value,
            Err(e) => return e,
        };
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
    let mask = match crate::userland::usercopy::read_unaligned::<u64>(mask_ptr) {
        Ok(value) => value,
        Err(e) => return e,
    };
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

pub fn getuid_handler(_: &mut SyscallArgs) -> i64 {
    0
}
pub fn getgid_handler(_: &mut SyscallArgs) -> i64 {
    0
}
pub fn geteuid_handler(_: &mut SyscallArgs) -> i64 {
    0
}
pub fn getegid_handler(_: &mut SyscallArgs) -> i64 {
    0
}

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
    use crate::userland::lifecycle::{
        alloc_pid, insert_process, mark_ring3_ready, with_current_process, ExitKind,
    };
    use crate::userland::user_state::UserState;

    // U7: fork now returns immediately to the parent without iretq'ing
    // into the child. The child is inserted into PROCESS_TABLE with a
    // populated `saved_user_state` (rax=0, parent's other GPRs +
    // RIP/RFLAGS/RSP at fork's SYSCALL boundary) and marked
    // `ring3_ready`. The next timer preempt (or any block-and-yield)
    // gives the child a slice; the scheduler round-robins between
    // parent and child thereafter.
    //
    // The parent's `fork()` syscall returns `child_pid` here; the
    // SYSCALL stub iretq's back to the parent at the post-SYSCALL
    // instruction with rax=child_pid in the usual way.

    // 1. Capture parent's user-mode callee-saved registers from the
    //    explicit slots the SYSCALL stub pushed onto the kernel stack.
    //    The historical `capture_callee_saved` naked helper read live
    //    registers and got Rust scratch values — the SYSCALL stub now
    //    pushes the user values to known offsets and the reader below
    //    is the only correct path.
    let saved =
        unsafe { crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs) };

    // 2. Read the user RIP (post-SYSCALL), RFLAGS, and original user
    //    R12 from the SYSCALL stub's saved-state slots above
    //    SyscallArgs. Layout is fixed by the stub:
    //       args +  56 = rcx (user RIP, post-SYSCALL)
    //       args +  64 = r11 (user RFLAGS)
    //       args + 112 = original user R12
    let user_rip;
    let user_rflags;
    let user_r12;
    unsafe {
        let p = args as *const SyscallArgs as *const u64;
        user_rip = core::ptr::read(p.add(7));
        user_rflags = core::ptr::read(p.add(8));
        user_r12 = crate::userland::user_state::read_user_r12(args as *const SyscallArgs);
        let _ = p; // silence unused warning if p is not otherwise used
    }

    // 3. Build the child's full register snapshot: same as parent at
    //    fork()'s post-SYSCALL boundary, except rax = 0 (the "I am
    //    the child" signal). When the scheduler eventually picks the
    //    child via `resume_ring3`, this is what gets restored.
    let child_saved_state = UserState {
        rax: 0, // child sees fork() return 0
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: saved.rbx,
        rbp: saved.rbp,
        rsp: saved.r12_register, // = user RSP from gs:[8]
        r12: user_r12,
        r13: saved.r13,
        r14: saved.r14,
        r15: saved.r15,
        rip: user_rip,
        rflags: user_rflags,
        // SYSCALL defines RCX/R11 as clobbered on return.
        rcx: 0,
        r11: 0,
    };

    // 4. Allocate the child PID. Pull parent's L4 frame under one lock,
    //    then build the child Process below.
    let child_pid = alloc_pid();
    let (parent_l4_frame, parent_vmas) = match with_current_process(|p| {
        p.address_space
            .as_ref()
            .map(|a| (a.l4_frame(), a.vmas().clone()))
    }) {
        Some(state) => state,
        None => {
            crate::debug_warn!("fork(): parent has no AddressSpace (test path?)");
            return ENOSYS;
        }
    };

    // 5. Eagerly clone the parent's address space (fresh L4 + copy of
    //    every leaf page in PML4[0]). Built on the parent's L4 — we
    //    haven't switched CR3 yet, and we don't intend to.
    let mut child_aspace =
        match crate::userland::address_space::AddressSpace::clone_for_child(parent_l4_frame) {
            Ok(a) => a,
            Err(e) => {
                crate::debug_error!("fork(): clone_for_child failed: {:?}", e);
                return -12; // ENOMEM
            }
        };
    *child_aspace.vmas_mut() = parent_vmas;

    // 6. Build the child Process. State pieces (FD table, cwd, brk,
    //    mmap) are cloned by value; address space ownership transfers.
    let child_process = with_current_process(|parent| crate::userland::lifecycle::Process {
        pid: child_pid,
        parent_pid: parent.pid,
        image: None, // child shares parent's image — kept implicitly
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: parent.brk_current,
        brk_base: parent.brk_base,
        mmap_next: parent.mmap_next,
        fd_table: parent.fd_table.clone(),
        network_wait: None,
        // POSIX timers are not inherited across fork.
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        // A fresh child is not mid-nanosleep.
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: parent.cwd.clone(),
        address_space: Some(child_aspace),
        // Phase 5 PR-B: child inherits parent's signal dispositions
        // and blocked mask. Pending mask resets to empty (POSIX:
        // pending signals are not inherited across fork).
        signal_state: parent.signal_state.fork_clone(),
        // Phase 5 PR-C1: child gets its own freshly-allocated kernel
        // stack so its SYSCALL handlers don't share rsp0 with the
        // parent's syscall handlers when both are alive concurrently.
        kernel_stack: Some(crate::userland::kernel_stack::KernelStack::new()),
        // U3: child shares parent's exe path (fork doesn't change the
        // running binary; execve replaces it).
        exe_path: parent.exe_path.clone(),
        // Demand-grown stack: child inherits the parent's exact stack
        // window. The parent's stack pages already copied into the
        // child's L4 via AddressSpace::clone_for_child above.
        stack_top: parent.stack_top,
        stack_bottom: parent.stack_bottom,
        stack_mapped_bottom: parent.stack_mapped_bottom,
        stack_max_growth_floor: parent.stack_max_growth_floor,
        growth_faults_remaining: parent.growth_faults_remaining,
        // U2: child inherits parent's FS_BASE (TLS pointer). musl in
        // the child may later re-call arch_prctl after fork to install
        // its own TCB.
        fs_base: parent.fs_base,
        // U2: fresh FPU state for the child (POSIX-Linux: FPU state is
        // not inherited across fork).
        fpu_state: crate::arch::x86_64::fpu::FpuState::fresh(),
        // U7: child's first resume reads this snapshot. rax=0 makes
        // child's fork() return 0; other regs match parent's state at
        // the SYSCALL boundary.
        saved_user_state: child_saved_state,
        // Routing: stdout/stderr flow to the same terminal as parent.
        terminal_id: parent.terminal_id,
    });

    // 7. Register child in PROCESS_TABLE and mark ready. The next
    //    scheduling decision (timer preempt, block-and-yield from
    //    parent, or top-level idle) picks the child via `resume_ring3`.
    insert_process(child_process);
    mark_ring3_ready(child_pid);

    crate::debug_info!(
        "fork(): registered child pid={} as Ready; parent returns immediately",
        child_pid
    );

    // 8. Parent's fork() returns the child PID via the normal SYSCALL
    //    stub return path. CR3, TSS.rsp0, GSBASE, FS_BASE, FPU all
    //    stay as the parent's — we never touched them.
    child_pid as i64
}

pub fn vfork_handler(args: &mut SyscallArgs) -> i64 {
    // vfork in real Linux runs the child sharing parent's memory until
    // exec/exit. We don't support that; route to ordinary COW fork.
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
    use crate::mm::paging::USER_MMAP_BASE;
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
    // Normalize once before the /bin namespace rewrite. `..` segments must
    // be collapsed before the prefix check.
    let normalized_path = crate::userland::lifecycle::with_current_process(|p| {
        crate::userland::path::normalize_path(&p.cwd, &raw_path)
    });
    // Virtual /bin namespace: rewrite the load path to either BB.ELF
    // (BusyBox applets) or GLAUNCH.ELF (kernel-side GUI apps) AND
    // override argv[0] so the chosen multicall binary's dispatcher picks
    // the requested applet. Linux preserves the caller's argv[0]
    // verbatim; we deviate here because both multicall binaries need
    // argv[0] to carry the applet name. Documented in
    // src/userland/bin_namespace.rs.
    let bin_rewrite = crate::userland::bin_namespace::apply_bin_rewrite(&normalized_path);
    let bin_applet = bin_rewrite.map(|(_, n)| n);
    let resolved_path = match bin_rewrite {
        Some((host_path, _)) => String::from(host_path),
        None => normalized_path,
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
    let (exec_file, bytes) = match crate::fs::file_handle::File::open_read(&resolved_path) {
        Ok(file) => match file.read_to_vec() {
            Ok(v) => (file, v),
            Err(ref e) => return map_file_err(e),
        },
        Err(ref e) => return map_file_err(e),
    };

    // 3. Build a fresh AddressSpace for the new image. If this fails,
    //    we haven't touched the old state yet — return cleanly.
    let mut new_aspace = match crate::userland::address_space::AddressSpace::new() {
        Ok(a) => a,
        Err(_) => return -12, // ENOMEM
    };

    // 4. Detach, but retain, the complete old VM transaction. Nothing in
    // the old image or page tables is modified until the replacement has
    // loaded, its stack has been built, and its VMA set is complete.
    let (old_image, old_aspace) = crate::userland::lifecycle::with_current_process(|p| {
        (p.image.take(), p.address_space.take())
    });

    // 6. Activate the new L4 and load the new ELF into it.
    // SAFETY: kernel half copied from kernel L4; the kernel code
    // post-CR3-write is still mapped.
    unsafe {
        new_aspace.activate();
    }

    let mut image = match crate::userland::loader::load_elf_file(&bytes, exec_file) {
        Ok(i) => i,
        Err(e) => {
            crate::debug_error!("execve(): load_elf failed: {:?}", e);
            if let Some(old) = old_aspace.as_ref() {
                unsafe {
                    old.activate();
                }
            }
            crate::userland::lifecycle::with_current_process(|p| {
                p.image = old_image;
                p.address_space = old_aspace;
            });
            return EINVAL;
        }
    };

    if new_aspace.initialize_vmas_from_image(&image).is_err() {
        // UserImage's construction rollback targets the currently active new
        // L4. Drop it before switching back to the old transaction.
        drop(image);
        if let Some(old) = old_aspace.as_ref() {
            unsafe {
                old.activate();
            }
        }
        crate::userland::lifecycle::with_current_process(|p| {
            p.image = old_image;
            p.address_space = old_aspace;
        });
        return ENOMEM;
    }

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
    // Demand-grown stack (U3): the new image carries its own stack
    // window. The old image's grown stack pages leak with the old
    // AddressSpace (bump allocator never reclaims anyway).
    let stack_initial_bottom = image.stack_initial_bottom;
    let stack_max_growth_floor = image.stack_max_growth_floor;
    let brk_base = image.brk_base;

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
    let user_rsp =
        super::build_initial_stack(stack_top, &phdr_bytes, e_phnum, &argv_refs, &envp_refs);

    // From commit onward the targeted AddressSpace walker is the sole page
    // owner; UserImage remains only executable metadata.
    image.transfer_mapping_ownership();

    // 10. Move new image and aspace onto the Process; reset brk/mmap
    //     anchors and exit info. Retain PID, parent_pid, FD table,
    //     cwd, continuation.
    crate::userland::lifecycle::with_current_process(|p| {
        p.image = Some(image);
        p.address_space = Some(new_aspace);
        p.brk_current = brk_base;
        p.brk_base = brk_base;
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
        // Demand-grown stack (U3): replace the stack window with the
        // new image's. exec resets the full growth budget.
        p.set_stack_window(
            stack_top,
            stack_initial_bottom,
            stack_initial_bottom,
            stack_max_growth_floor,
            crate::mm::paging::USER_STACK_MAX_GROWTH_PAGES,
        );
    });
    drop(old_image);
    drop(old_aspace);
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
        rdi: 0,
        rsi: 0,
        rdx: 0,
        r10: 0,
        r8: 0,
        r9: 0,
        rbx: 0,
        rbp: 0,
        rsp: user_rsp,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rip: entry,
        rflags: 0x202,
        rcx: 0,
        r11: 0,
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
    // U7: deliver to the parent in the regular PROCESS_TABLE (formerly
    // the PARENT_STASH slot). Returns ESRCH if `pid` doesn't match our
    // direct parent — kill(any, sig) lookup across arbitrary PIDs is
    // out of scope for this kernel.
    let parent_pid = crate::userland::lifecycle::with_current_process(|p| p.parent_pid as i32);
    if parent_pid == pid && pid != 0 {
        if sig == 0 {
            return 0;
        }
        let _ = crate::userland::lifecycle::with_process(pid as u32, |parent| {
            parent.signal_state.raise(sig);
        });
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
/// Read the frame, restore the user state AND the pre-delivery
/// signal mask, then `iretq` back to the pre-signal RIP/regs.
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
    // popped) sa_restorer pointer. The pre-delivery blocked mask sits
    // immediately after it; signum follows (not needed on return).
    let saved = match crate::userland::usercopy::read_unaligned::<UserState>(user_rsp) {
        Ok(saved) => saved,
        Err(e) => return e,
    };
    let saved_blocked = match crate::userland::usercopy::read_unaligned::<u64>(
        user_rsp + core::mem::size_of::<UserState>() as u64,
    ) {
        Ok(mask) => mask,
        Err(e) => return e,
    };

    // POSIX: restore the pre-delivery mask before resuming the
    // interrupted instruction. Without this, sa_mask (and the
    // implicitly-blocked signum-bit) leak into the rest of the
    // program's execution, corrupting subsequent signal delivery
    // decisions. zsh's parent post-fork was the load-bearing case:
    // SIGCHLD bit stayed blocked after the handler returned, so a
    // later check ran with the wrong mask and walked into a wild
    // pointer.
    crate::userland::lifecycle::with_current_process(|p| {
        p.signal_state.blocked = saved_blocked;
    });

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
///   user_rsp_at_handler_entry → [ sa_restorer            ]   8 bytes
///                                [ saved UserState       ]  144 bytes
///                                [ saved blocked mask    ]   8 bytes
///                                [ signum (i64)          ]   8 bytes
/// ```
/// Total 168 bytes, padded to 176 for 16-byte handler-entry RSP
/// alignment. The mask is saved here so `rt_sigreturn_handler` can
/// restore the pre-delivery `signal_state.blocked` — POSIX requires
/// the temporary handler mask (`sa_mask | bit(signum)`) installed
/// by the caller of `deliver_signal` to be reverted on handler
/// return.
///
/// SAFETY: `user_rsp_orig` must be a writable user-mapped address;
/// we write 168 bytes downward from there. The caller (the dispatcher)
/// reads it from the syscall stub's stashed `r12`, which is the
/// user's stack pointer at the point of the syscall — guaranteed
/// writable because the user just used it.
unsafe fn deliver_signal(
    signum: i32,
    action: crate::userland::signal::SigAction,
    args: &SyscallArgs,
    syscall_ret: i64,
    saved_blocked: u64,
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
    //    Read the user callee-saved set from the explicit slots the
    //    SYSCALL stub pushed (see `read_user_callee_saved` for layout).
    let p = args as *const SyscallArgs as *const u64;
    let user_rip = core::ptr::read(p.add(7));
    let user_rflags = core::ptr::read(p.add(8));
    let saved_regs =
        crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs);
    let user_r12_orig = crate::userland::user_state::read_user_r12(args as *const SyscallArgs);
    let user_rsp = saved_regs.r12_register; // = user RSP from gs:[8]
    let saved = UserState {
        rax: syscall_ret as u64,
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: saved_regs.rbx,
        rbp: saved_regs.rbp,
        rsp: user_rsp,
        r12: user_r12_orig,
        r13: saved_regs.r13,
        r14: saved_regs.r14,
        r15: saved_regs.r15,
        rip: user_rip,
        rflags: user_rflags,
        // SYSCALL defines RCX/R11 as clobbered on return.
        rcx: 0,
        r11: 0,
    };

    // 2. Allocate space on the user stack, 16-aligned. Space for
    //    [sa_restorer | UserState | saved_blocked | signum]; round up
    //    to the next 16-byte boundary so the handler entry RSP
    //    is aligned.
    const USER_STATE_SIZE: u64 = core::mem::size_of::<UserState>() as u64;
    const FRAME_SIZE: u64 = 8 + USER_STATE_SIZE + 8 + 8;
    let frame_total = (FRAME_SIZE + 15) & !15;
    let frame_addr = user_rsp - frame_total;

    // 3. Write the frame contents. We're running with CR3 = the user
    //    process's L4, so user-VA writes from kernel mode go to the
    //    right pages.
    let frame_write = (|| -> Result<(), i64> {
        crate::userland::usercopy::write_unaligned(frame_addr, &action.sa_restorer)?;
        crate::userland::usercopy::write_unaligned(frame_addr + 8, &saved)?;
        crate::userland::usercopy::write_unaligned(
            frame_addr + 8 + USER_STATE_SIZE,
            &saved_blocked,
        )?;
        crate::userland::usercopy::write_unaligned(
            frame_addr + 8 + USER_STATE_SIZE + 8,
            &(signum as u64),
        )
    })();
    if frame_write.is_err() {
        crate::userland::lifecycle::cleanup_user_process(
            crate::userland::lifecycle::AbnormalExit {
                vector: 14,
                error_code: Some(0x6),
                fault_addr: Some(VirtAddr::new(frame_addr)),
                fault_rip: VirtAddr::new(user_rip),
            },
        );
    }

    crate::debug_info!(
        "deliver_signal: sig={} handler={:#x} restorer={:#x} frame={:#x} saved_blocked={:#x}",
        signum,
        action.sa_handler,
        action.sa_restorer,
        frame_addr,
        saved_blocked,
    );

    // 4. Build a fresh UserState for the handler invocation.
    let handler_state = UserState {
        rax: 0,
        rdi: signum as u64, // handler(int sig)
        rsi: 0,             // siginfo_t* (SA_SIGINFO not supported)
        rdx: 0,             // ucontext_t*
        r10: 0,
        r8: 0,
        r9: 0,
        rbx: 0,
        rbp: 0,
        rsp: frame_addr,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rip: action.sa_handler,
        rflags: 0x202,
        rcx: 0,
        r11: 0,
    };

    let user_cs = crate::arch::x86_64::gdt::selectors().user_code.0 as u64;
    let user_ss = crate::arch::x86_64::gdt::selectors().user_data.0 as u64;
    super::iretq_to_user_with_regs(&handler_state as *const _, user_cs, user_ss);
}

/// Public wrapper so the dispatcher in `abi.rs` can call into the
/// delivery path without exposing the internal asm dance.
///
/// Atomically (under the Process lock) consumes a deliverable signal,
/// snapshots the pre-delivery `blocked` mask, and installs the POSIX
/// handler mask: `old_blocked | action.sa_mask | (1 << (sig-1))`,
/// stripping SIGKILL/SIGSTOP per POSIX. The snapshot is handed to
/// `deliver_signal` which writes it into the signal frame so
/// `rt_sigreturn_handler` can restore it when the handler returns.
/// Compute the blocked mask that should be active while a signal
/// handler runs: the pre-delivery mask, plus `sa_mask`, plus the
/// signum bit itself (POSIX default: a handler does not interrupt
/// itself unless `SA_NODEFER`, which we don't support yet). SIGKILL
/// and SIGSTOP are always stripped — POSIX guarantees they can never
/// be blocked.
pub fn handler_blocked_mask(old_blocked: u64, sa_mask: u64, signum: i32) -> u64 {
    use crate::userland::signal::{SIGKILL, SIGSTOP};
    let kill_stop_mask = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
    (old_blocked | sa_mask | (1u64 << (signum - 1))) & !kill_stop_mask
}

pub fn maybe_deliver_signal(args: &SyscallArgs, syscall_ret: i64) -> Option<i64> {
    // U9 invariant (production): the dispatcher tail calls us right
    // after a ring-3 SYSCALL returned through the dispatcher. The
    // SYSCALL came from ring 3 → `current_user_pid` is set to the
    // issuing process. If this fires with no current ring-3 process,
    // `with_current_process` would silently fall back to the sentinel
    // (PID 0) — harmless because nothing queues signals on the
    // sentinel, but a sign of a kernel-side bug. The check is gated
    // out of test builds because many tests drive `syscall_dispatch`
    // synthetically without a loaded user process.
    #[cfg(not(feature = "test"))]
    debug_assert!(
        crate::userland::lifecycle::current_user_pid().is_some(),
        "maybe_deliver_signal called with no current ring-3 process — would deliver to sentinel"
    );
    let prepared = crate::userland::lifecycle::with_current_process(|p| {
        let (sig, action) = p.signal_state.consume_deliverable()?;
        let old_blocked = p.signal_state.blocked;
        let handler_mask = handler_blocked_mask(old_blocked, action.sa_mask, sig);
        p.signal_state.blocked = handler_mask;
        Some((sig, action, old_blocked))
    });
    if let Some((sig, action, old_blocked)) = prepared {
        unsafe {
            deliver_signal(sig, action, args, syscall_ret, old_blocked);
        }
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
    if let Err(e) = crate::userland::usercopy::write_unaligned(fds_ptr, &read_fd) {
        let _ = with_fd_table_mut(|t| t.close(read_fd));
        let _ = with_fd_table_mut(|t| t.close(write_fd));
        return e;
    }
    if let Err(e) = crate::userland::usercopy::write_unaligned(fds_ptr + 4, &write_fd) {
        let _ = with_fd_table_mut(|t| t.close(read_fd));
        let _ = with_fd_table_mut(|t| t.close(write_fd));
        return e;
    }
    0
}

/// `wait4(pid, status, options, rusage) -> pid`.
///
/// U6 — POSIX-correct returns:
///   - Matching zombie present → reap and return its PID; status word
///     follows the WIFEXITED / WIFSIGNALED encoding driven by the
///     zombie's `signal_termination` field.
///   - No matching zombie, caller has no children → `-ECHILD` (POSIX).
///   - No matching zombie, caller HAS children, `WNOHANG` set → `0`.
///   - No matching zombie, caller HAS children, `WNOHANG` clear:
///     **today** returns `-ECHILD` (matches pre-U6 behavior so the
///     synchronous-fork callers don't regress); U5 will replace this
///     branch with `mark_ring3_blocked + yield` so the caller actually
///     parks. The wake path on the other side (`notify_parent_of_*`)
///     is already wired in U3.
///
/// WNOHANG is the only options bit handled today. WUNTRACED / WCONTINUED
/// require process-state tracking we don't implement.
pub fn wait4_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::abi::ECHILD;
    const WNOHANG: u64 = 1;

    let target = args.rdi as i32;
    let status_ptr = args.rsi;
    let options = args.rdx;
    let _rusage = args.r10;

    let me = crate::userland::lifecycle::current_pid();

    // Fast path: a matching zombie is already in the table.
    if let Some((pid, code, signal_termination)) =
        crate::userland::lifecycle::reap_zombie(target, me)
    {
        if status_ptr != 0 {
            // POSIX wait status encoding:
            //   Exited normally: bits 8..15 = exit code, bits 0..7 = 0
            //                    → WIFEXITED == true
            //   Killed by signal: bits 0..6 = signum, bit 7 = 0
            //                    → WIFSIGNALED == true (lo 7 bits 1..=126)
            let status = match signal_termination {
                Some(sig) => (sig as u32) & 0x7F,
                None => ((code as u32) & 0xFF) << 8,
            };
            if let Err(e) = crate::userland::usercopy::write_unaligned(status_ptr, &status) {
                return e;
            }
        }
        return pid as i64;
    }

    // No matching zombie. POSIX distinguishes "no children at all"
    // (-ECHILD) from "has children but none ready" (block, or return
    // 0 under WNOHANG).
    if !crate::userland::lifecycle::has_children(me) {
        return ECHILD;
    }

    if options & WNOHANG != 0 {
        // WNOHANG: return 0 to indicate "no status available yet."
        // Per POSIX `WIFEXITED(0)` is false, `WIFSIGNALED(0)` is false
        // — caller's loop interprets 0 as "try again later."
        return 0;
    }

    // U6: no matching zombie, has children, no WNOHANG → block until
    // a child exits (which calls `wake_ring3_blocked_on_child` →
    // moves us to ring3_ready) and re-fire the SYSCALL. The helper
    // captures the current user state with RIP rewound 2 bytes (the
    // SYSCALL instruction's length), marks us blocked, and yields to
    // the next runnable ring-3 process via `resume_ring3`. When we
    // eventually resume, our SYSCALL re-fires, this handler re-runs,
    // and the early reap_zombie at the top of this function finds
    // the matching zombie.
    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForChild { target },
        );
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

    // In-kernel ABI tests exercise the dispatcher without installing a
    // ring-3 process. There is no process to tear down or scheduler context
    // to yield from in that case; recording LAST_EXIT_CODE is the complete
    // synthetic contract. Some dispatcher tests select the persistent PID-0
    // kernel sentinel, which is synthetic too. Only a nonzero ring-3 PID may
    // follow the divergent lifecycle path below.
    if !matches!(
        crate::userland::lifecycle::current_user_pid(),
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID
    ) {
        return 0;
    }

    let (pid, parent_pid) =
        crate::userland::lifecycle::with_active_user(|au| (au.pid, au.parent_pid));

    // U7: every exit (top-level binary AND forked child) routes
    // through `notify_parent_of_exit` + `cooperative_exit`. The
    // exit path's `long_jump_to_run_or_halt` chooses the right
    // divergence: continuation present → long-jump to the
    // launching kernel thread (top-level binary); no continuation
    // → yield to the next runnable ring-3 process via resume_ring3
    // (forked child).
    //
    // `notify_parent_of_exit` is a no-op when parent_pid == 0 (top-
    // level kernel-launched binary — no userland parent).
    if parent_pid != 0 {
        crate::debug_info!(
            "USERLAND: child pid={} exit_group({}) — yielding to next ring-3",
            pid,
            code,
        );
    } else {
        crate::debug_info!(
            "USERLAND: exit_group({}) — long-jumping to run command",
            code
        );
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
const O_RDWR: u32 = 0o2;
const O_NONBLOCK: u32 = 0o4000;
const O_CREAT: u32 = 0o100;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;
/// Modify-the-world flags. Used to determine whether the caller is
/// asking for a write-side open.
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

const _R_OK: u32 = 4;
const _W_OK: u32 = 2;
const _X_OK: u32 = 1;

/// `clock_gettime` clock IDs we recognize. Realtime is anchored to the boot
/// RTC snapshot; monotonic remains PIT uptime.
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
        FE::InvalidPath => EINVAL,
        FE::ReadOnly => EROFS,
        FE::IsADirectory => EISDIR,
        FE::NotADirectory => ENOTDIR,
        FE::AlreadyExists => EEXIST,
        FE::NotEmpty => ENOTEMPTY,
        FE::DiskFull => ENOSPC,
        FE::BufferTooSmall => EFBIG,
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
        E::InvalidPath => ENOENT,
        E::NotAFile => EISDIR,
        E::NotADirectory => ENOTDIR,
        E::BufferTooSmall => EINVAL,
        E::NotImplemented => ENOSYS,
        E::IoError => EIO,
    }
}

/// Resolve a user path string against the active CWD into a normalized
/// kernel-side string. Runtime `/etc` lives in the root overlay.
fn resolve_user_path(ptr: u64) -> Result<String, i64> {
    let raw = copy_user_cstr(ptr)?;
    Ok(with_cwd(|cwd| normalize_path(cwd, &raw)))
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
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let cloexec = (flags & O_CLOEXEC) != 0;
    let want_write = (flags & O_WRITE_BITS) != 0 || (flags & O_ACCMODE) != O_RDONLY;
    // /bin namespace is always read-only — userland can't mutate the
    // synthesized applet entries.
    if want_write {
        use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir};
        if is_bin_dir(&path) || apply_bin_rewrite(&path).is_some() {
            return EPERM;
        }
        if crate::userland::etc::is_managed_path(&path) {
            return EROFS;
        }
        if !crate::fs::vfs::vfs_is_writable(&path) {
            return EROFS;
        }
    }

    // Virtual /bin namespace: opening /bin returns a directory FD that
    // getdents64 unpacks into the applet list; opening /bin/<applet>
    // returns a regular File backed by the underlying multicall binary
    // (BB.ELF or GLAUNCH.ELF) so tools that read or mmap their
    // argv[0] see something coherent.
    use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir};
    if is_bin_dir(&path) {
        return with_fd_table_mut(|t| t.alloc(FdSlot::VirtualBinDir { cursor: 0, cloexec }))
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE);
    }
    if let Some((host_path, _)) = apply_bin_rewrite(&path) {
        let handle = match crate::fs::file_handle::File::open_read(host_path) {
            Ok(h) => h,
            Err(ref e) => return map_file_err(e),
        };
        return with_fd_table_mut(|t| t.alloc(FdSlot::File { handle, cloexec }))
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE);
    }

    // Check whether the path exists and is a directory. Directories
    // open via the snapshot Directory API (no write semantics — Linux
    // permits O_DIRECTORY but mutating a dir fd is meaningless here).
    use crate::fs::filesystem::FileType;
    let meta = crate::fs::metadata(&path);

    // O_CREAT semantics: when set, NotFound is not an error — the
    // subsequent open will create the file. Other errors propagate.
    let want_create = (flags & O_CREAT) != 0;
    let meta_result = match meta {
        Ok(m) => Ok(m),
        Err(ref e) => {
            if want_create {
                Err(()) // signal "needs create" without leaking the underlying error type
            } else {
                return map_fs_err(e);
            }
        }
    };

    if let Ok(m) = meta_result {
        if m.file_type == FileType::Directory {
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
    }

    // Map Linux flags to our FileMode.
    let access = flags & O_ACCMODE;
    let mode = crate::fs::filesystem::FileMode {
        read: access == O_RDONLY || access == O_RDWR,
        write: want_write,
        append: (flags & O_APPEND) != 0,
        create: want_create,
        truncate: (flags & O_TRUNC) != 0,
    };

    let handle = match crate::fs::file_handle::File::open(&path, mode) {
        Ok(h) => h,
        Err(ref e) => return map_file_err(e),
    };
    with_fd_table_mut(|t| t.alloc(FdSlot::File { handle, cloexec }))
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
    let result = with_fd_table_mut(|t| t.close(fd)).err().unwrap_or(0);
    drop(slot);
    crate::net::drain_deferred_closes();
    crate::userland::lifecycle::wake_ring3_blocked_on_network(true);
    result
}

// ---------- Phase B: namespace mutations (mkdir/unlink/rmdir/rename/…) ----------

/// Validate `path` is not a `/bin` rewrite target. Mutations on the
/// synthesized applet namespace are always rejected with `EPERM`.
fn bin_namespace_mutation_check(path: &str) -> Option<i64> {
    use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir};
    if is_bin_dir(path) || apply_bin_rewrite(path).is_some() {
        return Some(EPERM);
    }
    None
}

/// Kernel-owned `/etc` entries may be read normally but namespace mutations
/// are rejected. Callers pass normalized paths from `resolve_user_path`, so
/// `.`/`..` aliases cannot bypass this component-bounded check.
fn managed_etc_mutation_check(path: &str) -> Option<i64> {
    crate::userland::etc::is_managed_path(path).then_some(EPERM)
}

pub fn mkdir_handler(args: &mut SyscallArgs) -> i64 {
    let path = match resolve_user_path(args.rdi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(e) = bin_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&path) {
        return e;
    }
    match crate::fs::vfs::vfs_mkdir(&path) {
        Ok(()) => 0,
        Err(ref e) => map_filesystem_err(e),
    }
}

pub fn mkdirat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let mut new_args = SyscallArgs {
        rax: args.rax,
        rdi: args.rsi,
        rsi: args.rdx,
        rdx: 0,
        r10: 0,
        r8: 0,
        r9: 0,
    };
    mkdir_handler(&mut new_args)
}

pub fn rmdir_handler(args: &mut SyscallArgs) -> i64 {
    let path = match resolve_user_path(args.rdi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(e) = bin_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&path) {
        return e;
    }
    if path == "/" {
        return EBUSY;
    }
    match crate::fs::vfs::vfs_rmdir(&path) {
        Ok(()) => 0,
        Err(ref e) => map_filesystem_err(e),
    }
}

pub fn unlink_handler(args: &mut SyscallArgs) -> i64 {
    let path = match resolve_user_path(args.rdi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(e) = bin_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&path) {
        return e;
    }
    match crate::fs::vfs::vfs_unlink(&path) {
        Ok(()) => 0,
        Err(ref e) => map_filesystem_err(e),
    }
}

pub fn unlinkat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let flags = args.rdx as u32;
    let path = match resolve_user_path(args.rsi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(e) = bin_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&path) {
        return e;
    }
    // AT_REMOVEDIR = 0x200
    if flags & 0x200 != 0 {
        if path == "/" {
            return EBUSY;
        }
        match crate::fs::vfs::vfs_rmdir(&path) {
            Ok(()) => 0,
            Err(ref e) => map_filesystem_err(e),
        }
    } else {
        match crate::fs::vfs::vfs_unlink(&path) {
            Ok(()) => 0,
            Err(ref e) => map_filesystem_err(e),
        }
    }
}

pub fn rename_handler(args: &mut SyscallArgs) -> i64 {
    let old = match resolve_user_path(args.rdi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new = match resolve_user_path(args.rsi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(e) = bin_namespace_mutation_check(&old) {
        return e;
    }
    if let Some(e) = bin_namespace_mutation_check(&new) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&old) {
        return e;
    }
    if let Some(e) = managed_etc_mutation_check(&new) {
        return e;
    }
    match crate::fs::vfs::vfs_rename(&old, &new) {
        Ok(()) => 0,
        // UnsupportedOperation from vfs_rename signals cross-mount.
        Err(crate::fs::filesystem::FilesystemError::UnsupportedOperation) => EXDEV,
        Err(ref e) => map_filesystem_err(e),
    }
}

pub fn renameat_handler(args: &mut SyscallArgs) -> i64 {
    let old_dfd = args.rdi as i32;
    let new_dfd = args.rdx as i32;
    if old_dfd != AT_FDCWD || new_dfd != AT_FDCWD {
        return ENOSYS;
    }
    let mut new_args = SyscallArgs {
        rax: args.rax,
        rdi: args.rsi,
        rsi: args.r10,
        rdx: 0,
        r10: 0,
        r8: 0,
        r9: 0,
    };
    rename_handler(&mut new_args)
}

/// `creat(path, mode) -> int` — equivalent to
/// `open(path, O_WRONLY|O_CREAT|O_TRUNC, mode)`.
pub fn creat_handler(args: &mut SyscallArgs) -> i64 {
    // O_WRONLY=1, O_CREAT=0o100, O_TRUNC=0o1000
    let flags = 1u32 | O_CREAT | O_TRUNC;
    open_common(AT_FDCWD, args.rdi, flags)
}

pub fn ftruncate_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let new_size = args.rsi;
    let handle = match with_fd_slot(fd) {
        Some(FdSlot::File { handle, .. }) => handle,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(_) | None => return EBADF,
    };
    if crate::userland::etc::is_managed_path(&handle.path()) {
        return EROFS;
    }
    // Use File::write through seek + write to (re)size the file. The
    // Filesystem trait doesn't expose truncate as a method yet
    // (deferred to Phase C plan); for tmpfs the open-handle table
    // already supports resize via the existing write path's
    // extend-on-write semantic. For a true truncate (including
    // shrink), we extend a write of zero bytes after seeking.
    //
    // TODO(Phase C): proper Filesystem::truncate. For now: tmpfs and
    // overlay both grow files via write-past-end; shrinks are
    // approximated by seeking to new_size and zero-writing only when
    // extending. Real shrink isn't supported yet.
    if handle.size() > new_size {
        // No shrink mechanism via current trait — return ENOSYS so
        // userland sees a clear unsupported signal rather than a
        // silent partial truncate.
        return ENOSYS;
    }
    if let Err(ref e) = handle.seek(new_size) {
        return map_file_err(e);
    }
    // Extend by writing a single zero byte at position new_size-1,
    // then truncate the position back. tmpfs's write extends with
    // zeros so this is correct.
    if new_size > 0 {
        if let Err(ref e) = handle.seek(new_size - 1) {
            return map_file_err(e);
        }
        if let Err(ref e) = handle.write(&[0u8]) {
            return map_file_err(e);
        }
    }
    0
}

pub fn truncate_handler(args: &mut SyscallArgs) -> i64 {
    let path = match resolve_user_path(args.rdi) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let new_size = args.rsi;
    if let Some(e) = bin_namespace_mutation_check(&path) {
        return e;
    }
    if crate::userland::etc::is_managed_path(&path) {
        return EROFS;
    }
    if !crate::fs::vfs::vfs_is_writable(&path) {
        return EROFS;
    }
    let mode = crate::fs::filesystem::FileMode {
        read: false,
        write: true,
        append: false,
        create: false,
        truncate: false,
    };
    let handle = match crate::fs::file_handle::File::open(&path, mode) {
        Ok(h) => h,
        Err(ref e) => return map_file_err(e),
    };
    if handle.size() > new_size {
        return ENOSYS;
    }
    if new_size > 0 {
        if let Err(ref e) = handle.seek(new_size - 1) {
            return map_file_err(e);
        }
        if let Err(ref e) = handle.write(&[0u8]) {
            return map_file_err(e);
        }
    }
    0
}

pub fn fsync_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    match with_fd_slot(fd) {
        Some(FdSlot::File { .. }) => {
            let _ = crate::fs::vfs::vfs_sync_all();
            0
        }
        Some(FdSlot::Directory { .. }) => 0,
        Some(_) | None => EBADF,
    }
}

pub fn fdatasync_handler(args: &mut SyscallArgs) -> i64 {
    fsync_handler(args)
}

pub fn sync_handler(_args: &mut SyscallArgs) -> i64 {
    // POSIX `sync` returns void and never fails. We surface
    // filesystem errors as -EIO but most mounts have a no-op sync.
    match crate::fs::vfs::vfs_sync_all() {
        Ok(()) => 0,
        Err(_) => 0,
    }
}

pub fn syncfs_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    if with_fd_slot(fd).is_none() {
        return EBADF;
    }
    sync_handler(args)
}

pub fn pread64_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let ptr = args.rsi;
    let len = args.rdx;
    let offset = args.r10;
    let handle = match with_fd_slot(fd) {
        Some(FdSlot::File { handle, .. }) => handle,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(_) | None => return EBADF,
    };
    if len > WRITE_MAX_LEN as u64 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }
    let prev = handle.position();
    if let Err(ref e) = handle.seek(offset) {
        return map_file_err(e);
    }
    let mut buf = alloc::vec![0u8; len as usize];
    let n = match handle.read(&mut buf) {
        Ok(n) => n,
        Err(ref e) => {
            let _ = handle.seek(prev);
            return map_file_err(e);
        }
    };
    let _ = handle.seek(prev);
    if let Err(e) = crate::userland::usercopy::copy_to_user(ptr, &buf[..n]) {
        return e;
    }
    n as i64
}

pub fn pwrite64_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let ptr = args.rsi;
    let len = args.rdx;
    let offset = args.r10;
    let handle = match with_fd_slot(fd) {
        Some(FdSlot::File { handle, .. }) => handle,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(_) | None => return EBADF,
    };
    if len > WRITE_MAX_LEN as u64 {
        return EFAULT;
    }
    if len == 0 {
        return 0;
    }
    let mut bytes = alloc::vec![0u8; len as usize];
    if let Err(e) = crate::userland::usercopy::copy_from_user(&mut bytes, ptr) {
        return e;
    }
    let prev = handle.position();
    if let Err(ref e) = handle.seek(offset) {
        return map_file_err(e);
    }
    let n = match handle.write(&bytes) {
        Ok(n) => n,
        Err(ref e) => {
            let _ = handle.seek(prev);
            return map_file_err(e);
        }
    };
    let _ = handle.seek(prev);
    n as i64
}

/// `sendfile(out_fd, in_fd, *offset, count) -> isize`
///
/// Copies up to `count` bytes from `in_fd` to `out_fd` inside the kernel,
/// avoiding the read/write/user-buffer round-trip. BusyBox's
/// `bb_full_fd_action` calls this with `offset == NULL` when copying from
/// a regular file (e.g. `cat`, `cp`'s read path), so the most important
/// shape to support is "in_fd is a regular file, out_fd is stdout/pipe".
///
/// Semantics:
/// - `in_fd` must be a regular file. Streams/pipes/dirs are rejected.
/// - `out_fd` may be stdout/stderr, a pipe write end, or a regular file.
/// - When `offset == NULL`, reads from `in_fd`'s current position and
///   advances it; when non-NULL, reads from `*offset`, leaves `in_fd`'s
///   position unchanged, and writes the next read position back to
///   `*offset`.
/// - Returns bytes copied (possibly less than `count` on short writes,
///   EOF, or a mid-transfer error); only returns `-errno` when nothing
///   was copied.
pub fn sendfile_handler(args: &mut SyscallArgs) -> i64 {
    let out_fd = args.rdi as i32;
    let in_fd = args.rsi as i32;
    let offset_ptr = args.rdx;
    let count = args.r10;

    let in_handle = match with_fd_slot(in_fd) {
        Some(FdSlot::File { handle, .. }) => handle,
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(_) => return EINVAL,
        None => return EBADF,
    };

    enum Out {
        StdoutErr,
        File(crate::lib::arc::Arc<crate::fs::file_handle::File>),
        Pipe(crate::userland::pipe::PipeWriteHandle),
    }
    let out = match with_fd_slot(out_fd) {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => Out::StdoutErr,
        Some(FdSlot::File { handle, .. }) => Out::File(handle),
        Some(FdSlot::PipeWrite(h, _)) => Out::Pipe(h),
        Some(FdSlot::Directory { .. }) | Some(FdSlot::VirtualBinDir { .. }) => return EISDIR,
        Some(_) | None => return EBADF,
    };

    let saved_pos = in_handle.position();
    if offset_ptr != 0 {
        let off = match crate::userland::usercopy::read_unaligned::<u64>(offset_ptr) {
            Ok(off) => off,
            Err(e) => return e,
        };
        if let Err(ref e) = in_handle.seek(off) {
            return map_file_err(e);
        }
    }

    const CHUNK: usize = 4096;
    let mut buf = vec![0u8; CHUNK];
    let mut remaining = count;
    let mut total: u64 = 0;
    let mut error: Option<i64> = None;

    while remaining > 0 {
        let want = core::cmp::min(remaining as usize, CHUNK);
        let n = match in_handle.read(&mut buf[..want]) {
            Ok(n) => n,
            Err(ref e) => {
                error = Some(map_file_err(e));
                break;
            }
        };
        if n == 0 {
            break;
        }
        let written = match &out {
            Out::StdoutErr => {
                let s = alloc::string::String::from_utf8_lossy(&buf[..n]);
                crate::print!("{}", s);
                n
            }
            Out::File(handle) => match handle.write(&buf[..n]) {
                Ok(w) => w,
                Err(ref e) => {
                    error = Some(map_file_err(e));
                    0
                }
            },
            Out::Pipe(handle) => {
                if handle.pipe().readers() == 0 {
                    error = Some(crate::userland::abi::EPIPE);
                    0
                } else {
                    handle.pipe().write(&buf[..n])
                }
            }
        };
        total += written as u64;
        remaining = remaining.saturating_sub(written as u64);
        if error.is_some() || written < n {
            break;
        }
    }

    if offset_ptr != 0 {
        let new_off = in_handle.position();
        let _ = in_handle.seek(saved_pos);
        if let Err(e) = crate::userland::usercopy::write_unaligned(offset_ptr, &new_off) {
            return if total > 0 { total as i64 } else { e };
        }
    }

    if total > 0 {
        return total as i64;
    }
    error.unwrap_or(0)
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
    let result = with_fd_table_mut(|t| t.dup2(oldfd, newfd))
        .map(|n| n as i64)
        .unwrap_or(EBADF);
    crate::net::drain_deferred_closes();
    crate::userland::lifecycle::wake_ring3_blocked_on_network(true);
    result
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
                Some(FdSlot::Socket { handle, .. }) => {
                    let nonblocking = crate::net::socket::nonblocking(handle.id()).unwrap_or(false);
                    (O_RDWR | if nonblocking { O_NONBLOCK } else { 0 }) as i64
                }
                Some(_) => O_RDONLY as i64,
                None => EBADF,
            }
        }
        F_SETFL => match with_fd_slot(fd) {
            Some(FdSlot::Socket { handle, .. }) => {
                crate::net::socket::set_nonblocking(handle.id(), arg & O_NONBLOCK as u64 != 0)
                    .map_or_else(crate::userland::network_syscalls::map_socket_error, |_| 0)
            }
            Some(_) => 0,
            None => EBADF,
        },
        _ => ENOSYS,
    }
}

// ---------- stat / access ----------

fn fill_stat(
    meta: &crate::fs::filesystem::DirectoryEntry,
    size_override: Option<u64>,
) -> LinuxStat {
    use crate::fs::filesystem::FileType;
    let is_dir = meta.file_type == FileType::Directory;
    let mut st = LinuxStat::default();
    st.st_mode = if is_dir {
        S_IFDIR | PERM_RX_ALL
    } else {
        S_IFREG | PERM_READ_ALL
    };
    st.st_nlink = if is_dir { 2 } else { 1 };
    st.st_uid = 0;
    st.st_gid = 0;
    st.st_size = size_override.unwrap_or(meta.size) as i64;
    st.st_blksize = 4096;
    st.st_blocks = (st.st_size + 511) / 512;
    st
}

fn write_stat(out_ptr: u64, st: &LinuxStat) -> i64 {
    crate::userland::usercopy::write_unaligned(out_ptr, st).map_or_else(|e| e, |_| 0)
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
/// regular file shadowing the appropriate multicall binary — `BB.ELF`
/// for BusyBox applets, `GLAUNCH.ELF` for GUI apps); `None` for any
/// other path.
fn stat_virtual_bin(path: &str) -> Option<LinuxStat> {
    use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir, merged_bin_entry_count};
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
    if let Some((host_path, _)) = apply_bin_rewrite(path) {
        // Stat shadows the underlying multicall binary. Pull its size
        // off the FAT mount so tools that mmap their argv[0] see a
        // sensible length. If the binary isn't staged the kernel
        // returns a zero-size record rather than failing — applet PATH
        // lookup still works (access() returns 0) and execve() will
        // report the real error when it fails to load.
        let size = crate::fs::metadata(host_path).map(|m| m.size).unwrap_or(0);
        let mut st = LinuxStat::default();
        st.st_mode = S_IFREG | PERM_RX_ALL;
        st.st_nlink = merged_bin_entry_count() as u64;
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
        Some(FdSlot::Socket { .. }) => {
            const S_IFSOCK: u32 = 0o140000;
            let mut st = LinuxStat::default();
            st.st_mode = S_IFSOCK | 0o600;
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
    let mut bytes = cwd.into_bytes();
    bytes.push(0);
    if let Err(e) = crate::userland::usercopy::copy_to_user(buf, &bytes) {
        return e;
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
#[derive(Default, Clone, Copy)]
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
    let ns = if clk == CLOCK_REALTIME {
        crate::time::realtime_ns()
    } else {
        crate::time::monotonic_ns()
    };
    let ts = LinuxTimespec {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
    };
    crate::userland::usercopy::write_unaligned(ts_ptr, &ts).map_or_else(|e| e, |_| 0)
}

pub fn gettimeofday_handler(args: &mut SyscallArgs) -> i64 {
    let tv_ptr = args.rdi;
    let _tz_ptr = args.rsi; // legacy timezone arg, ignored
    if tv_ptr == 0 {
        return 0;
    }
    let ns = crate::time::realtime_ns();
    let tv = LinuxTimeval {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_usec: ((ns % 1_000_000_000) / 1_000) as i64,
    };
    crate::userland::usercopy::write_unaligned(tv_ptr, &tv).map_or_else(|e| e, |_| 0)
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
    let mut bytes = alloc::vec![0u8; cap as usize];
    // xorshift64* — small, fast, deterministic enough for libc seed needs.
    let mut state: u64 = crate::time::monotonic_ns() ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 0xDEAD_BEEF_CAFE_BABE;
    }
    for i in 0..cap {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let byte = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8;
        bytes[i as usize] = byte;
    }
    crate::userland::usercopy::copy_to_user(buf, &bytes).map_or_else(|e| e, |_| cap as i64)
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
    use crate::userland::bin_namespace::{merged_bin_entries, merged_bin_entry_count};

    let start = with_fd_table_mut(|t| match t.get(fd) {
        Some(FdSlot::VirtualBinDir { cursor, .. }) => Some(*cursor),
        _ => None,
    })?;
    let total_records = merged_bin_entry_count() + 2; // ".", ".." + entries
    if start >= total_records {
        // EOF on this directory.
        return Some(0);
    }

    let mut staging: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(cap);
    let mut cursor = start;
    // Synthetic inode numbers — keep them deterministic and non-zero.
    let parent_seed = fnv1a_64(0xcbf2_9ce4_8422_2325, b"/bin");

    // Materialize merged entries once so we can index by cursor without
    // re-walking the iterator. Two ~150-entry &'static lists; allocation
    // is negligible vs. the syscall cost.
    let entries: alloc::vec::Vec<&'static str> = merged_bin_entries().collect();

    while cursor < total_records {
        let (name, d_type) = match cursor {
            0 => (".".as_bytes(), DT_DIR),
            1 => ("..".as_bytes(), DT_DIR),
            n => (entries[n - 2].as_bytes(), DT_REG),
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
    Some(
        crate::userland::usercopy::copy_to_user(dirp, &staging)
            .map_or_else(|e| e, |_| staging.len() as i64),
    )
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
    crate::userland::usercopy::copy_to_user(dirp, &staging)
        .map_or_else(|e| e, |_| staging.len() as i64)
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
    crate::userland::usercopy::write_unaligned(out_ptr, &u).map_or_else(|e| e, |_| 0)
}

// ---------- U3: musl-init / zsh-startup syscalls ----------

// poll/ppoll constants. POLLNVAL is the only one we generate when an
// fd isn't valid; the others are just bit copies from `events` to
// `revents` for valid stream fds.
const POLLIN: i16 = 0x0001;
const POLLOUT: i16 = 0x0004;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
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
    let timeout_ms = args.rdx as i32;
    let timeout_ticks = if timeout_ms < 0 {
        None
    } else {
        Some(((timeout_ms as u64) + 9) / 10)
    };
    poll_common(args, args.rdi, args.rsi, timeout_ticks)
}

/// `ppoll(fds, nfds, *timeout, *sigmask, sigsetsize) -> int`
///
/// Linux-x86-64 ppoll. We ignore the timespec, sigmask, and sigsetsize;
/// shape is identical to `poll` for our purposes.
pub fn ppoll_handler(args: &mut SyscallArgs) -> i64 {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Timespec {
        seconds: i64,
        nanoseconds: i64,
    }
    let timeout_ticks = if args.rdx == 0 {
        None
    } else {
        let timeout = match crate::userland::usercopy::read_unaligned::<Timespec>(args.rdx) {
            Ok(timeout) => timeout,
            Err(error) => return error,
        };
        if timeout.seconds < 0 || !(0..1_000_000_000).contains(&timeout.nanoseconds) {
            return EINVAL;
        }
        let milliseconds = (timeout.seconds as u64)
            .saturating_mul(1000)
            .saturating_add((timeout.nanoseconds as u64 + 999_999) / 1_000_000);
        Some((milliseconds + 9) / 10)
    };
    poll_common(args, args.rdi, args.rsi, timeout_ticks)
}

fn poll_common(args: &SyscallArgs, fds_ptr: u64, nfds: u64, timeout_ticks: Option<u64>) -> i64 {
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
    let mut ready = 0i64;
    let mut has_socket = false;
    crate::net::poll_once();
    for index in 0..nfds {
        let address = fds_ptr + index * core::mem::size_of::<PollFd>() as u64;
        let mut entry = match crate::userland::usercopy::read_unaligned::<PollFd>(address) {
            Ok(entry) => entry,
            Err(e) => return e,
        };
        let want = entry.events;
        // Linux ignores negative descriptors in a pollfd array. musl's DNS
        // resolver uses fd=-1 for inactive per-query TCP fallback slots.
        let revents = if entry.fd < 0 {
            0
        } else {
            match with_fd_slot(entry.fd) {
                Some(FdSlot::Socket { handle, .. }) => {
                    has_socket = true;
                    match crate::net::socket::readiness(handle.id()) {
                        Ok(state) => {
                            let mut events = 0;
                            if state.readable {
                                events |= want & POLLIN;
                            }
                            if state.writable {
                                events |= want & POLLOUT;
                            }
                            if state.error {
                                events |= POLLERR;
                            }
                            if state.hangup {
                                events |= POLLHUP;
                            }
                            events
                        }
                        Err(_) => POLLERR,
                    }
                }
                Some(_) => want & (POLLIN | POLLOUT), // preserve existing behavior
                None => POLLNVAL,
            }
        };
        entry.revents = revents;
        if revents != 0 {
            ready += 1;
        }
        if let Err(e) = crate::userland::usercopy::write_unaligned(address, &entry) {
            return e;
        }
    }
    if ready != 0 || timeout_ticks == Some(0) || !has_socket {
        if ready != 0 {
            crate::userland::lifecycle::clear_network_wait();
        }
        return ready;
    }
    let identity = fds_ptr ^ nfds.rotate_left(17);
    crate::userland::network_syscalls::block_poll(args, identity, timeout_ticks)
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
    crate::userland::usercopy::copy_to_user(buf_ptr, &bytes[..n]).map_or_else(|e| e, |_| n as i64)
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
        FdSlot::Socket { handle, .. } => alloc::format!("socket:[{}]", handle.id()),
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
    let r = LinuxRlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    };
    crate::userland::usercopy::write_unaligned(out_ptr, &r).map_or_else(|e| e, |_| 0)
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
    let zero = LinuxRusage::default();
    crate::userland::usercopy::write_unaligned(out_ptr, &zero).map_or_else(|e| e, |_| 0)
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

const ITIMER_REAL: i32 = 0;
const PIT_MICROSECONDS_PER_TICK: u64 = 10_000;

fn timeval_to_ticks(seconds: i64, microseconds: i64) -> Result<u64, i64> {
    if seconds < 0 || !(0..1_000_000).contains(&microseconds) {
        return Err(EINVAL);
    }
    let total = (seconds as u64)
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(microseconds as u64))
        .ok_or(EINVAL)?;
    if total == 0 {
        return Ok(0);
    }
    Ok(total.saturating_add(PIT_MICROSECONDS_PER_TICK - 1) / PIT_MICROSECONDS_PER_TICK)
}

fn ticks_to_timeval(ticks: u64) -> (i64, i64) {
    let total = ticks.saturating_mul(PIT_MICROSECONDS_PER_TICK);
    ((total / 1_000_000) as i64, (total % 1_000_000) as i64)
}

/// `setitimer(which: i32, *new_value, *old_value) -> int`.
///
/// ITIMER_REAL uses the monotonic 100 Hz PIT and queues SIGALRM from kernel
/// housekeeping. ITIMER_VIRTUAL and ITIMER_PROF remain unsupported.
pub fn setitimer_handler(args: &mut SyscallArgs) -> i64 {
    if args.rdi as i32 != ITIMER_REAL {
        return EINVAL;
    }

    let new_value = if args.rsi == 0 {
        LinuxItimerval::default()
    } else {
        match crate::userland::usercopy::read_unaligned::<LinuxItimerval>(args.rsi) {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
    let interval_ticks =
        match timeval_to_ticks(new_value.it_interval_sec, new_value.it_interval_usec) {
            Ok(value) => value,
            Err(error) => return error,
        };
    let value_ticks = match timeval_to_ticks(new_value.it_value_sec, new_value.it_value_usec) {
        Ok(value) => value,
        Err(error) => return error,
    };

    let old_ptr = args.rdx;
    if old_ptr != 0 {
        if let Err(error) = crate::userland::usercopy::ensure_user_range(
            old_ptr,
            core::mem::size_of::<LinuxItimerval>() as u64,
            true,
        ) {
            return error;
        }
    }

    let now = crate::arch::x86_64::interrupts::get_timer_ticks();
    let old_value = crate::userland::lifecycle::with_current_process(|process| {
        let remaining_ticks = process
            .real_timer
            .deadline_tick
            .map(|deadline| deadline.saturating_sub(now))
            .unwrap_or(0);
        let (interval_sec, interval_usec) = ticks_to_timeval(process.real_timer.interval_ticks);
        let (value_sec, value_usec) = ticks_to_timeval(remaining_ticks);
        let old = LinuxItimerval {
            it_interval_sec: interval_sec,
            it_interval_usec: interval_usec,
            it_value_sec: value_sec,
            it_value_usec: value_usec,
        };
        process.real_timer.interval_ticks = interval_ticks;
        process.real_timer.deadline_tick =
            (value_ticks != 0).then(|| now.saturating_add(value_ticks));
        old
    });

    if old_ptr != 0 {
        return crate::userland::usercopy::write_unaligned(old_ptr, &old_value)
            .map_or_else(|error| error, |_| 0);
    }
    0
}

/// `nanosleep(*req: *const timespec, *rem: *mut timespec) -> int`
///
/// Blocks the calling ring-3 process until the requested duration elapses
/// against the monotonic 100 Hz PIT, then returns 0. The duration is rounded
/// up to whole ticks so a sub-tick request still yields the CPU rather than
/// busy-spinning — self-driven ring-3 animation loops (e.g. `PAINTING.ELF`)
/// and zsh's `sleep`/`usleep` builtins both depend on this.
///
/// Signal interruption (`-EINTR` + remaining time in `rem`) is not modeled: a
/// woken-but-not-yet-elapsed sleeper simply re-blocks for the remainder. If
/// `rem` is non-null on completion, a zeroed timespec is written.
pub fn nanosleep_handler(args: &mut SyscallArgs) -> i64 {
    /// PIT period: 100 Hz ⇒ 10 ms ⇒ 10,000,000 ns per tick.
    const NS_PER_TICK: u64 = 10_000_000;
    const TICKS_PER_SEC: u64 = 100;

    let req_ptr = args.rdi;
    let rem_ptr = args.rsi;

    let requested_ticks = if req_ptr == 0 {
        0
    } else {
        match crate::userland::usercopy::read_unaligned::<LinuxTimespec>(req_ptr) {
            Ok(ts) => {
                let sec = ts.tv_sec.max(0) as u64;
                let nsec = ts.tv_nsec.clamp(0, 999_999_999) as u64;
                // Round the sub-second remainder up so any positive request
                // sleeps at least one tick.
                let frac_ticks = nsec.div_ceil(NS_PER_TICK);
                sec.saturating_mul(TICKS_PER_SEC).saturating_add(frac_ticks)
            }
            Err(e) => return e,
        }
    };

    match crate::userland::lifecycle::nanosleep_deadline(requested_ticks) {
        Some(deadline) => unsafe {
            crate::userland::switch::block_current_ring3_and_yield(
                args,
                crate::userland::lifecycle::Ring3BlockReason::Sleeping {
                    deadline_tick: deadline,
                },
            )
        },
        None => {
            if rem_ptr != 0 {
                let zero = LinuxTimespec::default();
                return crate::userland::usercopy::write_unaligned(rem_ptr, &zero)
                    .map_or_else(|e| e, |_| 0);
            }
            0
        }
    }
}

/// AgenticOS-internal syscall `gui_launch(name_ptr, name_len) -> 0 | -errno`.
///
/// Looks `name` up in the kernel-side GUI applet table
/// (`crate::commands::gui_launch_table::spawn_by_name`) and spawns the
/// matching kernel-side GUI app process. Invoked by `GLAUNCH.ELF`
/// (ring 3) — the user-typed `painting` in zsh resolves via the
/// `/bin/<gui_applet>` rewrite in [`crate::userland::bin_namespace`] to
/// the GUILAUNCH multicall binary with `argv[0] = "painting"`, which in
/// turn issues this syscall.
///
/// `name_len` is capped at 32 to bound the user copy. Unknown applets
/// return `-ENOENT`; spawn failure surfaces as the underlying errno.
pub fn gui_launch_handler(args: &mut SyscallArgs) -> i64 {
    const MAX_NAME_LEN: u64 = 32;
    let name_ptr = args.rdi;
    let name_len = args.rsi;

    crate::debug_info!(
        "[gui_launch] enter: name_ptr={:#x}, name_len={}",
        name_ptr,
        name_len,
    );

    if name_ptr == 0 {
        crate::debug_warn!("[gui_launch] EFAULT: null name_ptr");
        return EFAULT;
    }
    if name_len == 0 || name_len > MAX_NAME_LEN {
        crate::debug_warn!("[gui_launch] EINVAL: name_len={} out of range", name_len);
        return EINVAL;
    }
    let mut bytes = alloc::vec![0u8; name_len as usize];
    if let Err(e) = crate::userland::usercopy::copy_from_user(&mut bytes, name_ptr) {
        crate::debug_warn!("[gui_launch] user copy failed: errno={}", e);
        return e;
    }
    let name = match core::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(_) => {
            crate::debug_warn!("[gui_launch] EINVAL: name is not valid UTF-8");
            return EINVAL;
        }
    };

    crate::debug_info!("[gui_launch] name={:?}, dispatching to spawn_by_name", name);
    match crate::commands::gui_launch_table::spawn_by_name(name) {
        Ok(pid) => {
            crate::debug_info!("[gui_launch] spawned pid={:?}, returning 0", pid);
            0
        }
        Err(e) => {
            crate::debug_warn!("[gui_launch] spawn_by_name failed: errno={}", e);
            e
        }
    }
}
