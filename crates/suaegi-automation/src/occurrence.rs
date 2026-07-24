//! Occurrence math — verbatim port of the next/latest-occurrence half of Orca's
//! `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0).
//!
//! Cited line numbers below (e.g. `:564-604`) refer to that source file. This is THE risky
//! milestone: every `<` vs `<=` boundary, the `dtstart - 1` forward correction, the fixed
//! `DAY_MS` scan stepping (F3), and the hourly-ignores-byHour quirk are preserved verbatim —
//! not "fixed".
//!
//! F2: every civil↔instant conversion takes an explicit `tz: Tz` (never ambient Local). All
//! wall-clock helpers reuse `cron::civil` / `cron::resolve_local`, so the DST fold/gap policy
//! (fold → `.earliest()`; gap → roll one hour FORWARD, matching JS `Date.setHours`) is
//! IDENTICAL to `start_of_local_day`. Exact JS-`Date` fold parity is a deferred open question
//! (plan §3); the DST test suite asserts the ACTUAL behavior this code produces.

use chrono::{Datelike, Timelike};
use chrono_tz::Tz;

use crate::cron::{
    civil, cron_matches, resolve_local, start_of_local_day, CRON_SCAN_DAYS, DAY_CODES, DAY_MS,
};
use crate::rrule::{parse_schedule, ParsedRrule, ParsedSchedule, RruleFreq, ScheduleError};

/// 60_000 ms (`:8`). Not present in `cron.rs` (M1 needed only `DAY_MS`), so defined here.
const MINUTE_MS: i64 = 60 * 1000;

/// 3_600_000 ms (`:7`).
const HOUR_MS: i64 = 60 * 60 * 1000;

/// `CRON_SCAN_MINUTES = CRON_SCAN_DAYS * 24 * 60 = 4_743_360` (`:11`). Derived from the M1
/// `CRON_SCAN_DAYS` constant rather than redefined, so the two scan windows can never drift.
const CRON_SCAN_MINUTES: usize = CRON_SCAN_DAYS * 24 * 60;

/// Occurrence-computation error. A parse failure (invalid schedule) MUST propagate as
/// [`OccurrenceError::Schedule`] — it must never silently read as "no occurrence" (transient ≠
/// false-negative). A syntactically valid schedule whose next run cannot be found within the
/// scan window is the DISTINCT [`OccurrenceError::Unable`] — mirroring Orca's
/// `'Unable to compute next automation run.'` throw (`:587`, `:601`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceError {
    /// The schedule string itself failed to parse (RRULE or cron).
    Schedule(ScheduleError),
    /// `Unable to compute next automation run.` (`:587`, `:601`).
    Unable,
}

impl std::fmt::Display for OccurrenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OccurrenceError::Schedule(e) => write!(f, "{e}"),
            OccurrenceError::Unable => write!(f, "Unable to compute next automation run."),
        }
    }
}

impl std::error::Error for OccurrenceError {}

impl From<ScheduleError> for OccurrenceError {
    fn from(e: ScheduleError) -> Self {
        OccurrenceError::Schedule(e)
    }
}

// ---------------------------------------------------------------------------------------
// Local wall-clock helpers (F2 timezone injected; reuse cron's DST policy).
// ---------------------------------------------------------------------------------------

/// `floorToMinute` (`:484-488`): `date.setSeconds(0, 0)` — zero the seconds/millis of the
/// local wall-clock minute. The instant stays within the same civil minute (which is always a
/// valid time), so the resolve never hits a gap; a fold is resolved by the shared
/// `.earliest()` policy.
fn floor_to_minute(ms: i64, tz: Tz) -> i64 {
    let dt = civil(tz, ms);
    let naive = dt
        .date_naive()
        .and_hms_opt(dt.hour(), dt.minute(), 0)
        .expect("floored minute is a valid time");
    resolve_local(tz, naive).timestamp_millis()
}

/// `atLocalTime` (`:447-451`): take the LOCAL DATE that `day_ms` falls on, then
/// `setHours(hour, minute, 0, 0)`. Because it re-reads the local date of `day_ms` (which the
/// fixed-`DAY_MS` scan can drift off midnight, F3), the returned instant is anchored to that
/// day's calendar date, exactly like JS `Date.setHours`. A nonexistent local time (spring
/// gap) rolls FORWARD via `resolve_local`; an ambiguous one (fall fold) resolves to
/// `.earliest()`.
fn at_local_time(day_ms: i64, hour: i64, minute: i64, tz: Tz) -> i64 {
    let date = civil(tz, day_ms).date_naive();
    let naive = date
        .and_hms_opt(hour as u32, minute as u32, 0)
        .expect("hour 0..=23 / minute 0..=59 form a valid time");
    resolve_local(tz, naive).timestamp_millis()
}

/// `date.setMinutes(byMinute, 0, 0)` for the HOURLY path (`:592`, `:627`): keep the local
/// HOUR (byHour is IGNORED), set minute + zero seconds/millis. The date and hour are unchanged.
fn set_local_minute(ms: i64, minute: i64, tz: Tz) -> i64 {
    let dt = civil(tz, ms);
    let naive = dt
        .date_naive()
        .and_hms_opt(dt.hour(), minute as u32, 0)
        .expect("hour / minute 0..=59 form a valid time");
    resolve_local(tz, naive).timestamp_millis()
}

/// `dayMatches` (`:459-465`): DAILY matches every day; otherwise the local weekday code
/// (JS `getDay()`: 0 = Sunday .. 6 = Saturday) must be in the RRULE's BYDAY set.
fn day_matches(rule: &ParsedRrule, ms: i64, tz: Tz) -> bool {
    if rule.freq == RruleFreq::Daily {
        return true;
    }
    let index = civil(tz, ms).weekday().num_days_from_sunday() as usize;
    let code = DAY_CODES[index];
    rule.by_day.iter().any(|d| d.as_str() == code)
}

/// `scanDayCandidates` (`:467-482`): walk up to 370 days from the local start-of-day of
/// `anchor_ms`, building each day's `atLocalTime(byHour, byMinute)` candidate. Forward
/// (`direction == 1`) returns the first candidate STRICTLY after the anchor; backward
/// (`direction == -1`) returns the first candidate AT-OR-BEFORE the anchor (inclusive). The
/// day pointer steps by the FIXED `direction * DAY_MS` with NO re-floor (F3, bug-compatible).
fn scan_day_candidates(rule: &ParsedRrule, anchor_ms: i64, direction: i64, tz: Tz) -> Option<i64> {
    let mut day = start_of_local_day(anchor_ms, tz);
    for _ in 0..370 {
        let candidate = at_local_time(day, rule.by_hour, rule.by_minute, tz);
        if day_matches(rule, candidate, tz) {
            if direction == 1 && candidate > anchor_ms {
                return Some(candidate);
            }
            if direction == -1 && candidate <= anchor_ms {
                return Some(candidate);
            }
        }
        day += direction * DAY_MS;
    }
    None
}

// ---------------------------------------------------------------------------------------
// Public occurrence math.
// ---------------------------------------------------------------------------------------

/// `nextAutomationOccurrenceAfter` (`:564-604`): the next run STRICTLY after `after_ms`,
/// no earlier than `dtstart_ms`. Dispatched via `parse_schedule` (`=` → RRULE, else cron);
/// a parse error propagates as [`OccurrenceError::Schedule`].
///
/// - **CRON** (`:570-588`): floor `max(dtstart, after)` to the minute; advance one minute if
///   `<= after` (strict); if still `< dtstart`, re-floor `dtstart` and advance if off-minute;
///   then minute-scan for a match, else [`OccurrenceError::Unable`].
/// - **HOURLY** (`:589-597`): byHour is IGNORED — set the minute on `max(dtstart, after)`,
///   then add a fixed `HOUR_MS` if `<= after` OR `< dtstart`.
/// - **WEEKLY/DAILY** (`:599-603`): forward `scanDayCandidates` from `max(dtstart - 1, after)`
///   — the `dtstart - 1` correction compensates the strict `>` so a candidate landing exactly
///   on `dtstart` stays eligible.
pub fn next_automation_occurrence_after(
    schedule: &str,
    dtstart_ms: i64,
    after_ms: i64,
    tz: Tz,
) -> Result<i64, OccurrenceError> {
    match parse_schedule(schedule)? {
        ParsedSchedule::Cron(rule) => {
            let mut candidate = floor_to_minute(dtstart_ms.max(after_ms), tz);
            if candidate <= after_ms {
                candidate += MINUTE_MS;
            }
            if candidate < dtstart_ms {
                candidate = floor_to_minute(dtstart_ms, tz);
                if candidate < dtstart_ms {
                    candidate += MINUTE_MS;
                }
            }
            for _ in 0..CRON_SCAN_MINUTES {
                if cron_matches(&rule, candidate, tz) {
                    return Ok(candidate);
                }
                candidate += MINUTE_MS;
            }
            Err(OccurrenceError::Unable)
        }
        ParsedSchedule::Rrule(rule) => {
            if rule.freq == RruleFreq::Hourly {
                // byHour IGNORED: only the minute is pinned.
                let start = dtstart_ms.max(after_ms);
                let mut candidate = set_local_minute(start, rule.by_minute, tz);
                if candidate <= after_ms || candidate < dtstart_ms {
                    candidate += HOUR_MS;
                }
                return Ok(candidate);
            }
            // WEEKLY/DAILY: the `dtstart - 1` correction (crux).
            match scan_day_candidates(&rule, (dtstart_ms - 1).max(after_ms), 1, tz) {
                Some(candidate) => Ok(candidate),
                None => Err(OccurrenceError::Unable),
            }
        }
    }
}

/// `latestAutomationOccurrenceAtOrBefore` (`:606-636`): the most recent run AT OR BEFORE
/// `now_ms`, no earlier than `dtstart_ms`; `None` if there is none.
///
/// `now < dtstart` returns `Ok(None)` BEFORE parsing (`:611`), matching Orca — so an invalid
/// schedule with `now < dtstart` yields `None`, not an error. Once `now >= dtstart`, a parse
/// error propagates.
///
/// - **CRON** (`:615-624`): floor `now` to the minute, minute-scan BACKWARD while `>= dtstart`.
/// - **HOURLY** (`:625-633`): byHour IGNORED — set the minute on `now`, subtract a fixed
///   `HOUR_MS` if it overshot `now`; `Some` iff `>= dtstart`.
/// - **WEEKLY/DAILY** (`:634-635`): backward `scanDayCandidates` from `now`; `Some` iff
///   the found candidate is `>= dtstart`.
pub fn latest_automation_occurrence_at_or_before(
    schedule: &str,
    dtstart_ms: i64,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<i64>, OccurrenceError> {
    if now_ms < dtstart_ms {
        return Ok(None);
    }
    match parse_schedule(schedule)? {
        ParsedSchedule::Cron(rule) => {
            let mut candidate = floor_to_minute(now_ms, tz);
            let mut i = 0usize;
            while i < CRON_SCAN_MINUTES && candidate >= dtstart_ms {
                if cron_matches(&rule, candidate, tz) {
                    return Ok(Some(candidate));
                }
                candidate -= MINUTE_MS;
                i += 1;
            }
            Ok(None)
        }
        ParsedSchedule::Rrule(rule) => {
            if rule.freq == RruleFreq::Hourly {
                // byHour IGNORED.
                let mut candidate = set_local_minute(now_ms, rule.by_minute, tz);
                if candidate > now_ms {
                    candidate -= HOUR_MS;
                }
                return Ok(if candidate >= dtstart_ms {
                    Some(candidate)
                } else {
                    None
                });
            }
            Ok(match scan_day_candidates(&rule, now_ms, -1, tz) {
                Some(candidate) if candidate >= dtstart_ms => Some(candidate),
                _ => None,
            })
        }
    }
}
