use crate::arch::x86_64::rtc::{decode_snapshot, RawRtcSnapshot};
use crate::lib::test_utils::Testable;
use crate::time::{datetime_from_unix_seconds, unix_seconds_from_datetime, DateTime};

fn raw(hours: u8, status_b: u8) -> RawRtcSnapshot {
    RawRtcSnapshot {
        seconds: 0x56,
        minutes: 0x34,
        hours,
        day: 0x18,
        month: 0x07,
        year: 0x26,
        century: 0x20,
        status_b,
    }
}

fn test_bcd_24_hour_decode() {
    assert_eq!(
        decode_snapshot(raw(0x15, 0x02)).unwrap(),
        DateTime {
            year: 2026,
            month: 7,
            day: 18,
            hour: 15,
            minute: 34,
            second: 56,
        }
    );
}

fn test_binary_24_hour_decode() {
    let value = RawRtcSnapshot {
        seconds: 56,
        minutes: 34,
        hours: 15,
        day: 18,
        month: 7,
        year: 26,
        century: 20,
        status_b: 0x06,
    };
    assert_eq!(decode_snapshot(value).unwrap().hour, 15);
}

fn test_12_hour_decode_edges() {
    assert_eq!(decode_snapshot(raw(0x12, 0)).unwrap().hour, 0);
    assert_eq!(decode_snapshot(raw(0x92, 0)).unwrap().hour, 12);
    assert_eq!(decode_snapshot(raw(0x83, 0)).unwrap().hour, 15);
}

fn test_century_fallback_and_invalid_encoding() {
    let mut value = raw(0x15, 0x02);
    value.century = 0;
    assert_eq!(decode_snapshot(value).unwrap().year, 2026);
    value.month = 0x1A;
    assert!(decode_snapshot(value).is_err());
}

fn test_calendar_validation_and_leap_years() {
    assert!(DateTime {
        year: 2000,
        month: 2,
        day: 29,
        hour: 0,
        minute: 0,
        second: 0,
    }
    .is_valid());
    assert!(DateTime {
        year: 2024,
        month: 2,
        day: 29,
        hour: 23,
        minute: 59,
        second: 59,
    }
    .is_valid());
    assert!(!DateTime {
        year: 2100,
        month: 2,
        day: 29,
        hour: 0,
        minute: 0,
        second: 0,
    }
    .is_valid());
}

fn test_unix_epoch_and_known_timestamp() {
    let epoch = DateTime {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };
    assert_eq!(unix_seconds_from_datetime(epoch), Some(0));

    let y2k = DateTime {
        year: 2000,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };
    assert_eq!(unix_seconds_from_datetime(y2k), Some(946_684_800));
    assert_eq!(datetime_from_unix_seconds(946_684_800), Some(y2k));
}

fn test_calendar_round_trips_boundaries() {
    let values = [
        DateTime {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        },
        DateTime {
            year: 2024,
            month: 2,
            day: 29,
            hour: 12,
            minute: 34,
            second: 56,
        },
        DateTime {
            year: 2099,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
        },
    ];
    for value in values {
        let seconds = unix_seconds_from_datetime(value).unwrap();
        assert_eq!(datetime_from_unix_seconds(seconds), Some(value));
    }
}

fn test_live_qemu_rtc_is_plausible() {
    let now = crate::arch::x86_64::rtc::read_datetime().expect("QEMU RTC should be readable");
    assert!((2020..=2099).contains(&now.year));
    assert!(now.is_valid());
    assert!(crate::time::wall_clock_ns().is_some());
    assert!(crate::time::realtime_ns() > 1_500_000_000_000_000_000);
}

pub fn get_tests() -> &'static [&'static dyn Testable] {
    &[
        &test_bcd_24_hour_decode,
        &test_binary_24_hour_decode,
        &test_12_hour_decode_edges,
        &test_century_fallback_and_invalid_encoding,
        &test_calendar_validation_and_leap_years,
        &test_unix_epoch_and_known_timestamp,
        &test_calendar_round_trips_boundaries,
        &test_live_qemu_rtc_is_plausible,
    ]
}
