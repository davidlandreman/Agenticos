//! Kernel monotonic and RTC-anchored wall clocks.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const PIT_NANOSECONDS_PER_TICK: u64 = 10_000_000;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;
const SECONDS_PER_DAY: u64 = 86_400;

static WALL_CLOCK_VALID: AtomicBool = AtomicBool::new(false);
static WALL_CLOCK_EPOCH_SECONDS: AtomicU64 = AtomicU64::new(0);
static WALL_CLOCK_BASE_TICK: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    pub fn is_valid(&self) -> bool {
        if self.year < 1970
            || !(1..=12).contains(&self.month)
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
        {
            return false;
        }
        let max_day = days_in_month(self.year, self.month);
        self.day >= 1 && self.day <= max_day
    }
}

/// Anchor realtime to a stable CMOS snapshot. Failure is deliberately
/// non-fatal: realtime retains its historical uptime-from-zero fallback.
pub fn init() {
    let before = crate::arch::x86_64::interrupts::get_timer_ticks();
    match crate::arch::x86_64::rtc::read_datetime()
        .ok()
        .and_then(unix_seconds_from_datetime)
    {
        Some(epoch_seconds) => {
            let after = crate::arch::x86_64::interrupts::get_timer_ticks();
            let anchor_tick = before.saturating_add(after.saturating_sub(before) / 2);
            WALL_CLOCK_EPOCH_SECONDS.store(epoch_seconds, Ordering::Relaxed);
            WALL_CLOCK_BASE_TICK.store(anchor_tick, Ordering::Relaxed);
            WALL_CLOCK_VALID.store(true, Ordering::Release);
            crate::debug_info!(
                "wall clock: RTC anchor {} at PIT tick {}",
                epoch_seconds,
                anchor_tick
            );
        }
        None => {
            WALL_CLOCK_VALID.store(false, Ordering::Release);
            crate::debug_warn!(
                "wall clock: RTC unavailable or invalid; using monotonic realtime fallback"
            );
        }
    }
}

/// Monotonic nanoseconds since boot, derived from the 100 Hz PIT.
pub fn monotonic_ns() -> u64 {
    crate::arch::x86_64::interrupts::get_timer_ticks().saturating_mul(PIT_NANOSECONDS_PER_TICK)
}

/// RTC-anchored Unix nanoseconds, or `None` when boot could not establish a
/// valid wall clock.
pub fn wall_clock_ns() -> Option<u64> {
    if !WALL_CLOCK_VALID.load(Ordering::Acquire) {
        return None;
    }
    let epoch_seconds = WALL_CLOCK_EPOCH_SECONDS.load(Ordering::Relaxed);
    let base_tick = WALL_CLOCK_BASE_TICK.load(Ordering::Relaxed);
    let now_tick = crate::arch::x86_64::interrupts::get_timer_ticks();
    let elapsed_ns = now_tick
        .saturating_sub(base_tick)
        .saturating_mul(PIT_NANOSECONDS_PER_TICK);
    Some(
        epoch_seconds
            .saturating_mul(NANOSECONDS_PER_SECOND)
            .saturating_add(elapsed_ns),
    )
}

/// Linux realtime clock. Preserve the pre-RTC behavior if the hardware sample
/// was unavailable so existing userland still receives a progressing clock.
pub fn realtime_ns() -> u64 {
    wall_clock_ns().unwrap_or_else(monotonic_ns)
}

#[expect(
    dead_code,
    reason = "kernel wall-clock API retained for callers/diagnostics"
)]
pub fn utc_now() -> Option<DateTime> {
    let seconds = wall_clock_ns()? / NANOSECONDS_PER_SECOND;
    datetime_from_unix_seconds(seconds)
}

pub(crate) const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

pub(crate) const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_before_year(year: u16) -> u64 {
    let previous = u64::from(year.saturating_sub(1));
    previous * 365 + previous / 4 - previous / 100 + previous / 400
}

fn days_before_month(year: u16, month: u8) -> u64 {
    let mut days = 0u64;
    let mut current = 1u8;
    while current < month {
        days += u64::from(days_in_month(year, current));
        current += 1;
    }
    days
}

pub(crate) fn unix_seconds_from_datetime(value: DateTime) -> Option<u64> {
    if !value.is_valid() {
        return None;
    }
    let epoch_days = days_before_year(1970);
    let days = days_before_year(value.year)
        .checked_sub(epoch_days)?
        .checked_add(days_before_month(value.year, value.month))?
        .checked_add(u64::from(value.day - 1))?;
    days.checked_mul(SECONDS_PER_DAY)?
        .checked_add(u64::from(value.hour) * 3_600)?
        .checked_add(u64::from(value.minute) * 60)?
        .checked_add(u64::from(value.second))
}

pub(crate) fn datetime_from_unix_seconds(seconds: u64) -> Option<DateTime> {
    let day_index = seconds / SECONDS_PER_DAY;
    let seconds_in_day = seconds % SECONDS_PER_DAY;
    let absolute_day = days_before_year(1970).checked_add(day_index)?;

    // Find the containing year with a bounded binary search over DateTime's
    // representable range. This avoids iteration proportional to uptime.
    let mut low = 1970u32;
    let mut high = u16::MAX as u32 + 1;
    while low + 1 < high {
        let mid = low + (high - low) / 2;
        if days_before_year(mid as u16) <= absolute_day {
            low = mid;
        } else {
            high = mid;
        }
    }
    let year = low as u16;
    let mut day_of_year = absolute_day.checked_sub(days_before_year(year))?;
    let mut month = 1u8;
    while month <= 12 {
        let month_days = u64::from(days_in_month(year, month));
        if day_of_year < month_days {
            break;
        }
        day_of_year -= month_days;
        month += 1;
    }
    if month > 12 {
        return None;
    }

    Some(DateTime {
        year,
        month,
        day: day_of_year as u8 + 1,
        hour: (seconds_in_day / 3_600) as u8,
        minute: ((seconds_in_day % 3_600) / 60) as u8,
        second: (seconds_in_day % 60) as u8,
    })
}
