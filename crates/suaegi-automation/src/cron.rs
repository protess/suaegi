//! Cron parsing + validation — verbatim port of the cron half of Orca's
//! `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0).
//!
//! Cited line numbers below (e.g. `:498-509`) refer to that source file. This is a
//! faithful port: quirks are preserved, not "fixed".

use std::collections::HashSet;

use chrono::{Datelike, Duration, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;

/// Fixed 24h in milliseconds (`:6`). F3: day stepping adds this constant with NO
/// re-floor to local midnight — bug-compatible with Orca.
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Scan window for "does this cron ever fire" (`:10`): `9 * 366 = 3294` days. A valid
/// cron like Feb 29 can have an 8-year gap across non-leap centuries (2100 etc.), so a
/// 9-year window guarantees a hit.
pub const CRON_SCAN_DAYS: usize = 9 * 366;

/// Byte-length ceiling for a cron expression (`:12`, `2 * 1024`). F4: measured as Rust
/// UTF-8 byte length (`str::len`), NOT `.chars().count()`.
pub const AUTOMATION_CRON_EXPRESSION_MAX_BYTES: usize = 2 * 1024;

/// RRULE BYDAY codes, 0-indexed Sunday-first (`:43`). Used by M2/M4; kept here as the
/// canonical name table alongside the cron name maps.
pub const DAY_CODES: [&str; 7] = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];

/// Monday..Friday RRULE codes (`:44`).
pub const WEEKDAY_CODES: [&str; 5] = ["MO", "TU", "WE", "TH", "FR"];

/// Parse error, distinguishable from a valid-but-empty result. A parse failure MUST
/// propagate as `Err` — it must never silently read as an empty/valid schedule
/// (transient ≠ false-negative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronError {
    /// `Invalid cron {field}.` — a field failed to parse (`:106`, `:123`, ...).
    InvalidField(String),
    /// `Cron schedule must have five fields.` (`:184`).
    WrongFieldCount,
}

impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CronError::InvalidField(field) => write!(f, "Invalid cron {field}."),
            CronError::WrongFieldCount => write!(f, "Cron schedule must have five fields."),
        }
    }
}

impl std::error::Error for CronError {}

/// Parsed cron rule: the five value sets plus the two "restricted" flags. F1: restricted
/// is decided purely by SET CARDINALITY (`:208-209`), not by syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCron {
    pub minutes: HashSet<i64>,
    pub hours: HashSet<i64>,
    pub days_of_month: HashSet<i64>,
    pub months: HashSet<i64>,
    pub days_of_week: HashSet<i64>,
    /// `daysOfMonth.size !== 31` (`:208`).
    pub day_of_month_restricted: bool,
    /// `daysOfWeek.size !== 7` (`:209`).
    pub day_of_week_restricted: bool,
}

// ---------------------------------------------------------------------------------------
// Name tables (`:45-68`).
// ---------------------------------------------------------------------------------------

/// `MONTH_NAMES` (`:45-58`): JAN=1 .. DEC=12 (1-indexed). `name` is already uppercased.
fn month_name(name: &str) -> Option<i64> {
    match name {
        "JAN" => Some(1),
        "FEB" => Some(2),
        "MAR" => Some(3),
        "APR" => Some(4),
        "MAY" => Some(5),
        "JUN" => Some(6),
        "JUL" => Some(7),
        "AUG" => Some(8),
        "SEP" => Some(9),
        "OCT" => Some(10),
        "NOV" => Some(11),
        "DEC" => Some(12),
        _ => None,
    }
}

/// `DAY_NAMES` (`:59-68`): 2-letter SU=0..SA=6 (the `DAY_CODES` index) AND 3-letter
/// SUN=0..SAT=6. `name` is already uppercased. So `MON-FRI` = 1..5.
fn day_name(name: &str) -> Option<i64> {
    match name {
        "SU" | "SUN" => Some(0),
        "MO" | "MON" => Some(1),
        "TU" | "TUE" => Some(2),
        "WE" | "WED" => Some(3),
        "TH" | "THU" => Some(4),
        "FR" | "FRI" => Some(5),
        "SA" | "SAT" => Some(6),
        _ => None,
    }
}

/// Day-of-week normalize (`:199`): `7` (Sunday) folds to `0`. Applied both when
/// boundary-checking and when expanding, and — critically — BEFORE the cardinality
/// heuristic runs (F1), so `0-7` and `1-7` collapse to the full 7-element set.
fn normalize_dow(value: i64) -> i64 {
    if value == 7 {
        0
    } else {
        value
    }
}

type NameLookup = fn(&str) -> Option<i64>;
type Normalize = fn(i64) -> i64;

// ---------------------------------------------------------------------------------------
// Numeric parsing.
// ---------------------------------------------------------------------------------------

/// Mirror of JavaScript `Number(string)` for the inputs cron fields produce. Returns
/// `NaN` for anything JS would reject, so callers can replicate `Number.isInteger`.
///
/// JS `Number`: trims whitespace; `""` → 0; accepts `0x`/`0o`/`0b` integer literals,
/// decimals, and exponents. Rust's `f64::from_str` covers decimals/exponents but rejects
/// the radix prefixes (and would accept `inf`/`nan` spellings — harmless here since those
/// are non-integers and get rejected downstream just as JS's `NaN`/`Infinity` do).
pub(crate) fn js_number(raw: &str) -> f64 {
    let s = js_trim(raw);
    if s.is_empty() {
        return 0.0;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).map_or(f64::NAN, |v| v as f64);
    }
    if let Some(oct) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        return i64::from_str_radix(oct, 8).map_or(f64::NAN, |v| v as f64);
    }
    if let Some(bin) = s.strip_prefix("0b").or_else(|| s.strip_prefix("0B")) {
        return i64::from_str_radix(bin, 2).map_or(f64::NAN, |v| v as f64);
    }
    s.parse::<f64>().unwrap_or(f64::NAN)
}

/// True iff `value` is a finite integer — the `Number.isInteger` predicate.
pub(crate) fn is_js_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// `parseCronNumber` (`:101-109`): uppercase → name-map lookup → else `Number(...)`.
/// Non-integer → error.
fn parse_cron_number(value: &str, names: Option<NameLookup>, field: &str) -> Result<i64, CronError> {
    let normalized = value.to_uppercase();
    let parsed = match names.and_then(|lookup| lookup(&normalized)) {
        Some(named) => named as f64,
        None => js_number(&normalized),
    };
    if !is_js_integer(parsed) {
        return Err(CronError::InvalidField(field.to_string()));
    }
    Ok(parsed as i64)
}

/// `parseCronField` (`:111-179`): list(`,`) → step(`/`) → range/wildcard/single. Validates
/// the step is an integer ≥ 1, ranges have two non-empty ends, boundary-checks BOTH pre-
/// and post-normalize values against `[min, max]`, rejects `start > end`, expands with
/// `normalize`, and rejects an empty result.
fn parse_cron_field(
    value: &str,
    min: i64,
    max: i64,
    field: &str,
    names: Option<NameLookup>,
    normalize: Option<Normalize>,
) -> Result<HashSet<i64>, CronError> {
    let invalid = || CronError::InvalidField(field.to_string());
    let mut result: HashSet<i64> = HashSet::new();

    for raw_part in value.split(',') {
        let part = js_trim(raw_part);
        if part.is_empty() {
            return Err(invalid());
        }

        let step_parts: Vec<&str> = part.split('/').collect();
        if step_parts.len() > 2 {
            return Err(invalid());
        }
        let range_part = step_parts[0];
        if range_part.is_empty() {
            return Err(invalid());
        }
        // step: absent → 1; present → Number(stepPart), must be an integer ≥ 1
        // (`:133-136`). `"5/"` → stepPart "" → Number("")=0 → step < 1 → error.
        let step = if step_parts.len() == 2 {
            let n = js_number(step_parts[1]);
            if !is_js_integer(n) {
                return Err(invalid());
            }
            n as i64
        } else {
            1
        };
        if step < 1 {
            return Err(invalid());
        }

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if range_part.contains('-') {
            let range_parts: Vec<&str> = range_part.split('-').collect();
            if range_parts.len() != 2 || range_parts[0].is_empty() || range_parts[1].is_empty() {
                return Err(invalid());
            }
            let start = parse_cron_number(range_parts[0], names, field)?;
            let end = parse_cron_number(range_parts[1], names, field)?;
            (start, end)
        } else {
            let start = parse_cron_number(range_part, names, field)?;
            (start, start)
        };

        let normalized_start = normalize.map_or(start, |f| f(start));
        let normalized_end = normalize.map_or(end, |f| f(end));
        if start < min
            || start > max
            || end < min
            || end > max
            || normalized_start < min
            || normalized_start > max
            || normalized_end < min
            || normalized_end > max
            || start > end
        {
            return Err(invalid());
        }

        let mut v = start;
        while v <= end {
            result.insert(normalize.map_or(v, |f| f(v)));
            v += step;
        }
    }

    if result.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

/// `parseCronExpression` (`:181-211`): tokenize with `maxFields = 6`, require exactly five
/// fields, parse each per the §2.3 table, and set the restricted flags from set cardinality.
///
/// Field parse order mirrors the source exactly (day-of-month, then day-of-week, then
/// minute/hour/month via the returned object literal, `:187-207`) so that when several
/// fields are malformed the SAME error surfaces first.
pub fn parse_cron_expression(expression: &str) -> Result<ParsedCron, CronError> {
    let parts = get_automation_cron_expression_fields(expression, 6);
    if parts.len() != 5 {
        return Err(CronError::WrongFieldCount);
    }
    let minute = &parts[0];
    let hour = &parts[1];
    let day_of_month = &parts[2];
    let month = &parts[3];
    let day_of_week = &parts[4];

    let days_of_month = parse_cron_field(day_of_month, 1, 31, "day of month", None, None)?;
    let days_of_week = parse_cron_field(
        day_of_week,
        0,
        7,
        "day of week",
        Some(day_name),
        Some(normalize_dow),
    )?;
    let minutes = parse_cron_field(minute, 0, 59, "minute", None, None)?;
    let hours = parse_cron_field(hour, 0, 23, "hour", None, None)?;
    let months = parse_cron_field(month, 1, 12, "month", Some(month_name), None)?;

    let day_of_month_restricted = days_of_month.len() != 31;
    let day_of_week_restricted = days_of_week.len() != 7;

    Ok(ParsedCron {
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        day_of_month_restricted,
        day_of_week_restricted,
    })
}

/// `getAutomationCronExpressionFields` (`:213-236`): byte guard FIRST (F4), then a
/// regex-free linear tokenizer splitting on the Unicode whitespace set (`:238-252`),
/// stopping once `max_fields` tokens are produced.
pub fn get_automation_cron_expression_fields(expression: &str, max_fields: usize) -> Vec<String> {
    // F4: Rust UTF-8 byte length. `> 2048` reject, `== 2048` accept.
    if expression.len() > AUTOMATION_CRON_EXPRESSION_MAX_BYTES {
        return Vec::new();
    }
    let mut fields: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in expression.chars() {
        if is_automation_cron_field_whitespace(ch as u32) {
            if !current.is_empty() {
                fields.push(std::mem::take(&mut current));
                if fields.len() >= max_fields {
                    return fields;
                }
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        fields.push(current);
    }
    fields
}

/// Unicode whitespace codepoints treated as field separators (`:238-252`). All are BMP,
/// so iterating over `char`s (codepoints) matches Orca's UTF-16 `charCodeAt` scan.
fn is_automation_cron_field_whitespace(code: u32) -> bool {
    code == 32
        || (9..=13).contains(&code)
        || code == 160
        || code == 5760
        || (8192..=8202).contains(&code)
        || code == 8232
        || code == 8233
        || code == 8239
        || code == 8287
        || code == 12288
        || code == 65279
}

/// JS-faithful trim. Rust's `str::trim()` strips the Unicode `White_Space` property, which
/// **diverges** from ECMAScript `String.prototype.trim` / `Number()` at two codepoints:
/// U+FEFF (JS strips, Rust keeps) and U+0085/NEL (Rust strips, JS keeps). Every `.trim()` in
/// Orca's source is a JS trim, so we replicate its exact set — which is byte-for-byte the
/// [`is_automation_cron_field_whitespace`] set (Orca deliberately built the tokenizer whitespace
/// set to match JS trim). Use this wherever the JS source calls `.trim()` or relies on
/// `Number()`'s leading/trailing-whitespace stripping — never bare `str::trim()`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(|ch: char| is_automation_cron_field_whitespace(ch as u32))
}

// ---------------------------------------------------------------------------------------
// Local wall-clock helpers (F2: explicit timezone, never ambient Local).
// ---------------------------------------------------------------------------------------

/// Resolve a naive local datetime to a concrete instant in `tz`, tolerating DST folds/gaps.
/// Fold (ambiguous, fall-back) policy: pick `.earliest()`. Gap (nonexistent, spring-forward)
/// policy: advance one hour past the gap — mirroring JS `Date.setHours`, which rolls a
/// nonexistent local time FORWARD. M3's occurrence math reuses this exact helper so its
/// `at_local_time`/`floor_to_minute` share one DST policy with `start_of_local_day`.
pub(crate) fn resolve_local(tz: Tz, naive: NaiveDateTime) -> chrono::DateTime<Tz> {
    if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
        return dt;
    }
    // Spring-forward gap: advance past it.
    tz.from_local_datetime(&(naive + Duration::hours(1)))
        .earliest()
        .expect("local datetime resolvable after DST gap")
}

/// Civil datetime for `ms` in `tz`. A UTC instant maps to exactly one local time.
pub(crate) fn civil(tz: Tz, ms: i64) -> chrono::DateTime<Tz> {
    tz.timestamp_millis_opt(ms)
        .single()
        .expect("timestamp maps to a single local time")
}

/// `startOfLocalDay` (`:453-457`): set local wall-clock to 00:00:00.000 and return the ms.
pub fn start_of_local_day(ms: i64, tz: Tz) -> i64 {
    let date = civil(tz, ms).date_naive();
    let midnight = date
        .and_hms_milli_opt(0, 0, 0, 0)
        .expect("midnight is a valid time");
    resolve_local(tz, midnight).timestamp_millis()
}

// ---------------------------------------------------------------------------------------
// Matching (F2 timezone injected; JS getDay() = 0 Sunday .. 6 Saturday).
// ---------------------------------------------------------------------------------------

/// `cronDateMatches` (`:498-509`) — THE crux. Month mismatch → false. Then DOM/DOW
/// OR-vs-AND: if BOTH day fields are restricted, OR the two; otherwise AND them.
pub fn cron_date_matches(rule: &ParsedCron, ms: i64, tz: Tz) -> bool {
    let date = civil(tz, ms);
    let month = i64::from(date.month()); // chrono month() is 1-12, matching getMonth()+1
    if !rule.months.contains(&month) {
        return false;
    }
    let day_of_month = i64::from(date.day());
    // JS getDay(): 0=Sunday..6=Saturday. chrono num_days_from_sunday() matches exactly.
    let day_of_week = i64::from(date.weekday().num_days_from_sunday());
    let dom_matches = rule.days_of_month.contains(&day_of_month);
    let dow_matches = rule.days_of_week.contains(&day_of_week);
    if rule.day_of_month_restricted && rule.day_of_week_restricted {
        dom_matches || dow_matches
    } else {
        dom_matches && dow_matches
    }
}

/// `cronMatches` (`:490-496`): date matches AND the local hour/minute are in their sets.
pub fn cron_matches(rule: &ParsedCron, ms: i64, tz: Tz) -> bool {
    if !cron_date_matches(rule, ms, tz) {
        return false;
    }
    let date = civil(tz, ms);
    rule.hours.contains(&i64::from(date.hour())) && rule.minutes.contains(&i64::from(date.minute()))
}

/// `cronHasPossibleOccurrence` (`:511-520`): scan `CRON_SCAN_DAYS` days from local midnight,
/// stepping by the FIXED `DAY_MS` with NO re-floor (F3, bug-compatible).
pub fn cron_has_possible_occurrence(rule: &ParsedCron, anchor_ms: i64, tz: Tz) -> bool {
    let mut day = start_of_local_day(anchor_ms, tz);
    for _ in 0..CRON_SCAN_DAYS {
        if cron_date_matches(rule, day, tz) {
            return true;
        }
        day += DAY_MS;
    }
    false
}

/// `isValidAutomationCronSchedule` (`:274-281`): parse the cron directly (no `=` dispatch),
/// then check a run is possible. F2: `now_ms` + `tz` are explicit params — this never reads
/// the system clock. A parse error → `false` (but it is an `Err` internally, never silently
/// empty).
pub fn is_valid_automation_cron_schedule(expression: &str, now_ms: i64, tz: Tz) -> bool {
    match parse_cron_expression(js_trim(expression)) {
        Ok(rule) => cron_has_possible_occurrence(&rule, now_ms, tz),
        Err(_) => false,
    }
}
