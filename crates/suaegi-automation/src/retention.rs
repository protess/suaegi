//! Run retention + numbering + identity — verbatim port of Orca's
//! `src/shared/automation-run-retention.ts`, the `AutomationRunStatus` half of
//! `src/shared/automations-types.ts` (`:8-30`), and `src/shared/automation-run-identity.ts`
//! (`:1-16`).
//!
//! Cited line numbers refer to those source files. This is a faithful port: quirks are
//! preserved, not "fixed" — especially F6 (STABLE sort, NO id tie-break), the negative-cap
//! clamp, in-flight runs NEVER evicted, and survivors returned in original append order.

use std::collections::HashMap;

/// `MAX_AUTOMATION_RUNS_PER_AUTOMATION` (`automation-run-retention.ts:3`).
pub const MAX_AUTOMATION_RUNS_PER_AUTOMATION: i64 = 100;

/// `AutomationRunStatus` (`automations-types.ts:8-17`). The three in-flight statuses
/// (pending/dispatching/dispatched) are NEVER evictable; the rest are final.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomationRunStatus {
    Pending,
    Dispatching,
    Dispatched,
    Completed,
    SkippedPrecheck,
    SkippedMissed,
    SkippedUnavailable,
    SkippedNeedsInteractiveAuth,
    DispatchFailed,
}

/// `isFinalAutomationRunStatus` (`automations-types.ts:21-30`): the statuses a run can never
/// leave — `completed`, `dispatch_failed`, and the four `skipped_*` — are the ONLY evictable
/// ones. Dropping any status from this list would let [`prune_automation_runs`] evict an
/// in-flight run whose completion lands later.
pub fn is_final_automation_run_status(status: AutomationRunStatus) -> bool {
    matches!(
        status,
        AutomationRunStatus::Completed
            | AutomationRunStatus::DispatchFailed
            | AutomationRunStatus::SkippedPrecheck
            | AutomationRunStatus::SkippedMissed
            | AutomationRunStatus::SkippedUnavailable
            | AutomationRunStatus::SkippedNeedsInteractiveAuth
    )
}

/// One automation run — the fields the retention/numbering logic touches (`AutomationRun` in
/// `automations-types.ts`). `run_number` is `None` for legacy runs that predate the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub status: AutomationRunStatus,
    pub created_at: i64,
    pub scheduled_for: i64,
    pub run_number: Option<i64>,
}

/// `pruneAutomationRuns` (`automation-run-retention.ts:7-27`). Evicts ONLY final runs and only
/// past the per-automation cap; in-flight runs ALWAYS survive. Grouped by `automation_id`,
/// each group sorted DESC `created_at` then DESC `scheduled_for` — F6: a STABLE sort with NO
/// `id` tie-break, so ties preserve input order deterministically (matching JS's stable sort).
/// The cap is negative-clamped (`max(0, cap)`) so a negative cap keeps nothing rather than
/// dropping from the tail. The result preserves the ORIGINAL append order: it is a filter over
/// `runs` retaining every kept-final id plus every non-final run.
pub fn prune_automation_runs(runs: &[AutomationRun], max_per_automation: i64) -> Vec<AutomationRun> {
    // Why: a dispatched run's completion can land hours later — only final runs are evictable.
    let mut groups: HashMap<&str, Vec<&AutomationRun>> = HashMap::new();
    for run in runs.iter().filter(|r| is_final_automation_run_status(r.status)) {
        groups.entry(&run.automation_id).or_default().push(run);
    }

    let cap = max_per_automation.max(0) as usize;
    let mut kept: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for group in groups.values_mut() {
        // F6: STABLE sort (`slice::sort_by` is stable). Comparator ONLY on created_at then
        // scheduled_for — NO id tie-break; equal elements keep their input order.
        group.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.scheduled_for.cmp(&a.scheduled_for))
        });
        for run in group.iter().take(cap) {
            kept.insert(&run.id);
        }
    }

    // Survivors keep their original append order — callers index by position.
    runs.iter()
        .filter(|run| {
            kept.contains(run.id.as_str()) || !is_final_automation_run_status(run.status)
        })
        .cloned()
        .collect()
}

/// `backfillAutomationRunNumbers` (`automation-run-retention.ts:38-54`). Stamps `run_number`
/// onto unnumbered runs, continuing from the HIGHEST number the automation already carries
/// (seeded at 0), NOT from append position — so a downgrade that appends unnumbered runs after
/// high-numbered survivors never reissues a held number. Numbered runs are left untouched.
pub fn backfill_automation_run_numbers(runs: &[AutomationRun]) -> Vec<AutomationRun> {
    let mut highest_per_automation: HashMap<&str, i64> = HashMap::new();
    for run in runs {
        if let Some(number) = run.run_number {
            let highest = highest_per_automation
                .get(run.automation_id.as_str())
                .copied()
                .unwrap_or(0);
            highest_per_automation.insert(&run.automation_id, highest.max(number));
        }
    }
    // Second pass needs mutable ownership of the map keyed by owned strings, so re-key by
    // owned automation_id (the borrow above ends before we take these clones).
    let mut highest_owned: HashMap<String, i64> = highest_per_automation
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    runs.iter()
        .map(|run| {
            if run.run_number.is_some() {
                return run.clone();
            }
            let next = highest_owned
                .get(run.automation_id.as_str())
                .copied()
                .unwrap_or(0)
                + 1;
            highest_owned.insert(run.automation_id.clone(), next);
            AutomationRun {
                run_number: Some(next),
                ..run.clone()
            }
        })
        .collect()
}

/// `nextAutomationRunNumber` (`automation-run-retention.ts:57-65`): `reduce(max(n, runNumber ??
/// 0), init = len) + 1`. Seeding with the count lets legacy (unnumbered) runs still advance;
/// the max keeps it above the highest surviving number after a prune.
pub fn next_automation_run_number(runs_for_automation: &[AutomationRun]) -> i64 {
    runs_for_automation
        .iter()
        .fold(runs_for_automation.len() as i64, |n, run| {
            n.max(run.run_number.unwrap_or(0))
        })
        + 1
}

// ---------------------------------------------------------------------------------------
// Run identity (`automation-run-identity.ts:1-16`): runContext-first, legacy projectId fallback.
// ---------------------------------------------------------------------------------------

/// A run's execution context (`WorkspaceRunContext`), the part the identity helpers read. Both
/// fields are optional: a missing `run_context` OR a missing field falls back to the legacy id.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutomationRunContext {
    pub repo_id: Option<String>,
    pub project_id: Option<String>,
}

/// The identity-relevant slice of an `Automation` (`Pick<Automation, 'projectId' |
/// 'runContext'>`, `automation-run-identity.ts:3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationIdentity {
    pub project_id: String,
    pub run_context: Option<AutomationRunContext>,
}

/// `getAutomationLegacyRepoId` (`automation-run-identity.ts:5-7`): the bare legacy `projectId`.
pub fn get_automation_legacy_repo_id(project_id: &str) -> String {
    project_id.to_string()
}

/// `getAutomationRunRepoId` (`automation-run-identity.ts:9-11`):
/// `runContext?.repoId ?? legacyRepoId`.
pub fn get_automation_run_repo_id(automation: &AutomationIdentity) -> String {
    automation
        .run_context
        .as_ref()
        .and_then(|ctx| ctx.repo_id.clone())
        .unwrap_or_else(|| get_automation_legacy_repo_id(&automation.project_id))
}

/// `getAutomationRunProjectId` (`automation-run-identity.ts:13-15`):
/// `runContext?.projectId ?? legacyRepoId`.
pub fn get_automation_run_project_id(automation: &AutomationIdentity) -> String {
    automation
        .run_context
        .as_ref()
        .and_then(|ctx| ctx.project_id.clone())
        .unwrap_or_else(|| get_automation_legacy_repo_id(&automation.project_id))
}
