//! Kernel cryptographic random broker.
//!
//! The broker exposes trusted platform bytes directly. It intentionally has no
//! timer/MAC/input fallback and no home-grown software PRNG.

use crate::arch::x86_64::interrupt_guard::InterruptMutex;
use crate::arch::x86_64::random::CpuRandom;
use crate::drivers::virtio::rng::VirtioRng;
use crate::{debug_info, debug_warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomError {
    Unavailable,
    DeviceFailure,
    CpuFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    VirtioRng,
    Rdrand,
}

struct RandomState {
    virtio: Option<VirtioRng>,
    cpu: Option<CpuRandom>,
    selected: Option<SourceKind>,
}

impl RandomState {
    const fn new() -> Self {
        Self {
            virtio: None,
            cpu: None,
            selected: None,
        }
    }
}

static RANDOM: InterruptMutex<RandomState> = InterruptMutex::new(RandomState::new());

pub fn init() {
    let cpu = CpuRandom::discover();
    let mut virtio = VirtioRng::discover();
    let mut virtio_ready = false;
    if let Some(driver) = virtio.as_mut() {
        let mut probe = [0u8; 32];
        match driver.fill_bytes(&mut probe) {
            Ok(()) => virtio_ready = true,
            Err(error) => debug_warn!("VirtIO RNG probe failed: {:?}", error),
        }
        probe.fill(0);
    }

    let selected = if virtio_ready {
        Some(SourceKind::VirtioRng)
    } else if cpu.is_some() {
        Some(SourceKind::Rdrand)
    } else {
        None
    };
    let mut state = RANDOM.lock();
    state.virtio = virtio;
    state.cpu = cpu;
    state.selected = selected;
    match selected {
        Some(SourceKind::VirtioRng) => debug_info!("[entropy] source=virtio-rng"),
        Some(SourceKind::Rdrand) => debug_info!("[entropy] source=rdrand"),
        None => debug_warn!("[entropy] unavailable; secure process/network startup disabled"),
    }
}

#[cfg(feature = "test")]
pub fn source_kind() -> Option<SourceKind> {
    RANDOM.lock().selected
}

pub fn fill_bytes(out: &mut [u8]) -> Result<(), RandomError> {
    if out.is_empty() {
        return Ok(());
    }
    let mut state = RANDOM.lock();
    match state.selected {
        Some(SourceKind::VirtioRng) => {
            if state
                .virtio
                .as_mut()
                .is_some_and(|driver| driver.fill_bytes(out).is_ok())
            {
                return Ok(());
            }
            out.fill(0);
            debug_warn!("[entropy] virtio-rng failed; attempting rdrand fallback");
            if let Some(cpu) = state.cpu {
                if cpu.fill_bytes(out).is_ok() {
                    state.selected = Some(SourceKind::Rdrand);
                    return Ok(());
                }
            }
            state.selected = None;
            out.fill(0);
            Err(RandomError::DeviceFailure)
        }
        Some(SourceKind::Rdrand) => {
            if state.cpu.is_some_and(|cpu| cpu.fill_bytes(out).is_ok()) {
                Ok(())
            } else {
                state.selected = None;
                out.fill(0);
                Err(RandomError::CpuFailure)
            }
        }
        None => {
            out.fill(0);
            Err(RandomError::Unavailable)
        }
    }
}
