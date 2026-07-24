//! M3 occurrence oracle — ported from Orca's `src/shared/automation-schedules.test.ts`
//! (cases 1, 2, 3, 10, 17, 21) plus the F7 cron-boundary pins (plan §1) that the JS oracle
//! never covers. All timestamp tests pin `Etc/UTC` (Codex F2) so civil dates are
//! host-independent and deterministic; the DST-sensitive behavior lives in
//! `occurrence_dst_test.rs`.
//!
//! Every test here is mutation-verifiable: it FAILS if the specific `<`/`<=` boundary,
//! `dtstart - 1` correction, or hourly-ignores-byHour quirk it guards is broken (no hollow
//! tests — this repo has shipped ≥5).

use chrono::{TimeZone, Timelike};
use chrono_tz::Etc::UTC;
use suaegi_automation::{
    build_automation_rrule, latest_automation_occurrence_at_or_before,
    next_automation_occurrence_after, AutomationSchedulePreset, OccurrenceError,
};

/// A UTC wall-clock instant in epoch milliseconds (minute resolution).
fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    UTC.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp_millis()
}

/// A UTC instant with seconds — needed for the off-the-minute F7 pins.
fn ms_s(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    UTC.with_ymd_and_hms(y, mo, d, h, mi, s)
        .single()
        .expect("valid UTC instant")
        .timestamp_millis()
}

// =======================================================================================
// Oracle cases (verbatim from the JS suite).
// =======================================================================================

/// Case 1 (:31) — `latest` hourly uses the current wall-clock minute; byHour is IGNORED.
/// Hourly@:00, dtstart 2026-05-12T00:00, now 2026-05-13T14:20 → 14:00 (NOT 09:00).
#[test]
fn oracle_1_latest_hourly_ignores_by_hour() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Hourly, 9, 0, None);
    let latest = latest_automation_occurrence_at_or_before(
        &rrule,
        ms(2026, 5, 12, 0, 0),
        ms(2026, 5, 13, 14, 20),
        UTC,
    )
    .unwrap();
    assert_eq!(latest, Some(ms(2026, 5, 13, 14, 0)));
}

/// Case 2 (:41) [off-the-minute] — a future hourly dtstart off the scheduled minute advances.
/// Hourly@:00, dtstart 2026-05-13T10:30, after 09:00 → 11:00.
#[test]
fn oracle_2_next_hourly_off_the_minute_advances() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Hourly, 9, 0, None);
    let next = next_automation_occurrence_after(
        &rrule,
        ms(2026, 5, 13, 10, 30),
        ms(2026, 5, 13, 9, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 13, 11, 0));
}

/// Case 3 (:51) — weekday schedules never return a weekend candidate.
/// Weekdays@09:30, dtstart 2026-05-01, after 2026-05-15T12:00 → Mon 2026-05-18T09:30.
#[test]
fn oracle_3_next_weekdays_excludes_weekend() {
    use chrono::Datelike;
    let rrule = build_automation_rrule(AutomationSchedulePreset::Weekdays, 9, 30, None);
    let next = next_automation_occurrence_after(
        &rrule,
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 18, 9, 30));
    let civil = UTC.timestamp_millis_opt(next).single().unwrap();
    assert_eq!(civil.weekday().num_days_from_sunday(), 1, "Monday");
    assert_eq!(civil.hour(), 9);
    assert_eq!(civil.minute(), 30);
}

/// Case 10 (:111) — custom cron, both directions. `15 10 * * 1-5`:
/// next(dtstart 05-01, after 05-15T12:00) → 2026-05-18T10:15 (Fri 12:00 → Mon 10:15);
/// latest → 2026-05-15T10:15 (Fri 10:15 < 12:00).
#[test]
fn oracle_10_cron_both_directions() {
    let next = next_automation_occurrence_after(
        "15 10 * * 1-5",
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 18, 10, 15));

    let latest = latest_automation_occurrence_at_or_before(
        "15 10 * * 1-5",
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(latest, Some(ms(2026, 5, 15, 10, 15)));
}

/// Case 17 (:192) — all-value DOM field (`*/1` = size 31) is unrestricted → AND with DOW →
/// Mondays only. `0 9 */1 * MON` next(after 05-15T12:00) → 2026-05-18T09:00.
#[test]
fn oracle_17_unrestricted_dom_and_dow_monday() {
    let next = next_automation_occurrence_after(
        "0 9 */1 * MON",
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 18, 9, 0));
}

/// Case 21 (:219) [leap-day] — `0 0 29 2 *` next(after 05-15T12:00, 2026) → 2028-02-29T00:00.
/// DOM 29 & Feb → AND; 2027 has no Feb 29, 2028 is a leap year. The ~9-year minute-scan
/// window finds it (a real long scan, but well within `CRON_SCAN_MINUTES`).
#[test]
fn oracle_21_leap_day() {
    let next = next_automation_occurrence_after(
        "0 0 29 2 *",
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2028, 2, 29, 0, 0));
}

// =======================================================================================
// F7 cron-boundary pins (plan §1) — the review-flagged oracle gap: the JS suite tests only
// the hourly off-minute path, NOT the cron double-correction (:571-580). These are the
// regression pins we ADD, each mutation-verified.
// =======================================================================================

/// (a) dtstart > after, dtstart off the minute → candidate ceils to the NEXT minute.
/// `* * * * *` (every minute), dtstart 12:00:30, after 11:00:00 → 2026-05-15T12:01:00.
#[test]
fn f7_a_dtstart_after_off_minute_ceils_up() {
    let next = next_automation_occurrence_after(
        "* * * * *",
        ms_s(2026, 5, 15, 12, 0, 30),
        ms(2026, 5, 15, 11, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 15, 12, 1));
}

/// (b) dtstart == after, exactly on a matching minute → the strict `<= after` SKIPS it and
/// returns the NEXT match. `0 12 * * *` daily, dtstart == after == 2026-05-15T12:00 →
/// 2026-05-16T12:00. *Mutation:* `candidate <= after` → `< after` returns 05-15T12:00 (FAIL).
#[test]
fn f7_b_dtstart_equals_after_on_match_skips_strictly() {
    let boundary = ms(2026, 5, 15, 12, 0);
    let next = next_automation_occurrence_after("0 12 * * *", boundary, boundary, UTC).unwrap();
    assert_eq!(next, ms(2026, 5, 16, 12, 0));
    assert_ne!(next, boundary, "the exact `after` minute must not be returned");
}

/// (c) dtstart < after, after off the minute → floor then `<= after` advances one minute.
/// `* * * * *`, dtstart 10:00:00, after 12:00:30 → 2026-05-15T12:01:00.
#[test]
fn f7_c_after_off_minute_floors_then_advances() {
    let next = next_automation_occurrence_after(
        "* * * * *",
        ms(2026, 5, 15, 10, 0),
        ms_s(2026, 5, 15, 12, 0, 30),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 15, 12, 1));
}

/// (d) after == dtstart - 1 with dtstart exactly matching → dtstart is ELIGIBLE (weekly/daily
/// path, proving the `dtstart - 1` correction). Daily@09:00, dtstart 2026-05-15T09:00,
/// after = dtstart - 1ms → returns dtstart. *Mutation:* dropping `dtstart - 1` makes the
/// anchor `dtstart`, the strict `>` skips it, and it returns 05-16T09:00 (FAIL).
#[test]
fn f7_d_dtstart_minus_one_keeps_dtstart_eligible() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 9, 0, None);
    let dtstart = ms(2026, 5, 15, 9, 0);
    let next = next_automation_occurrence_after(&rrule, dtstart, dtstart - 1, UTC).unwrap();
    assert_eq!(next, dtstart);
}

/// (e) the first ceiled minute does NOT match → the scan continues to the real next match.
/// `30 12 * * *` daily, dtstart 2026-05-15T00:00, after 12:00 → advances to 12:01 (no match),
/// scans on to 2026-05-15T12:30.
#[test]
fn f7_e_scan_continues_past_non_matching_ceil() {
    let next = next_automation_occurrence_after(
        "30 12 * * *",
        ms(2026, 5, 15, 0, 0),
        ms(2026, 5, 15, 12, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 15, 12, 30));
}

// =======================================================================================
// Additional strictness/None pins (mutation coverage the oracle cases leave open).
// =======================================================================================

/// Forward `scanDayCandidates` uses STRICT `>`: a candidate landing exactly on the anchor is
/// skipped. Daily@09:00, dtstart 05-10, after = 2026-05-12T09:00 (itself a candidate) →
/// 2026-05-13T09:00. *Mutation:* forward `>` → `>=` returns 05-12T09:00 (FAIL).
#[test]
fn forward_scan_is_strictly_after_anchor() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 9, 0, None);
    let next = next_automation_occurrence_after(
        &rrule,
        ms(2026, 5, 10, 9, 0),
        ms(2026, 5, 12, 9, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(next, ms(2026, 5, 13, 9, 0));
}

/// Backward `scanDayCandidates` is INCLUSIVE (`<= anchor`): a candidate exactly at `now` is
/// returned. Daily@09:00, now = 2026-05-13T09:00 → 2026-05-13T09:00 itself.
#[test]
fn backward_scan_is_inclusive_of_anchor() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 9, 0, None);
    let latest = latest_automation_occurrence_at_or_before(
        &rrule,
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 13, 9, 0),
        UTC,
    )
    .unwrap();
    assert_eq!(latest, Some(ms(2026, 5, 13, 9, 0)));
}

/// `latest` returns `None` (NOT an error, NOT a bogus 0) when `now < dtstart`, and this
/// happens BEFORE parsing — so even an unparseable schedule yields `None` here (:611).
#[test]
fn latest_before_dtstart_is_none_without_parsing() {
    let out = latest_automation_occurrence_at_or_before(
        "this is not a valid schedule",
        ms(2026, 5, 15, 0, 0),
        ms(2026, 5, 14, 0, 0), // now < dtstart
        UTC,
    )
    .unwrap();
    assert_eq!(out, None);
}

/// A parse error MUST propagate (transient ≠ false-negative), never silently become `None`.
/// Case 7 (:90): WEEKLY without BYDAY is `Invalid recurrence day.` — surfaced as an `Err`,
/// both for `next` and for `latest` once `now >= dtstart`.
#[test]
fn parse_error_propagates_not_swallowed() {
    let rrule = "FREQ=WEEKLY;BYHOUR=9;BYMINUTE=0";
    let next = next_automation_occurrence_after(rrule, ms(2026, 5, 1, 0, 0), ms(2026, 5, 2, 0, 0), UTC);
    assert!(matches!(next, Err(OccurrenceError::Schedule(_))));

    // now >= dtstart, so the parse actually runs and its error surfaces.
    let latest = latest_automation_occurrence_at_or_before(
        rrule,
        ms(2026, 5, 1, 0, 0),
        ms(2026, 5, 2, 0, 0),
        UTC,
    );
    assert!(matches!(latest, Err(OccurrenceError::Schedule(_))));
}

/// `latest` for a cron with no match at/above `dtstart` within range returns `None`.
/// `0 12 * * *` daily-noon, dtstart 2026-05-15T12:01 (just after today's noon), now
/// 2026-05-15T12:30 → the only ≤now noon is 12:00 < dtstart → None.
#[test]
fn latest_cron_none_when_no_match_at_or_after_dtstart() {
    let latest = latest_automation_occurrence_at_or_before(
        "0 12 * * *",
        ms(2026, 5, 15, 12, 1),
        ms(2026, 5, 15, 12, 30),
        UTC,
    )
    .unwrap();
    assert_eq!(latest, None);
}
