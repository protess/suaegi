//! suaegi-automation — Orca automation scheduling engine (Rust port).
//!
//! Milestone M1 ports the cron parser + validator from Orca's
//! `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0) VERBATIM, preserving
//! every subtle behavior — including the DOM/DOW OR-vs-AND crux, the set-cardinality
//! "restricted" heuristic (F1), the byte-length guard before tokenizing (F4), and the
//! fixed `DAY_MS` day stepping (F3). All date arithmetic is local wall-clock; callers
//! inject an explicit IANA timezone (`chrono_tz::Tz`) rather than reading ambient Local
//! (F2). See `docs/superpowers/plans/2026-07-24-automation-schedules.md` §2 M1.

mod cron;
mod rrule;

pub use cron::{
    cron_date_matches, cron_has_possible_occurrence, cron_matches,
    get_automation_cron_expression_fields, is_valid_automation_cron_schedule,
    parse_cron_expression, start_of_local_day, CronError, ParsedCron, AUTOMATION_CRON_EXPRESSION_MAX_BYTES,
    CRON_SCAN_DAYS, DAY_CODES, DAY_MS, WEEKDAY_CODES,
};

// M2 — RRULE parse/build + `=`-dispatched schedule validation. See
// `docs/superpowers/plans/2026-07-24-automation-schedules.md` §2 M2.
pub use rrule::{
    build_automation_cron_schedule, build_automation_rrule, is_valid_automation_schedule,
    parse_automation_rrule, parse_rrule, parse_schedule, try_parse_automation_rrule,
    AutomationRruleParts, AutomationSchedulePreset, ParsedRrule, ParsedSchedule, RruleError,
    RruleFreq, ScheduleError,
};

// Re-export the timezone type so downstream crates and integration tests can name the
// injected IANA timezone (F2) without taking a direct chrono-tz dependency.
pub use chrono_tz::Tz;
