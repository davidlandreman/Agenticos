//! IPv4 networking subsystem.
//!
//! smoltcp is deliberately contained behind this module. Hardware-facing
//! VirtIO details live in `drivers`, while public kernel callers use the
//! AgenticOS socket and configuration abstractions defined here.

pub mod abi;
mod config;
mod resolver_config;
pub mod socket;
mod stack;

pub use config::NetworkConfig;
#[cfg(feature = "test")]
pub use resolver_config::resolver_tests;

use alloc::string::String;
use lazy_static::lazy_static;
use spin::Mutex;

use stack::NetworkStack;

lazy_static! {
    static ref NETWORK: Mutex<Option<NetworkStack>> = Mutex::new(None);
}

pub fn init() {
    let Some(stack) = NetworkStack::new() else {
        crate::debug_info!("Network unavailable (no supported VirtIO NIC)");
        return;
    };
    {
        let _interrupt_guard = crate::arch::x86_64::interrupt_guard::InterruptGuard::disable();
        *NETWORK.lock() = Some(stack);
    }
    crate::process::spawn_process(String::from("net-rx-tx"), None, network_worker);
}

fn network_worker() {
    loop {
        let _changed = poll_once();
        let sleep_ticks = with_stack_mut(NetworkStack::next_poll_ticks).unwrap_or(10);
        crate::process::sleep_ticks_with_contract(
            sleep_ticks,
            Some(crate::process::entity::LatencyContract::new(2)),
        );
    }
}

pub fn poll_once() -> bool {
    socket::drain_deferred_closes();
    let outcome = with_stack_mut(NetworkStack::poll_once).unwrap_or_default();
    if outcome.config_changed {
        resolver_config::publish(outcome.config);
    }
    crate::userland::lifecycle::wake_ring3_blocked_on_network(outcome.changed);
    outcome.changed
}

pub fn drain_deferred_closes() {
    socket::drain_deferred_closes();
}

#[cfg(feature = "test")]
pub fn is_available() -> bool {
    with_stack_mut(|_| ()).is_some()
}

#[cfg(feature = "test")]
pub fn config() -> NetworkConfig {
    with_stack_mut(|stack| stack.config()).unwrap_or_default()
}

pub fn counters() -> Option<crate::drivers::virtio::net::NetDriverCounters> {
    with_stack_mut(|stack| stack.counters())
}

/// Owned snapshot of the socket registry for `/proc/agenticos/sockets`.
/// Bounded: builds the whole vector inside one `NETWORK` critical
/// section and returns it by value. Empty when the stack is absent.
pub fn socket_snapshot() -> alloc::vec::Vec<socket::SocketSnapshot> {
    with_stack_mut(|stack| stack.socket_snapshot()).unwrap_or_default()
}

#[cfg(feature = "test")]
pub fn wait_for_config_ticks(timeout_ticks: u64) -> Option<NetworkConfig> {
    let start = crate::arch::x86_64::interrupts::get_timer_ticks();
    loop {
        poll_once();
        let snapshot = config();
        if snapshot.configured {
            return Some(snapshot);
        }
        let now = crate::arch::x86_64::interrupts::get_timer_ticks();
        if now.wrapping_sub(start) >= timeout_ticks {
            return None;
        }
        x86_64::instructions::hlt();
    }
}

fn with_stack_mut<R>(f: impl FnOnce(&mut NetworkStack) -> R) -> Option<R> {
    // Kernel threads are timer-preemptible. A ring-3 SYSCALL enters with IF
    // cleared, so spinning here after preempting a lock holder would freeze
    // the entire single-core VM. Keep IF masked for every NETWORK critical
    // section and restore the caller's prior interrupt state on exit.
    let _interrupt_guard = crate::arch::x86_64::interrupt_guard::InterruptGuard::disable();
    NETWORK.lock().as_mut().map(f)
}
