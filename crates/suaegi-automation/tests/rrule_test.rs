//! M2 RRULE oracle — ported from Orca's `src/shared/automation-schedules.test.ts`
//! (cases 4, 5, 6, 7, 11, 12-build, 20) plus the F5 `=`-dispatch pins from the plan §1.
//! Clock-dependent validity tests pin `Etc/UTC` (Codex F2) for host-independent civil dates.
//!
//! Every test here is mutation-verifiable: it FAILS if the specific logic it guards is
//! broken (no hollow tests — this repo has shipped ≥5).

use chrono::TimeZone;
use chrono_tz::Etc::UTC;
use suaegi_automation::{
    build_automation_cron_schedule, build_automation_rrule, is_valid_automation_cron_schedule,
    is_valid_automation_schedule, parse_automation_rrule, parse_rrule, parse_schedule,
    try_parse_automation_rrule, AutomationRruleParts, AutomationSchedulePreset, ParsedSchedule,
    RruleError, RruleFreq, ScheduleError,
};

/// A UTC wall-clock instant in epoch milliseconds.
fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    UTC.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp_millis()
}

/// Fixed, now-independent anchor for validity scans (2026-05-15T12:00:00Z, a Friday).
fn anchor() -> i64 {
    ms(2026, 5, 15, 12, 0)
}

// -------------------------------------------------------------------------------------
// Oracle case 4 (:63) — weekly build→parse round-trip.
// -------------------------------------------------------------------------------------

#[test]
fn weekly_build_parse_round_trips() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Weekly, 16, 45, Some(3));
    assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=WE;BYHOUR=16;BYMINUTE=45");
    assert_eq!(
        parse_automation_rrule(&rrule).unwrap(),
        AutomationRruleParts {
            preset: AutomationSchedulePreset::Weekly,
            hour: 16,
            minute: 45,
            day_of_week: 3,
        }
    );
}

// -------------------------------------------------------------------------------------
// Oracle case 5 (:73) — Sunday (dow 0) is PRESERVED, never coerced to Monday.
// -------------------------------------------------------------------------------------

#[test]
fn weekly_sunday_round_trips_without_coercion() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Weekly, 10, 15, Some(0));
    // DAY_CODES[0] == "SU".
    assert_eq!(rrule, "FREQ=WEEKLY;BYDAY=SU;BYHOUR=10;BYMINUTE=15");
    let parsed = parse_automation_rrule(&rrule).unwrap();
    assert_eq!(
        parsed,
        AutomationRruleParts {
            preset: AutomationSchedulePreset::Weekly,
            hour: 10,
            minute: 15,
            day_of_week: 0,
        }
    );
    // The load-bearing assertion: 0, not 1.
    assert_eq!(
        parsed.day_of_week, 0,
        "Sunday must stay 0, not fold to Monday"
    );
}

// -------------------------------------------------------------------------------------
// Oracle case 6 (:83) — malformed BYDAY is rejected, not remapped.
// -------------------------------------------------------------------------------------

#[test]
fn malformed_weekly_byday_is_rejected() {
    assert_eq!(
        parse_automation_rrule("FREQ=WEEKLY;BYDAY=NO;BYHOUR=10;BYMINUTE=15"),
        Err(RruleError::InvalidDay)
    );
    // A mix of a valid and an invalid code still fails (tryParse → None).
    assert_eq!(
        try_parse_automation_rrule("FREQ=WEEKLY;BYDAY=MO,NO;BYHOUR=10;BYMINUTE=15"),
        None
    );
    // Error message matches Orca's exactly.
    assert_eq!(
        RruleError::InvalidDay.to_string(),
        "Invalid recurrence day."
    );
}

// -------------------------------------------------------------------------------------
// Oracle case 7 (:90) — WEEKLY requires a BYDAY; a bare weekly rule is invalid.
// -------------------------------------------------------------------------------------

#[test]
fn weekly_without_byday_is_invalid() {
    let rrule = "FREQ=WEEKLY;BYHOUR=9;BYMINUTE=0";
    assert!(!is_valid_automation_schedule(rrule, anchor(), UTC));
    assert_eq!(parse_rrule(rrule), Err(RruleError::InvalidDay));
    assert_eq!(parse_automation_rrule(rrule), Err(RruleError::InvalidDay));
}

// -------------------------------------------------------------------------------------
// Oracle case 11 (:127) — buildAutomationCronSchedule.
// -------------------------------------------------------------------------------------

#[test]
fn builds_cron_from_presets() {
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Hourly, 9, 15, None),
        "15 * * * *",
        "hourly cron omits the hour field"
    );
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Daily, 9, 15, None),
        "15 9 * * *"
    );
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Weekdays, 9, 15, None),
        "15 9 * * 1-5"
    );
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Weekly, 9, 15, Some(0)),
        "15 9 * * 0"
    );
}

// -------------------------------------------------------------------------------------
// buildAutomationRrule table + the hourly-OMITS-BYHOUR quirk (:530-540).
// -------------------------------------------------------------------------------------

#[test]
fn builds_rrule_from_presets() {
    let hourly = build_automation_rrule(AutomationSchedulePreset::Hourly, 9, 15, None);
    assert_eq!(hourly, "FREQ=HOURLY;BYMINUTE=15");
    // The load-bearing quirk: hourly must NOT emit a BYHOUR field.
    assert!(
        !hourly.contains("BYHOUR"),
        "hourly RRULE must omit BYHOUR, got {hourly}"
    );

    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Daily, 9, 15, None),
        "FREQ=DAILY;BYHOUR=9;BYMINUTE=15"
    );
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Weekdays, 9, 15, None),
        "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=15"
    );
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Weekly, 9, 15, Some(0)),
        "FREQ=WEEKLY;BYDAY=SU;BYHOUR=9;BYMINUTE=15"
    );
}

// -------------------------------------------------------------------------------------
// Clamp: hour → [0,23], minute → [0,59], negatives → 0 (:528-529, :549-550).
// -------------------------------------------------------------------------------------

#[test]
fn build_clamps_out_of_range_hour_and_minute() {
    // Over-range clamps down.
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Daily, 25, 70, None),
        "FREQ=DAILY;BYHOUR=23;BYMINUTE=59"
    );
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Daily, 25, 70, None),
        "59 23 * * *"
    );
    // Negatives clamp up to 0.
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Daily, -5, -1, None),
        "FREQ=DAILY;BYHOUR=0;BYMINUTE=0"
    );
    // dayOfWeek clamps to [0,6]: 9 → 6 (SA), -1 → 0 (SU).
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Weekly, 9, 15, Some(9)),
        "FREQ=WEEKLY;BYDAY=SA;BYHOUR=9;BYMINUTE=15"
    );
    assert_eq!(
        build_automation_cron_schedule(AutomationSchedulePreset::Weekly, 9, 15, Some(-1)),
        "15 9 * * 0"
    );
    // Default dayOfWeek is 1 (Monday) when unspecified.
    assert_eq!(
        build_automation_rrule(AutomationSchedulePreset::Weekly, 9, 15, None),
        "FREQ=WEEKLY;BYDAY=MO;BYHOUR=9;BYMINUTE=15"
    );
}

// -------------------------------------------------------------------------------------
// parseRrule: defaults, range validation, FREQ whitelist (:78-89).
// -------------------------------------------------------------------------------------

#[test]
fn parse_rrule_defaults_and_ranges() {
    // Defaults: BYHOUR 9, BYMINUTE 0.
    let daily = parse_rrule("FREQ=DAILY").unwrap();
    assert_eq!(daily.freq, RruleFreq::Daily);
    assert_eq!(daily.by_hour, 9);
    assert_eq!(daily.by_minute, 0);

    // Out-of-range hour/minute.
    assert_eq!(
        parse_rrule("FREQ=DAILY;BYHOUR=24"),
        Err(RruleError::InvalidHour)
    );
    assert_eq!(
        parse_rrule("FREQ=DAILY;BYHOUR=-1"),
        Err(RruleError::InvalidHour)
    );
    assert_eq!(
        parse_rrule("FREQ=DAILY;BYMINUTE=60"),
        Err(RruleError::InvalidMinute)
    );
    // Non-integer (Number.isInteger).
    assert_eq!(
        parse_rrule("FREQ=DAILY;BYHOUR=9.5"),
        Err(RruleError::InvalidHour)
    );

    // Keys are uppercased; values are not.
    let lowered = parse_rrule("freq=DAILY;byhour=7").unwrap();
    assert_eq!(lowered.by_hour, 7);
}

// -------------------------------------------------------------------------------------
// FREQ whitelist — case 8's parse portion (:78-81).
// -------------------------------------------------------------------------------------

#[test]
fn unsupported_freq_is_rejected() {
    assert_eq!(
        parse_rrule("FREQ=YEARLY"),
        Err(RruleError::UnsupportedRecurrence)
    );
    assert_eq!(
        parse_rrule("FREQ=MONTHLY"),
        Err(RruleError::UnsupportedRecurrence)
    );
    // Missing FREQ entirely.
    assert_eq!(
        parse_rrule("BYHOUR=9"),
        Err(RruleError::UnsupportedRecurrence)
    );
    // And invalid at the schedule level (it contains '=' → RRULE path).
    assert!(!is_valid_automation_schedule("FREQ=YEARLY", anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// HOURLY/DAILY placeholder dayOfWeek = 1; weekdays detection (:290-298).
// -------------------------------------------------------------------------------------

#[test]
fn hourly_daily_weekdays_reverse_map() {
    let hourly = parse_automation_rrule("FREQ=HOURLY;BYMINUTE=5").unwrap();
    assert_eq!(hourly.preset, AutomationSchedulePreset::Hourly);
    assert_eq!(hourly.minute, 5);
    assert_eq!(hourly.day_of_week, 1, "hourly carries placeholder dow 1");

    let daily = parse_automation_rrule("FREQ=DAILY;BYHOUR=8;BYMINUTE=30").unwrap();
    assert_eq!(daily.preset, AutomationSchedulePreset::Daily);
    assert_eq!(daily.day_of_week, 1);

    // Exactly MO,TU,WE,TH,FR → weekdays.
    let weekdays =
        parse_automation_rrule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=9;BYMINUTE=0").unwrap();
    assert_eq!(weekdays.preset, AutomationSchedulePreset::Weekdays);
    assert_eq!(weekdays.day_of_week, 1);

    // A multi-day set that is NOT the exact weekday list, and length != 1 → InvalidDay.
    assert_eq!(
        parse_automation_rrule("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=0"),
        Err(RruleError::InvalidDay)
    );
}

// -------------------------------------------------------------------------------------
// Oracle case 20 (:213) — `=` dispatch: RRULE valid via schedule, rejected by cron-only.
// -------------------------------------------------------------------------------------

#[test]
fn daily_rrule_valid_via_schedule_but_rejected_by_cron_only_validator() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 9, 0, None);
    assert_eq!(rrule, "FREQ=DAILY;BYHOUR=9;BYMINUTE=0");
    // Schedule-level validator dispatches on '=' → RRULE path → valid.
    assert!(is_valid_automation_schedule(&rrule, anchor(), UTC));
    // Cron-only validator calls parse_cron_expression directly → 1 field ≠ 5 → invalid.
    assert!(!is_valid_automation_cron_schedule(&rrule, anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// F5 (plan §1) — `=` dispatch is LITERAL; cron-looking input with `=` routes to RRULE.
// -------------------------------------------------------------------------------------

#[test]
fn equals_dispatch_routes_cron_looking_input_to_rrule() {
    // `0 9 * * MON=1` contains '=' → RRULE parser (NOT cron). No FREQ → RRULE error.
    let err = parse_schedule("0 9 * * MON=1").unwrap_err();
    assert!(
        matches!(err, ScheduleError::Rrule(RruleError::UnsupportedRecurrence)),
        "'=' input must route to the RRULE parser, got {err:?}"
    );
    assert!(!is_valid_automation_schedule(
        "0 9 * * MON=1",
        anchor(),
        UTC
    ));

    // A pure cron string (no '=') routes to the cron parser.
    match parse_schedule("15 10 * * 1-5") {
        Ok(ParsedSchedule::Cron(_)) => {}
        other => panic!("cron string must route to the cron parser, got {other:?}"),
    }

    // FREQ=DAILY (no spaces) routes to RRULE and is valid.
    match parse_schedule("FREQ=DAILY") {
        Ok(ParsedSchedule::Rrule(_)) => {}
        other => panic!("FREQ=DAILY must route to the RRULE parser, got {other:?}"),
    }
}

#[test]
fn equals_dispatch_malformed_variants() {
    // `FREQ =DAILY`: the space is part of the KEY ("FREQ ") after split('='), so FREQ is
    // absent → invalid.
    assert!(!is_valid_automation_schedule("FREQ =DAILY", anchor(), UTC));
    assert_eq!(
        parse_rrule("FREQ =DAILY"),
        Err(RruleError::UnsupportedRecurrence)
    );

    // Double `==`: split('=') → ["FREQ", "", "DAILY"], value "" is falsy → entry skipped →
    // FREQ absent → invalid.
    assert_eq!(
        parse_rrule("FREQ==DAILY"),
        Err(RruleError::UnsupportedRecurrence)
    );

    // Multi-`=` where the first two segments form a valid pair: the rest is IGNORED (JS
    // destructuring `const [key, value] = part.split('=')`), so this is a valid daily rule.
    assert_eq!(
        parse_rrule("FREQ=DAILY=IGNORED").unwrap().freq,
        RruleFreq::Daily
    );
}

// -------------------------------------------------------------------------------------
// parse_schedule error wrapping — cron branch surfaces a Cron error, not an Rrule one.
// -------------------------------------------------------------------------------------

#[test]
fn parse_schedule_wraps_cron_errors_on_the_cron_branch() {
    // No '=' → cron branch; a bad field count is a ScheduleError::Cron.
    let err = parse_schedule("0 9 * *").unwrap_err();
    assert!(
        matches!(err, ScheduleError::Cron(_)),
        "cron-branch failure must be a Cron error, got {err:?}"
    );
}
