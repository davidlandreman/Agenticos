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
    let (l4, _) = x86_64::registers::control::Cr3::read();
    shadow::cpu::initialize_kernel(
        l4.start_address().as_u64(),
        crate::arch::x86_64::percpu::kernel_rsp_top(),
    );
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
        b"fatal-page-fault" => {
            const UNMAPPED: u64 = 0xffff_f000_0000_0000;
            unsafe {
                core::arch::asm!(
                    "mov rax, qword ptr [rdi]",
                    in("rdi") UNMAPPED,
                    out("rax") _,
                    options(nostack, readonly),
                );
            }
            panic!("fatal page-fault injection unexpectedly returned");
        }
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
        b"cont-signal-wake" => {
            let token = 0xfeed_0001;
            let pid = 0x7fff_ff01;
            shadow::io::submitted(token, 0, pid, 1, 7, 4096);
            shadow::io::completed(token, 0, 4096);
            shadow::io::queue_wake(token, pid);
            shadow::io::reject_generic_io_wake(pid, token);
            panic!("strict continuation wake injection did not escalate");
        }
        b"cont-invalid-stack" => {
            let token = 0xfeed_0002;
            let pid = 0x7fff_ff02;
            let mut context = crate::process::CpuContext::default();
            context.rip = 0xffff_8000_0000_1000;
            context.rsp = 0x1800;
            shadow::continuation::allocate(pid, token, 0, 0x1000, 0x2000);
            shadow::continuation::published(pid, &context);
            shadow::continuation::wake(pid, token);
            context.rsp = 0;
            shadow::continuation::dispatch(pid, &context);
            panic!("strict invalid continuation stack injection did not escalate");
        }
        b"as-destroy-active" => {
            let generation = shadow::address_space::allocate(0x1234_5000);
            shadow::address_space::publish_owner(generation, 0x7fff_ff03, 1);
            shadow::address_space::activate(generation, 0x1234_5000);
            shadow::address_space::begin_destroy(generation);
            panic!("strict active address-space destroy injection did not escalate");
        }
        b"stack-retire-active" => {
            let pid = 0x7fff_ff04;
            let generation = shadow::stack::allocate(0x3000, 0x5000);
            shadow::stack::publish_owner(generation, pid);
            shadow::stack::activate(generation, pid, 0x5000);
            shadow::stack::begin_retire(generation);
            panic!("strict active stack retirement injection did not escalate");
        }
        b"mm-double-release" => {
            shadow::memory::inject_double_release(0);
            panic!("strict frame double-release injection did not escalate");
        }
        b"mm-wrong-unmap" => {
            shadow::memory::unmap_leaf(0x7fff_0001, 0x4000, 0x9000, 1);
            panic!("strict wrong-unmap injection did not escalate");
        }
        b"mm-wx" => {
            shadow::memory::report_topology(shadow::memory::MM_004, 0x4000, 0x7fff_0002, 0x3);
            panic!("strict W+X injection did not escalate");
        }
        b"lock-recursion" => {
            shadow::locks::inject_recursion();
            panic!("strict lock recursion injection did not escalate");
        }
        b"lock-wrong-owner" => {
            shadow::locks::inject_wrong_owner();
            panic!("strict lock wrong-owner injection did not escalate");
        }
        b"lock-wrong-context" => {
            shadow::locks::inject_wrong_context();
            panic!("strict lock context injection did not escalate");
        }
        b"lock-cycle" => {
            shadow::locks::inject_cycle();
            panic!("strict lock cycle injection did not escalate");
        }
        b"cpu-wrong-cr3" => {
            shadow::cpu::inject_wrong_cr3();
            panic!("strict CPU CR3 mismatch injection did not escalate");
        }
        b"cpu-wrong-order" => {
            shadow::cpu::inject_wrong_order();
            panic!("strict CPU handoff ordering injection did not escalate");
        }
        b"cpu-wrong-pid" => {
            shadow::cpu::inject_wrong_pid();
            panic!("strict CPU PID mismatch injection did not escalate");
        }
        b"cpu-kernel-cr3" => {
            shadow::cpu::inject_wrong_kernel_cr3();
            panic!("strict CPU kernel CR3 mismatch injection did not escalate");
        }
        b"cpu-wrong-publication" => {
            shadow::cpu::inject_wrong_publication();
            panic!("strict CPU publication mismatch injection did not escalate");
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
