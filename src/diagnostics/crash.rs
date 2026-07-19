//! First-failure crash owner and panic-safe debugcon exporter.

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use super::registers::{
    capture_live, RegisterSnapshot, FIDELITY_CPU_PUSHED, FIDELITY_HANDLER_LIVE,
};
use super::wire::{crc32, fnv1a64, SectionKind, Writer, HEADER_LEN, MAGIC, SCHEMA_VERSION};

const ARENA_LEN: usize = 256 * 1024;
const DEBUGCON_PORT: u16 = 0xe9;
const STREAM_PREAMBLE: &[u8] = b"\0AGCRASH\x1e";
const STREAM_COMPLETE: &[u8] = b"AGEND\0";
const FLAG_TRUNCATED: u64 = 1 << 0;
const FLAG_NESTED: u64 = 1 << 1;

const STATE_IDLE: u8 = 0;
const STATE_CAPTURING: u8 = 1;
const STATE_COMPLETE: u8 = 2;

#[derive(Clone, Copy)]
#[allow(
    dead_code,
    reason = "v1 wire IDs reserved for invariant and incident records"
)]
#[repr(u8)]
pub enum RecordKind {
    Fatal = 1,
    Invariant = 2,
    UserIncident = 3,
}

#[derive(Clone, Copy)]
pub struct Trigger {
    pub kind: RecordKind,
    pub vector: u8,
    pub fidelity: u8,
    pub reason_hash: u64,
    pub error_code: u64,
    pub fault_address: u64,
    pub rip: u64,
    pub file_hash: u64,
    pub line: u32,
    pub column: u32,
}

struct Arena(UnsafeCell<[u8; ARENA_LEN]>);
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; ARENA_LEN]));
static STATE: AtomicU8 = AtomicU8::new(STATE_IDLE);
static RECORD_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static NESTED_COUNT: AtomicU64 = AtomicU64::new(0);

const BUILD_HASH_A: u64 = fnv1a64(env!("AGENTICOS_BUILD_GIT_SHA").as_bytes())
    ^ fnv1a64(env!("AGENTICOS_BUILD_GIT_DIRTY").as_bytes()).rotate_left(17);
const BUILD_HASH_B: u64 = fnv1a64(env!("AGENTICOS_BUILD_RUSTC").as_bytes());
const BUILD_HASH_C: u64 = fnv1a64(env!("AGENTICOS_BUILD_DIAGNOSTICS").as_bytes());

fn build_id() -> [u8; 20] {
    let mut id = [0u8; 20];
    id[..8].copy_from_slice(&BUILD_HASH_A.to_le_bytes());
    id[8..16].copy_from_slice(&BUILD_HASH_B.to_le_bytes());
    id[16..20].copy_from_slice(&(BUILD_HASH_C as u32).to_le_bytes());
    id
}

pub fn begin_panic(info: &PanicInfo<'_>) -> ! {
    let (file_hash, line, column) = if let Some(location) = info.location() {
        (
            fnv1a64(location.file().as_bytes()),
            location.line(),
            location.column(),
        )
    } else {
        (0, 0, 0)
    };
    begin(Trigger {
        kind: RecordKind::Fatal,
        vector: 0xff,
        fidelity: FIDELITY_HANDLER_LIVE,
        reason_hash: fnv1a64(b"rust-panic"),
        error_code: 0,
        fault_address: read_cr2(),
        rip: 0,
        file_hash,
        line,
        column,
    })
}

pub fn begin_trap(
    reason: &'static str,
    vector: u8,
    error_code: Option<u64>,
    fault_address: Option<u64>,
    rip: u64,
) -> ! {
    begin(Trigger {
        kind: RecordKind::Fatal,
        vector,
        fidelity: FIDELITY_CPU_PUSHED,
        reason_hash: fnv1a64(reason.as_bytes()),
        error_code: error_code.unwrap_or(0),
        fault_address: fault_address.unwrap_or(0),
        rip,
        file_hash: 0,
        line: 0,
        column: 0,
    })
}

fn read_cr2() -> u64 {
    let value: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack)) };
    value
}

fn begin(trigger: Trigger) -> ! {
    x86_64::instructions::interrupts::disable();
    if STATE
        .compare_exchange(
            STATE_IDLE,
            STATE_CAPTURING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        NESTED_COUNT.fetch_add(1, Ordering::Relaxed);
        super::trace::record_early(
            super::trace::EventKind::NestedFatal,
            trigger.vector.into(),
            0,
        );
        halt_or_exit();
    }

    let owner_cpu = if super::percpu_ready() {
        crate::arch::x86_64::percpu::cpu_id().min(7) as u8
    } else {
        0
    };
    super::trace::record_on(
        owner_cpu as usize,
        super::trace::EventKind::FatalElected,
        trigger.vector.into(),
        trigger.reason_hash,
        trigger.rip,
        0,
    );
    let registers = capture_live(trigger.rip, trigger.fidelity);
    let length = unsafe { serialize(&mut *ARENA.0.get(), owner_cpu, trigger, registers) };
    unsafe {
        emit(STREAM_PREAMBLE);
        emit(&(&*ARENA.0.get())[..length]);
        emit(STREAM_COMPLETE);
    }
    STATE.store(STATE_COMPLETE, Ordering::Release);
    crate::arch::x86_64::smp::freeze_other_cpus();
    halt_or_exit()
}

unsafe fn serialize(
    arena: &mut [u8; ARENA_LEN],
    owner_cpu: u8,
    trigger: Trigger,
    registers: RegisterSnapshot,
) -> usize {
    let mut writer = Writer::new(arena);
    let _ = writer.zeros(HEADER_LEN);

    writer.section(SectionKind::RunMetadata, 1, 0, |section| {
        let feature_mask = u32::from(cfg!(feature = "diagnostics"))
            | (u32::from(cfg!(feature = "diagnostics-strict")) << 1);
        section.u8(super::personality() as u8);
        section.u8(0);
        section.u16(0);
        section.u32(feature_mask);
        write_bounded_string(section, env!("AGENTICOS_BUILD_GIT_SHA"));
        write_bounded_string(section, env!("AGENTICOS_BUILD_GIT_DIRTY"));
        write_bounded_string(section, env!("AGENTICOS_BUILD_RUSTC"));
        write_bounded_string(section, env!("AGENTICOS_BUILD_DIAGNOSTICS"));
    });
    writer.section(SectionKind::Trigger, 1, 0, |section| {
        section.u8(trigger.kind as u8);
        section.u8(trigger.vector);
        section.u8(trigger.fidelity);
        section.u8(0);
        section.u64(trigger.reason_hash);
        section.u64(trigger.error_code);
        section.u64(trigger.fault_address);
        section.u64(trigger.rip);
        section.u64(trigger.file_hash);
        section.u32(trigger.line);
        section.u32(trigger.column);
    });
    writer.section(SectionKind::CpuSnapshots, 1, 0, |section| {
        section.u8(1);
        section.u8(owner_cpu);
        section.u16(core::mem::size_of::<RegisterSnapshot>() as u16);
        for value in [
            registers.rip,
            registers.rsp,
            registers.rbp,
            registers.rflags,
            registers.cr0,
            registers.cr2,
            registers.cr3,
            registers.cr4,
            registers.fs_base,
            registers.gs_base,
            registers.current_pid,
        ] {
            section.u64(value);
        }
        section.u8(registers.fidelity);
        section.raw(&registers._reserved);
    });
    writer.section(SectionKind::TraceTail, 1, 0, |section| {
        const EXPORT_PER_CPU: usize = 128;
        let cpu_count = crate::arch::x86_64::percpu::initialized_cpu_count()
            .clamp(1, crate::arch::x86_64::acpi::MAX_CPUS);
        section.u16(cpu_count as u16);
        section.u16(super::trace::RING_LEN as u16);
        section.u16(EXPORT_PER_CPU as u16);
        section.u16(0);
        for cpu in 0..cpu_count {
            let (next, overwrites, drops) = super::trace::counters(cpu);
            section.u8(cpu as u8);
            section.u8(0);
            section.u16(0);
            section.u64(next);
            section.u64(overwrites);
            section.u64(drops);
            let first = next.saturating_sub(EXPORT_PER_CPU as u64).max(1);
            let count_at = section.len();
            section.u32(0);
            let mut count = 0u32;
            for sequence in first..next {
                let index = sequence as usize % super::trace::RING_LEN;
                if let Some(record) = super::trace::snapshot(cpu, index) {
                    if record.sequence != sequence {
                        continue;
                    }
                    for value in [
                        record.sequence,
                        record.tsc,
                        record.tick,
                        record.causal_epoch,
                        record.subject,
                        record.arg0,
                        record.arg1,
                        record.meta,
                    ] {
                        section.u64(value);
                    }
                    count += 1;
                }
            }
            section.patch_u32(count_at, count);
        }
    });
    if let Some(violation) = super::shadow::first() {
        writer.section(SectionKind::Violation, 1, 0, |section| {
            section.u32(violation.invariant_id);
            section.u8(violation.severity);
            section.u8(violation.cpu);
            section.u8(violation.mode);
            section.u8(violation.domain);
            for value in [
                violation.epoch,
                violation.subject,
                violation.expected0,
                violation.observed0,
                violation.expected1,
                violation.observed1,
                violation.trace_sequence,
            ] {
                section.u64(value);
            }
        });
    }
    writer.section(SectionKind::Backtrace, 1, 1, |section| {
        section.u16(0);
        section.u8(5); // Unavailable until stack bounds are crash-readable.
        section.u8(0);
    });
    writer.section(SectionKind::Footer, 1, 0, |section| {
        section.u64(NESTED_COUNT.load(Ordering::Relaxed));
        section.u32(0x434f_4d50); // "COMP"
        section.u32(0);
    });

    let mut flags = 0u64;
    if writer.truncated() {
        flags |= FLAG_TRUNCATED;
    }
    if NESTED_COUNT.load(Ordering::Relaxed) != 0 {
        flags |= FLAG_NESTED;
    }
    let total_len = writer.len();
    let initialized = crate::arch::x86_64::percpu::initialized_cpu_count().clamp(1, 8);
    let online_mask = ((1u16 << initialized) - 1) as u8;
    let captured_mask = 1u8 << owner_cpu;
    let sequence = RECORD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let build_id = build_id();

    arena[..8].copy_from_slice(&MAGIC);
    arena[8..10].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
    arena[10..12].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    arena[12..16].copy_from_slice(&(total_len as u32).to_le_bytes());
    arena[16..24].copy_from_slice(&flags.to_le_bytes());
    arena[24..40].copy_from_slice(&super::identity::run_id());
    arena[40..60].copy_from_slice(&build_id);
    arena[60] = owner_cpu;
    arena[61] = online_mask;
    arena[62] = captured_mask;
    arena[63] = trigger.kind as u8;
    arena[64..72].copy_from_slice(&sequence.to_le_bytes());
    let payload_crc = crc32(&arena[HEADER_LEN..total_len]);
    arena[72..76].copy_from_slice(&payload_crc.to_le_bytes());
    arena[76..80].fill(0);
    let header_crc = crc32(&arena[..HEADER_LEN]);
    arena[76..80].copy_from_slice(&header_crc.to_le_bytes());
    total_len
}

fn write_bounded_string(writer: &mut Writer<'_>, value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(u16::MAX as usize);
    writer.u16(len as u16);
    writer.raw(&bytes[..len]);
}

unsafe fn emit(bytes: &[u8]) {
    use x86_64::instructions::port::Port;
    let mut port = Port::<u8>::new(DEBUGCON_PORT);
    for &byte in bytes {
        port.write(byte);
    }
}

fn halt_or_exit() -> ! {
    #[cfg(feature = "test")]
    {
        crate::lib::test_utils::exit_qemu(crate::lib::test_utils::QemuExitCode::Failed);
    }
    loop {
        x86_64::instructions::hlt();
    }
}
