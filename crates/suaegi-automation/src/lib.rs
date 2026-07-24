//! suaegi-automation — Orca automation scheduling engine (Rust port).
//!
//! Milestone M1 ports the cron parser + validator from Orca's
//! `src/shared/automation-schedules.ts` (@ v1.4.150-rc.0) VERBATIM, preserving
//! every subtle behavior — including the DOM/DOW OR-vs-AND crux, the set-cardinality
//! "restricted" heuristic (F1), the byte-length guard before tokenizing (F4), and the
//! fixed `DAY_MS` day stepping (F3). All date arithmetic is local wall-clock; callers
//! inject an explicit IANA timezone (`chrono_tz::Tz`) rather than reading ambient Local
//! (F2). See `docs/superpowers/plans/2026-07-24-automation-schedules.md` §2 M1.

mod classify;
mod cron;
mod occurrence;
mod retention;
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

// M3 — occurrence math (next/latest run computation), the risky core. TZ is an explicit
// param on every entry point (F2). See
// `docs/superpowers/plans/2026-07-24-automation-schedules.md` §2 M3.
pub use occurrence::{
    latest_automation_occurrence_at_or_before, next_automation_occurrence_after, OccurrenceError,
};

// M4 — schedule classification + hardcoded-English human labels. The deterministic core
// (`ScheduleKind`) is TZ/locale-independent; only labels carry English text (no `Intl`). The
// clock dependency (possible-occurrence check) takes explicit `now_ms` + `tz` (F2). See
// `docs/superpowers/plans/2026-07-24-automation-schedules.md` §2 M4.
pub use classify::{
    classify_automation_cron_schedule, classify_parsed_cron_schedule, format_automation_schedule,
    format_time, ScheduleKind,
};

// M4 — run retention (cap/prune), run numbering (backfill/next), and run identity. Pure data
// transforms: no clock, no TZ. See the plan §2 M4.
pub use retention::{
    backfill_automation_run_numbers, get_automation_legacy_repo_id, get_automation_run_project_id,
    get_automation_run_repo_id, is_final_automation_run_status, next_automation_run_number,
    prune_automation_runs, AutomationIdentity, AutomationRun, AutomationRunContext,
    AutomationRunStatus, MAX_AUTOMATION_RUNS_PER_AUTOMATION,
};

// Re-export the timezone type so downstream crates and integration tests can name the
// injected IANA timezone (F2) without taking a direct chrono-tz dependency.
pub use chrono_tz::Tz;
