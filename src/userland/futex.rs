//! Minimal Linux futex support used by musl pthreads.

use alloc::collections::{BTreeMap, VecDeque};

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::arch::x86_64::syscall::SyscallArgs;
use crate::userland::abi::{EAGAIN, EINTR, EINVAL, ENOSYS, ETIMEDOUT};

const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
const FUTEX_REQUEUE: u32 = 3;
const FUTEX_PRIVATE_FLAG: u32 = 128;
const NS_PER_TICK: u64 = 10_000_000;
const TICKS_PER_SEC: u64 = 100;
const MAX_WAITERS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FutexKey {
    tgid: u32,
    address: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completion {
    Waiting,
    Woken,
    TimedOut,
    Interrupted,
}

struct Registry {
    queues: BTreeMap<FutexKey, VecDeque<u32>>,
    completion: BTreeMap<u32, Completion>,
}

impl Registry {
    const fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
            completion: BTreeMap::new(),
        }
    }

    fn remove_waiter(&mut self, tid: u32) {
        let mut empty = alloc::vec::Vec::new();
        for (key, queue) in &mut self.queues {
            queue.retain(|candidate| *candidate != tid);
            if queue.is_empty() {
                empty.push(*key);
            }
        }
        for key in empty {
            self.queues.remove(&key);
        }
    }
}

static FUTEXES: InterruptMutex<Registry> = InterruptMutex::new(Registry::new());

#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn key(address: u64) -> Result<FutexKey, i64> {
    if address & 3 != 0 {
        return Err(EINVAL);
    }
    // Probe the user mapping up front. Both private and process-shared
    // operations are keyed by TGID in this first implementation.
    crate::userland::usercopy::read_unaligned::<u32>(address)?;
    Ok(FutexKey {
        tgid: crate::userland::lifecycle::current_tgid(),
        address,
    })
}

fn cancel_timeout(tid: u32) {
    let _ = crate::process::timer::cancel(crate::process::timer::TimerKey {
        entity: crate::process::entity::EntityId::UserProcess(tid),
        kind: crate::process::timer::TimerKind::UserFutex,
    });
}

fn wake_locked(registry: &mut Registry, key: FutexKey, count: usize) -> alloc::vec::Vec<u32> {
    let mut waking = alloc::vec::Vec::new();
    let mut empty = false;
    if let Some(queue) = registry.queues.get_mut(&key) {
        for _ in 0..count {
            let Some(tid) = queue.pop_front() else { break };
            registry.completion.insert(tid, Completion::Woken);
            waking.push(tid);
        }
        empty = queue.is_empty();
    }
    if empty {
        registry.queues.remove(&key);
    }
    waking
}

fn publish_wakes(waking: alloc::vec::Vec<u32>) {
    for tid in waking {
        cancel_timeout(tid);
        crate::userland::lifecycle::mark_ring3_ready(tid);
    }
}

pub fn wake_address(tgid: u32, address: u64, count: usize) -> usize {
    if address == 0 || address & 3 != 0 {
        return 0;
    }
    let waking = wake_locked(&mut FUTEXES.lock(), FutexKey { tgid, address }, count);
    let count = waking.len();
    publish_wakes(waking);
    count
}

pub fn interrupt_wait(tid: u32) {
    let interrupted = {
        let mut registry = FUTEXES.lock();
        if registry.completion.get(&tid) != Some(&Completion::Waiting) {
            false
        } else {
            registry.remove_waiter(tid);
            registry.completion.insert(tid, Completion::Interrupted);
            true
        }
    };
    if interrupted {
        cancel_timeout(tid);
    }
}

pub fn discard_task(tid: u32) {
    {
        let mut registry = FUTEXES.lock();
        registry.remove_waiter(tid);
        registry.completion.remove(&tid);
    }
    cancel_timeout(tid);
}

pub fn expire_wait(tid: u32) {
    let expired = {
        let mut registry = FUTEXES.lock();
        if registry.completion.get(&tid) != Some(&Completion::Waiting) {
            false
        } else {
            registry.remove_waiter(tid);
            registry.completion.insert(tid, Completion::TimedOut);
            true
        }
    };
    if expired {
        crate::userland::lifecycle::mark_ring3_ready(tid);
    }
}

pub fn handler(args: &mut SyscallArgs) -> i64 {
    let op = args.rsi as u32;
    if op & !(0x7f | FUTEX_PRIVATE_FLAG) != 0 {
        return ENOSYS;
    }
    let command = op & 0x7f;
    let first = match key(args.rdi) {
        Ok(key) => key,
        Err(error) => return error,
    };

    match command {
        FUTEX_WAIT => {
            let Some(tid) = crate::userland::lifecycle::current_user_pid() else {
                return EAGAIN;
            };
            let prior = FUTEXES.lock().completion.remove(&tid);
            match prior {
                Some(Completion::Woken) => return 0,
                Some(Completion::TimedOut) => return ETIMEDOUT,
                Some(Completion::Interrupted) => return EINTR,
                Some(Completion::Waiting) | None => {}
            }
            let observed = match crate::userland::usercopy::read_unaligned::<u32>(args.rdi) {
                Ok(value) => value,
                Err(error) => return error,
            };
            if observed != args.rdx as u32 {
                return EAGAIN;
            }
            let deadline_tick = if args.r10 == 0 {
                None
            } else {
                let timeout = match crate::userland::usercopy::read_unaligned::<Timespec>(args.r10)
                {
                    Ok(value) => value,
                    Err(error) => return error,
                };
                if timeout.tv_sec < 0 || timeout.tv_nsec < 0 || timeout.tv_nsec >= 1_000_000_000 {
                    return EINVAL;
                }
                let ticks = (timeout.tv_sec as u64)
                    .saturating_mul(TICKS_PER_SEC)
                    .saturating_add((timeout.tv_nsec as u64).div_ceil(NS_PER_TICK));
                if ticks == 0 {
                    return ETIMEDOUT;
                }
                Some(crate::arch::x86_64::interrupts::get_timer_ticks().saturating_add(ticks))
            };
            {
                let mut registry = FUTEXES.lock();
                if registry.completion.len() >= MAX_WAITERS
                    || registry.queues.get(&first).map_or(0, VecDeque::len) >= 64
                {
                    return EAGAIN;
                }
                registry.queues.entry(first).or_default().push_back(tid);
                registry.completion.insert(tid, Completion::Waiting);
            }
            unsafe {
                crate::userland::switch::block_current_ring3_and_yield(
                    args,
                    crate::userland::lifecycle::Ring3BlockReason::WaitingForFutex {
                        tgid: first.tgid,
                        address: first.address,
                        deadline_tick,
                    },
                )
            }
        }
        FUTEX_WAKE => {
            let waking = wake_locked(&mut FUTEXES.lock(), first, args.rdx as usize);
            let count = waking.len();
            publish_wakes(waking);
            count as i64
        }
        FUTEX_REQUEUE => {
            let second = match key(args.r8) {
                Ok(key) if key.tgid == first.tgid => key,
                Ok(_) => return EINVAL,
                Err(error) => return error,
            };
            let (waking, moved) = {
                let mut registry = FUTEXES.lock();
                let waking = wake_locked(&mut registry, first, args.rdx as usize);
                let mut moved_tids = alloc::vec::Vec::new();
                let mut empty = false;
                if let Some(queue) = registry.queues.get_mut(&first) {
                    for _ in 0..args.r10 as usize {
                        let Some(tid) = queue.pop_front() else { break };
                        moved_tids.push(tid);
                    }
                    empty = queue.is_empty();
                }
                if empty {
                    registry.queues.remove(&first);
                }
                let moved = moved_tids.len();
                registry
                    .queues
                    .entry(second)
                    .or_default()
                    .extend(moved_tids);
                (waking, moved)
            };
            let count = waking.len() + moved;
            publish_wakes(waking);
            count as i64
        }
        _ => ENOSYS,
    }
}
