//! Booted coverage for trusted platform entropy and the kernel broker.

use crate::lib::test_utils::Testable;

fn test_qemu_uses_virtio_rng() {
    assert_eq!(
        crate::random::source_kind(),
        Some(crate::random::SourceKind::VirtioRng),
        "test QEMU must select its host-backed VirtIO RNG"
    );
}

fn test_consecutive_random_blocks_differ() {
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];
    crate::random::fill_bytes(&mut first).expect("first entropy request failed");
    crate::random::fill_bytes(&mut second).expect("second entropy request failed");
    assert_ne!(first, second, "consecutive entropy blocks repeated");
}

fn test_rdrand_cpuid_detection() {
    assert!(
        crate::arch::x86_64::random::cpuid_reports_rdrand(1 << 30),
        "RDRAND feature bit was not recognized"
    );
    assert!(!crate::arch::x86_64::random::cpuid_reports_rdrand(0));
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_qemu_uses_virtio_rng,
        &test_consecutive_random_blocks_differ,
        &test_rdrand_cpuid_detection,
    ]
}
