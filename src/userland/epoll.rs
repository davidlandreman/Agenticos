//! Bounded Linux epoll implementation for the ring-3 FD table.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::x86_64::syscall::SyscallArgs;
use crate::lib::arc::Arc;
use crate::userland::abi::{EBADF, EEXIST, EFAULT, EINVAL, EMFILE, ENOENT, ENOSYS};
use crate::userland::fdtable::{FdSlot, FD_TABLE_SIZE};

const EPOLL_CLOEXEC: u32 = 0x80000;
const EPOLLIN: u32 = 0x001;
const EPOLLPRI: u32 = 0x002;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;
const EPOLLRDHUP: u32 = 0x2000;
const EPOLLET: u32 = 1 << 31;
const SUPPORTED_EVENTS: u32 =
    EPOLLIN | EPOLLPRI | EPOLLOUT | EPOLLERR | EPOLLHUP | EPOLLRDHUP | EPOLLET;

const EPOLL_CTL_ADD: i32 = 1;
const EPOLL_CTL_DEL: i32 = 2;
const EPOLL_CTL_MOD: i32 = 3;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct EpollEvent {
    events: u32,
    data: u64,
}

const _: () = assert!(core::mem::size_of::<EpollEvent>() == 12);

#[derive(Clone)]
struct Registration {
    slot: FdSlot,
    events: u32,
    data: u64,
    last_ready: u32,
    last_generation: u64,
    revision: u64,
}

pub struct EpollInstance {
    registrations: Mutex<BTreeMap<i32, Registration>>,
    next_revision: core::sync::atomic::AtomicU64,
}

impl EpollInstance {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            registrations: Mutex::new(BTreeMap::new()),
            next_revision: core::sync::atomic::AtomicU64::new(1),
        })
    }

    fn revision(&self) -> u64 {
        self.next_revision
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed)
    }

    pub fn is_ready(&self) -> bool {
        self.registrations.lock().values().any(|registration| {
            crate::userland::syscalls::fd_slot_readiness(&registration.slot)
                .map(|state| ready_mask(state, registration.events) != 0)
                .unwrap_or(true)
        })
    }

    pub fn prune_open_description(&self, description: &FdSlot) {
        let mut registrations = self.registrations.lock();
        let before = registrations.len();
        registrations
            .retain(|_, registration| !registration.slot.same_open_description(description));
        let changed = registrations.len() != before;
        drop(registrations);
        if changed {
            crate::userland::readiness::notify_changed();
        }
    }
}

fn create(flags: u32) -> i64 {
    if flags & !EPOLL_CLOEXEC != 0 {
        return EINVAL;
    }
    let slot = FdSlot::Epoll {
        handle: EpollInstance::new(),
        cloexec: flags & EPOLL_CLOEXEC != 0,
    };
    crate::userland::lifecycle::with_current_group(|process| process.fd_table.alloc(slot))
        .map_or(EMFILE, i64::from)
}

pub fn epoll_create_handler(args: &mut SyscallArgs) -> i64 {
    if args.rdi as i32 <= 0 {
        EINVAL
    } else {
        create(0)
    }
}

pub fn epoll_create1_handler(args: &mut SyscallArgs) -> i64 {
    create(args.rdi as u32)
}

fn instance(fd: i32) -> Result<Arc<EpollInstance>, i64> {
    crate::userland::lifecycle::with_current_group(|process| match process.fd_table.get(fd) {
        Some(FdSlot::Epoll { handle, .. }) => Ok(handle.clone()),
        Some(_) => Err(EINVAL),
        None => Err(EBADF),
    })
}

pub fn epoll_ctl_handler(args: &mut SyscallArgs) -> i64 {
    let epfd = args.rdi as i32;
    let op = args.rsi as i32;
    let target_fd = args.rdx as i32;
    if epfd == target_fd {
        return EINVAL;
    }
    let epoll = match instance(epfd) {
        Ok(instance) => instance,
        Err(error) => return error,
    };
    let target = crate::userland::syscalls::fd_slot(target_fd);
    let target = match target {
        Some(FdSlot::Epoll { .. }) => return EINVAL,
        Some(slot) => slot,
        None => return EBADF,
    };

    let event = if op == EPOLL_CTL_DEL {
        EpollEvent::default()
    } else {
        match crate::userland::usercopy::read_unaligned::<EpollEvent>(args.r10) {
            Ok(event) => event,
            Err(_) => return EFAULT,
        }
    };
    if event.events & !SUPPORTED_EVENTS != 0 {
        return EINVAL;
    }

    let mut registrations = epoll.registrations.lock();
    let result = match op {
        EPOLL_CTL_ADD => {
            if registrations.contains_key(&target_fd) {
                EEXIST
            } else if registrations.len() >= FD_TABLE_SIZE {
                crate::userland::abi::ENOSPC
            } else {
                let generation = match &target {
                    FdSlot::EventFd { handle, .. } => handle.generation(),
                    _ => 0,
                };
                registrations.insert(
                    target_fd,
                    Registration {
                        slot: target,
                        events: event.events,
                        data: event.data,
                        last_ready: 0,
                        // A currently-ready eventfd must be reported once
                        // after ADD, so start one generation behind.
                        last_generation: generation.saturating_sub(1),
                        revision: epoll.revision(),
                    },
                );
                0
            }
        }
        EPOLL_CTL_MOD => match registrations.get_mut(&target_fd) {
            Some(registration) => {
                registration.events = event.events;
                registration.data = event.data;
                registration.last_ready = 0;
                registration.revision = epoll.revision();
                0
            }
            None => ENOENT,
        },
        EPOLL_CTL_DEL => {
            if registrations.remove(&target_fd).is_some() {
                0
            } else {
                ENOENT
            }
        }
        _ => EINVAL,
    };
    drop(registrations);
    if result == 0 {
        crate::userland::readiness::notify_changed();
    }
    result
}

fn ready_mask(state: crate::userland::syscalls::FdReady, wanted: u32) -> u32 {
    let mut ready = 0;
    if state.readable {
        ready |= wanted & EPOLLIN;
    }
    if state.writable {
        ready |= wanted & EPOLLOUT;
    }
    if state.error {
        ready |= EPOLLERR;
    }
    if state.hangup {
        ready |= EPOLLHUP | (wanted & EPOLLRDHUP);
    }
    ready
}

fn wait_common(args: &SyscallArgs, reject_nonnull_mask: bool) -> i64 {
    let epfd = args.rdi as i32;
    let events_pointer = args.rsi;
    let maxevents = args.rdx as i32;
    let timeout_ms = args.r10 as i32;
    if maxevents <= 0 || maxevents as usize > FD_TABLE_SIZE || timeout_ms < -1 {
        return EINVAL;
    }
    if reject_nonnull_mask && args.r8 != 0 {
        return ENOSYS;
    }
    let output_len = maxevents as u64 * core::mem::size_of::<EpollEvent>() as u64;
    if crate::userland::usercopy::ensure_user_range(events_pointer, output_len, true).is_err() {
        return EFAULT;
    }
    let epoll = match instance(epfd) {
        Ok(instance) => instance,
        Err(error) => return error,
    };
    crate::net::poll_once();
    let observed = crate::userland::readiness::sequence();
    let snapshot: Vec<(i32, Registration)> = epoll
        .registrations
        .lock()
        .iter()
        .map(|(fd, registration)| (*fd, registration.clone()))
        .collect();
    let mut output = Vec::new();
    for (fd, registration) in snapshot {
        if output.len() == maxevents as usize {
            break;
        }
        let state = match crate::userland::syscalls::fd_slot_readiness(&registration.slot) {
            Ok(state) => state,
            Err(_) => crate::userland::syscalls::FdReady {
                error: true,
                ..crate::userland::syscalls::FdReady::default()
            },
        };
        let current = ready_mask(state, registration.events);
        let edge = registration.events & EPOLLET != 0;
        let generation = match &registration.slot {
            FdSlot::EventFd { handle, .. } => Some(handle.generation()),
            _ => None,
        };
        let deliver = if !edge {
            current
        } else if let Some(generation) = generation {
            if current != 0 && generation != registration.last_generation {
                current
            } else {
                0
            }
        } else {
            current & !registration.last_ready
        };

        if edge {
            let mut registrations = epoll.registrations.lock();
            if let Some(live) = registrations.get_mut(&fd) {
                if live.revision == registration.revision {
                    live.last_ready = current;
                    if let Some(generation) = generation {
                        live.last_generation = generation;
                    }
                }
            }
        }
        if deliver != 0 {
            output.push(EpollEvent {
                events: deliver,
                data: registration.data,
            });
        }
    }

    for (index, event) in output.iter().enumerate() {
        let address = events_pointer + index as u64 * core::mem::size_of::<EpollEvent>() as u64;
        if crate::userland::usercopy::write_unaligned(address, event).is_err() {
            return EFAULT;
        }
    }
    if !output.is_empty() || timeout_ms == 0 {
        crate::userland::lifecycle::clear_network_wait();
        return output.len() as i64;
    }
    let timeout_ticks = if timeout_ms < 0 {
        None
    } else {
        Some((timeout_ms as u64).div_ceil(10))
    };
    let identity = (Arc::as_ptr(&epoll) as usize as u64)
        ^ events_pointer.rotate_left(13)
        ^ (maxevents as u64).rotate_left(31);
    crate::userland::readiness::block(args, identity, timeout_ticks, observed)
}

pub fn epoll_wait_handler(args: &mut SyscallArgs) -> i64 {
    wait_common(args, false)
}

pub fn epoll_pwait_handler(args: &mut SyscallArgs) -> i64 {
    wait_common(args, true)
}
