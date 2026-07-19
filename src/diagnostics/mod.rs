//! Panic-safe diagnostics substrate.
//!
//! The minimal personality is always present. Rich recorder capacity and
//! shadow-policy enforcement are selected by Cargo features; none of the
//! crash path depends on the heap or a production lock.

pub mod crash;
mod identity;
pub mod registers;
pub mod shadow;
pub mod trace;
pub mod wire;

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Personality {
    Minimal = 0,
    Record = 1,
    Strict = 2,
}

static EARLY_READY: AtomicBool = AtomicBool::new(false);
static PERCPU_READY: AtomicBool = AtomicBool::new(false);
static PERSONALITY: AtomicU8 = AtomicU8::new(Personality::Minimal as u8);

pub fn early_init() {
    identity::init();
    #[cfg(feature = "test")]
    crash::init_test_policy();
    let configured = match env!("AGENTICOS_BUILD_DIAGNOSTICS") {
        "strict" if cfg!(feature = "diagnostics-strict") => Personality::Strict,
        "record" if cfg!(feature = "diagnostics") => Personality::Record,
        _ => Personality::Minimal,
    };
    PERSONALITY.store(configured as u8, Ordering::Release);
    EARLY_READY.store(true, Ordering::Release);
    trace::record_early(trace::EventKind::DiagnosticsEnabled, configured as u64, 0);
}

pub fn percpu_init() {
    PERCPU_READY.store(true, Ordering::Release);
    trace::record(trace::EventKind::CpuOnline, 0, 0, 0, 0);
}

/// Rich shadow tables are introduced domain-by-domain. The first-violation
/// latch is static and active in every personality.
pub fn shadow_init() {
    trace::record(trace::EventKind::BootPhase, 0, 3, personality() as u64, 0);
}

#[cfg(feature = "test")]
pub fn maybe_inject_crash() {
    let mut value = [0u8; 32];
    let Some(length) = crate::drivers::fw_cfg::read_file("opt/agenticos/crash_inject", &mut value)
    else {
        return;
    };
    let end = value[..length]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(length);
    match &value[..end] {
        b"panic" => panic!("diagnostic crash injection"),
        b"sched-duplicate" => {
            use crate::diagnostics::shadow::scheduler::{OperationKind, Transition};
            use crate::process::entity::EntityId;

            let id = EntityId::UserProcess(0x7fff_ff00);
            shadow::scheduler::register(id);
            shadow::scheduler::make_ready(id, true);
            shadow::scheduler::dispatch(id);
            shadow::scheduler::apply(
                id,
                Transition {
                    operation: OperationKind::ForceRunning,
                    cpu: 1,
                    published: true,
                    allow_running_exit: false,
                },
            );
            panic!("strict scheduler corruption injection did not escalate");
        }
        _ => {}
    }
}

pub fn personality() -> Personality {
    match PERSONALITY.load(Ordering::Acquire) {
        2 => Personality::Strict,
        1 => Personality::Record,
        _ => Personality::Minimal,
    }
}

pub(crate) fn percpu_ready() -> bool {
    PERCPU_READY.load(Ordering::Acquire)
}
