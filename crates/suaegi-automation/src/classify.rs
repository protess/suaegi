//! Schedule classification + human labels — verbatim port of the classify/format half of
//! Orca's `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0).
//!
//! Cited line numbers below (e.g. `:377-422`) refer to that source file. This is a faithful
//! port: quirks are preserved, not "fixed".
//!
//! Split, per the plan §2 M4 (research §6): the DETERMINISTIC core — the `kind` plus its
//! `minute`/`hour`/`day_of_week` — is TZ- and locale-INDEPENDENT, computed from set
//! cardinality. Only the human `label` is locale-dependent, and Orca's label is built with
//! `Intl.DateTimeFormat`. We DO NOT port `Intl`: the labels are HARDCODED ENGLISH mirroring
//! the `en-US` oracle (`ScheduleKind::label`). The single clock dependency —
//! `classifyParsedCronSchedule`'s `cronHasPossibleOccurrence(rule, Date.now())` check — takes
//! an explicit `now_ms` + `tz` (F2), never the ambient clock.

use chrono_tz::Tz;

use crate::cron::{
    cron_has_possible_occurrence, js_trim, parse_cron_expression, ParsedCron,
};
use crate::rrule::{
    parse_automation_rrule, parse_schedule, AutomationRruleParts, AutomationSchedulePreset,
    ParsedSchedule,
};

/// Deterministic classification of a cron schedule (`:35-41`, `:377-422`). Mirrors the TS
/// discriminated union WITHOUT the `label` field — the label is derived on demand by
/// [`ScheduleKind::label`], keeping the locale layer separate from the TZ/locale-independent
/// core (research §6). `day_of_week` uses JS `getDay()` numbering: 0 = Sunday .. 6 = Saturday.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleKind {
    /// Single minute, every hour, unrestricted calendar + day-of-week (`:392-397`).
    Hourly { minute: i64 },
    /// Single minute + hour, unrestricted calendar + day-of-week (`:401-402`).
    Daily { hour: i64, minute: i64 },
    /// Single minute + hour, days-of-week EXACTLY {1,2,3,4,5} (`:404-405`).
    Weekdays { hour: i64, minute: i64 },
    /// Single minute + hour + a single day-of-week (`:407-418`).
    Weekly {
        hour: i64,
        minute: i64,
        day_of_week: i64,
    },
    /// Valid but not one of the recognized presets (`:421`).
    Custom,
    /// No possible occurrence within the scan window (`:378-380`).
    Invalid,
}

/// Weekday names anchored so index 0 = Sunday (`:371`, `:409`: Orca formats
/// `new Date(2026, 0, 4 + dayOfWeek)` and 2026-01-04 is a Sunday). HARDCODED ENGLISH — the
/// `en-US` `Intl.DateTimeFormat(weekday: 'long')` output — never a locale formatter.
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

impl ScheduleKind {
    /// The human label (`:379`, `:396`, `:400-417`, `:421`). HARDCODED ENGLISH mirroring the
    /// `en-US` oracle — NO `Intl`. Hourly/Custom/Invalid labels are locale-independent
    /// (`padStart` / literals); Daily/Weekdays/Weekly embed [`format_time`]'s 12-hour clock.
    pub fn label(&self) -> String {
        match self {
            ScheduleKind::Hourly { minute } => format!("Hourly at :{minute:02}"),
            ScheduleKind::Daily { hour, minute } => {
                format!("Daily at {}", format_time(*hour, *minute))
            }
            ScheduleKind::Weekdays { hour, minute } => {
                format!("Weekdays at {}", format_time(*hour, *minute))
            }
            ScheduleKind::Weekly {
                hour,
                minute,
                day_of_week,
            } => {
                // `day_of_week` is validated 0..=6 by the classifier (a single set value from a
                // field bounded [0,7] then 7→0-normalized), so this index is always in range.
                let day = WEEKDAY_NAMES[(*day_of_week).rem_euclid(7) as usize];
                format!("{day}s at {}", format_time(*hour, *minute))
            }
            ScheduleKind::Custom => "Custom schedule".to_string(),
            ScheduleKind::Invalid => "Invalid schedule".to_string(),
        }
    }
}

/// `formatTime` (`:325-332`): `Intl.DateTimeFormat(undefined, {hour: 'numeric', minute:
/// '2-digit'})`. HARDCODED to the `en-US` 12-hour output, byte-for-byte with the oracle's
/// `formatTimeForTest`: `{h 1-12}:{mm}` + a single ASCII space + `AM`/`PM`. (Node's ICU on this
/// host emits a plain U+0020 here; see the deviations note in the port report.) The `Date`'s
/// calendar day is irrelevant to this field, so no clock/TZ is read.
pub fn format_time(hour: i64, minute: i64) -> String {
    let h24 = hour.rem_euclid(24);
    let period = if h24 < 12 { "AM" } else { "PM" };
    let h12 = match h24 % 12 {
        0 => 12,
        h => h,
    };
    format!("{h12}:{minute:02} {period}")
}

/// `getSingleSetValue` (`:334-339`): the sole element if the set has exactly one, else `None`.
fn get_single_set_value(values: &std::collections::HashSet<i64>) -> Option<i64> {
    if values.len() != 1 {
        return None;
    }
    values.iter().copied().next()
}

/// `setContainsExactly` (`:341-346`): the set is exactly `expected` (same length, all present).
fn set_contains_exactly(values: &std::collections::HashSet<i64>, expected: &[i64]) -> bool {
    if values.len() != expected.len() {
        return false;
    }
    expected.iter().all(|v| values.contains(v))
}

/// `setContainsRange` (`:348-358`): the set is exactly the inclusive integer range `[min, max]`.
fn set_contains_range(values: &std::collections::HashSet<i64>, min: i64, max: i64) -> bool {
    if values.len() as i64 != max - min + 1 {
        return false;
    }
    (min..=max).all(|v| values.contains(&v))
}

/// `classifyParsedCronSchedule` (`:377-422`). Occurrence-impossible → `Invalid` FIRST. Then
/// hourly (single minute, full 0-23 hours, unrestricted calendar + DOW); then, requiring a
/// single minute AND hour AND unrestricted calendar: daily (unrestricted DOW), weekdays
/// (DOW == {1,2,3,4,5}), weekly (single DOW). Everything else is `Custom`. F2: `now_ms` + `tz`
/// are explicit (the source read `Date.now()` for the possibility check).
pub fn classify_parsed_cron_schedule(rule: &ParsedCron, now_ms: i64, tz: Tz) -> ScheduleKind {
    if !cron_has_possible_occurrence(rule, now_ms, tz) {
        return ScheduleKind::Invalid;
    }
    let minute = get_single_set_value(&rule.minutes);
    let hour = get_single_set_value(&rule.hours);
    let unrestricted_day_of_month = !rule.day_of_month_restricted;
    let unrestricted_month = set_contains_range(&rule.months, 1, 12);
    let unrestricted_day_of_week = !rule.day_of_week_restricted;
    let unrestricted_calendar = unrestricted_day_of_month && unrestricted_month;

    if let Some(minute) = minute {
        if set_contains_range(&rule.hours, 0, 23)
            && unrestricted_calendar
            && unrestricted_day_of_week
        {
            return ScheduleKind::Hourly { minute };
        }
    }

    if let (Some(minute), Some(hour)) = (minute, hour) {
        if unrestricted_calendar {
            if unrestricted_day_of_week {
                return ScheduleKind::Daily { hour, minute };
            }
            if set_contains_exactly(&rule.days_of_week, &[1, 2, 3, 4, 5]) {
                return ScheduleKind::Weekdays { hour, minute };
            }
            if let Some(day_of_week) = get_single_set_value(&rule.days_of_week) {
                return ScheduleKind::Weekly {
                    hour,
                    minute,
                    day_of_week,
                };
            }
        }
    }

    ScheduleKind::Custom
}

/// `classifyAutomationCronSchedule` (`:424-432`): trim, parse as a cron expression DIRECTLY
/// (no `=` dispatch), classify. A parse error → `Invalid` (but internally it is an `Err`,
/// never silently valid). F2: `now_ms` + `tz` explicit.
pub fn classify_automation_cron_schedule(schedule: &str, now_ms: i64, tz: Tz) -> ScheduleKind {
    match parse_cron_expression(js_trim(schedule)) {
        Ok(rule) => classify_parsed_cron_schedule(&rule, now_ms, tz),
        Err(_) => ScheduleKind::Invalid,
    }
}

/// `formatParsedRruleSchedule` (`:360-375`): the RRULE branch of the human label. Hourly uses
/// the stored minute; the others embed [`format_time`]; weekly's weekday name is anchored so
/// index 0 = Sunday (Sunday is PRESERVED, never coerced).
fn format_parsed_rrule_schedule(schedule: &AutomationRruleParts) -> String {
    match schedule.preset {
        AutomationSchedulePreset::Hourly => format!("Hourly at :{:02}", schedule.minute),
        AutomationSchedulePreset::Daily => {
            format!("Daily at {}", format_time(schedule.hour, schedule.minute))
        }
        AutomationSchedulePreset::Weekdays => {
            format!("Weekdays at {}", format_time(schedule.hour, schedule.minute))
        }
        // `weekly` (and the `custom` union arm, unreachable from parse_automation_rrule).
        AutomationSchedulePreset::Weekly | AutomationSchedulePreset::Custom => {
            let day = WEEKDAY_NAMES[schedule.day_of_week.rem_euclid(7) as usize];
            format!("{day}s at {}", format_time(schedule.hour, schedule.minute))
        }
    }
}

/// `formatAutomationSchedule` (`:434-445`): the human label for ANY schedule string. `=`
/// dispatch chooses cron vs RRULE (F5); the cron path classifies and takes its label, the
/// RRULE path re-parses via [`parse_automation_rrule`]. Any parse error → `'Invalid schedule'`.
/// F2: `now_ms` + `tz` explicit (the cron path's classify reads them).
pub fn format_automation_schedule(schedule: &str, now_ms: i64, tz: Tz) -> String {
    let trimmed = js_trim(schedule);
    match parse_schedule(trimmed) {
        Ok(ParsedSchedule::Cron(rule)) => classify_parsed_cron_schedule(&rule, now_ms, tz).label(),
        // Orca re-parses the trimmed string with `parseAutomationRrule` here (`:441`), which
        // can throw for RRULEs that `parseSchedule` accepted but the preset mapper rejects
        // (e.g. multi-code non-weekday BYDAY) → caught → 'Invalid schedule'.
        Ok(ParsedSchedule::Rrule(_)) => match parse_automation_rrule(trimmed) {
            Ok(parts) => format_parsed_rrule_schedule(&parts),
            Err(_) => "Invalid schedule".to_string(),
        },
        Err(_) => "Invalid schedule".to_string(),
    }
}
