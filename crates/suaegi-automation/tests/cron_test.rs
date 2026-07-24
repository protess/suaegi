//! M1 cron oracle — ported from Orca's `src/shared/automation-schedules.test.ts`
//! plus the F1 acceptance matrix from the plan §1. All timestamp-based tests pin
//! `Etc/UTC` (Codex F2) so civil dates are host-independent and deterministic.
//!
//! Every test here is written to be mutation-verifiable: it FAILS if the specific logic
//! it guards is broken (no hollow tests — this repo has shipped ≥5).

use chrono::TimeZone;
use chrono_tz::Etc::UTC;
use std::collections::HashSet;
use suaegi_automation::{
    cron_date_matches, cron_matches, get_automation_cron_expression_fields,
    is_valid_automation_cron_schedule, parse_cron_expression, AUTOMATION_CRON_EXPRESSION_MAX_BYTES,
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

fn set(values: &[i64]) -> HashSet<i64> {
    values.iter().copied().collect()
}

// -------------------------------------------------------------------------------------
// Sanity: the calendar assumptions the OR/AND tests lean on.
// -------------------------------------------------------------------------------------

#[test]
fn calendar_assumptions_hold() {
    use chrono::Datelike;
    // 0 = Sunday .. 6 = Saturday, matching JS getDay().
    let dow = |y, mo, d| {
        UTC.with_ymd_and_hms(y, mo, d, 0, 0, 0)
            .single()
            .unwrap()
            .weekday()
            .num_days_from_sunday()
    };
    assert_eq!(dow(2026, 5, 1), 5, "2026-05-01 is a Friday");
    assert_eq!(dow(2026, 5, 18), 1, "2026-05-18 is a Monday");
    assert_eq!(dow(2026, 5, 19), 2, "2026-05-19 is a Tuesday");
    assert_eq!(dow(2028, 2, 29), 2, "2028-02-29 is a Tuesday (leap day)");
}

// -------------------------------------------------------------------------------------
// Oracle case 13 (:149) — regex-free tokenizer over the Unicode whitespace set.
// -------------------------------------------------------------------------------------

#[test]
fn tokenizer_splits_on_unicode_whitespace() {
    // "15" + NBSP(U+00A0) + "10\n*\t*\rMON-FRI"
    let expr = "15\u{00A0}10\n*\t*\rMON-FRI";
    let fields = get_automation_cron_expression_fields(expr, 5);
    assert_eq!(fields, vec!["15", "10", "*", "*", "MON-FRI"]);
    // And it parses to a valid weekdays schedule.
    assert!(is_valid_automation_cron_schedule(expr, anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// Oracle case 14 (:160) — oversized-paste byte guard runs BEFORE tokenizing (F4).
// -------------------------------------------------------------------------------------

#[test]
fn oversized_expression_is_rejected_before_tokenizing() {
    let huge = "secret-cron-field ".repeat(2048); // 18 * 2048 = 36864 bytes
    assert!(huge.len() > AUTOMATION_CRON_EXPRESSION_MAX_BYTES);
    // Byte guard returns empty — the tokenizer never runs.
    assert!(get_automation_cron_expression_fields(&huge, 5).is_empty());
    assert!(!is_valid_automation_cron_schedule(&huge, anchor(), UTC));
}

#[test]
fn byte_guard_boundary_is_utf8_len() {
    // F4: `s.len() > 2048` rejects; `== 2048` accepts. Pad a valid cron with spaces.
    let base = "0 0 * * *"; // 9 bytes → 5 fields
    let at_limit = format!("{base}{}", " ".repeat(AUTOMATION_CRON_EXPRESSION_MAX_BYTES - base.len()));
    assert_eq!(at_limit.len(), AUTOMATION_CRON_EXPRESSION_MAX_BYTES);
    assert_eq!(
        get_automation_cron_expression_fields(&at_limit, 5),
        vec!["0", "0", "*", "*", "*"],
        "exactly 2048 bytes must still tokenize"
    );

    let over_limit = format!("{at_limit} ");
    assert_eq!(over_limit.len(), AUTOMATION_CRON_EXPRESSION_MAX_BYTES + 1);
    assert!(
        get_automation_cron_expression_fields(&over_limit, 5).is_empty(),
        "2049 bytes must be rejected"
    );
}

// -------------------------------------------------------------------------------------
// Oracle case 18 (:203) — malformed separators.
// -------------------------------------------------------------------------------------

#[test]
fn malformed_separators_are_invalid() {
    // Two steps in one field (`*/15/2`) → stepParts.length > 2.
    assert!(parse_cron_expression("*/15/2 9 * * *").is_err());
    assert!(!is_valid_automation_cron_schedule("*/15/2 9 * * *", anchor(), UTC));
    // Empty range end (`1--5`).
    assert!(parse_cron_expression("0 9 1--5 * *").is_err());
    assert!(!is_valid_automation_cron_schedule("0 9 1--5 * *", anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// Oracle case 19 (:208) — no possible run: DOM31 && Feb, AND, never matches.
// -------------------------------------------------------------------------------------

#[test]
fn dom31_in_february_never_runs() {
    // Parses fine, but the 3294-day scan finds no match.
    assert!(parse_cron_expression("0 0 31 2 *").is_ok());
    assert!(!is_valid_automation_cron_schedule("0 0 31 2 *", anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// Oracle cases 16/17 — DOM/DOW OR (both restricted) vs AND (either unrestricted).
// -------------------------------------------------------------------------------------

#[test]
fn both_restricted_day_fields_use_or() {
    // `0 9 1 * MON`: DOM {1} restricted, DOW {1} restricted → OR.
    let rule = parse_cron_expression("0 9 1 * MON").unwrap();
    assert!(rule.day_of_month_restricted && rule.day_of_week_restricted);

    // The 1st-but-not-Monday matches (via DOM). 2026-05-01 is a Friday.
    assert!(cron_date_matches(&rule, ms(2026, 5, 1, 9, 0), UTC));
    // A Monday-not-the-1st matches (via DOW). 2026-05-18 is a Monday.
    assert!(cron_date_matches(&rule, ms(2026, 5, 18, 9, 0), UTC));
    // Neither the 1st nor a Monday → no match. 2026-05-19 is a Tuesday.
    assert!(!cron_date_matches(&rule, ms(2026, 5, 19, 9, 0), UTC));
}

#[test]
fn unrestricted_dom_uses_and_monday_only() {
    // `0 9 */1 * MON`: DOM {1..31} size 31 → unrestricted → AND → Mondays only.
    let rule = parse_cron_expression("0 9 */1 * MON").unwrap();
    assert!(!rule.day_of_month_restricted);
    assert!(rule.day_of_week_restricted);

    // Monday matches.
    assert!(cron_date_matches(&rule, ms(2026, 5, 18, 9, 0), UTC));
    // The 1st (a Friday) does NOT match — this is the OR→AND discriminator.
    assert!(!cron_date_matches(&rule, ms(2026, 5, 1, 9, 0), UTC));
    // Another non-Monday does not match.
    assert!(!cron_date_matches(&rule, ms(2026, 5, 19, 9, 0), UTC));
}

#[test]
fn full_dow_ranges_are_unrestricted_and_valid() {
    // `0 9 * * 0-7` and `0 9 * * 1-7` both fold to the full 7-element DOW set.
    for expr in ["0 9 * * 0-7", "0 9 * * 1-7"] {
        let rule = parse_cron_expression(expr).unwrap();
        assert!(!rule.day_of_week_restricted, "{expr} should be unrestricted");
        assert!(is_valid_automation_cron_schedule(expr, anchor(), UTC), "{expr} should be valid");
    }
}

// -------------------------------------------------------------------------------------
// F1 acceptance matrix (plan §1).
// -------------------------------------------------------------------------------------

#[test]
fn f1_dom_syntaxes_that_collapse_to_unrestricted() {
    let full_dom_list = (1..=31).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let full_dom_expr = format!("0 9 {full_dom_list} * MON");
    for expr in [
        "0 9 */1 * MON".to_string(),
        "0 9 1-31 * MON".to_string(),
        full_dom_expr,
    ] {
        let rule = parse_cron_expression(&expr).unwrap();
        assert!(!rule.day_of_month_restricted, "{expr}: DOM must be unrestricted");
        assert!(rule.day_of_week_restricted, "{expr}: DOW MON must be restricted");
        // Unrestricted DOM → AND → Monday only.
        assert!(cron_date_matches(&rule, ms(2026, 5, 18, 9, 0), UTC), "{expr}: Monday matches");
        assert!(!cron_date_matches(&rule, ms(2026, 5, 1, 9, 0), UTC), "{expr}: Friday-1st excluded");
    }
}

#[test]
fn f1_dow_syntaxes_that_collapse_to_unrestricted() {
    let full_dow_list = "0,1,2,3,4,5,6";
    for expr in ["0 9 * * 0-7", "0 9 * * 1-7", &format!("0 9 * * {full_dow_list}")] {
        let rule = parse_cron_expression(expr).unwrap();
        assert!(!rule.day_of_week_restricted, "{expr}: DOW must be unrestricted");
    }
}

#[test]
fn f1_sunday_seven_normalizes_before_cardinality() {
    // `7` folds to `0` (:199), so `1-7` yields exactly {0,1,2,3,4,5,6}, size 7.
    let rule = parse_cron_expression("0 9 * * 1-7").unwrap();
    assert_eq!(rule.days_of_week, set(&[0, 1, 2, 3, 4, 5, 6]));
    assert!(!rule.day_of_week_restricted);

    // A bare `7` is Sunday (0), a single restricted value.
    let sun = parse_cron_expression("0 9 * * 7").unwrap();
    assert_eq!(sun.days_of_week, set(&[0]));
    assert!(sun.day_of_week_restricted);
}

#[test]
fn f1_requires_exactly_five_fields() {
    assert_eq!(parse_cron_expression("0 9 * *"), Err(suaegi_automation::CronError::WrongFieldCount));
    assert_eq!(
        parse_cron_expression("0 9 * * * *"),
        Err(suaegi_automation::CronError::WrongFieldCount)
    );
    assert!(parse_cron_expression("0 9 * * *").is_ok());
}

// -------------------------------------------------------------------------------------
// Leap-day VALIDITY (subset of oracle case 21) — the exact next-occurrence ts is M3.
// -------------------------------------------------------------------------------------

#[test]
fn leap_day_schedule_is_valid() {
    // `0 0 29 2 *`: DOM {29} restricted, DOW unrestricted → AND → Feb 29. There IS a
    // Feb 29 within the 9-year scan window (2028), so it is valid.
    assert!(is_valid_automation_cron_schedule("0 0 29 2 *", anchor(), UTC));
}

// -------------------------------------------------------------------------------------
// Name tables.
// -------------------------------------------------------------------------------------

#[test]
fn name_tables_parse_correctly() {
    // MON-FRI → {1,2,3,4,5}.
    let weekdays = parse_cron_expression("0 0 * * MON-FRI").unwrap();
    assert_eq!(weekdays.days_of_week, set(&[1, 2, 3, 4, 5]));

    // Month names: MAR → 3 in the month field.
    let march = parse_cron_expression("0 0 1 MAR *").unwrap();
    assert_eq!(march.months, set(&[3]));

    // Both 2-letter and 3-letter day codes resolve (SU/SUN = 0, SA/SAT = 6).
    assert_eq!(parse_cron_expression("0 0 * * SU").unwrap().days_of_week, set(&[0]));
    assert_eq!(parse_cron_expression("0 0 * * SUN").unwrap().days_of_week, set(&[0]));
    assert_eq!(parse_cron_expression("0 0 * * SAT").unwrap().days_of_week, set(&[6]));
}

// -------------------------------------------------------------------------------------
// cron_matches — hour/minute set membership (guards the .has(hour) && .has(minute) gate).
// -------------------------------------------------------------------------------------

#[test]
fn cron_matches_checks_hour_and_minute() {
    let rule = parse_cron_expression("15 10 * * *").unwrap();
    assert!(cron_matches(&rule, ms(2026, 5, 15, 10, 15), UTC));
    // Wrong minute.
    assert!(!cron_matches(&rule, ms(2026, 5, 15, 10, 16), UTC));
    // Wrong hour.
    assert!(!cron_matches(&rule, ms(2026, 5, 15, 11, 15), UTC));
}

// -------------------------------------------------------------------------------------
// Field-parser edge cases (step validation, range bounds, empty parts).
// -------------------------------------------------------------------------------------

#[test]
fn field_parser_rejects_bad_steps_and_ranges() {
    // Step must be a positive integer.
    assert!(parse_cron_expression("*/0 9 * * *").is_err(), "step 0 rejected");
    assert!(parse_cron_expression("5/ 9 * * *").is_err(), "empty step rejected");
    // Out-of-range single value.
    assert!(parse_cron_expression("60 9 * * *").is_err(), "minute 60 out of range");
    assert!(parse_cron_expression("0 24 * * *").is_err(), "hour 24 out of range");
    assert!(parse_cron_expression("0 9 0 * *").is_err(), "day-of-month 0 out of range");
    // start > end.
    assert!(parse_cron_expression("0 9 * * 5-1").is_err(), "reversed range rejected");
    // Empty list part.
    assert!(parse_cron_expression("0 9 1,,3 * *").is_err(), "empty list part rejected");
}

#[test]
fn valid_step_expands_correctly() {
    // `*/15` on minutes → {0,15,30,45}.
    let rule = parse_cron_expression("*/15 * * * *").unwrap();
    assert_eq!(rule.minutes, set(&[0, 15, 30, 45]));
    // A stepped range `10-20/5` → {10,15,20}.
    let stepped = parse_cron_expression("10-20/5 * * * *").unwrap();
    assert_eq!(stepped.minutes, set(&[10, 15, 20]));
}
