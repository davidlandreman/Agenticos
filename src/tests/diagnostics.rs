use crate::diagnostics::{trace, wire};
use crate::lib::test_utils::Testable;

fn test_crc32_golden_vector() {
    assert_eq!(wire::crc32(b"123456789"), 0xcbf4_3926);
}

fn test_wire_layout_contract() {
    assert_eq!(core::mem::size_of::<wire::CapsuleHeader>(), 80);
    assert_eq!(core::mem::size_of::<wire::SectionHeader>(), 16);
    assert_eq!(wire::MAGIC, *b"AGCRASH\0");
}

fn test_trace_commit_and_wrap() {
    // CPU 7 is a valid possible-CPU recorder even when the test VM boots
    // fewer CPUs, which keeps this synthetic writer isolated from live CPUs.
    let cpu = crate::arch::x86_64::acpi::MAX_CPUS - 1;
    let (start, overwritten_before, _) = trace::counters(cpu);
    let writes = trace::RING_LEN as u64 + 5;
    for value in 0..writes {
        trace::record_on(
            cpu,
            trace::EventKind::BootPhase,
            value,
            value ^ 0x55aa,
            0,
            0,
        );
    }
    let (next, overwritten_after, _) = trace::counters(cpu);
    assert_eq!(next, start + writes);
    let expected_new_overwrites = (next.saturating_sub(1 + trace::RING_LEN as u64))
        .saturating_sub(start.saturating_sub(1 + trace::RING_LEN as u64));
    assert_eq!(
        overwritten_after - overwritten_before,
        expected_new_overwrites
    );
    let newest = next - 1;
    let record = trace::snapshot(cpu, newest as usize % trace::RING_LEN)
        .expect("committed newest trace record");
    assert_eq!(record.sequence, newest);
    assert_eq!(record.subject, writes - 1);
    assert_eq!(record.arg0, (writes - 1) ^ 0x55aa);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_crc32_golden_vector,
        &test_wire_layout_contract,
        &test_trace_commit_and_wrap,
    ]
}
