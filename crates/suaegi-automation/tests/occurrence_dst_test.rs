//! M3 DST suite — `America/Los_Angeles`, exercising forward AND backward scans across both
//! spring-forward (2026-03-08) and fall-back (2026-11-01), including candidates that land in
//! the NONEXISTENT hour (spring gap) and the REPEATED hour (fall fold).
//!
//! These tests DOCUMENT the port's behavior; they do not "fix" it (F3). Two policies are
//! pinned and asserted as the ACTUAL output this code produces, so any future change is caught:
//!
//!   * GAP (nonexistent local time, spring-forward): rolls FORWARD one hour, matching JS
//!     `Date.setHours` (02:30 → 03:30).
//!   * FOLD (ambiguous local time, fall-back): resolves to the EARLIER instant (`.earliest()`,
//!     i.e. still PDT/UTC-7), the same policy `start_of_local_day` already uses.
//!
//! The FIXED `DAY_MS` day stepping (F3, no re-floor to midnight) is directly observable: a
//! BACKWARD scan across spring-forward SKIPS 2026-03-08 entirely (see the last test), which is
//! exactly where a "cleaner" calendar-day stepping would diverge — that mutation is caught here.

use chrono::{Datelike, TimeZone, Timelike, Utc};
use chrono_tz::America::Los_Angeles as LA;
use suaegi_automation::{
    build_automation_rrule, latest_automation_occurrence_at_or_before,
    next_automation_occurrence_after, AutomationSchedulePreset,
};

/// An unambiguous instant expressed in UTC (Z), in epoch millis.
fn utc_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("valid UTC instant")
        .timestamp_millis()
}

/// A LOS ANGELES local wall-clock instant, in epoch millis. Only used for inputs at
/// unambiguous (non-transition) local times, so `.single()` is safe.
fn la(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    LA.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .single()
        .expect("unambiguous LA local instant")
        .timestamp_millis()
}

/// The LA civil (hour, minute) of an instant — for asserting the wall-clock the port lands on.
fn la_civil(ms: i64) -> (u32, u32) {
    let dt = LA.timestamp_millis_opt(ms).single().unwrap();
    (dt.hour(), dt.minute())
}

// =======================================================================================
// Spring-forward (2026-03-08): 02:00 → 03:00, the 02:00–02:59 local hour does NOT exist.
// =======================================================================================

/// FORWARD scan, candidate in the NONEXISTENT hour. Daily@02:30 with the next occurrence on
/// 2026-03-08 — 02:30 does not exist, so it rolls FORWARD to 03:30 PDT (= 10:30Z).
#[test]
fn spring_forward_next_daily_gap_rolls_forward() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 2, 30, None);
    let next = next_automation_occurrence_after(
        &rrule,
        la(2026, 3, 1, 0, 0),
        la(2026, 3, 7, 12, 0),
        LA,
    )
    .unwrap();
    // 03:30 PDT — the gap rolled the 02:30 candidate forward one hour.
    assert_eq!(next, utc_ms(2026, 3, 8, 10, 30));
    assert_eq!(la_civil(next), (3, 30), "02:30 rolled forward to 03:30");
}

/// BACKWARD scan, candidate in the NONEXISTENT hour. Daily@02:30, now 2026-03-08T12:00 →
/// the most-recent 02:30 is on 03-08, gap-rolled to 03:30 PDT (= 10:30Z).
#[test]
fn spring_forward_latest_daily_gap_rolls_forward() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 2, 30, None);
    let latest = latest_automation_occurrence_at_or_before(
        &rrule,
        la(2026, 3, 1, 0, 0),
        la(2026, 3, 8, 12, 0),
        LA,
    )
    .unwrap();
    assert_eq!(latest, Some(utc_ms(2026, 3, 8, 10, 30)));
    assert_eq!(la_civil(latest.unwrap()), (3, 30));
}

// =======================================================================================
// Fall-back (2026-11-01): 02:00 → 01:00, the 01:00–01:59 local hour occurs TWICE.
// =======================================================================================

/// FORWARD scan, candidate in the REPEATED hour. Daily@01:30 with the next occurrence on
/// 2026-11-01 — 01:30 is ambiguous, resolved to the EARLIER (PDT/UTC-7) instant = 08:30Z.
#[test]
fn fall_back_next_daily_fold_picks_earliest() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 1, 30, None);
    let next = next_automation_occurrence_after(
        &rrule,
        la(2026, 10, 1, 0, 0),
        la(2026, 10, 31, 12, 0),
        LA,
    )
    .unwrap();
    // 01:30 PDT (earlier), NOT 01:30 PST (which would be 09:30Z).
    assert_eq!(next, utc_ms(2026, 11, 1, 8, 30));
    assert_ne!(next, utc_ms(2026, 11, 1, 9, 30), "must be the EARLIER fold instant");
    assert_eq!(la_civil(next), (1, 30));
}

/// BACKWARD scan, candidate in the REPEATED hour. Daily@01:30, now 2026-11-01T12:00 → the
/// most-recent 01:30 resolves to the EARLIER (PDT) instant = 08:30Z.
#[test]
fn fall_back_latest_daily_fold_picks_earliest() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Daily, 1, 30, None);
    let latest = latest_automation_occurrence_at_or_before(
        &rrule,
        la(2026, 10, 1, 0, 0),
        la(2026, 11, 1, 12, 0),
        LA,
    )
    .unwrap();
    assert_eq!(latest, Some(utc_ms(2026, 11, 1, 8, 30)));
    assert_eq!(la_civil(latest.unwrap()), (1, 30));
}

// =======================================================================================
// FIXED DAY_MS drift (F3) made visible — the mutation detector for "fixed → calendar-day".
// =======================================================================================

/// A BACKWARD scan across spring-forward SKIPS 2026-03-08. With the fixed `DAY_MS` step, the
/// day pointer (00:00 PDT on 03-09, = 07:00Z) minus 24h lands at 03-08T07:00Z, which is BEFORE
/// the 03-08 10:00Z transition and so reads as 03-07 23:00 PST — the local date jumps straight
/// from 03-09 to 03-07, never visiting 03-08.
///
/// So a weekly Sunday@12:00 schedule, scanning back from Monday 2026-03-09T12:00, does NOT
/// find Sunday 2026-03-08; it finds the PREVIOUS Sunday, 2026-03-01T12:00 PST (= 20:00Z).
///
/// *Mutation:* switching the fixed `day += direction * DAY_MS` to calendar-day stepping would
/// visit 03-08 and return 2026-03-08T12:00 instead → this assertion FAILS. That is the DST
/// suite "detecting" the F3 stepping, as the plan requires.
#[test]
fn spring_forward_backward_fixed_day_ms_skips_march_8() {
    let rrule = build_automation_rrule(AutomationSchedulePreset::Weekly, 12, 0, Some(0)); // SU
    let latest = latest_automation_occurrence_at_or_before(
        &rrule,
        la(2026, 2, 1, 0, 0),        // dtstart well before, so a Some result is guaranteed
        la(2026, 3, 9, 12, 0),       // now = Monday after spring-forward
        LA,
    )
    .unwrap();

    // Sanity: both candidate days really are Sundays.
    assert_eq!(
        LA.timestamp_millis_opt(la(2026, 3, 8, 0, 0)).single().unwrap().weekday(),
        chrono::Weekday::Sun
    );
    assert_eq!(
        LA.timestamp_millis_opt(la(2026, 3, 1, 0, 0)).single().unwrap().weekday(),
        chrono::Weekday::Sun
    );

    // Fixed DAY_MS skipped 03-08 → the answer is 03-01, NOT 03-08.
    assert_eq!(latest, Some(utc_ms(2026, 3, 1, 20, 0)), "2026-03-01T12:00 PST");
    assert_ne!(
        latest,
        Some(la(2026, 3, 8, 12, 0)),
        "calendar-day stepping would (wrongly) return 03-08"
    );
    assert_eq!(la_civil(latest.unwrap()), (12, 0));
}
