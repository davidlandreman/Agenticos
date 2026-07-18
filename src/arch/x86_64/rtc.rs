//! PC CMOS real-time clock access.
//!
//! The RTC is sampled once during boot. Consumers use the generic `crate::time`
//! clock, which advances that sample with PIT ticks; rendering and syscalls do
//! not perform CMOS port I/O.

use x86_64::instructions::{interrupts, port::Port};

use crate::time::DateTime;

const CMOS_INDEX_PORT: u16 = 0x70;
const CMOS_DATA_PORT: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;
const REG_CENTURY: u8 = 0x32;

const STATUS_A_UPDATE_IN_PROGRESS: u8 = 1 << 7;
const STATUS_B_24_HOUR: u8 = 1 << 1;
const STATUS_B_BINARY: u8 = 1 << 2;
const HOUR_PM: u8 = 1 << 7;

/// Port-read bound for one update-in-progress wait. The RTC's update window is
/// normally well below a millisecond; the bound prevents broken hardware from
/// stalling boot indefinitely.
const UIP_POLL_LIMIT: usize = 100_000;
const SNAPSHOT_RETRIES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcError {
    UpdateInProgressTimeout,
    UnstableSnapshot,
    InvalidEncoding,
    InvalidDateTime,
}

/// Raw register image retained as a separate type so encoding logic is
/// hardware-independent and testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawRtcSnapshot {
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub day: u8,
    pub month: u8,
    pub year: u8,
    pub century: u8,
    pub status_b: u8,
}

/// Read and decode a stable RTC snapshot.
pub fn read_datetime() -> Result<DateTime, RtcError> {
    interrupts::without_interrupts(|| {
        let result = read_stable_snapshot().and_then(decode_snapshot);

        // Every indexed register access below sets bit 7 to mask NMI. Restore
        // NMI-enabled state before interrupts can be re-enabled by the scope.
        unsafe {
            let mut index = Port::<u8>::new(CMOS_INDEX_PORT);
            index.write(REG_SECONDS);
        }
        result
    })
}

fn read_stable_snapshot() -> Result<RawRtcSnapshot, RtcError> {
    for _ in 0..SNAPSHOT_RETRIES {
        wait_until_not_updating()?;
        let first = unsafe { read_snapshot() };
        wait_until_not_updating()?;
        let second = unsafe { read_snapshot() };
        if first == second {
            return Ok(first);
        }
    }
    Err(RtcError::UnstableSnapshot)
}

fn wait_until_not_updating() -> Result<(), RtcError> {
    for _ in 0..UIP_POLL_LIMIT {
        let status = unsafe { read_register(REG_STATUS_A) };
        if status & STATUS_A_UPDATE_IN_PROGRESS == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(RtcError::UpdateInProgressTimeout)
}

unsafe fn read_snapshot() -> RawRtcSnapshot {
    RawRtcSnapshot {
        seconds: read_register(REG_SECONDS),
        minutes: read_register(REG_MINUTES),
        hours: read_register(REG_HOURS),
        day: read_register(REG_DAY),
        month: read_register(REG_MONTH),
        year: read_register(REG_YEAR),
        century: read_register(REG_CENTURY),
        status_b: read_register(REG_STATUS_B),
    }
}

/// Read one CMOS register while keeping NMI masked for the snapshot duration.
unsafe fn read_register(register: u8) -> u8 {
    let mut index = Port::<u8>::new(CMOS_INDEX_PORT);
    let mut data = Port::<u8>::new(CMOS_DATA_PORT);
    index.write(register | 0x80);
    data.read()
}

pub(crate) fn decode_snapshot(raw: RawRtcSnapshot) -> Result<DateTime, RtcError> {
    let binary = raw.status_b & STATUS_B_BINARY != 0;
    let decode_required = |value| decode_value(value, binary).ok_or(RtcError::InvalidEncoding);

    let second = decode_required(raw.seconds)?;
    let minute = decode_required(raw.minutes)?;

    let pm = raw.hours & HOUR_PM != 0;
    let mut hour = decode_required(raw.hours & !HOUR_PM)?;
    if raw.status_b & STATUS_B_24_HOUR == 0 {
        if !(1..=12).contains(&hour) {
            return Err(RtcError::InvalidDateTime);
        }
        hour %= 12;
        if pm {
            hour += 12;
        }
    }

    let day = decode_required(raw.day)?;
    let month = decode_required(raw.month)?;
    let short_year = decode_required(raw.year)? as u16;

    // ACPI normally identifies the century register. This early PC kernel has
    // no ACPI table parser for that field yet, so use the conventional 0x32
    // register when it contains a plausible century and otherwise constrain
    // the deployment fallback to 2000-2099.
    let century = decode_value(raw.century, binary)
        .filter(|value| (19..=99).contains(value))
        .unwrap_or(20) as u16;
    let date_time = DateTime {
        year: century.saturating_mul(100).saturating_add(short_year),
        month,
        day,
        hour,
        minute,
        second,
    };

    if date_time.is_valid() {
        Ok(date_time)
    } else {
        Err(RtcError::InvalidDateTime)
    }
}

fn decode_value(value: u8, binary: bool) -> Option<u8> {
    if binary {
        return Some(value);
    }
    let high = value >> 4;
    let low = value & 0x0F;
    if high > 9 || low > 9 {
        return None;
    }
    Some(high * 10 + low)
}
