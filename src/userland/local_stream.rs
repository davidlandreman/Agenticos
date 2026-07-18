//! In-kernel full-duplex AF_UNIX stream pairs built from two bounded pipes.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::arc::Arc;
use crate::userland::abi::{EAFNOSUPPORT, EAGAIN, EFAULT, EINVAL, EMFILE, EPIPE, EPROTONOSUPPORT};
use crate::userland::fdtable::FdSlot;
use crate::userland::pipe::{Pipe, PipeReadHandle, PipeWriteHandle};

const AF_UNIX: i32 = 1;
const SOCK_STREAM: i32 = 1;
const SOCK_NONBLOCK: i32 = 0x800;
const SOCK_CLOEXEC: i32 = 0x80000;
const SOCK_TYPE_MASK: i32 = 0xf;

static NEXT_LOCAL_STREAM_ID: AtomicU64 = AtomicU64::new(1);

pub struct LocalStreamEndpoint {
    id: u64,
    inbound: PipeReadHandle,
    outbound: PipeWriteHandle,
    nonblocking: AtomicBool,
}

impl LocalStreamEndpoint {
    fn pair(nonblocking: bool) -> (Arc<Self>, Arc<Self>) {
        let zero_to_one = Pipe::new();
        let one_to_zero = Pipe::new();
        let zero = Arc::new(Self {
            id: NEXT_LOCAL_STREAM_ID.fetch_add(1, Ordering::Relaxed),
            inbound: PipeReadHandle::new(one_to_zero.clone(), false),
            outbound: PipeWriteHandle::new(zero_to_one.clone(), false),
            nonblocking: AtomicBool::new(nonblocking),
        });
        let one = Arc::new(Self {
            id: NEXT_LOCAL_STREAM_ID.fetch_add(1, Ordering::Relaxed),
            inbound: PipeReadHandle::new(zero_to_one, false),
            outbound: PipeWriteHandle::new(one_to_zero, false),
            nonblocking: AtomicBool::new(nonblocking),
        });
        (zero, one)
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, value: bool) {
        self.nonblocking.store(value, Ordering::Release);
    }

    pub fn readiness(&self) -> (bool, bool, bool, bool) {
        let peer_write_closed = self.inbound.pipe().writers() == 0;
        let peer_read_closed = self.outbound.pipe().readers() == 0;
        (
            self.inbound.pipe().len() != 0 || peer_write_closed,
            !peer_read_closed && self.outbound.pipe().has_capacity(),
            peer_read_closed,
            peer_write_closed,
        )
    }

    pub fn read(args: &SyscallArgs, handle: &Arc<Self>, pointer: u64, len: u64) -> i64 {
        let count = core::cmp::min(len, 4096);
        if let Err(error) = crate::userland::usercopy::ensure_user_range(pointer, count, true) {
            return error;
        }
        let observed = crate::userland::readiness::sequence();
        let mut bytes = alloc::vec![0u8; count as usize];
        let count = handle.inbound.pipe().read(&mut bytes);
        if count != 0 {
            return crate::userland::usercopy::copy_to_user(pointer, &bytes[..count])
                .map_or_else(|error| error, |_| count as i64);
        }
        if handle.inbound.pipe().writers() == 0 {
            return 0;
        }
        if handle.nonblocking() {
            return EAGAIN;
        }
        crate::userland::readiness::block(args, handle.id, None, observed)
    }

    pub fn write(args: &SyscallArgs, handle: &Arc<Self>, pointer: u64, len: u64) -> i64 {
        if handle.outbound.pipe().readers() == 0 {
            return EPIPE;
        }
        let count = core::cmp::min(len, 4096) as usize;
        let mut bytes = alloc::vec![0u8; count];
        if let Err(error) = crate::userland::usercopy::copy_from_user(&mut bytes, pointer) {
            return error;
        }
        let observed = crate::userland::readiness::sequence();
        let written = handle.outbound.pipe().write(&bytes);
        if written != 0 {
            return written as i64;
        }
        if handle.outbound.pipe().readers() == 0 {
            return EPIPE;
        }
        if handle.nonblocking() {
            return EAGAIN;
        }
        crate::userland::readiness::block(args, handle.id, None, observed)
    }
}

impl Drop for LocalStreamEndpoint {
    fn drop(&mut self) {
        crate::userland::readiness::notify_changed();
    }
}

pub fn socketpair_handler(args: &mut SyscallArgs) -> i64 {
    let domain = args.rdi as i32;
    let socket_type = args.rsi as i32;
    let protocol = args.rdx as i32;
    let output = args.r10;
    if domain != AF_UNIX {
        return EAFNOSUPPORT;
    }
    if socket_type & SOCK_TYPE_MASK != SOCK_STREAM {
        return EPROTONOSUPPORT;
    }
    if socket_type & !(SOCK_TYPE_MASK | SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 || protocol != 0 {
        return EINVAL;
    }
    if crate::userland::usercopy::ensure_user_range(output, 8, true).is_err() {
        return EFAULT;
    }

    let nonblocking = socket_type & SOCK_NONBLOCK != 0;
    let cloexec = socket_type & SOCK_CLOEXEC != 0;
    let (zero, one) = LocalStreamEndpoint::pair(nonblocking);
    let allocated = crate::userland::lifecycle::with_current_group(|process| {
        let first = process.fd_table.alloc(FdSlot::LocalStream {
            handle: zero,
            cloexec,
        })?;
        let second = match process.fd_table.alloc(FdSlot::LocalStream {
            handle: one,
            cloexec,
        }) {
            Some(fd) => fd,
            None => {
                let _ = process.fd_table.close(first);
                return None;
            }
        };
        Some((first, second))
    });
    let Some((first, second)) = allocated else {
        return EMFILE;
    };
    let pair = [first, second];
    if crate::userland::usercopy::copy_to_user(output, unsafe {
        core::slice::from_raw_parts(pair.as_ptr().cast::<u8>(), 8)
    })
    .is_err()
    {
        crate::userland::lifecycle::with_current_group(|process| {
            let _ = process.fd_table.close(first);
            let _ = process.fd_table.close(second);
        });
        return EFAULT;
    }
    crate::userland::readiness::notify_changed();
    0
}
