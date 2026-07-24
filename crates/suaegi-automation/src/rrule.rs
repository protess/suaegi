//! RRULE parse/build + schedule dispatch — verbatim port of the RRULE half of Orca's
//! `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0).
//!
//! Cited line numbers below (e.g. `:70-99`) refer to that source file. This is a faithful
//! port: quirks are preserved, not "fixed". The supported RRULE surface is a tiny subset —
//! `FREQ` ∈ {HOURLY, DAILY, WEEKLY} plus `BYHOUR`/`BYMINUTE`/`BYDAY` — with Orca-specific
//! behaviors (hourly OMITS BYHOUR when built, `=` dispatch is literal).

use chrono_tz::Tz;

use crate::cron::{
    cron_has_possible_occurrence, is_js_integer, js_number, parse_cron_expression, CronError,
    ParsedCron, DAY_CODES, WEEKDAY_CODES,
};

/// RRULE frequency (`:16`). Only these three are supported; anything else is
/// `RruleError::UnsupportedRecurrence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RruleFreq {
    Hourly,
    Daily,
    Weekly,
}

/// The `AutomationSchedulePreset` union (`automations-types.ts:32`). `parse_automation_rrule`
/// returns only the first four; `Custom` exists to mirror the TS union. The build functions
/// treat anything that is not Hourly/Weekdays/Weekly as the DAILY default arm, exactly like
/// Orca's runtime fall-through (its `Exclude<..,'custom'>` is a compile-time-only annotation
/// with no runtime branch — `:530-540`, `:551-561`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationSchedulePreset {
    Hourly,
    Daily,
    Weekdays,
    Weekly,
    Custom,
}

/// Parse error, distinguishable from a valid result. A parse failure MUST propagate as
/// `Err` — it must never silently read as a valid/empty schedule (transient ≠ false-negative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RruleError {
    /// `Unsupported automation recurrence.` (`:80`).
    UnsupportedRecurrence,
    /// `Invalid recurrence hour.` (`:85`).
    InvalidHour,
    /// `Invalid recurrence minute.` (`:88`).
    InvalidMinute,
    /// `Invalid recurrence day.` (`:96`, `:300`, `:305`).
    InvalidDay,
}

impl std::fmt::Display for RruleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            RruleError::UnsupportedRecurrence => "Unsupported automation recurrence.",
            RruleError::InvalidHour => "Invalid recurrence hour.",
            RruleError::InvalidMinute => "Invalid recurrence minute.",
            RruleError::InvalidDay => "Invalid recurrence day.",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for RruleError {}

/// Parsed RRULE (`:14-20`): the frequency plus resolved hour/minute and the raw BYDAY codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRrule {
    pub freq: RruleFreq,
    /// Raw BYDAY codes, un-uppercased, empty-filtered (`:90`). Only validated for WEEKLY.
    pub by_day: Vec<String>,
    pub by_hour: i64,
    pub by_minute: i64,
}

/// `parseRrule` (`:70-99`): split on `;` → `key=value`, keys UPPERCASED into a map (values
/// left as-is). `FREQ` must be one of the three supported frequencies. `BYHOUR` defaults to
/// `9`, `BYMINUTE` to `0`, each validated as a JS integer in range. BYDAY is comma-split with
/// empty parts filtered; WEEKLY additionally requires a non-empty BYDAY whose every code is in
/// `DAY_CODES` (SU..SA, case-sensitive — values are NOT uppercased).
pub fn parse_rrule(rrule: &str) -> Result<ParsedRrule, RruleError> {
    // `key.toUpperCase()` on the KEY only; the VALUE is stored verbatim (`:73-75`). JS
    // destructuring `const [key, value] = part.split('=')` takes the first two `=`-segments
    // and ignores the rest, requiring both to be truthy (non-empty) to record the entry.
    let mut entries: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for part in rrule.split(';') {
        let mut segments = part.split('=');
        let key = segments.next().unwrap_or("");
        let value = segments.next();
        if let Some(value) = value {
            if !key.is_empty() && !value.is_empty() {
                entries.insert(key.to_uppercase(), value.to_string());
            }
        }
    }

    let freq = match entries.get("FREQ").map(String::as_str) {
        Some("HOURLY") => RruleFreq::Hourly,
        Some("DAILY") => RruleFreq::Daily,
        Some("WEEKLY") => RruleFreq::Weekly,
        _ => return Err(RruleError::UnsupportedRecurrence),
    };

    // `Number(entries.get('BYHOUR') ?? '9')` then `Number.isInteger` + range (`:82-89`).
    let by_hour = js_number(entries.get("BYHOUR").map(String::as_str).unwrap_or("9"));
    let by_minute = js_number(entries.get("BYMINUTE").map(String::as_str).unwrap_or("0"));
    // Explicit bounds mirror Orca's `!Number.isInteger(x) || x < min || x > max` verbatim
    // (`:84-89`); kept literal rather than a range `contains` for source fidelity.
    #[allow(clippy::manual_range_contains)]
    if !is_js_integer(by_hour) || by_hour < 0.0 || by_hour > 23.0 {
        return Err(RruleError::InvalidHour);
    }
    #[allow(clippy::manual_range_contains)]
    if !is_js_integer(by_minute) || by_minute < 0.0 || by_minute > 59.0 {
        return Err(RruleError::InvalidMinute);
    }

    // `(get('BYDAY') ?? '').split(',').filter(Boolean)` (`:90`).
    let by_day: Vec<String> = entries
        .get("BYDAY")
        .map(String::as_str)
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    // WEEKLY-only validation (`:91-97`): non-empty BYDAY, every code in DAY_CODES.
    if freq == RruleFreq::Weekly
        && (by_day.is_empty() || by_day.iter().any(|day| !DAY_CODES.contains(&day.as_str())))
    {
        return Err(RruleError::InvalidDay);
    }

    Ok(ParsedRrule {
        freq,
        by_day,
        by_hour: by_hour as i64,
        by_minute: by_minute as i64,
    })
}

/// Editing view of a parsed RRULE (`:283-288`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRruleParts {
    pub preset: AutomationSchedulePreset,
    pub hour: i64,
    pub minute: i64,
    pub day_of_week: i64,
}

/// `parseAutomationRrule` (`:283-313`): reverse-map a parsed RRULE to an editable preset.
/// HOURLY/DAILY carry a placeholder `dayOfWeek = 1`. WEEKLY whose BYDAY joins to exactly
/// `MO,TU,WE,TH,FR` becomes `weekdays`; otherwise a single BYDAY code maps to its
/// `DAY_CODES` index — CRITICALLY, Sunday (index 0) is PRESERVED, never coerced to Monday.
pub fn parse_automation_rrule(rrule: &str) -> Result<AutomationRruleParts, RruleError> {
    let rule = parse_rrule(rrule)?;
    match rule.freq {
        RruleFreq::Hourly => Ok(AutomationRruleParts {
            preset: AutomationSchedulePreset::Hourly,
            hour: rule.by_hour,
            minute: rule.by_minute,
            day_of_week: 1,
        }),
        RruleFreq::Daily => Ok(AutomationRruleParts {
            preset: AutomationSchedulePreset::Daily,
            hour: rule.by_hour,
            minute: rule.by_minute,
            day_of_week: 1,
        }),
        RruleFreq::Weekly => {
            // `rule.byDay.join(',') === WEEKDAY_CODES.join(',')` (`:296`).
            if rule.by_day.join(",") == WEEKDAY_CODES.join(",") {
                return Ok(AutomationRruleParts {
                    preset: AutomationSchedulePreset::Weekdays,
                    hour: rule.by_hour,
                    minute: rule.by_minute,
                    day_of_week: 1,
                });
            }
            // `if (rule.byDay.length !== 1)` (`:299`).
            if rule.by_day.len() != 1 {
                return Err(RruleError::InvalidDay);
            }
            // `DAY_CODES.indexOf(dayCode)` — `< 0` → error (`:303-306`).
            let day_code = &rule.by_day[0];
            let day_of_week = match DAY_CODES.iter().position(|&c| c == day_code.as_str()) {
                Some(index) => index as i64,
                None => return Err(RruleError::InvalidDay),
            };
            Ok(AutomationRruleParts {
                preset: AutomationSchedulePreset::Weekly,
                hour: rule.by_hour,
                minute: rule.by_minute,
                day_of_week,
            })
        }
    }
}

/// `tryParseAutomationRrule` (`:315-323`): null-safe wrapper — `None` on any parse error.
pub fn try_parse_automation_rrule(rrule: &str) -> Option<AutomationRruleParts> {
    parse_automation_rrule(rrule).ok()
}

/// `Math.max(0, Math.min(23, Math.floor(value)))` (`:528`, `:549`). Rust integers are already
/// floored, so this is the clamp only — the JS `floor` is a no-op over the integer domain.
fn clamp_hour(value: i64) -> i64 {
    value.clamp(0, 23)
}

/// `Math.max(0, Math.min(59, Math.floor(value)))` (`:529`, `:550`).
fn clamp_minute(value: i64) -> i64 {
    value.clamp(0, 59)
}

/// `buildAutomationRrule` (`:522-541`): preset → RRULE string, hour/minute clamped.
/// CRITICAL quirk: the `hourly` preset OMITS BYHOUR entirely (`FREQ=HOURLY;BYMINUTE=${m}`),
/// mirrored by the hourly occurrence math which ignores byHour. Anything not
/// hourly/weekdays/weekly falls through to the DAILY default arm (`:540`).
pub fn build_automation_rrule(
    preset: AutomationSchedulePreset,
    hour: i64,
    minute: i64,
    day_of_week: Option<i64>,
) -> String {
    let hour = clamp_hour(hour);
    let minute = clamp_minute(minute);
    match preset {
        AutomationSchedulePreset::Hourly => format!("FREQ=HOURLY;BYMINUTE={minute}"),
        AutomationSchedulePreset::Weekdays => {
            format!("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR={hour};BYMINUTE={minute}")
        }
        AutomationSchedulePreset::Weekly => {
            // `DAY_CODES[Math.max(0, Math.min(6, Math.floor(args.dayOfWeek ?? 1)))]` (`:537`).
            let index = day_of_week.unwrap_or(1).clamp(0, 6) as usize;
            let day = DAY_CODES[index];
            format!("FREQ=WEEKLY;BYDAY={day};BYHOUR={hour};BYMINUTE={minute}")
        }
        AutomationSchedulePreset::Daily | AutomationSchedulePreset::Custom => {
            format!("FREQ=DAILY;BYHOUR={hour};BYMINUTE={minute}")
        }
    }
}

/// `buildAutomationCronSchedule` (`:543-562`): preset → 5-field cron string, hour/minute
/// clamped. CRITICAL quirk: the `hourly` preset is `${m} * * * *` (no hour field pinned).
pub fn build_automation_cron_schedule(
    preset: AutomationSchedulePreset,
    hour: i64,
    minute: i64,
    day_of_week: Option<i64>,
) -> String {
    let hour = clamp_hour(hour);
    let minute = clamp_minute(minute);
    match preset {
        AutomationSchedulePreset::Hourly => format!("{minute} * * * *"),
        AutomationSchedulePreset::Weekdays => format!("{minute} {hour} * * 1-5"),
        AutomationSchedulePreset::Weekly => {
            // `Math.max(0, Math.min(6, Math.floor(args.dayOfWeek ?? 1)))` (`:558`).
            let day = day_of_week.unwrap_or(1).clamp(0, 6);
            format!("{minute} {hour} * * {day}")
        }
        AutomationSchedulePreset::Daily | AutomationSchedulePreset::Custom => {
            format!("{minute} {hour} * * *")
        }
    }
}

/// A parsed schedule (`:33`): either an RRULE or a cron rule, chosen by `parse_schedule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedSchedule {
    Rrule(ParsedRrule),
    Cron(ParsedCron),
}

/// Combined parse error for the `=`-dispatched `parse_schedule`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    Rrule(RruleError),
    Cron(CronError),
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleError::Rrule(e) => write!(f, "{e}"),
            ScheduleError::Cron(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ScheduleError {}

/// `parseSchedule` (`:254-260`): trim, then dispatch on `=`. F5 (LITERAL): ANY `=` in the
/// trimmed input routes to the RRULE parser — even cron-looking input like `0 9 * * MON=1`.
/// A `FREQ=...` string therefore never reaches the cron parser.
pub fn parse_schedule(schedule: &str) -> Result<ParsedSchedule, ScheduleError> {
    let trimmed = schedule.trim();
    if trimmed.contains('=') {
        parse_rrule(trimmed)
            .map(ParsedSchedule::Rrule)
            .map_err(ScheduleError::Rrule)
    } else {
        parse_cron_expression(trimmed)
            .map(ParsedSchedule::Cron)
            .map_err(ScheduleError::Cron)
    }
}

/// `isValidAutomationSchedule` (`:262-272`): dispatch via `parse_schedule`. An RRULE is valid
/// if it parses (parse enforces its own validity); a cron is valid only if a run is possible
/// within the scan window. F2: `now_ms` + `tz` are explicit — this never reads the system
/// clock. A parse error → `false` (but internally it is an `Err`, never silently valid).
pub fn is_valid_automation_schedule(schedule: &str, now_ms: i64, tz: Tz) -> bool {
    match parse_schedule(schedule) {
        Ok(ParsedSchedule::Rrule(_)) => true,
        Ok(ParsedSchedule::Cron(rule)) => cron_has_possible_occurrence(&rule, now_ms, tz),
        Err(_) => false,
    }
}
