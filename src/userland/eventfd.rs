//! Linux eventfd/eventfd2 open-file description.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::arc::Arc;
use crate::userland::abi::{EAGAIN, EFAULT, EINVAL, EMFILE};
use crate::userland::fdtable::FdSlot;

pub const EFD_SEMAPHORE: u32 = 0x1;
pub const EFD_NONBLOCK: u32 = 0x800;
pub const EFD_CLOEXEC: u32 = 0x80000;
const EVENTFD_MAX: u64 = u64::MAX - 1;

pub struct EventFd {
    counter: Mutex<u64>,
    semaphore: bool,
    nonblocking: AtomicBool,
    generation: AtomicU64,
}

impl EventFd {
    fn new(initial: u32, flags: u32) -> Arc<Self> {
        Arc::new(Self {
            counter: Mutex::new(initial as u64),
            semaphore: flags & EFD_SEMAPHORE != 0,
            nonblocking: AtomicBool::new(flags & EFD_NONBLOCK != 0),
            generation: AtomicU64::new(u64::from(initial != 0)),
        })
    }

    pub fn nonblocking(&self) -> bool {
        self.nonblocking.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, value: bool) {
        self.nonblocking.store(value, Ordering::Release);
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn readiness(&self) -> (bool, bool) {
        let value = *self.counter.lock();
        (value != 0, value < EVENTFD_MAX)
    }

    fn try_read(&self) -> Option<u64> {
        let mut counter = self.counter.lock();
        if *counter == 0 {
            return None;
        }
        let value = if self.semaphore { 1 } else { *counter };
        if self.semaphore {
            *counter -= 1;
        } else {
            *counter = 0;
        }
        drop(counter);
        crate::userland::readiness::notify_changed();
        Some(value)
    }

    fn try_write(&self, value: u64) -> Result<(), ()> {
        let mut counter = self.counter.lock();
        if value > EVENTFD_MAX || *counter > EVENTFD_MAX - value {
            return Err(());
        }
        *counter += value;
        drop(counter);
        if value != 0 {
            self.generation.fetch_add(1, Ordering::AcqRel);
            crate::userland::readiness::notify_changed();
        }
        Ok(())
    }
}

fn create(initial: u32, flags: u32) -> i64 {
    if flags & !(EFD_SEMAPHORE | EFD_NONBLOCK | EFD_CLOEXEC) != 0 {
        return EINVAL;
    }
    let slot = FdSlot::EventFd {
        handle: EventFd::new(initial, flags),
        cloexec: flags & EFD_CLOEXEC != 0,
    };
    crate::userland::lifecycle::with_current_group(|process| process.fd_table.alloc(slot))
        .map_or(EMFILE, i64::from)
}

pub fn eventfd_handler(args: &mut SyscallArgs) -> i64 {
    create(args.rdi as u32, 0)
}

pub fn eventfd2_handler(args: &mut SyscallArgs) -> i64 {
    create(args.rdi as u32, args.rsi as u32)
}

pub fn read(args: &SyscallArgs, handle: &Arc<EventFd>, pointer: u64, len: u64) -> i64 {
    if len != 8 {
        return EINVAL;
    }
    if let Err(error) = crate::userland::usercopy::ensure_user_range(pointer, 8, true) {
        return error;
    }
    let observed = crate::userland::readiness::sequence();
    if let Some(value) = handle.try_read() {
        return crate::userland::usercopy::write_unaligned(pointer, &value)
            .map_or_else(|_| EFAULT, |_| 8);
    }
    if handle.nonblocking() {
        return EAGAIN;
    }
    let identity = Arc::as_ptr(handle) as usize as u64;
    crate::userland::readiness::block(args, identity, None, observed)
}

pub fn write(args: &SyscallArgs, handle: &Arc<EventFd>, pointer: u64, len: u64) -> i64 {
    if len != 8 {
        return EINVAL;
    }
    let value = match crate::userland::usercopy::read_unaligned::<u64>(pointer) {
        Ok(value) => value,
        Err(error) => return error,
    };
    if value == u64::MAX {
        return EINVAL;
    }
    let observed = crate::userland::readiness::sequence();
    match handle.try_write(value) {
        Ok(()) => 8,
        Err(()) if handle.nonblocking() => EAGAIN,
        Err(()) => {
            let identity = Arc::as_ptr(handle) as usize as u64;
            crate::userland::readiness::block(args, identity, None, observed)
        }
    }
}
