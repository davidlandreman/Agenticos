//! CPUID-gated x86-64 hardware random fallback.

use core::arch::x86_64::{__cpuid, _rdrand64_step};

const RDRAND_CPUID_BIT: u32 = 1 << 30;
const RETRIES_PER_WORD: usize = 10;

#[derive(Debug, Clone, Copy)]
pub struct CpuRandom;

impl CpuRandom {
    pub fn discover() -> Option<Self> {
        // SAFETY: CPUID is available on the x86-64 target.
        let features = unsafe { __cpuid(1) };
        if !cpuid_reports_rdrand(features.ecx) {
            return None;
        }
        let source = Self;
        let mut probe = [0u8; 8];
        if source.fill_bytes(&mut probe).is_err() {
            probe.fill(0);
            return None;
        }
        probe.fill(0);
        Some(source)
    }

    pub fn fill_bytes(&self, out: &mut [u8]) -> Result<(), ()> {
        let mut offset = 0usize;
        while offset < out.len() {
            let word = next_word()?;
            let bytes = word.to_ne_bytes();
            let count = (out.len() - offset).min(bytes.len());
            out[offset..offset + count].copy_from_slice(&bytes[..count]);
            offset += count;
        }
        Ok(())
    }
}

fn next_word() -> Result<u64, ()> {
    for _ in 0..RETRIES_PER_WORD {
        let mut value = 0u64;
        // SAFETY: CpuRandom is constructed only after CPUID reports RDRAND.
        if unsafe { _rdrand64_step(&mut value) } == 1 {
            return Ok(value);
        }
        core::hint::spin_loop();
    }
    Err(())
}

pub(crate) fn cpuid_reports_rdrand(ecx: u32) -> bool {
    ecx & RDRAND_CPUID_BIT != 0
}
