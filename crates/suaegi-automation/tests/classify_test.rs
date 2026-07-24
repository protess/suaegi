//! M4 classify + label oracle — ported from Orca's `src/shared/automation-schedules.test.ts`
//! (cases 8, 9, 12, 15, 16) plus mutation-targeted pins from the plan §2 M4.
//!
//! Clock-dependent classification (the possible-occurrence gate) pins `Etc/UTC` (Codex F2) for
//! host-independent civil dates. Labels are HARDCODED ENGLISH (no `Intl`); the time format is
//! the `en-US` 12-hour output byte-for-byte with the oracle's `formatTimeForTest` — `{h}:{mm}`
//! + one ASCII space + `AM`/`PM`.
//!
//! Every test here is mutation-verifiable: it FAILS if the specific logic it guards is broken.

use chrono::TimeZone;
use chrono_tz::Etc::UTC;
use suaegi_automation::{
    classify_automation_cron_schedule, format_automation_schedule, format_time, ScheduleKind,
};

/// A UTC wall-clock instant in epoch milliseconds.
fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    UTC.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp_millis()
}

/// Fixed, now-independent anchor for the possibility gate (2026-05-15T12:00:00Z, a Friday).
fn now() -> i64 {
    ms(2026, 5, 15, 12, 0)
}

fn classify(expr: &str) -> ScheduleKind {
    classify_automation_cron_schedule(expr, now(), UTC)
}

fn label(expr: &str) -> String {
    format_automation_schedule(expr, now(), UTC)
}

// ---------------------------------------------------------------------------------------
// format_time — the en-US 12-hour clock the Daily/Weekly labels embed.
// ---------------------------------------------------------------------------------------

#[test]
fn format_time_matches_en_us_12_hour_clock() {
    assert_eq!(format_time(10, 15), "10:15 AM");
    assert_eq!(format_time(12, 30), "12:30 PM");
    assert_eq!(format_time(0, 0), "12:00 AM"); // midnight → 12 AM
    assert_eq!(format_time(12, 0), "12:00 PM"); // noon → 12 PM
    assert_eq!(format_time(9, 5), "9:05 AM"); // minute zero-padded, hour not
    assert_eq!(format_time(23, 59), "11:59 PM");
}

// ---------------------------------------------------------------------------------------
// Oracle case 8 (`:103`) — unsupported FREQ → 'Invalid schedule'.
// ---------------------------------------------------------------------------------------

#[test]
fn case8_unsupported_freq_labels_invalid() {
    assert_eq!(label("FREQ=YEARLY"), "Invalid schedule");
}

// ---------------------------------------------------------------------------------------
// Oracle case 9 (`:107`) — hourly RRULE label uses the stored minute, zero-padded.
// ---------------------------------------------------------------------------------------

#[test]
fn case9_hourly_rrule_label() {
    assert_eq!(label("FREQ=HOURLY;BYMINUTE=5"), "Hourly at :05");
}

// ---------------------------------------------------------------------------------------
// Oracle case 12 (`:140`) — friendly labels for simple cron schedules.
// ---------------------------------------------------------------------------------------

#[test]
fn case12_friendly_cron_labels() {
    assert_eq!(label("5 * * * *"), "Hourly at :05");
    assert_eq!(label("15 10 * * *"), "Daily at 10:15 AM");
    assert_eq!(label("15 10 * * MON-FRI"), "Weekdays at 10:15 AM");
    // dow 7 normalized to 0 → Sunday.
    assert_eq!(label("30 12 * * 7"), "Sundays at 12:30 PM");
}

// ---------------------------------------------------------------------------------------
// Oracle case 15 (`:171`) — classification of weekdays / weekly for edit flows.
// ---------------------------------------------------------------------------------------

#[test]
fn case15_classifies_weekdays_and_weekly() {
    assert_eq!(
        classify("15 10 * * MON-FRI"),
        ScheduleKind::Weekdays {
            hour: 10,
            minute: 15
        }
    );
    assert_eq!(
        classify("30 12 * * 7"),
        ScheduleKind::Weekly {
            hour: 12,
            minute: 30,
            day_of_week: 0
        }
    );
}

// ---------------------------------------------------------------------------------------
// Oracle case 16 (`:185`) — valid-but-unsupported cron → 'Custom schedule'.
// ---------------------------------------------------------------------------------------

#[test]
fn case16_valid_unsupported_cron_is_custom() {
    // Multi-minute set → no single minute.
    assert_eq!(label("*/30 9-17 * * MON-FRI"), "Custom schedule");
    // Restricted day-of-month, unrestricted DOW → restricted calendar.
    assert_eq!(label("0 9 1 * *"), "Custom schedule");
    // BOTH day fields restricted → OR semantics, not a single preset.
    assert_eq!(label("0 9 1 * MON"), "Custom schedule");
    // Multi-hour set → no single hour.
    assert_eq!(label("0 9,17 * * MON-FRI"), "Custom schedule");

    // And each classifies as Custom, not a preset.
    assert_eq!(classify("0 9 1 * MON"), ScheduleKind::Custom);
    assert_eq!(classify("0 9,17 * * MON-FRI"), ScheduleKind::Custom);
}

// ---------------------------------------------------------------------------------------
// Deterministic-core pins (mutation targets beyond the oracle).
// ---------------------------------------------------------------------------------------

#[test]
fn hourly_requires_single_minute_full_hours_unrestricted() {
    // Single minute + full 0-23 hours + unrestricted calendar/DOW → hourly.
    assert_eq!(classify("5 * * * *"), ScheduleKind::Hourly { minute: 5 });
    // A restricted hour set (not the full 0-23) is NOT hourly.
    assert_eq!(classify("5 0-22 * * *"), ScheduleKind::Custom);
}

#[test]
fn daily_requires_single_minute_hour_unrestricted_dow() {
    assert_eq!(
        classify("15 10 * * *"),
        ScheduleKind::Daily {
            hour: 10,
            minute: 15
        }
    );
}

#[test]
fn weekdays_is_exactly_monday_through_friday() {
    // {1,2,3,4,5} → weekdays.
    assert_eq!(
        classify("15 10 * * 1-5"),
        ScheduleKind::Weekdays {
            hour: 10,
            minute: 15
        }
    );
    // {1,2,3,4} (Mon-Thu) is NOT the weekdays set → weekly? No: 4 distinct days, not single,
    // not exactly {1..5} → Custom.
    assert_eq!(classify("15 10 * * 1-4"), ScheduleKind::Custom);
    // Sat+Sun is not weekdays either.
    assert_eq!(classify("15 10 * * 6,0"), ScheduleKind::Custom);
}

#[test]
fn weekly_normalizes_dow_7_to_sunday_zero() {
    // Both `0` and `7` must classify as Sunday (day_of_week 0) and label "Sundays".
    let via_zero = classify("30 12 * * 0");
    let via_seven = classify("30 12 * * 7");
    assert_eq!(
        via_zero,
        ScheduleKind::Weekly {
            hour: 12,
            minute: 30,
            day_of_week: 0
        }
    );
    assert_eq!(via_seven, via_zero);
    assert_eq!(via_seven.label(), "Sundays at 12:30 PM");
}

#[test]
fn weekly_weekday_names_span_the_week() {
    // dayOfWeek 0..6 anchored Sunday-first.
    let cases = [
        ("30 12 * * 0", "Sundays at 12:30 PM"),
        ("30 12 * * 1", "Mondays at 12:30 PM"),
        ("30 12 * * 2", "Tuesdays at 12:30 PM"),
        ("30 12 * * 3", "Wednesdays at 12:30 PM"),
        ("30 12 * * 4", "Thursdays at 12:30 PM"),
        ("30 12 * * 5", "Fridays at 12:30 PM"),
        ("30 12 * * 6", "Saturdays at 12:30 PM"),
    ];
    for (expr, expected) in cases {
        assert_eq!(label(expr), expected, "label for {expr}");
    }
}

#[test]
fn impossible_cron_classifies_and_labels_invalid() {
    // Feb 31 never occurs → Invalid (the possibility gate runs FIRST).
    assert_eq!(classify("0 0 31 2 *"), ScheduleKind::Invalid);
    assert_eq!(label("0 0 31 2 *"), "Invalid schedule");
}

#[test]
fn malformed_cron_labels_invalid() {
    // A parse error (bad separators) → Invalid, never a silent Custom.
    assert_eq!(classify("*/15/2 9 * * *"), ScheduleKind::Invalid);
    assert_eq!(label("0 9 1--5 * *"), "Invalid schedule");
}

// ---------------------------------------------------------------------------------------
// RRULE label path (dispatched on `=`).
// ---------------------------------------------------------------------------------------

#[test]
fn rrule_labels_cover_every_preset() {
    assert_eq!(label("FREQ=HOURLY;BYMINUTE=5"), "Hourly at :05");
    assert_eq!(label("FREQ=DAILY;BYHOUR=10;BYMINUTE=15"), "Daily at 10:15 AM");
    assert_eq!(
        label("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=10;BYMINUTE=15"),
        "Weekdays at 10:15 AM"
    );
    // Sunday (SU, index 0) is preserved in the RRULE path too.
    assert_eq!(
        label("FREQ=WEEKLY;BYDAY=SU;BYHOUR=12;BYMINUTE=30"),
        "Sundays at 12:30 PM"
    );
    // A WEEKLY with multiple non-weekday codes parses but the preset mapper rejects it →
    // 'Invalid schedule'.
    assert_eq!(
        label("FREQ=WEEKLY;BYDAY=MO,TU;BYHOUR=10;BYMINUTE=15"),
        "Invalid schedule"
    );
}
