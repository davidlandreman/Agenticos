//! Linux x86-64 syscall handlers.
//!
//! The surface implements what musl + libstdc++ static `hello` actually
//! exercises during startup and the C++ iostream write path:
//!
//! - **Real**: `write`, `writev`, `read` (EOF stub on stdin), `mmap`
//!   (anonymous private only), `munmap`, `mprotect` (no-op), `brk`,
//!   `arch_prctl(ARCH_SET_FS|ARCH_GET_FS)`, `exit_group`, `ioctl(TCGETS)`
//!   (returns `-ENOTTY` so libstdc++ picks full buffering).
//! - **Thread runtime**: task IDs, TLS-bearing `clone`, clear-child-tid,
//!   and robust-list registration.
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
    validate_user_slice, EACCES, EAGAIN, EBADF, EBUSY, EEXIST, EFAULT, EFBIG, EINTR, EINVAL, EIO,
    EISDIR, EMFILE, ENOENT, ENOMEM, ENOSPC, ENOSYS, ENOTDIR, ENOTEMPTY, ENOTTY, EOPNOTSUPP, EPERM,
    ERANGE, EROFS, ESPIPE, ESRCH, EXDEV, LAST_EXIT_CODE,
};
use crate::userland::fdtable::{FdSlot, FdTable, FD_TABLE_SIZE};
use crate::userland::path::{copy_user_cstr, normalize_path};
use alloc::string::String;
use alloc::vec;
use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

/// Kernel staging-buffer bound for a single write trip. Not a call cap:
/// file and terminal writes loop over chunks of this size, so arbitrary
/// user lengths succeed while kernel-heap spikes stay bounded. Pipe and
/// socket writes return a POSIX short write at this bound instead — a
/// blocked pipe/socket restarts the whole SYSCALL (RIP rewind), so bytes
/// already consumed by a chunk loop would be duplicated on the re-fire.
const WRITE_MAX_LEN: usize = 4096;
/// Maximum iovec entries per `writev`. libstdc++'s underlying stdio
/// rarely emits more than 2-3 iovecs at a time; 16 is plenty.
const WRITEV_MAX_IOV: usize = 16;
/// Maximum total bytes per `writev` (sum of iov_len). Matches Linux's
/// MAX_RW_COUNT; per-iov chunking keeps kernel memory bounded below this.
const WRITEV_MAX_TOTAL: u64 = 0x7fff_f000;
/// `readv` uses the same bounded vector count and Linux MAX_RW_COUNT ceiling.
const READV_MAX_IOV: usize = WRITEV_MAX_IOV;
const READV_MAX_TOTAL: u64 = WRITEV_MAX_TOTAL;
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

const UTIME_NOW: i64 = 0x3fff_ffff;
const UTIME_OMIT: i64 = 0x3fff_fffe;

// ---------- write / writev / read ----------

/// Longest prefix of `chunk` that does not end partway through a
/// multi-byte UTF-8 sequence. Chunked terminal writes would otherwise
/// render U+FFFD at every staging-buffer seam whenever a multi-byte
/// character straddles two chunks. Returns the full length when the
/// tail is ASCII or is not valid UTF-8 anyway (progress guarantee).
pub(crate) fn utf8_safe_chunk_len(chunk: &[u8]) -> usize {
    let len = chunk.len();
    let scan = core::cmp::min(len, 3);
    for back in 1..=scan {
        let byte = chunk[len - back];
        if byte & 0x80 == 0 {
            return len;
        }
        if byte & 0xC0 == 0xC0 {
            let need = if byte >= 0xF0 {
                4
            } else if byte >= 0xE0 {
                3
            } else {
                2
            };
            return if need > back { len - back } else { len };
        }
        // 0b10xxxxxx continuation byte: keep scanning back.
    }
    len
}

/// Copy `len` bytes from user `ptr` into `handle` in staging-buffer
/// chunks. Returns bytes written (short on a mid-stream error or a
/// filesystem short write) or a negative errno when nothing was written.
fn write_file_chunked(
    handle: &crate::lib::arc::Arc<crate::fs::file_handle::File>,
    ptr: u64,
    len: u64,
) -> i64 {
    let mut staging = alloc::vec![0u8; core::cmp::min(len as usize, WRITE_MAX_LEN)];
    let mut written: u64 = 0;
    while written < len {
        let chunk = core::cmp::min((len - written) as usize, WRITE_MAX_LEN);
        let buf = &mut staging[..chunk];
        if let Err(e) = crate::userland::usercopy::copy_from_user(buf, ptr + written) {
            return if written > 0 { written as i64 } else { e };
        }
        match handle.write(buf) {
            Ok(n) => {
                written += n as u64;
                if n < chunk {
                    break;
                }
            }
            Err(ref e) => {
                return if written > 0 {
                    written as i64
                } else {
                    map_file_err(e)
                }
            }
        }
    }
    written as i64
}

/// Copy `len` bytes from user `ptr` and hand them to `emit` as text, in
/// staging-buffer chunks with multi-byte UTF-8 sequences kept intact
/// across chunk seams. Returns bytes consumed or a negative errno when
/// nothing was emitted.
fn write_terminal_chunked(ptr: u64, len: u64, emit: &mut dyn FnMut(&str)) -> i64 {
    let mut staging = alloc::vec![0u8; core::cmp::min(len as usize, WRITE_MAX_LEN)];
    let mut written: u64 = 0;
    while written < len {
        let want = core::cmp::min((len - written) as usize, WRITE_MAX_LEN);
        let buf = &mut staging[..want];
        if let Err(e) = crate::userland::usercopy::copy_from_user(buf, ptr + written) {
            return if written > 0 { written as i64 } else { e };
        }
        let take = if written + want as u64 == len {
            // Final chunk: emit everything, seams are no longer a concern.
            want
        } else {
            match utf8_safe_chunk_len(buf) {
                0 => want,
                n => n,
            }
        };
        // Lossy: invalid UTF-8 bytes become U+FFFD rather than dropping
        // the entire call. A strict `from_utf8` here would silently
        // swallow writes that mix valid text with binary data (e.g.
        // cat'ing a partially-binary file).
        let text = alloc::string::String::from_utf8_lossy(&buf[..take]);
        emit(&text);
        written += take as u64;
    }
    written as i64
}

/// `write(fd: i32, buf: *const u8, count: usize) -> isize`
///
/// Routes through the FD table: stdout/stderr go to the process's
/// terminal; file writes go through the VFS (read-only mounts surface
/// `-EROFS` at open time). Arbitrary lengths are supported: files and
/// terminals chunk kernel-side; pipes and sockets return POSIX short
/// writes at the staging bound.
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
        EventFd(crate::lib::arc::Arc<crate::userland::eventfd::EventFd>),
        LocalStream(crate::lib::arc::Arc<crate::userland::local_stream::LocalStreamEndpoint>),
    }
    let slot = with_fd_slot(fd);
    let target = match slot {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => Target::StdoutErr,
        Some(FdSlot::File { handle, .. }) => Target::File(handle),
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
        Some(FdSlot::PipeWrite(handle, _)) => Target::Pipe(handle),
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Socket { handle, .. }) => Target::Socket(handle.id()),
        Some(FdSlot::EventFd { handle, .. }) => Target::EventFd(handle),
        Some(FdSlot::LocalStream { handle, .. }) => Target::LocalStream(handle),
        // Discard sink: validate the buffer, report it fully written.
        Some(FdSlot::DevNull { .. }) => {
            if let Err(e) = crate::userland::usercopy::ensure_user_range(ptr, len, false) {
                return e;
            }
            return len as i64;
        }
        // /proc snapshots are read-only.
        Some(FdSlot::VirtualFile { .. })
        | Some(FdSlot::Urandom { .. })
        | Some(FdSlot::GuiEvents { .. })
        | Some(FdSlot::Epoll { .. }) => return EBADF,
        Some(FdSlot::Stdin) | None => return EBADF,
    };

    if len == 0 {
        return 0;
    }

    match target {
        Target::Pipe(handle) => {
            // Sample before inspecting the pipe. If a reader changes the
            // state before our blocked reason becomes visible, the switch
            // path observes the sequence change and re-readies us.
            let observed_sequence = crate::userland::readiness::sequence();
            // Short-write cap: see the WRITE_MAX_LEN comment. A full
            // pipe blocks by restarting the whole SYSCALL, so chunking
            // here would duplicate already-consumed bytes.
            let take = core::cmp::min(len, WRITE_MAX_LEN as u64) as usize;
            let mut staging = alloc::vec![0u8; take];
            if let Err(e) = crate::userland::usercopy::copy_from_user(&mut staging, ptr) {
                return e;
            }
            let slice = staging.as_slice();
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
            if handle.nonblocking() {
                return EAGAIN;
            }
            // This syscall restarts by abandoning its kernel frame rather
            // than unwinding it. Release temporary fd/staging clones first;
            // otherwise each block leaks a pipe endpoint and EOF/EPIPE can
            // never become observable after the real peer exits.
            drop(staging);
            drop(handle);
            unsafe {
                crate::userland::switch::block_current_ring3_and_yield(
                    args,
                    crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeWrite {
                        observed_sequence,
                    },
                )
            }
        }
        Target::File(handle) => write_file_chunked(&handle, ptr, len),
        Target::Socket(id) => {
            // Same short-write rationale as pipes: a blocking socket
            // restarts the SYSCALL, so only one staging trip is safe.
            let take = core::cmp::min(len, WRITE_MAX_LEN as u64) as usize;
            let mut staging = alloc::vec![0u8; take];
            if let Err(e) = crate::userland::usercopy::copy_from_user(&mut staging, ptr) {
                return e;
            }
            crate::userland::network_syscalls::write_connected(args, id, &staging)
        }
        Target::EventFd(handle) => crate::userland::eventfd::write(args, &handle, ptr, len),
        Target::LocalStream(handle) => {
            crate::userland::local_stream::LocalStreamEndpoint::write(args, &handle, ptr, len)
        }
        Target::StdoutErr => {
            // U8/bugfix: route to THIS process's terminal_id, not the
            // global CURRENT_OUTPUT_TERMINAL. With multiple ring-3
            // processes (one per terminal), the global is wrong —
            // last-launcher wins, so zsh1's writes would land in
            // terminal 2's window.
            let dest_terminal = crate::userland::lifecycle::with_current_group(|p| p.terminal_id);
            let mut emit = |s: &str| match dest_terminal {
                Some(tid) => crate::window::terminal::write_to_terminal_id(tid, s),
                None => crate::print!("{}", s),
            };
            write_terminal_chunked(ptr, len, &mut emit)
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
        LocalStream(crate::lib::arc::Arc<crate::userland::local_stream::LocalStreamEndpoint>),
        /// `/dev/null`: validated iovecs count as fully written.
        Sink,
    }
    let target = match with_fd_slot(fd) {
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => Target::StdoutErr,
        Some(FdSlot::File { handle, .. }) => Target::File(handle),
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
        Some(FdSlot::DevNull { .. }) => Target::Sink,
        Some(FdSlot::PipeWrite(handle, _)) => Target::Pipe(handle),
        Some(FdSlot::PipeRead(_, _)) => return EBADF,
        Some(FdSlot::Socket { handle, .. }) => Target::Socket(handle.id()),
        Some(FdSlot::LocalStream { handle, .. }) => Target::LocalStream(handle),
        // /proc snapshots are read-only.
        Some(FdSlot::VirtualFile { .. })
        | Some(FdSlot::Urandom { .. })
        | Some(FdSlot::GuiEvents { .. })
        | Some(FdSlot::EventFd { .. })
        | Some(FdSlot::Epoll { .. }) => return EBADF,
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
        crate::userland::lifecycle::with_current_group(|p| p.terminal_id)
    } else {
        None
    };

    // Now emit every iov in order. Short writes break the loop so
    // POSIX writev's "stop at the first short write" semantics hold.
    // Files and terminals chunk kernel-side (arbitrary iov lengths);
    // pipes and sockets take one staging trip per iov and short-write
    // at the staging bound (see the WRITE_MAX_LEN comment).
    let mut written: u64 = 0;
    let mut pipe_block_sequence = None;
    for (base, len) in iovecs {
        if len == 0 {
            continue;
        }
        match &target {
            Target::Sink => {
                let _ = base;
                written += len;
            }
            Target::StdoutErr => {
                let mut emit = |s: &str| match dest_terminal {
                    Some(tid) => crate::window::terminal::write_to_terminal_id(tid, s),
                    None => crate::print!("{}", s),
                };
                let n = write_terminal_chunked(base, len, &mut emit);
                if n < 0 {
                    return if written > 0 { written as i64 } else { n };
                }
                written += n as u64;
                if (n as u64) < len {
                    break;
                }
            }
            Target::Pipe(handle) => {
                // See write_handler's check-to-park handshake. Each iovec is
                // its own possible blocking attempt, so sample immediately
                // before checking this one.
                let observed_sequence = crate::userland::readiness::sequence();
                let take = core::cmp::min(len, WRITE_MAX_LEN as u64) as usize;
                let mut bytes = alloc::vec![0u8; take];
                if let Err(e) = crate::userland::usercopy::copy_from_user(&mut bytes, base) {
                    return if written > 0 { written as i64 } else { e };
                }
                let slice = bytes.as_slice();
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
                if handle.nonblocking() {
                    return EAGAIN;
                }
                pipe_block_sequence = Some(observed_sequence);
                break;
            }
            Target::File(handle) => {
                let n = write_file_chunked(handle, base, len);
                if n < 0 {
                    return if written > 0 { written as i64 } else { n };
                }
                written += n as u64;
                if (n as u64) < len {
                    break;
                }
            }
            Target::Socket(id) => {
                let take = core::cmp::min(len, WRITE_MAX_LEN as u64) as usize;
                let mut bytes = alloc::vec![0u8; take];
                if let Err(e) = crate::userland::usercopy::copy_from_user(&mut bytes, base) {
                    return if written > 0 { written as i64 } else { e };
                }
                let result = crate::userland::network_syscalls::write_connected(args, *id, &bytes);
                if result < 0 {
                    return if written > 0 { written as i64 } else { result };
                }
                written += result as u64;
                if result as u64 != len {
                    break;
                }
            }
            Target::LocalStream(handle) => {
                let result = crate::userland::local_stream::LocalStreamEndpoint::write(
                    args, handle, base, len,
                );
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
    if let Some(observed_sequence) = pipe_block_sequence {
        // See write_handler: the divergent restart path must not retain the
        // cloned Target::Pipe handle on its abandoned kernel frame.
        drop(target);
        unsafe {
            crate::userland::switch::block_current_ring3_and_yield(
                args,
                crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeWrite {
                    observed_sequence,
                },
            )
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
    read_fd_once(args, fd, ptr, len)
}

/// `readv(fd: i32, iov: *const iovec, iovcnt: i32) -> isize`.
///
/// Validate the complete vector before advancing the open-file description,
/// then read entries in order. A short read ends the operation, matching the
/// Linux/POSIX scatter-read contract. Blocking on the first entry is safe:
/// the scheduler restarts the original `readv` syscall with untouched user
/// registers. Once bytes have been consumed, an error is reported as a short
/// read instead of losing the progress already made.
pub fn readv_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let iov_ptr = args.rsi;
    let iovcnt = args.rdx as i64;

    if iovcnt < 0 || iovcnt as usize > READV_MAX_IOV {
        return EINVAL;
    }
    let mut total_len = 0u64;
    let mut iovecs = alloc::vec::Vec::with_capacity(iovcnt as usize);
    for index in 0..iovcnt as u64 {
        let Some(entry) = iov_ptr.checked_add(index * 16) else {
            return EFAULT;
        };
        let base = match crate::userland::usercopy::read_unaligned::<u64>(entry) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let len = match crate::userland::usercopy::read_unaligned::<u64>(entry + 8) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Err(error) = crate::userland::usercopy::ensure_user_range(base, len, true) {
            return error;
        }
        total_len = match total_len.checked_add(len) {
            Some(total) if total <= READV_MAX_TOTAL => total,
            _ => return EINVAL,
        };
        iovecs.push((base, len));
    }

    let mut read = 0u64;
    for (base, len) in iovecs {
        if len == 0 {
            continue;
        }
        let result = read_fd_once(args, fd, base, len);
        if result < 0 {
            return if read > 0 { read as i64 } else { result };
        }
        read += result as u64;
        if result as u64 != len {
            break;
        }
    }
    read as i64
}

fn read_fd_once(args: &SyscallArgs, fd: i32, ptr: u64, len: u64) -> i64 {
    let cap = core::cmp::min(len, READ_MAX_LEN as u64);
    let slot = with_fd_slot(fd);
    match slot {
        Some(FdSlot::Stdin) => read_stdin_blocking(args, ptr, cap),
        Some(FdSlot::Stdout) | Some(FdSlot::Stderr) => EBADF,
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => EISDIR,
        // Empty source: immediate EOF.
        Some(FdSlot::DevNull { .. }) => 0,
        Some(FdSlot::Urandom { .. }) => {
            if let Err(e) = validate_user_slice(ptr, cap) {
                return e;
            }
            let mut staging = vec![0u8; cap as usize];
            if crate::random::fill_bytes(&mut staging).is_err() {
                return EIO;
            }
            crate::userland::usercopy::copy_to_user(ptr, &staging)
                .map_or_else(|e| e, |_| staging.len() as i64)
        }
        Some(FdSlot::VirtualFile { data, cursor, .. }) => {
            // Serve from the open-time snapshot; advance the per-fd
            // cursor. Returns 0 at EOF like a regular file.
            let start = cursor.min(data.len());
            let n = core::cmp::min(cap as usize, data.len() - start);
            if n > 0 {
                if let Err(e) =
                    crate::userland::usercopy::copy_to_user(ptr, &data[start..start + n])
                {
                    return e;
                }
                with_fd_table_mut(|t| {
                    if let Some(FdSlot::VirtualFile { cursor: c, .. }) = t.get_mut(fd) {
                        *c = start + n;
                    }
                });
            }
            n as i64
        }
        Some(FdSlot::GuiEvents { handle, .. }) => {
            let event_size = core::mem::size_of::<crate::userland::gui::GuiEvent>();
            if cap < event_size as u64 {
                return EINVAL;
            }
            if crate::userland::lifecycle::current_user_pid() != Some(handle.owner_pid()) {
                return EBADF;
            }
            let max_events = cap as usize / event_size;
            if let Err(error) = validate_user_slice(ptr, (max_events * event_size) as u64) {
                return error;
            }
            let mut events = alloc::vec::Vec::with_capacity(max_events);
            while events.len() < max_events {
                match crate::userland::gui::pop_event(handle.owner_pid()) {
                    Some(event) => events.push(event),
                    None => break,
                }
            }
            if events.is_empty() {
                if handle.nonblocking() {
                    return EAGAIN;
                }
                unsafe {
                    crate::userland::switch::block_current_ring3_and_yield(
                        args,
                        crate::userland::lifecycle::Ring3BlockReason::WaitingForGuiEvent,
                    );
                }
            }
            let byte_len = events.len() * event_size;
            let bytes =
                unsafe { core::slice::from_raw_parts(events.as_ptr().cast::<u8>(), byte_len) };
            crate::userland::usercopy::copy_to_user(ptr, bytes)
                .map_or_else(|error| error, |_| byte_len as i64)
        }
        Some(FdSlot::PipeRead(handle, _)) => {
            // Sample before inspecting the pipe. A writer may append data (or
            // the final writer may close) after the empty check but before we
            // publish the blocked reason; the post-publication sequence
            // recheck prevents that wake from being lost.
            let observed_sequence = crate::userland::readiness::sequence();
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
            if handle.nonblocking() {
                return EAGAIN;
            }
            drop(staging);
            drop(handle);
            unsafe {
                crate::userland::switch::block_current_ring3_and_yield(
                    args,
                    crate::userland::lifecycle::Ring3BlockReason::WaitingForPipeRead {
                        observed_sequence,
                    },
                );
            }
        }
        Some(FdSlot::PipeWrite(_, _)) => EBADF,
        Some(FdSlot::Socket { handle, .. }) => {
            crate::userland::network_syscalls::read_connected(args, handle.id(), ptr, cap as usize)
        }
        Some(FdSlot::EventFd { handle, .. }) => {
            crate::userland::eventfd::read(args, &handle, ptr, len)
        }
        Some(FdSlot::LocalStream { handle, .. }) => {
            crate::userland::local_stream::LocalStreamEndpoint::read(args, &handle, ptr, len)
        }
        Some(FdSlot::Epoll { .. }) => EBADF,
        Some(FdSlot::File { handle, .. }) => {
            // Stage the read inside a kernel buffer so the FAT/block path
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

    // No input is available. Block the user entity in the shared scheduler.
    // When input arrives, the stdin push path makes it runnable in the same
    // queue as kernel workers; our SYSCALL re-fires when selected and
    // this handler re-runs from the top — re-checks the queue,
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
    if (flags & MAP_PRIVATE) == 0 {
        return ENOSYS;
    }
    let fixed = flags & MAP_FIXED != 0;
    if fixed && (addr_hint & 0xfff != 0 || addr_hint == 0) {
        // MAP_FIXED demands an exact page-aligned address; musl's
        // mallocng meta allocator and aligned-allocation trim path both
        // rely on it. A misaligned or null fixed address is EINVAL.
        return EINVAL;
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
        crate::userland::lifecycle::with_current_group(|process| {
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

    // `(result, Some(l4))` when a MAP_FIXED insert replaced an existing
    // range and its resident leaves must be torn down after the VMA
    // mutation (same split-then-unmap ordering as munmap).
    use x86_64::structures::paging::{PhysFrame, Size4KiB};
    let (result, fixed_l4): (i64, Option<PhysFrame<Size4KiB>>) =
        crate::userland::lifecycle::with_current_group(|process| {
            let Some(space) = process.address_space.as_mut() else {
                return (ENOMEM, None);
            };
            let stack_floor = space
                .vmas()
                .as_slice()
                .iter()
                .find_map(|vma| {
                    matches!(vma.backing, VmaBacking::Stack { .. }).then_some(vma.start)
                })
                .unwrap_or(crate::mm::paging::USER_STACK_TOP);
            let hinted_end = addr_hint.checked_add(len);
            let (addr, replaced_l4) = if fixed {
                // MAP_FIXED: place at exactly addr_hint, evicting any
                // VMAs already there (Linux semantics). Removing the
                // range first also splits partial overlaps.
                let Some(end) = hinted_end else {
                    return (EINVAL, None);
                };
                let l4 = space.l4_frame();
                if space.vmas_mut().remove(addr_hint, end).is_err() {
                    return (EINVAL, None);
                }
                (addr_hint, Some(l4))
            } else if addr_hint & 0xfff == 0
                && hinted_end.is_some_and(|end| space.vmas().is_free(addr_hint, end))
            {
                (addr_hint, None)
            } else {
                match space
                    .vmas()
                    .find_gap_top_down(len, stack_floor.saturating_sub(1024 * 1024))
                {
                    Ok(address) => (address, None),
                    Err(_) => return (ENOMEM, None),
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
                return (ENOMEM, None);
            };
            if space.vmas_mut().insert(vma).is_err() {
                return (ENOMEM, None);
            }
            (addr as i64, replaced_l4)
        });

    // Drop stale hardware leaves under a replaced MAP_FIXED range so the
    // fresh mapping (often PROT_NONE) faults in cleanly instead of
    // exposing the old page contents.
    if let Some(l4) = fixed_l4 {
        let end = result as u64 + len;
        crate::mm::memory::with_memory_mapper(|mapper| {
            let mut page = result as u64;
            while page < end {
                if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
                    let _ = mapper.unmap_page_from(l4, VirtAddr::new(page));
                }
                page += 0x1000;
            }
        });
    }
    result
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
    let l4 = crate::userland::lifecycle::with_current_group(|process| {
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
    let l4 = crate::userland::lifecycle::with_current_group(|process| {
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
    let base = crate::userland::lifecycle::with_current_group(|p| p.brk_base);
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
    let l4 = crate::userland::lifecycle::with_current_group(|process| {
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
            crate::debug_trace!("arch_prctl(SET_FS, {:#x}) accepted", addr);
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

// ---------- thread runtime / signals ----------

/// `set_tid_address(tidptr: *mut int) -> pid_t`
///
/// Register the word cleared and futex-woken when this task exits.
pub fn set_tid_address_handler(args: &mut SyscallArgs) -> i64 {
    let tid = crate::arch::x86_64::percpu::current_user_pid()
        .unwrap_or(crate::userland::lifecycle::KERNEL_PID);
    crate::userland::lifecycle::set_clear_child_tid(tid, args.rdi);
    tid as i64
}

/// Record musl's per-task robust-list head. Robust owner-death recovery is
/// deliberately deferred, but retaining the ABI state makes registration
/// task-correct and leaves a single place to add that recovery later.
pub fn set_robust_list_handler(args: &mut SyscallArgs) -> i64 {
    if args.rsi != 24 {
        return EINVAL;
    }
    let tid = crate::arch::x86_64::percpu::current_user_pid()
        .unwrap_or(crate::userland::lifecycle::KERNEL_PID);
    crate::userland::lifecycle::set_robust_list(tid, args.rdi, args.rsi as usize);
    0
}

pub fn gettid_handler(_: &mut SyscallArgs) -> i64 {
    crate::arch::x86_64::percpu::current_user_pid()
        .unwrap_or(crate::userland::lifecycle::KERNEL_PID) as i64
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

/// `rt_sigsuspend(*mask, sigsetsize) -> int` — atomically replace the
/// blocked-signal mask and sleep until a signal becomes actionable.
///
/// POSIX: atomically replace the signal mask with `*mask`, suspend
/// until a deliverable signal arrives, run its handler, then return
/// with the original mask restored.
///
/// The first entry saves the caller's original mask and installs the supplied
/// temporary mask. If that exposes an already-pending signal, returning
/// `-EINTR` lets the dispatcher tail deliver it immediately. Otherwise the
/// process is parked until a signal raise calls `wake_ring3_for_signal`.
/// Blocking rewinds RIP to re-fire the syscall, while the wake path sets
/// `pending_syscall_interrupt`; the dispatcher therefore delivers the signal
/// with an interrupted-syscall return before this handler runs again.
///
/// Signal delivery consumes `suspend_restore_mask` into the signal frame, so
/// `rt_sigreturn` restores the caller's original mask after the handler.
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
    // the temporary mask cannot swallow KILL/STOP during suspension.
    let kill_stop_mask = (1u64 << (SIGKILL - 1)) | (1u64 << (SIGSTOP - 1));
    let sanitized = mask & !kill_stop_mask;
    let should_block = crate::userland::lifecycle::with_current_process(|p| {
        if p.signal_state.suspend_restore_mask.is_none() {
            p.signal_state.suspend_restore_mask = Some(p.signal_state.blocked);
        }
        p.signal_state.blocked = sanitized;
        !p.signal_state.has_actionable_pending()
    });
    if !should_block {
        return EINTR;
    }

    unsafe {
        crate::userland::switch::block_current_ring3_and_yield(
            args,
            crate::userland::lifecycle::Ring3BlockReason::WaitingForSignal,
        );
    }
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
    crate::userland::lifecycle::with_current_group(|p| p.parent_pid as i64)
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
    fork_like(args, None)
}

/// Shared body of `fork`, `vfork`, and the posix_spawn `clone` profile.
/// `child_rsp_override` is the caller-supplied child stack for the spawn
/// profile (`clone(CLONE_VM|CLONE_VFORK|SIGCHLD, stack, ...)`): the child
/// resumes at the post-SYSCALL instruction with rax=0 and RSP switched to
/// that stack, exactly what musl's `__clone` asm expects before it pops
/// the argument and calls the child function.
fn fork_like(args: &mut SyscallArgs, child_rsp_override: Option<u64>) -> i64 {
    use crate::userland::lifecycle::{
        alloc_pid, insert_process, mark_ring3_ready, with_current_process, ExitKind,
    };
    use crate::userland::user_state::UserState;

    let tgid = crate::userland::lifecycle::current_tgid();
    if crate::userland::lifecycle::group_member_count(tgid) != 1 {
        return EAGAIN;
    }

    // U7: fork now returns immediately to the parent without iretq'ing
    // into the child. The child is inserted into PROCESS_TABLE with a
    // populated `saved_user_state` (rax=0, parent's other GPRs +
    // RIP/RFLAGS/RSP at fork's SYSCALL boundary) and marks its tagged
    // entity ready. The next timer preempt (or any block-and-yield)
    // gives the child a slice; the shared scheduler round-robins between
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
        // Spawn-profile clone switches the child onto its caller-supplied
        // stack; plain fork keeps the parent's RSP.
        rsp: child_rsp_override.unwrap_or(saved.r12_register), // = user RSP from gs:[8]
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
        fd_table: parent.fd_table.fork_clone(),
        umask: parent.umask,
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
        signal_alt_stack: parent.signal_alt_stack,
        membarrier_private_registered: false,
        // Phase 5 PR-C1: child gets its own freshly-allocated kernel
        // stack so its SYSCALL handlers don't share rsp0 with the
        // parent's syscall handlers when both are alive concurrently.
        kernel_stack: Some(crate::userland::kernel_stack::KernelStack::new()),
        // U3: child shares parent's exe path (fork doesn't change the
        // running binary; execve replaces it).
        exe_path: parent.exe_path.clone(),
        cmdline: parent.cmdline.clone(),
        // CPU time is per-process, not inherited (POSIX: child's
        // tms_utime starts at zero).
        utime_ticks: 0,
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
        kernel_continuation: None,
        // Routing: stdout/stderr flow to the same terminal as parent.
        terminal_id: parent.terminal_id,
    });

    // 7. Register child in PROCESS_TABLE and mark ready. The next
    //    scheduling decision (timer preempt, block-and-yield from
    //    parent, or top-level idle) picks the child via `resume_ring3`.
    insert_process(child_process);
    mark_ring3_ready(child_pid);

    crate::debug_trace!(
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

pub fn clone_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::lifecycle::{alloc_pid, register_thread_task, ExitKind, Process};
    use crate::userland::user_state::UserState;

    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_DETACHED: u64 = 0x0040_0000;
    const MUSL_THREAD_FLAGS: u64 = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_DETACHED;

    let flags = args.rdi;

    // musl posix_spawn profile: clone(CLONE_VM|CLONE_VFORK|SIGCHLD,
    // stack, 0, 0, 0). Like vfork_handler we substitute copy semantics
    // for the shared-VM/parent-suspend contract: musl's spawn protocol
    // never reads parent memory written by the child — success/failure
    // travels over a CLOEXEC status pipe — so a COW-fork child that
    // resumes on the caller-supplied stack is observationally correct.
    // libiberty's pex (the GCC driver), musl system(), and popen() all
    // spawn subprocesses through this profile.
    const CLONE_VFORK: u64 = 0x0000_4000;
    const SIGCHLD: u64 = 17;
    const MUSL_SPAWN_FLAGS: u64 = CLONE_VM | CLONE_VFORK | SIGCHLD;
    if flags == MUSL_SPAWN_FLAGS {
        if args.rsi == 0 {
            return EINVAL;
        }
        return fork_like(args, Some(args.rsi));
    }

    if flags != MUSL_THREAD_FLAGS || args.rsi == 0 || args.rdx == 0 || args.r10 == 0 {
        return EINVAL;
    }

    let saved =
        unsafe { crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs) };
    let raw = args as *const SyscallArgs as *const u64;
    let child_state = UserState {
        rax: 0,
        rdi: args.rdi,
        rsi: args.rsi,
        rdx: args.rdx,
        r10: args.r10,
        r8: args.r8,
        r9: args.r9,
        rbx: saved.rbx,
        rbp: saved.rbp,
        rsp: args.rsi,
        r12: unsafe { crate::userland::user_state::read_user_r12(args as *const SyscallArgs) },
        r13: saved.r13,
        r14: saved.r14,
        r15: saved.r15,
        rip: unsafe { core::ptr::read(raw.add(7)) },
        rflags: unsafe { core::ptr::read(raw.add(8)) },
        rcx: 0,
        r11: 0,
    };

    let tid = alloc_pid();
    if crate::userland::usercopy::write_unaligned(args.rdx, &tid).is_err() {
        return EFAULT;
    }
    let tgid = crate::userland::lifecycle::current_tgid();
    // FXSAVE requires 16-byte alignment. Capture into the parent's state in
    // its heap-backed Process slot, then clone the bytes for the new task;
    // a local FpuState temporary is not reliably aligned on every syscall
    // stack entry path.
    let fpu_state = crate::userland::lifecycle::with_current_process(|parent| {
        crate::userland::lifecycle::save_user_cpu_state(parent);
        parent.fpu_state.clone()
    });
    let task = crate::userland::lifecycle::with_current_process(|parent| Process {
        pid: tid,
        parent_pid: parent.parent_pid,
        image: None,
        exit_kind: ExitKind::None,
        exit_code: 0,
        brk_current: 0,
        brk_base: 0,
        mmap_next: 0,
        fd_table: FdTable::new(),
        umask: parent.umask,
        network_wait: None,
        real_timer: crate::userland::lifecycle::RealTimerState::disarmed(),
        sleep_deadline: None,
        pending_syscall_interrupt: false,
        cwd: String::new(),
        address_space: None,
        signal_state: parent.signal_state.fork_clone(),
        // Linux clears an alternate stack for CLONE_VM without CLONE_VFORK.
        signal_alt_stack: crate::userland::signal::SignalAltStack::default(),
        membarrier_private_registered: false,
        kernel_stack: Some(crate::userland::kernel_stack::KernelStack::new()),
        exe_path: None,
        cmdline: alloc::vec::Vec::new(),
        utime_ticks: 0,
        stack_top: 0,
        stack_bottom: 0,
        stack_mapped_bottom: 0,
        stack_max_growth_floor: 0,
        growth_faults_remaining: 0,
        fs_base: args.r8,
        fpu_state,
        saved_user_state: child_state,
        kernel_continuation: None,
        terminal_id: None,
    });

    if register_thread_task(tid, tgid, task).is_err() {
        let zero = 0u32;
        let _ = crate::userland::usercopy::write_unaligned(args.rdx, &zero);
        return EAGAIN;
    }
    crate::userland::lifecycle::set_clear_child_tid(tid, args.r10);
    crate::userland::lifecycle::mark_ring3_ready(tid);
    tid as i64
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

    let tgid = crate::userland::lifecycle::current_tgid();
    if crate::userland::lifecycle::group_member_count(tgid) != 1 {
        return EAGAIN;
    }

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
    let normalized_path = crate::userland::lifecycle::with_current_group(|p| {
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
    let mut at_random = [0u8; 16];
    if crate::random::fill_bytes(&mut at_random).is_err() {
        return EIO;
    }

    // 3. Build a fresh AddressSpace for the new image. If this fails,
    //    we haven't touched the old state yet — return cleanly.
    let mut new_aspace = match crate::userland::address_space::AddressSpace::new() {
        Ok(a) => a,
        Err(_) => return -12, // ENOMEM
    };

    // 4. Detach, but retain, the complete old VM transaction. Nothing in
    // the old image or page tables is modified until the replacement has
    // loaded, its stack has been built, and its VMA set is complete.
    let (old_image, old_aspace) = crate::userland::lifecycle::with_current_group(|p| {
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
            crate::userland::lifecycle::with_current_group(|p| {
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
        crate::userland::lifecycle::with_current_group(|p| {
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
        // GCC self-relocates: it derives GCC_EXEC_PREFIX from argv[0] and
        // set_std_prefix REPLACES the configured /host/gcc prefix with the
        // derived one. A bare "gcc" resolved via /bin would derive "/" and
        // the driver could no longer find cc1/collect2. Handing it its
        // real staged path makes make_relative_prefix derive exactly the
        // configured prefix. Every other applet gets the plain name.
        argv_refs[0] = if applet == "gcc" {
            resolved_path.as_str()
        } else {
            applet
        };
    }
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();
    let user_rsp = super::build_initial_stack(
        stack_top,
        &phdr_bytes,
        e_phnum,
        &argv_refs,
        &envp_refs,
        &at_random,
    );

    // From commit onward the targeted AddressSpace walker is the sole page
    // owner; UserImage remains only executable metadata.
    image.transfer_mapping_ownership();

    // 10. Move new image and aspace onto the Process; reset brk/mmap
    //     anchors and exit info. Retain PID, parent_pid, FD table,
    //     cwd, continuation.
    new_aspace.publish_owner(tgid);
    let closed_on_exec = crate::userland::lifecycle::with_current_group(|p| {
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
        p.cmdline = crate::userland::lifecycle::capped_cmdline(&argv_refs);
        // POSIX: exec closes every descriptor with FD_CLOEXEC set. musl
        // posix_spawn's success signal is the CLOEXEC status pipe
        // closing here.
        let closed_on_exec = p.fd_table.take_cloexec();
        // Phase 5 PR-B: POSIX semantics — exec resets signal
        // dispositions but preserves the blocked mask. Pending
        // signals are also preserved across exec.
        let preserved_blocked = p.signal_state.blocked;
        let preserved_pending = p.signal_state.pending;
        p.signal_state = crate::userland::signal::SignalState::new();
        p.signal_state.blocked = preserved_blocked;
        p.signal_state.pending = preserved_pending;
        p.signal_alt_stack = crate::userland::signal::SignalAltStack::default();
        p.membarrier_private_registered = false;
        // Demand-grown stack (U3): replace the stack window with the
        // new image's. exec resets the full growth budget.
        p.set_stack_window(
            stack_top,
            stack_initial_bottom,
            stack_initial_bottom,
            stack_max_growth_floor,
            crate::mm::paging::USER_STACK_MAX_GROWTH_PAGES,
        );
        closed_on_exec
    });
    // Pipe endpoint destruction can wake a parent blocked in the musl/Git
    // successful-exec handshake. Drop only after with_current_group releases
    // PROCESS_TABLE, matching exit-time fd teardown's lock discipline.
    drop(closed_on_exec);
    // set_tid_address(2) and set_robust_list(2) register pointers into the
    // pre-exec address space. This path rejects multithreaded exec above, so
    // the sole group member is the leader named by `tgid`.
    crate::userland::lifecycle::reset_task_exec_metadata(tgid);
    drop(old_image);
    drop(old_aspace);
    set_user_va_bounds(bounds);
    // Phase 3 termios: a freshly exec'd process gets a default tty.
    crate::userland::tty::install_default();

    crate::debug_trace!(
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
    // No process groups: pid must name one live ring-3 process. The
    // single-user model has no permission checks — any process may
    // signal any other (this is what lets a ring-3 task manager
    // implement End Task through the ordinary kill path).
    if pid <= 0 {
        return ESRCH;
    }
    let raised = crate::userland::lifecycle::with_process(pid as u32, |target| {
        if sig != 0 {
            target.signal_state.raise(sig);
        }
    });
    if raised.is_none() {
        return ESRCH;
    }
    if sig != 0 && pid != crate::userland::lifecycle::current_pid() as i32 {
        // If the target is parked in a blocking syscall, unblock it so
        // the pending signal is examined at its next dispatcher entry.
        crate::userland::lifecycle::wake_ring3_for_signal(pid as u32);
    }
    0
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
/// this point the user RSP in the syscall stub's per-CPU scratch
/// slot points just past the popped restorer, i.e. at the saved
/// `UserState` we wrote when delivering the signal.
///
/// Read the frame, restore the user state AND the pre-delivery
/// signal mask, then `iretq` back to the pre-signal RIP/regs.
pub fn rt_sigreturn_handler(_args: &mut SyscallArgs) -> i64 {
    use crate::userland::user_state::UserState;
    // The SYSCALL entry stub records the authoritative user RSP in
    // gs:[8] before switching to the kernel stack. Do not read live
    // r12 here: the compiler may use that callee-saved register inside
    // this Rust call chain, while the per-CPU scratch slot remains
    // stable until iretq.
    let user_rsp = unsafe { crate::userland::user_state::read_user_rsp() };

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
/// we write 168 bytes downward from there. The caller reads it from
/// the syscall stub's per-CPU user-RSP scratch slot, which contains
/// the user's stack pointer at the point of the syscall.
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
    /// System V AMD64 red-zone size preserved below the interrupted RSP.
    const RED_ZONE_BYTES: u64 = 128;
    let frame_total = (FRAME_SIZE + 15) & !15;
    // System V AMD64 §3.2.2 red zone: the 128 bytes below RSP belong to
    // the interrupted function and may hold live temporaries it stored
    // without adjusting RSP. The kernel MUST NOT place the signal frame
    // there — Linux subtracts 128 before building the frame. Skipping
    // this clobbers the interrupted function's red-zone data, so after
    // `rt_sigreturn` it resumes with a corrupted saved pointer/return
    // slot and jumps to garbage a few instructions later (observed as a
    // `#PF` at RIP≈-4 on return from git's SIGALRM progress handler).
    // A signal delivered onto a fresh sigaltstack has no red zone to
    // preserve, so the skip applies only to the normal-stack path.
    let on_alt_stack = action.sa_flags & crate::userland::signal::SA_ONSTACK != 0
        && crate::userland::lifecycle::with_current_process(|process| {
            let alt = process.signal_alt_stack;
            alt.enabled && !alt.contains(user_rsp)
        });
    let frame_stack_top = if on_alt_stack {
        crate::userland::lifecycle::with_current_process(|process| process.signal_alt_stack.top())
            .unwrap_or(user_rsp - RED_ZONE_BYTES)
    } else {
        user_rsp - RED_ZONE_BYTES
    };
    let frame_addr = match frame_stack_top.checked_sub(frame_total) {
        Some(address) => address,
        None => {
            crate::userland::lifecycle::cleanup_user_process(
                crate::userland::lifecycle::AbnormalExit {
                    vector: 14,
                    error_code: Some(0x6),
                    fault_addr: Some(VirtAddr::new(0)),
                    fault_rip: VirtAddr::new(user_rip),
                },
            );
        }
    };

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

    crate::debug_trace!(
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

/// Consume one deliverable handled signal and install the mask that must be
/// active while its user handler runs. The returned mask is the one the
/// signal frame must restore: normally the pre-delivery mask, or the original
/// pre-`sigsuspend` mask when delivery completes a suspended wait.
pub fn prepare_deliverable_signal() -> Option<(i32, crate::userland::signal::SigAction, u64)> {
    crate::userland::lifecycle::with_current_process(|p| {
        let (sig, action) = p.signal_state.consume_deliverable()?;
        let delivery_mask = p.signal_state.blocked;
        let restore_mask = p
            .signal_state
            .suspend_restore_mask
            .take()
            .unwrap_or(delivery_mask);
        let handler_mask = handler_blocked_mask(delivery_mask, action.sa_mask, sig);
        p.signal_state.blocked = handler_mask;
        Some((sig, action, restore_mask))
    })
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
    // Fatal default dispositions first: a pending unblocked signal
    // with no handler whose default action terminates (SIGKILL,
    // SIGTERM without a trap, …) kills the process here, before the
    // handler-delivery path runs. Only a real nonzero ring-3 PID may
    // take the divergent exit path (same guard as exit_group_handler
    // — synthetic dispatcher tests have no scheduler context to yield
    // from).
    maybe_terminate_pending_fatal_signal();
    if let Some((sig, action, restore_mask)) = prepare_deliverable_signal() {
        unsafe {
            deliver_signal(sig, action, args, syscall_ret, restore_mask);
        }
    }
    None
}

/// Terminate the current ring-3 process if it has an unblocked fatal-default
/// signal pending. Called both at the syscall tail and from timer/reschedule
/// interrupt paths so a CPU-bound process cannot outrun SIGKILL/SIGTERM.
pub fn maybe_terminate_pending_fatal_signal() {
    if matches!(
        crate::userland::lifecycle::current_user_pid(),
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID
    ) {
        let fatal = crate::userland::lifecycle::with_current_process(|p| {
            p.signal_state.take_fatal_default()
        });
        if let Some(sig) = fatal {
            let (pid, parent_pid) =
                crate::userland::lifecycle::with_active_user(|au| (au.pid, au.parent_pid));
            let code = 128 + sig as i64; // shell convention for the exit code
            crate::debug_info!(
                "USERLAND: pid={} killed by signal {} (default action)",
                pid,
                sig
            );
            *LAST_EXIT_CODE.lock() = Some(code);
            crate::userland::lifecycle::notify_parent_of_signaled_exit(pid, parent_pid, sig, code);
            crate::userland::lifecycle::cooperative_exit(code);
        }
    }
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
/// `O_NONBLOCK` is stored on each endpoint's shared open-file description.
pub fn pipe2_handler(args: &mut SyscallArgs) -> i64 {
    pipe2_common(args.rdi, args.rsi as u32)
}

fn pipe2_common(fds_ptr: u64, flags: u32) -> i64 {
    use crate::userland::fdtable::FdSlot;
    use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

    if fds_ptr == 0 {
        return EFAULT;
    }
    if flags & !(O_CLOEXEC | O_NONBLOCK) != 0 {
        return EINVAL;
    }
    let cloexec = (flags & O_CLOEXEC) != 0;
    let nonblocking = (flags & O_NONBLOCK) != 0;

    let pipe = Pipe::new();
    let read_handle = PipeReadHandle::new(pipe.clone(), nonblocking);
    let write_handle = PipeWriteHandle::new(pipe, nonblocking);

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
    // a child exits (which calls `wake_ring3_blocked_on_child` and marks
    // our scheduler entity ready) and re-fire the SYSCALL. The helper
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

pub fn exit_thread_handler(args: &mut SyscallArgs) -> i64 {
    let code = args.rdi as i32 as i64;
    *LAST_EXIT_CODE.lock() = Some(code);
    if !matches!(
        crate::userland::lifecycle::current_user_pid(),
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID
    ) {
        return 0;
    }
    crate::userland::lifecycle::cooperative_thread_exit(code)
}

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
        crate::userland::lifecycle::with_current_group(|au| (au.pid, au.parent_pid));

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
        crate::debug_trace!(
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
    let _ = (pid, parent_pid);
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
const O_WRONLY: u32 = 0o1;
const O_RDWR: u32 = 0o2;
const O_NONBLOCK: u32 = 0o4000;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
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
const S_IFCHR: u32 = 0o020000;
const PERM_READ_ALL: u32 = 0o444;
const PERM_RX_ALL: u32 = 0o555;

// ---------- helpers ----------

/// Acquire a clone of the FD slot at `fd`. Releases the `ActiveUser`
/// mutex before returning so subsequent FS calls don't risk lock-order
/// inversion with the FAT layer.
pub(crate) fn fd_slot(fd: i32) -> Option<FdSlot> {
    if fd < 0 || (fd as usize) >= FD_TABLE_SIZE {
        return None;
    }
    crate::userland::lifecycle::with_active_user(|au| au.fd_table.get(fd).cloned())
}

fn with_fd_slot(fd: i32) -> Option<FdSlot> {
    fd_slot(fd)
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
        FE::UnsupportedFeature => EOPNOTSUPP,
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

    // Resolve the synthetic device nodes before mounted filesystems so no
    // disk entry can shadow them. Any other `/dev/*` path retains its VFS
    // behavior. `/dev/null` is the one writable node — git opens it O_RDWR
    // at startup (`sanitize_stdfds`) and shells redirect into it.
    if let Some(node) = crate::userland::devfs::classify(&path) {
        let slot = match node {
            crate::userland::devfs::DeviceNode::Directory => {
                if want_write {
                    return EACCES;
                }
                FdSlot::VirtualDevDir { cursor: 0, cloexec }
            }
            crate::userland::devfs::DeviceNode::Urandom => {
                if want_write {
                    return EACCES;
                }
                FdSlot::Urandom { cloexec }
            }
            crate::userland::devfs::DeviceNode::Null => FdSlot::DevNull { cloexec },
        };
        return with_fd_table_mut(|t| t.alloc(slot))
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE);
    }
    // /bin namespace is always read-only — userland can't mutate the
    // synthesized applet entries.
    if want_write {
        use crate::userland::bin_namespace::{apply_bin_rewrite, is_bin_dir};
        if is_bin_dir(&path) || apply_bin_rewrite(&path).is_some() {
            return EPERM;
        }
        // /proc is synthesized and strictly read-only.
        if crate::userland::procfs::is_proc_path(&path) {
            return EACCES;
        }
        if crate::userland::etc::is_managed_path(&path) {
            return EROFS;
        }
        if !crate::fs::vfs::vfs_is_writable(&path) {
            return EROFS;
        }
    }

    // Synthetic /proc namespace: content (or the directory listing) is
    // generated once here — the fd owns the snapshot.
    if crate::userland::procfs::is_proc_path(&path) {
        use crate::lib::arc::Arc;
        return match crate::userland::procfs::open_node(&path) {
            Some(crate::userland::procfs::ProcNode::File(data)) => with_fd_table_mut(|t| {
                t.alloc(FdSlot::VirtualFile {
                    data: Arc::new(data),
                    path: Arc::new(path.clone()),
                    cursor: 0,
                    cloexec,
                })
            })
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE),
            Some(crate::userland::procfs::ProcNode::Dir(entries)) => with_fd_table_mut(|t| {
                t.alloc(FdSlot::VirtualDir {
                    entries: Arc::new(entries),
                    path: Arc::new(path.clone()),
                    cursor: 0,
                    cloexec,
                })
            })
            .map(|fd| fd as i64)
            .unwrap_or(EMFILE),
            None => ENOENT,
        };
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
        return with_fd_table_mut(|t| {
            t.alloc(FdSlot::File {
                handle,
                status_flags: O_RDONLY,
                cloexec,
            })
        })
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
        if want_create && (flags & O_EXCL) != 0 {
            return EEXIST;
        }
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
    let status_flags = access | (flags & (O_APPEND | O_NONBLOCK));
    with_fd_table_mut(|t| {
        t.alloc(FdSlot::File {
            handle,
            status_flags,
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
    let result = with_fd_table_mut(|t| t.close(fd)).err().unwrap_or(0);
    let prune_from = slot.as_ref().filter(|description| {
        result == 0
            && !crate::userland::lifecycle::with_current_group(|process| {
                process.fd_table.contains_open_description(description)
            })
    });
    if let Some(description) = prune_from {
        let epolls = crate::userland::lifecycle::with_current_group(|process| {
            process.fd_table.epoll_instances()
        });
        for epoll in epolls {
            epoll.prune_open_description(description);
        }
    }
    drop(slot);
    crate::net::drain_deferred_closes();
    crate::userland::lifecycle::wake_ring3_blocked_on_network(true);
    crate::userland::readiness::notify_changed();
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

/// The synthesized `/proc` namespace rejects every mutation.
fn proc_namespace_mutation_check(path: &str) -> Option<i64> {
    crate::userland::procfs::is_proc_path(path).then_some(EPERM)
}

/// The synthesized `/dev` namespace is entirely kernel-owned.
fn dev_namespace_mutation_check(path: &str) -> Option<i64> {
    crate::userland::devfs::is_dev_path(path).then_some(EPERM)
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
    if let Some(e) = proc_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&path) {
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
    if let Some(e) = proc_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&path) {
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
    if let Some(e) = proc_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&path) {
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
    if let Some(e) = proc_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&path) {
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
    if let Some(e) = proc_namespace_mutation_check(&old) {
        return e;
    }
    if let Some(e) = proc_namespace_mutation_check(&new) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&old) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&new) {
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

pub fn link_handler(args: &mut SyscallArgs) -> i64 {
    let old = match resolve_user_path(args.rdi) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let new = match resolve_user_path(args.rsi) {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Some(error) = bin_namespace_mutation_check(&old)
        .or_else(|| bin_namespace_mutation_check(&new))
        .or_else(|| managed_etc_mutation_check(&old))
        .or_else(|| managed_etc_mutation_check(&new))
        .or_else(|| proc_namespace_mutation_check(&old))
        .or_else(|| proc_namespace_mutation_check(&new))
        .or_else(|| dev_namespace_mutation_check(&old))
        .or_else(|| dev_namespace_mutation_check(&new))
    {
        return error;
    }
    match crate::fs::vfs::vfs_link(&old, &new) {
        Ok(()) => 0,
        Err(crate::fs::filesystem::FilesystemError::UnsupportedOperation) => EXDEV,
        Err(ref error) => map_filesystem_err(error),
    }
}

pub fn linkat_handler(args: &mut SyscallArgs) -> i64 {
    if args.rdi as i32 != AT_FDCWD || args.rdx as i32 != AT_FDCWD {
        return ENOSYS;
    }
    let mut forwarded = SyscallArgs {
        rax: args.rax,
        rdi: args.rsi,
        rsi: args.r10,
        rdx: 0,
        r10: 0,
        r8: 0,
        r9: 0,
    };
    link_handler(&mut forwarded)
}

pub fn symlink_handler(args: &mut SyscallArgs) -> i64 {
    let target = match copy_user_cstr(args.rdi) {
        Ok(target) => target,
        Err(error) => return error,
    };
    let link_path = match resolve_user_path(args.rsi) {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Some(error) = bin_namespace_mutation_check(&link_path)
        .or_else(|| managed_etc_mutation_check(&link_path))
        .or_else(|| proc_namespace_mutation_check(&link_path))
        .or_else(|| dev_namespace_mutation_check(&link_path))
    {
        return error;
    }
    match crate::fs::vfs::vfs_symlink(&target, &link_path) {
        Ok(()) => 0,
        Err(ref error) => map_filesystem_err(error),
    }
}

pub fn symlinkat_handler(args: &mut SyscallArgs) -> i64 {
    if args.rsi as i32 != AT_FDCWD {
        return ENOSYS;
    }
    let mut forwarded = SyscallArgs {
        rax: args.rax,
        rdi: args.rdi,
        rsi: args.rdx,
        rdx: 0,
        r10: 0,
        r8: 0,
        r9: 0,
    };
    symlink_handler(&mut forwarded)
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
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
        Some(_) | None => return EBADF,
    };
    if crate::userland::etc::is_managed_path(&handle.path()) {
        return EROFS;
    }
    handle
        .truncate(new_size)
        .map_or_else(|ref error| map_file_err(error), |_| 0)
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
    if let Some(e) = proc_namespace_mutation_check(&path) {
        return e;
    }
    if let Some(e) = dev_namespace_mutation_check(&path) {
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
    handle
        .truncate(new_size)
        .map_or_else(|ref error| map_file_err(error), |_| 0)
}

pub fn fsync_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    match with_fd_slot(fd) {
        Some(FdSlot::File { handle, .. }) => handle
            .sync(false)
            .map_or_else(|ref error| map_file_err(error), |_| 0),
        Some(FdSlot::Directory { .. }) => 0,
        Some(_) | None => EBADF,
    }
}

pub fn fdatasync_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    match with_fd_slot(fd) {
        Some(FdSlot::File { handle, .. }) => handle
            .sync(true)
            .map_or_else(|ref error| map_file_err(error), |_| 0),
        Some(FdSlot::Directory { .. }) => 0,
        Some(_) | None => EBADF,
    }
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
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
        Some(FdSlot::Urandom { .. }) => return ESPIPE,
        // pread on /dev/null: EOF at every offset.
        Some(FdSlot::DevNull { .. }) => return 0,
        Some(FdSlot::VirtualFile { data, .. }) => {
            // Positional read from the open-time snapshot; per-fd
            // cursor untouched, mirroring pread semantics.
            if len > WRITE_MAX_LEN as u64 {
                return EFAULT;
            }
            if len == 0 {
                return 0;
            }
            let start = (offset as usize).min(data.len());
            let n = core::cmp::min(len as usize, data.len() - start);
            if n > 0 {
                if let Err(e) =
                    crate::userland::usercopy::copy_to_user(ptr, &data[start..start + n])
                {
                    return e;
                }
            }
            return n as i64;
        }
        Some(_) | None => return EBADF,
    };
    if len == 0 {
        return 0;
    }
    // Short read at the staging bound (POSIX-legal); libc loops.
    let len = core::cmp::min(len, READ_MAX_LEN as u64);
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
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
        // Discard sink at every offset.
        Some(FdSlot::DevNull { .. }) => {
            if let Err(e) = crate::userland::usercopy::ensure_user_range(ptr, len, false) {
                return e;
            }
            return len as i64;
        }
        Some(_) | None => return EBADF,
    };
    if len == 0 {
        return 0;
    }
    let prev = handle.position();
    if let Err(ref e) = handle.seek(offset) {
        return map_file_err(e);
    }
    // Chunked write at the seeked position; `File::write` advances the
    // shared position, so consecutive chunks land contiguously. The
    // original position is restored afterwards per pwrite semantics.
    let ret = write_file_chunked(&handle, ptr, len);
    let _ = handle.seek(prev);
    ret
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
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
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
        Some(FdSlot::Directory { .. })
        | Some(FdSlot::VirtualBinDir { .. })
        | Some(FdSlot::VirtualDir { .. })
        | Some(FdSlot::VirtualDevDir { .. }) => return EISDIR,
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
    // Match write(2)/writev(2): stdout and stderr belong to the issuing
    // process's terminal, not the legacy global print target. BusyBox cat
    // reaches this path for regular files, so routing through crate::print!
    // would report a successful copy while drawing outside the terminal.
    let dest_terminal = if matches!(&out, Out::StdoutErr) {
        crate::userland::lifecycle::with_current_group(|p| p.terminal_id)
    } else {
        None
    };

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
                match dest_terminal {
                    Some(tid) => crate::window::terminal::write_to_terminal_id(tid, &s),
                    None => crate::print!("{}", s),
                }
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
        // /dev/null is seekable and always at offset 0, like Linux.
        Some(FdSlot::DevNull { .. }) => return 0,
        Some(FdSlot::VirtualFile { data, cursor, .. }) => {
            // Snapshot-backed /proc file: seek within [0, len].
            let new_pos: i64 = match whence {
                SEEK_SET => offset,
                SEEK_CUR => (cursor as i64).saturating_add(offset),
                SEEK_END => (data.len() as i64).saturating_add(offset),
                _ => return EINVAL,
            };
            if new_pos < 0 {
                return EINVAL;
            }
            let clamped = (new_pos as usize).min(data.len());
            with_fd_table_mut(|t| {
                if let Some(FdSlot::VirtualFile { cursor: c, .. }) = t.get_mut(fd) {
                    *c = clamped;
                }
            });
            return clamped as i64;
        }
        Some(FdSlot::VirtualDir { .. }) => {
            if whence == SEEK_SET && offset == 0 {
                with_fd_table_mut(|t| {
                    if let Some(FdSlot::VirtualDir { cursor, .. }) = t.get_mut(fd) {
                        *cursor = 0;
                    }
                });
                return 0;
            }
            return ESPIPE;
        }
        Some(FdSlot::VirtualDevDir { .. }) => {
            if whence == SEEK_SET && offset == 0 {
                with_fd_table_mut(|t| {
                    if let Some(FdSlot::VirtualDevDir { cursor, .. }) = t.get_mut(fd) {
                        *cursor = 0;
                    }
                });
                return 0;
            }
            return ESPIPE;
        }
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
    // Keep a temporary reference to a replaced endpoint so its final Drop
    // (and any pipe EOF/EPIPE wake) happens after PROCESS_TABLE is unlocked.
    let replaced = if oldfd != newfd {
        with_fd_slot(newfd)
    } else {
        None
    };
    let result = with_fd_table_mut(|t| t.dup2(oldfd, newfd))
        .map(|n| n as i64)
        .unwrap_or(EBADF);
    drop(replaced);
    crate::net::drain_deferred_closes();
    crate::userland::lifecycle::wake_ring3_blocked_on_network(true);
    crate::userland::readiness::notify_changed();
    result
}

/// `fcntl(fd, cmd, arg) -> int`. Implements just enough of the cmd
/// surface for libc startup: F_DUPFD, F_DUPFD_CLOEXEC, F_GETFD,
/// F_SETFD, F_GETFL, and F_SETFL. Socket and pipe nonblocking state lives
/// on the shared open-file description so duplicated fds observe changes.
pub fn fcntl_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let cmd = args.rsi as i32;
    let arg = args.rdx;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            if with_fd_slot(fd).is_none() {
                return EBADF;
            }
            if arg > i32::MAX as u64 || arg >= FD_TABLE_SIZE as u64 {
                return EINVAL;
            }
            with_fd_table_mut(|t| t.dup_from(fd, arg as i32, cmd == F_DUPFD_CLOEXEC))
                .map(|new| new as i64)
                .unwrap_or(EMFILE)
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
        F_GETFL => match with_fd_slot(fd) {
            Some(FdSlot::File { status_flags, .. }) => status_flags as i64,
            Some(FdSlot::Socket { handle, .. }) => {
                let nonblocking = crate::net::socket::nonblocking(handle.id()).unwrap_or(false);
                (O_RDWR | if nonblocking { O_NONBLOCK } else { 0 }) as i64
            }
            Some(FdSlot::PipeRead(handle, _)) => {
                (O_RDONLY | if handle.nonblocking() { O_NONBLOCK } else { 0 }) as i64
            }
            Some(FdSlot::PipeWrite(handle, _)) => {
                (O_WRONLY | if handle.nonblocking() { O_NONBLOCK } else { 0 }) as i64
            }
            Some(FdSlot::GuiEvents { handle, .. }) => {
                (O_RDONLY | if handle.nonblocking() { O_NONBLOCK } else { 0 }) as i64
            }
            Some(FdSlot::EventFd { handle, .. }) => {
                (O_RDWR | if handle.nonblocking() { O_NONBLOCK } else { 0 }) as i64
            }
            Some(FdSlot::Epoll { .. }) => O_RDONLY as i64,
            Some(FdSlot::LocalStream { handle, .. }) => {
                (O_RDWR | if handle.nonblocking() { O_NONBLOCK } else { 0 }) as i64
            }
            Some(_) => O_RDONLY as i64,
            None => EBADF,
        },
        F_SETFL => match with_fd_slot(fd) {
            Some(FdSlot::Socket { handle, .. }) => {
                crate::net::socket::set_nonblocking(handle.id(), arg & O_NONBLOCK as u64 != 0)
                    .map_or_else(crate::userland::network_syscalls::map_socket_error, |_| 0)
            }
            Some(FdSlot::PipeRead(handle, _)) => {
                handle.set_nonblocking(arg & O_NONBLOCK as u64 != 0);
                0
            }
            Some(FdSlot::PipeWrite(handle, _)) => {
                handle.set_nonblocking(arg & O_NONBLOCK as u64 != 0);
                0
            }
            Some(FdSlot::File { .. }) => with_fd_table_mut(|table| {
                let Some(FdSlot::File { status_flags, .. }) = table.get_mut(fd) else {
                    return EBADF;
                };
                *status_flags =
                    (*status_flags & O_ACCMODE) | (arg as u32 & (O_APPEND | O_NONBLOCK));
                0
            }),
            Some(FdSlot::GuiEvents { handle, .. }) => {
                handle.set_nonblocking(arg & O_NONBLOCK as u64 != 0);
                0
            }
            Some(FdSlot::EventFd { handle, .. }) => {
                handle.set_nonblocking(arg & O_NONBLOCK as u64 != 0);
                0
            }
            Some(FdSlot::Epoll { .. }) => 0,
            Some(FdSlot::LocalStream { handle, .. }) => {
                handle.set_nonblocking(arg & O_NONBLOCK as u64 != 0);
                0
            }
            Some(_) => 0,
            None => EBADF,
        },
        _ => ENOSYS,
    }
}

// ---------- stat / access ----------

fn fill_unix_stat(meta: &crate::fs::filesystem::UnixMetadata) -> LinuxStat {
    let mut stat = LinuxStat::default();
    stat.st_ino = meta.inode;
    stat.st_nlink = meta.links;
    stat.st_mode = meta.mode;
    stat.st_uid = meta.uid;
    stat.st_gid = meta.gid;
    stat.st_size = meta.size as i64;
    stat.st_blksize = meta.block_size as i64;
    stat.st_blocks = meta.blocks_512 as i64;
    stat.st_atime = meta.accessed.seconds as i64;
    stat.st_atime_nsec = meta.accessed.nanoseconds as u64;
    stat.st_mtime = meta.modified.seconds as i64;
    stat.st_mtime_nsec = meta.modified.nanoseconds as u64;
    stat.st_ctime = meta.changed.seconds as i64;
    stat.st_ctime_nsec = meta.changed.nanoseconds as u64;
    stat
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
    if let Some(st) = stat_virtual_dev(&path) {
        return write_stat(out_ptr, &st);
    }
    if crate::userland::procfs::is_proc_path(&path) {
        return match stat_virtual_proc(&path) {
            Some(st) => write_stat(out_ptr, &st),
            None => ENOENT,
        };
    }
    let meta = match crate::fs::vfs::vfs_unix_metadata(&path) {
        Ok(m) => m,
        Err(ref e) => return map_filesystem_err(e),
    };
    let st = fill_unix_stat(&meta);
    write_stat(out_ptr, &st)
}

pub fn lstat_handler(args: &mut SyscallArgs) -> i64 {
    let path = match resolve_user_path(args.rdi) {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Some(stat) = stat_virtual_bin(&path) {
        return write_stat(args.rsi, &stat);
    }
    if let Some(stat) = stat_virtual_dev(&path) {
        return write_stat(args.rsi, &stat);
    }
    if crate::userland::procfs::is_proc_path(&path) {
        return match stat_virtual_proc(&path) {
            Some(stat) => write_stat(args.rsi, &stat),
            None => ENOENT,
        };
    }
    let metadata = match crate::fs::vfs::vfs_symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(ref error) => return map_filesystem_err(error),
    };
    write_stat(args.rsi, &fill_unix_stat(&metadata))
}

/// Synthesize a `LinuxStat` for the `/proc` namespace. Files report
/// `st_size = 0` like Linux procfs — readers must loop `read()` to
/// EOF rather than sizing buffers off stat.
fn stat_virtual_proc(path: &str) -> Option<LinuxStat> {
    let kind = crate::userland::procfs::classify(path)?;
    let mut st = LinuxStat::default();
    match kind {
        crate::userland::procfs::ProcNodeKind::Dir => {
            st.st_mode = S_IFDIR | PERM_RX_ALL;
            st.st_nlink = 2;
        }
        crate::userland::procfs::ProcNodeKind::File => {
            st.st_mode = S_IFREG | PERM_READ_ALL;
            st.st_nlink = 1;
        }
    }
    st.st_blksize = 4096;
    Some(st)
}

/// Synthesize metadata for the kernel-owned `/dev` namespace.
fn stat_virtual_dev(path: &str) -> Option<LinuxStat> {
    let mut st = LinuxStat::default();
    match crate::userland::devfs::classify(path)? {
        crate::userland::devfs::DeviceNode::Directory => {
            st.st_mode = S_IFDIR | PERM_RX_ALL;
            st.st_nlink = 2;
        }
        crate::userland::devfs::DeviceNode::Urandom => {
            st.st_mode = S_IFCHR | PERM_READ_ALL;
            st.st_nlink = 1;
            // Linux's /dev/urandom is character device major 1, minor 9.
            st.st_rdev = (1 << 8) | 9;
        }
        crate::userland::devfs::DeviceNode::Null => {
            st.st_mode = S_IFCHR | 0o666;
            st.st_nlink = 1;
            // Linux's /dev/null is character device major 1, minor 3.
            st.st_rdev = (1 << 8) | 3;
        }
    }
    st.st_blksize = 4096;
    Some(st)
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
            let meta = match handle.metadata() {
                Ok(m) => m,
                Err(ref e) => return map_file_err(e),
            };
            let st = fill_unix_stat(&meta);
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
        Some(FdSlot::LocalStream { .. }) => {
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
                let meta = match crate::fs::vfs::vfs_unix_metadata(&path) {
                    Ok(m) => m,
                    Err(ref e) => return map_filesystem_err(e),
                };
                fill_unix_stat(&meta)
            };
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::VirtualBinDir { .. }) => {
            // Synthesized /bin — same shape stat() reports for the path.
            let st = stat_virtual_bin("/bin").expect("/bin is always virtual");
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::VirtualFile { data, .. }) => {
            // For an open fd we know the snapshot length — report it,
            // unlike path-stat which returns 0 (Linux procfs parity).
            let mut st = LinuxStat::default();
            st.st_mode = S_IFREG | PERM_READ_ALL;
            st.st_nlink = 1;
            st.st_size = data.len() as i64;
            st.st_blksize = 4096;
            st.st_blocks = (st.st_size + 511) / 512;
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::VirtualDir { .. }) => {
            let mut st = LinuxStat::default();
            st.st_mode = S_IFDIR | PERM_RX_ALL;
            st.st_nlink = 2;
            st.st_blksize = 4096;
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::VirtualDevDir { .. }) => {
            let st = stat_virtual_dev("/dev").expect("/dev is always virtual");
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::Urandom { .. }) => {
            let st = stat_virtual_dev("/dev/urandom").expect("urandom is always virtual");
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::DevNull { .. }) => {
            let st = stat_virtual_dev("/dev/null").expect("null is always virtual");
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::GuiEvents { .. }) => {
            let st = LinuxStat {
                st_mode: S_IFCHR | 0o600,
                st_blksize: core::mem::size_of::<crate::userland::gui::GuiEvent>() as i64,
                ..LinuxStat::default()
            };
            write_stat(out_ptr, &st)
        }
        Some(FdSlot::EventFd { .. }) | Some(FdSlot::Epoll { .. }) => {
            let mut st = LinuxStat::default();
            st.st_mode = S_IFREG | 0o600;
            st.st_nlink = 1;
            st.st_blksize = 4096;
            write_stat(out_ptr, &st)
        }
        None => EBADF,
    }
}

/// `newfstatat(dirfd, path, statbuf, flags)` — only `AT_FDCWD` is
/// supported for `dirfd`.
pub fn newfstatat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let path_ptr = args.rsi;
    let out_ptr = args.rdx;
    const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
    let flags = args.r10;
    if flags & !AT_SYMLINK_NOFOLLOW != 0 {
        return EINVAL;
    }
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(st) = stat_virtual_bin(&path) {
        return write_stat(out_ptr, &st);
    }
    if let Some(st) = stat_virtual_dev(&path) {
        return write_stat(out_ptr, &st);
    }
    if crate::userland::procfs::is_proc_path(&path) {
        return match stat_virtual_proc(&path) {
            Some(st) => write_stat(out_ptr, &st),
            None => ENOENT,
        };
    }
    let meta = match if flags & AT_SYMLINK_NOFOLLOW != 0 {
        crate::fs::vfs::vfs_symlink_metadata(&path)
    } else {
        crate::fs::vfs::vfs_unix_metadata(&path)
    } {
        Ok(m) => m,
        Err(ref e) => return map_filesystem_err(e),
    };
    let st = fill_unix_stat(&meta);
    write_stat(out_ptr, &st)
}

pub fn access_handler(args: &mut SyscallArgs) -> i64 {
    let path_ptr = args.rdi;
    let mode = args.rsi as u32;
    access_common(path_ptr, mode)
}

pub fn faccessat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    let path_ptr = args.rsi;
    let mode = args.rdx as u32;
    access_common(path_ptr, mode)
}

fn access_common(path_ptr: u64, mode: u32) -> i64 {
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
    if crate::userland::procfs::is_proc_path(&path) {
        return if crate::userland::procfs::classify(&path).is_some() {
            0
        } else {
            ENOENT
        };
    }
    if let Some(node) = crate::userland::devfs::classify(&path) {
        // /dev/null is the one writable device node.
        if matches!(node, crate::userland::devfs::DeviceNode::Null) {
            return 0;
        }
        return if mode & _W_OK != 0 { EACCES } else { 0 };
    }
    if crate::fs::exists(&path) {
        0
    } else {
        ENOENT
    }
}

/// `chmod(path, mode) -> int`. Validated success no-op: FAT and tmpfs
/// carry no permission bits and `execve` performs no +x check, so the
/// only observable POSIX behavior is the existence check. TinyCC (and
/// other toolchains) chmod their output executable after writing it.
pub fn chmod_handler(args: &mut SyscallArgs) -> i64 {
    let path_ptr = args.rdi;
    let _mode = args.rsi as u32;
    let path = match resolve_user_path(path_ptr) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if crate::userland::bin_namespace::is_bin_dir(&path)
        || crate::userland::bin_namespace::apply_bin_rewrite(&path).is_some()
    {
        return EPERM;
    }
    if crate::fs::exists(&path) {
        0
    } else {
        ENOENT
    }
}

/// `fchmod(fd, mode) -> int`. Same no-op semantics as `chmod`, keyed
/// on descriptor validity instead of a path.
pub fn fchmod_handler(args: &mut SyscallArgs) -> i64 {
    let fd = args.rdi as i32;
    let _mode = args.rsi as u32;
    match with_fd_slot(fd) {
        Some(_) => 0,
        None => EBADF,
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
        Some(FdSlot::Directory { handle, .. }) => handle.path(),
        Some(FdSlot::VirtualDir { path, .. }) => (*path).clone(),
        Some(FdSlot::VirtualBinDir { .. }) => String::from("/bin"),
        Some(FdSlot::VirtualDevDir { .. }) => String::from("/dev"),
        Some(_) => return ENOTDIR,
        None => return EBADF,
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

    // Kernel-synthesized namespaces participate in path resolution just like
    // mounted directories. BusyBox top relies on `chdir("/proc")` before it
    // opens stat/meminfo/loadavg by relative name.
    if crate::userland::bin_namespace::is_bin_dir(&path) {
        set_cwd(path);
        return 0;
    }
    if crate::userland::bin_namespace::apply_bin_rewrite(&path).is_some() {
        return ENOTDIR;
    }
    if crate::userland::procfs::is_proc_path(&path) {
        return match crate::userland::procfs::classify(&path) {
            Some(crate::userland::procfs::ProcNodeKind::Dir) => {
                set_cwd(path);
                0
            }
            Some(crate::userland::procfs::ProcNodeKind::File) => ENOTDIR,
            None => ENOENT,
        };
    }
    if crate::userland::devfs::is_dev_path(&path) {
        return match crate::userland::devfs::classify(&path) {
            Some(crate::userland::devfs::DeviceNode::Directory) => {
                set_cwd(path);
                0
            }
            Some(
                crate::userland::devfs::DeviceNode::Urandom
                | crate::userland::devfs::DeviceNode::Null,
            ) => ENOTDIR,
            None => ENOENT,
        };
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

/// `umask(mask) -> previous_mask`. Permission enforcement remains minimal,
/// but the state is process-local, inherited across fork, and retained across
/// exec so BFD and future toolchains observe normal POSIX behavior.
pub fn umask_handler(args: &mut SyscallArgs) -> i64 {
    crate::userland::lifecycle::with_current_group(|process| {
        let old = process.umask;
        process.umask = args.rdi as u32 & 0o777;
        old as i64
    })
}

fn decode_utimens_value(
    value: LinuxTimespec,
    now: crate::fs::filesystem::UnixTimestamp,
) -> Result<Option<crate::fs::filesystem::UnixTimestamp>, i64> {
    match value.tv_nsec {
        UTIME_NOW => Ok(Some(now)),
        UTIME_OMIT => Ok(None),
        0..=999_999_999 if value.tv_sec >= 0 => Ok(Some(crate::fs::filesystem::UnixTimestamp {
            seconds: value.tv_sec as u64,
            nanoseconds: value.tv_nsec as u32,
        })),
        _ => Err(EINVAL),
    }
}

/// `utimensat(AT_FDCWD, path, times, 0)` with Linux `UTIME_NOW` and
/// `UTIME_OMIT` support. Directory-fd-relative and no-follow variants stay
/// outside the current path ABI and fail explicitly.
pub fn utimensat_handler(args: &mut SyscallArgs) -> i64 {
    let dirfd = args.rdi as i32;
    let path_ptr = args.rsi;
    let times_ptr = args.rdx;
    let flags = args.r10;
    if dirfd != AT_FDCWD {
        return ENOSYS;
    }
    if flags != 0 {
        return EINVAL;
    }
    let path = match resolve_user_path(path_ptr) {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Some(error) = bin_namespace_mutation_check(&path) {
        return error;
    }
    if let Some(error) = managed_etc_mutation_check(&path) {
        return error;
    }
    if let Some(error) = proc_namespace_mutation_check(&path) {
        return error;
    }
    if let Some(error) = dev_namespace_mutation_check(&path) {
        return error;
    }

    let now = crate::fs::filesystem::UnixTimestamp::from_nanoseconds(crate::time::realtime_ns());
    let (accessed, modified) = if times_ptr == 0 {
        (Some(now), Some(now))
    } else {
        let atime: LinuxTimespec = match crate::userland::usercopy::read_unaligned(times_ptr) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let mtime: LinuxTimespec = match crate::userland::usercopy::read_unaligned(times_ptr + 16) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let accessed = match decode_utimens_value(atime, now) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let modified = match decode_utimens_value(mtime, now) {
            Ok(value) => value,
            Err(error) => return error,
        };
        (accessed, modified)
    };

    crate::fs::vfs::vfs_set_times(&path, accessed, modified)
        .map_or_else(|ref error| map_filesystem_err(error), |_| 0)
}

/// `getrandom(buf, len, flags) -> ssize_t` backed by the kernel's trusted
/// platform random broker. Unknown flags are rejected; GRND_RANDOM uses the
/// Linux 512-byte single-call bound while the ordinary source uses 4 KiB.
pub fn getrandom_handler(args: &mut SyscallArgs) -> i64 {
    let buf = args.rdi;
    let len = args.rsi;
    let flags = args.rdx;
    const GRND_NONBLOCK: u64 = 0x1;
    const GRND_RANDOM: u64 = 0x2;

    if flags & !(GRND_NONBLOCK | GRND_RANDOM) != 0 {
        return EINVAL;
    }
    if len == 0 {
        return 0;
    }
    let max = if flags & GRND_RANDOM != 0 { 512 } else { 4096 };
    let cap = core::cmp::min(len, max);
    if let Err(error) = validate_user_slice(buf, cap) {
        return error;
    }
    let mut bytes = alloc::vec![0u8; cap as usize];
    if crate::random::fill_bytes(&mut bytes).is_err() {
        return if flags & GRND_NONBLOCK != 0 {
            EAGAIN
        } else {
            EIO
        };
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
const DT_CHR: u8 = 2;
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

/// `getdents64` dispatch for synthesized `/proc` directories
/// (`FdSlot::VirtualDir`). Same record format and cursor encoding as
/// `getdents64_virtual_bin`: cursor 0/1 emit `.`/`..`, cursor `n ≥ 2`
/// emits `entries[n - 2]`. Returns `None` to fall through.
fn getdents64_virtual_dir(fd: i32, dirp: u64, cap: usize) -> Option<i64> {
    let (entries, dir_path, start) = with_fd_table_mut(|t| match t.get(fd) {
        Some(FdSlot::VirtualDir {
            entries,
            path,
            cursor,
            ..
        }) => Some((entries.clone(), path.clone(), *cursor)),
        _ => None,
    })?;
    let total_records = entries.len() + 2;
    if start >= total_records {
        return Some(0);
    }

    let mut staging: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(cap);
    let mut cursor = start;
    let parent_seed = fnv1a_64(0xcbf2_9ce4_8422_2325, dir_path.as_bytes());

    while cursor < total_records {
        let (name, d_type) = match cursor {
            0 => (".".as_bytes(), DT_DIR),
            1 => ("..".as_bytes(), DT_DIR),
            n => {
                let (name, is_dir) = &entries[n - 2];
                (name.as_bytes(), if *is_dir { DT_DIR } else { DT_REG })
            }
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
        return Some(EINVAL);
    }

    with_fd_table_mut(|t| {
        if let Some(FdSlot::VirtualDir { cursor: c, .. }) = t.get_mut(fd) {
            *c = cursor;
        }
    });
    Some(
        crate::userland::usercopy::copy_to_user(dirp, &staging)
            .map_or_else(|e| e, |_| staging.len() as i64),
    )
}

/// `getdents64` dispatch for the synthetic `/dev` directory.
fn getdents64_virtual_dev(fd: i32, dirp: u64, cap: usize) -> Option<i64> {
    let start = with_fd_table_mut(|t| match t.get(fd) {
        Some(FdSlot::VirtualDevDir { cursor, .. }) => Some(*cursor),
        _ => None,
    })?;
    const RECORDS: [(&[u8], u8); 4] = [
        (b".", DT_DIR),
        (b"..", DT_DIR),
        (b"null", DT_CHR),
        (b"urandom", DT_CHR),
    ];
    if start >= RECORDS.len() {
        return Some(0);
    }

    let parent_seed = fnv1a_64(0xcbf2_9ce4_8422_2325, b"/dev");
    let mut staging = alloc::vec::Vec::with_capacity(cap);
    let mut cursor = start;
    while cursor < RECORDS.len() {
        let (name, d_type) = RECORDS[cursor];
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
        return Some(EINVAL);
    }
    with_fd_table_mut(|t| {
        if let Some(FdSlot::VirtualDevDir { cursor: c, .. }) = t.get_mut(fd) {
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
    // Synthetic /proc directories carry their listing snapshot in the
    // fd slot itself.
    if let Some(written) = getdents64_virtual_dir(fd, dirp, cap as usize) {
        return written;
    }
    if let Some(written) = getdents64_virtual_dev(fd, dirp, cap as usize) {
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

// poll/ppoll constants used by the shared descriptor-readiness snapshot.
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

#[derive(Clone, Copy, Default)]
pub(crate) struct FdReady {
    pub(crate) readable: bool,
    pub(crate) writable: bool,
    pub(crate) error: bool,
    pub(crate) hangup: bool,
}

/// Snapshot readiness without consuming data. `with_fd_slot` clones the
/// handle while holding the process table and releases that lock before a
/// socket readiness query takes the network lock.
fn fd_readiness(fd: i32) -> Result<FdReady, i64> {
    let slot = with_fd_slot(fd).ok_or(EBADF)?;
    fd_slot_readiness(&slot)
}

pub(crate) fn fd_slot_readiness(slot: &FdSlot) -> Result<FdReady, i64> {
    match slot {
        FdSlot::Stdin => Ok(FdReady {
            readable: crate::userland::stdin::queued_len_for_current_process() != 0,
            ..FdReady::default()
        }),
        FdSlot::Stdout | FdSlot::Stderr => Ok(FdReady {
            writable: true,
            ..FdReady::default()
        }),
        FdSlot::Urandom { .. } => Ok(FdReady {
            readable: true,
            ..FdReady::default()
        }),
        FdSlot::DevNull { .. } => Ok(FdReady {
            readable: true,
            writable: true,
            ..FdReady::default()
        }),
        FdSlot::GuiEvents { handle, .. } => {
            if crate::userland::lifecycle::current_user_pid() != Some(handle.owner_pid()) {
                return Err(EBADF);
            }
            Ok(FdReady {
                readable: crate::userland::gui::has_events(handle.owner_pid()),
                ..FdReady::default()
            })
        }
        FdSlot::File { .. }
        | FdSlot::VirtualFile { .. }
        | FdSlot::Directory { .. }
        | FdSlot::VirtualBinDir { .. }
        | FdSlot::VirtualDevDir { .. }
        | FdSlot::VirtualDir { .. } => Ok(FdReady {
            readable: true,
            writable: true,
            ..FdReady::default()
        }),
        FdSlot::PipeRead(handle, _) => {
            let eof = handle.pipe().writers() == 0;
            Ok(FdReady {
                readable: handle.pipe().len() != 0 || eof,
                hangup: eof,
                ..FdReady::default()
            })
        }
        FdSlot::PipeWrite(handle, _) => {
            let no_readers = handle.pipe().readers() == 0;
            Ok(FdReady {
                writable: !no_readers && handle.pipe().has_capacity(),
                error: no_readers,
                ..FdReady::default()
            })
        }
        FdSlot::Socket { handle, .. } => crate::net::socket::readiness(handle.id())
            .map(|state| FdReady {
                readable: state.readable,
                writable: state.writable,
                error: state.error,
                hangup: state.hangup,
            })
            .map_err(crate::userland::network_syscalls::map_socket_error),
        FdSlot::EventFd { handle, .. } => {
            let (readable, writable) = handle.readiness();
            Ok(FdReady {
                readable,
                writable,
                ..FdReady::default()
            })
        }
        FdSlot::Epoll { handle, .. } => Ok(FdReady {
            readable: handle.is_ready(),
            ..FdReady::default()
        }),
        FdSlot::LocalStream { handle, .. } => {
            let (readable, writable, error, hangup) = handle.readiness();
            Ok(FdReady {
                readable,
                writable,
                error,
                hangup,
            })
        }
    }
}

/// `poll(fds: *mut pollfd, nfds: nfds_t, timeout: int) -> int`
///
/// Validates the user pollfd array, samples shared readiness for streams,
/// files, pipes, and sockets, and parks the process until an input, pipe,
/// socket, close, or timeout wakeup makes it worth sampling again.
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
/// Linux-x86-64 ppoll. The timeout is honored; temporary signal masks are
/// not implemented yet.
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
    crate::net::poll_once();
    // poll_once may itself publish network progress. Sample after it so the
    // lost-wake guard covers only changes concurrent with the descriptor scan,
    // rather than forcing a spurious immediate restart for work scanned below.
    let observed_sequence = crate::userland::readiness::sequence();
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
            match fd_readiness(entry.fd) {
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
                Err(EBADF) => POLLNVAL,
                Err(_) => POLLERR,
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
    if ready != 0 || timeout_ticks == Some(0) {
        crate::userland::lifecycle::clear_network_wait();
        return ready;
    }
    let identity = fds_ptr ^ nfds.rotate_left(17);
    crate::userland::readiness::block(args, identity, timeout_ticks, observed_sequence)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SelectTimeval {
    seconds: i64,
    microseconds: i64,
}

fn select_read_mask(pointer: u64, nfds: usize) -> Result<u64, i64> {
    if pointer == 0 || nfds == 0 {
        return Ok(0);
    }
    let value = crate::userland::usercopy::read_unaligned::<u64>(pointer)?;
    let valid = if nfds == 64 {
        u64::MAX
    } else {
        (1u64 << nfds) - 1
    };
    Ok(value & valid)
}

fn select_write_mask(pointer: u64, value: u64, nfds: usize) -> Result<(), i64> {
    if pointer == 0 || nfds == 0 {
        return Ok(());
    }
    crate::userland::usercopy::write_unaligned(pointer, &value)
}

/// Linux x86-64 `select(2)`. Links uses this as its central terminal, pipe,
/// timer, and socket event loop.
pub fn select_handler(args: &mut SyscallArgs) -> i64 {
    let nfds_signed = args.rdi as i64;
    if nfds_signed < 0 || nfds_signed as usize > crate::userland::fdtable::FD_TABLE_SIZE {
        return EINVAL;
    }
    let nfds = nfds_signed as usize;
    let read_in = match select_read_mask(args.rsi, nfds) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let write_in = match select_read_mask(args.rdx, nfds) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let except_in = match select_read_mask(args.r10, nfds) {
        Ok(value) => value,
        Err(error) => return error,
    };

    let timeout_ticks = if args.r8 == 0 {
        None
    } else {
        let timeout = match crate::userland::usercopy::read_unaligned::<SelectTimeval>(args.r8) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if timeout.seconds < 0 || !(0..1_000_000).contains(&timeout.microseconds) {
            return EINVAL;
        }
        let milliseconds = (timeout.seconds as u64)
            .saturating_mul(1000)
            .saturating_add((timeout.microseconds as u64 + 999) / 1000);
        Some((milliseconds + 9) / 10)
    };

    crate::net::poll_once();
    let observed_sequence = crate::userland::readiness::sequence();
    let requested = read_in | write_in | except_in;
    let mut read_out = 0u64;
    let mut write_out = 0u64;
    let except_out = 0u64;
    let mut ready = 0i64;
    for fd in 0..nfds {
        let bit = 1u64 << fd;
        if requested & bit == 0 {
            continue;
        }
        let state = match fd_readiness(fd as i32) {
            Ok(state) => state,
            Err(EBADF) => return EBADF,
            Err(error) => return error,
        };
        if read_in & bit != 0 && (state.readable || state.error || state.hangup) {
            read_out |= bit;
            ready += 1;
        }
        if write_in & bit != 0 && (state.writable || state.error) {
            write_out |= bit;
            ready += 1;
        }
    }

    if ready == 0 && timeout_ticks != Some(0) {
        let identity = args.rsi
            ^ args.rdx.rotate_left(11)
            ^ args.r10.rotate_left(23)
            ^ (nfds as u64).rotate_left(37);
        // Diverges while blocked; returns only when the restart-stable
        // absolute deadline has expired.
        let _ = crate::userland::readiness::block(args, identity, timeout_ticks, observed_sequence);
    } else {
        crate::userland::lifecycle::clear_network_wait();
    }

    if let Err(error) = select_write_mask(args.rsi, read_out, nfds) {
        return error;
    }
    if let Err(error) = select_write_mask(args.rdx, write_out, nfds) {
        return error;
    }
    if let Err(error) = select_write_mask(args.r10, except_out, nfds) {
        return error;
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

/// Linux `sched_yield(2)`. The full voluntary ring-3 handoff is wired after
/// the descriptor primitives; returning success is sufficient for synthetic
/// dispatch tests and is replaced by the switch helper in this feature.
pub fn sched_yield_handler(args: &mut SyscallArgs) -> i64 {
    if crate::arch::x86_64::percpu::current_user_pid().is_none() {
        return 0;
    }
    unsafe { crate::userland::switch::yield_current_ring3(args) }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxStackT {
    ss_sp: u64,
    ss_flags: i32,
    _padding: u32,
    ss_size: u64,
}

pub fn sigaltstack_handler(args: &mut SyscallArgs) -> i64 {
    const SS_ONSTACK: i32 = 1;
    const SS_DISABLE: i32 = 2;
    const MINSIGSTKSZ: u64 = 2048;

    let user_rsp = if crate::arch::x86_64::percpu::current_user_pid().is_some() {
        unsafe {
            crate::userland::user_state::read_user_callee_saved(args as *const SyscallArgs)
                .r12_register
        }
    } else {
        0
    };
    let current =
        crate::userland::lifecycle::with_current_process(|process| process.signal_alt_stack);

    if args.rsi != 0 {
        let old = LinuxStackT {
            ss_sp: current.sp,
            ss_flags: if !current.enabled {
                SS_DISABLE
            } else if current.contains(user_rsp) {
                SS_ONSTACK
            } else {
                0
            },
            _padding: 0,
            ss_size: current.size,
        };
        if let Err(error) = crate::userland::usercopy::write_unaligned(args.rsi, &old) {
            return error;
        }
    }
    if args.rdi == 0 {
        return 0;
    }
    if current.contains(user_rsp) {
        return EPERM;
    }
    let new: LinuxStackT = match crate::userland::usercopy::read_unaligned(args.rdi) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if new.ss_flags == SS_DISABLE {
        crate::userland::lifecycle::with_current_process(|process| {
            process.signal_alt_stack = crate::userland::signal::SignalAltStack::default();
        });
        return 0;
    }
    if new.ss_flags != 0 {
        return EINVAL;
    }
    if new.ss_size < MINSIGSTKSZ {
        return ENOMEM;
    }
    let Some(end) = new.ss_sp.checked_add(new.ss_size) else {
        return ENOMEM;
    };
    if VirtAddr::try_new(new.ss_sp).is_err() || VirtAddr::try_new(end).is_err() {
        return ENOMEM;
    }
    let writable = crate::userland::lifecycle::with_current_group(|process| {
        process.address_space.as_ref().map(|space| {
            space
                .vmas()
                .covers(new.ss_sp, new.ss_size, crate::userland::vm::VmProt::WRITE)
        })
    });
    let writable = writable.unwrap_or_else(|| {
        crate::userland::abi::user_va_bounds()
            .is_some_and(|bounds| new.ss_sp >= bounds.start && end <= bounds.end)
    });
    if !writable {
        return ENOMEM;
    }
    crate::userland::lifecycle::with_current_process(|process| {
        process.signal_alt_stack = crate::userland::signal::SignalAltStack {
            sp: new.ss_sp,
            size: new.ss_size,
            enabled: true,
        };
    });
    0
}

pub fn membarrier_handler(args: &mut SyscallArgs) -> i64 {
    const MEMBARRIER_CMD_QUERY: u64 = 0;
    const MEMBARRIER_CMD_PRIVATE_EXPEDITED: u64 = 1 << 3;
    const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: u64 = 1 << 4;
    if args.rsi != 0 {
        return EINVAL;
    }
    match args.rdi {
        MEMBARRIER_CMD_QUERY => {
            (MEMBARRIER_CMD_PRIVATE_EXPEDITED | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED) as i64
        }
        MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            if !crate::userland::lifecycle::current_group_has_single_cpu_execution() {
                return ENOSYS;
            }
            crate::userland::lifecycle::with_current_group(|process| {
                process.membarrier_private_registered = true;
            });
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        }
        MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            if !crate::userland::lifecycle::current_group_has_single_cpu_execution() {
                return ENOSYS;
            }
            let registered = crate::userland::lifecycle::with_current_group(|process| {
                process.membarrier_private_registered
            });
            if !registered {
                return EPERM;
            }
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            0
        }
        _ => EINVAL,
    }
}

pub fn madvise_handler(args: &mut SyscallArgs) -> i64 {
    use crate::userland::vm::{VmProt, VmaBacking};
    const MADV_NORMAL: u64 = 0;
    const MADV_RANDOM: u64 = 1;
    const MADV_SEQUENTIAL: u64 = 2;
    const MADV_WILLNEED: u64 = 3;
    const MADV_DONTNEED: u64 = 4;
    const MADV_FREE: u64 = 8;

    let address = args.rdi;
    let length = args.rsi;
    let advice = args.rdx;
    if address & 0xfff != 0 {
        return EINVAL;
    }
    if length == 0 {
        return 0;
    }
    let Some(rounded) = length.checked_add(0xfff).map(|value| value & !0xfff) else {
        return EINVAL;
    };
    let Some(end) = address.checked_add(rounded) else {
        return EINVAL;
    };
    if !matches!(
        advice,
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED | MADV_DONTNEED | MADV_FREE
    ) {
        return EINVAL;
    }
    let discard = advice == MADV_DONTNEED || advice == MADV_FREE;
    let l4 = crate::userland::lifecycle::with_current_group(|process| {
        let space = process.address_space.as_ref()?;
        if !space.vmas().covers(address, rounded, VmProt::NONE) {
            return None;
        }
        if discard {
            let mut cursor = address;
            while cursor < end {
                let vma = space.vmas().find(cursor)?;
                if !matches!(
                    vma.backing,
                    VmaBacking::Anonymous | VmaBacking::FilePrivate { .. }
                ) {
                    return None;
                }
                cursor = vma.end.min(end);
            }
        }
        Some(space.l4_frame())
    });
    let Some(l4) = l4 else {
        return ENOMEM;
    };
    if discard {
        crate::mm::memory::with_memory_mapper(|mapper| {
            let mut page = address;
            while page < end {
                if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
                    let _ = mapper.unmap_page_from(l4, VirtAddr::new(page));
                }
                page += 0x1000;
            }
        });
    }
    0
}

pub fn mremap_handler(args: &mut SyscallArgs) -> i64 {
    const MREMAP_MAYMOVE: u64 = 1;
    let old_address = args.rdi;
    let old_length = args.rsi;
    let new_length = args.rdx;
    let flags = args.r10;
    if old_address & 0xfff != 0
        || old_length == 0
        || new_length == 0
        || flags & !MREMAP_MAYMOVE != 0
        || old_length > MMAP_MAX_LEN
        || new_length > MMAP_MAX_LEN
    {
        return EINVAL;
    }
    let Some(old_size) = old_length.checked_add(0xfff).map(|value| value & !0xfff) else {
        return EINVAL;
    };
    let Some(new_size) = new_length.checked_add(0xfff).map(|value| value & !0xfff) else {
        return EINVAL;
    };
    let Some(old_end) = old_address.checked_add(old_size) else {
        return EINVAL;
    };
    let allocation = crate::userland::lifecycle::with_current_group(|process| {
        process
            .address_space
            .as_ref()?
            .vmas()
            .anonymous_allocation(old_address, old_end)
    });
    let Some(allocation) = allocation else {
        return EINVAL;
    };
    if old_size == new_size {
        return old_address as i64;
    }

    if new_size < old_size {
        let new_end = old_address + new_size;
        let l4 = crate::userland::lifecycle::with_current_group(|process| {
            let space = process.address_space.as_mut()?;
            space
                .vmas_mut()
                .replace_anonymous_allocation(old_address, old_end, old_address, new_end)
                .ok()?;
            Some(space.l4_frame())
        });
        let Some(l4) = l4 else {
            return ENOMEM;
        };
        crate::mm::memory::with_memory_mapper(|mapper| {
            let mut page = new_end;
            while page < old_end {
                if mapper.leaf_info(l4, VirtAddr::new(page)).is_some() {
                    let _ = mapper.unmap_page_from(l4, VirtAddr::new(page));
                }
                page += 0x1000;
            }
        });
        return old_address as i64;
    }

    let new_end = match old_address.checked_add(new_size) {
        Some(end) => end,
        None => return ENOMEM,
    };
    let grew_in_place = crate::userland::lifecycle::with_current_group(|process| {
        let Some(space) = process.address_space.as_mut() else {
            return false;
        };
        if !space.vmas().is_free(old_end, new_end) {
            return false;
        }
        space
            .vmas_mut()
            .replace_anonymous_allocation(old_address, old_end, old_address, new_end)
            .is_ok()
    });
    if grew_in_place {
        return old_address as i64;
    }
    if flags & MREMAP_MAYMOVE == 0 {
        return ENOMEM;
    }

    // Reserve a destination VMA before touching page tables. The syscall is
    // non-preemptible, but this ordering also gives rollback a precise piece
    // of metadata to remove if page-table allocation runs out of frames.
    let destination = crate::userland::lifecycle::with_current_group(|process| {
        let space = process.address_space.as_mut()?;
        let stack_floor = space
            .vmas()
            .as_slice()
            .iter()
            .find_map(|vma| {
                matches!(vma.backing, crate::userland::vm::VmaBacking::Stack { .. })
                    .then_some(vma.start)
            })
            .unwrap_or(crate::mm::paging::USER_STACK_TOP);
        let destination = space
            .vmas()
            .find_gap_top_down(new_size, stack_floor.saturating_sub(1024 * 1024))
            .ok()?;
        let moved = crate::userland::vm::Vma::new(
            destination,
            destination + new_size,
            allocation.prot,
            crate::userland::vm::VmaBacking::Anonymous,
        )
        .ok()?;
        space.vmas_mut().insert(moved).ok()?;
        Some((destination, space.l4_frame()))
    });
    let Some((destination, l4)) = destination else {
        return ENOMEM;
    };

    let mut moved_offsets = alloc::vec::Vec::new();
    let move_result = crate::mm::memory::with_memory_mapper(|mapper| {
        let mut offset = 0;
        while offset < old_size {
            match mapper.move_user_page(
                l4,
                VirtAddr::new(old_address + offset),
                VirtAddr::new(destination + offset),
            ) {
                Ok(true) => moved_offsets.push(offset),
                Ok(false) => {}
                Err(_) => {
                    for rollback in moved_offsets.iter().rev().copied() {
                        let _ = mapper.move_user_page(
                            l4,
                            VirtAddr::new(destination + rollback),
                            VirtAddr::new(old_address + rollback),
                        );
                    }
                    return false;
                }
            }
            offset += 0x1000;
        }
        true
    })
    .unwrap_or(false);
    if !move_result {
        crate::userland::lifecycle::with_current_group(|process| {
            if let Some(space) = process.address_space.as_mut() {
                let _ = space.vmas_mut().remove(destination, destination + new_size);
            }
        });
        return ENOMEM;
    }

    crate::userland::lifecycle::with_current_group(|process| {
        if let Some(space) = process.address_space.as_mut() {
            let _ = space.vmas_mut().remove(old_address, old_end);
        }
    });
    destination as i64
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
/// Other paths fall through to the mounted filesystem's symlink implementation.
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
    let raw_path = match copy_user_cstr(path_ptr) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let path = with_cwd(|cwd| normalize_path(cwd, &raw_path));
    if path.len() > READLINK_MAX_PATH {
        return ERANGE;
    }
    let bytes = match resolve_proc_link(&path) {
        Some(target) => target.into_bytes(),
        None if path.starts_with("/proc/self/fd/") => return ENOENT,
        None => match crate::fs::vfs::vfs_read_link(&path) {
            Ok(target) => target,
            Err(crate::fs::filesystem::FilesystemError::InvalidPath) => return ENOENT,
            Err(ref error) => return map_filesystem_err(error),
        },
    };
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
        FdSlot::VirtualDevDir { .. } => String::from("/dev"),
        FdSlot::Urandom { .. } => String::from("/dev/urandom"),
        FdSlot::DevNull { .. } => String::from("/dev/null"),
        FdSlot::Socket { handle, .. } => alloc::format!("socket:[{}]", handle.id()),
        FdSlot::GuiEvents { .. } => String::from("anon_inode:[agenticos-gui]"),
        FdSlot::EventFd { .. } => String::from("anon_inode:[eventfd]"),
        FdSlot::Epoll { .. } => String::from("anon_inode:[eventpoll]"),
        FdSlot::LocalStream { handle, .. } => alloc::format!("socket:[{}]", handle.id()),
        FdSlot::VirtualFile { path, .. } | FdSlot::VirtualDir { path, .. } => String::clone(&path),
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

/// Linux `struct sysinfo` layout (x86-64, musl-compatible): 112 bytes.
/// BusyBox `free` and `uptime` read `uptime`, `totalram`/`freeram`/
/// `sharedram`, `procs`, and `mem_unit`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxSysinfo {
    uptime: i64,
    loads: [u64; 3],
    totalram: u64,
    freeram: u64,
    sharedram: u64,
    bufferram: u64,
    totalswap: u64,
    freeswap: u64,
    procs: u16,
    pad: u16,
    pad2: u32,
    totalhigh: u64,
    freehigh: u64,
    mem_unit: u32,
    tail_pad: u32,
}
const _SYSINFO_SIZE: () = assert!(core::mem::size_of::<LinuxSysinfo>() == 112);

/// `sysinfo(*info) -> int`. Real uptime + physical-memory numbers from
/// the frame allocator; load averages and swap report zero (we track
/// neither). `mem_unit = 1` → all ram fields are bytes.
pub fn sysinfo_handler(args: &mut SyscallArgs) -> i64 {
    let out_ptr = args.rdi;
    let (uptime, totalram, freeram, sharedram, procs) = crate::userland::procfs::sysinfo_snapshot();
    let info = LinuxSysinfo {
        uptime,
        totalram,
        freeram,
        sharedram,
        procs,
        mem_unit: 1,
        ..Default::default()
    };
    crate::userland::usercopy::write_unaligned(out_ptr, &info).map_or_else(|e| e, |_| 0)
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
    let old_value = crate::userland::lifecycle::with_current_group(|process| {
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
    if let Some(pid) = crate::userland::lifecycle::current_user_pid() {
        crate::userland::lifecycle::sync_real_timer(pid);
    }

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
/// busy-spinning — self-driven ring-3 animation loops (`PAINTING.ELF`,
/// `TASKMGR.ELF`) and zsh's `sleep`/`usleep` builtins all depend on this.
///
/// Restart mechanics: blocking parks the process as
/// `Ring3BlockReason::Sleeping { deadline_tick }` with RIP rewound so the
/// SYSCALL re-fires on wake. The absolute deadline is restart-stable via
/// `Process.sleep_deadline` (see [`nanosleep_deadline`]), so a
/// woken-and-re-blocked sleeper cannot extend its own timeout;
/// `process_expired_sleeps` (PIT ISR + housekeeping backstops) readies the
/// process at the deadline.
///
/// Signals: `wake_ring3_for_signal` / ITIMER expiry unblock the sleeper,
/// clear `sleep_deadline`, and set `pending_syscall_interrupt` — the
/// re-fired SYSCALL enters the dispatcher as `-EINTR` and the signal
/// (handler or fatal default) is processed there. POSIX gap: `rem` is not
/// populated on EINTR (callers that loop on EINTR re-sleep the full
/// duration); acceptable until a consumer needs it.
///
/// Synthetic dispatch (tests, sentinel PID 0) cannot yield — a valid
/// request from that context returns 0 immediately.
pub fn nanosleep_handler(args: &mut SyscallArgs) -> i64 {
    /// PIT period: 100 Hz ⇒ 10 ms ⇒ 10,000,000 ns per tick.
    const NS_PER_TICK: u64 = 10_000_000;
    const TICKS_PER_SEC: u64 = 100;

    let req_ptr = args.rdi;
    let rem_ptr = args.rsi;
    if req_ptr == 0 {
        return EFAULT;
    }
    let req: LinuxTimespec = match crate::userland::usercopy::read_unaligned(req_ptr) {
        Ok(ts) => ts,
        Err(e) => return e,
    };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    // Round the sub-second remainder up so any positive request sleeps
    // at least one tick.
    let requested_ticks = (req.tv_sec as u64)
        .saturating_mul(TICKS_PER_SEC)
        .saturating_add((req.tv_nsec as u64).div_ceil(NS_PER_TICK));

    let write_zero_rem = || -> i64 {
        if rem_ptr == 0 {
            return 0;
        }
        let zero = LinuxTimespec::default();
        crate::userland::usercopy::write_unaligned(rem_ptr, &zero).map_or_else(|e| e, |_| 0)
    };

    // No scheduler context to yield from in synthetic dispatch.
    if !matches!(
        crate::userland::lifecycle::current_user_pid(),
        Some(pid) if pid != crate::userland::lifecycle::KERNEL_PID
    ) {
        return write_zero_rem();
    }

    match crate::userland::lifecycle::nanosleep_deadline(requested_ticks) {
        Some(deadline) => unsafe {
            crate::userland::switch::block_current_ring3_and_yield(
                args,
                crate::userland::lifecycle::Ring3BlockReason::Sleeping {
                    deadline_tick: deadline,
                },
            )
        },
        // Elapsed (or zero-length) — done.
        None => write_zero_rem(),
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
