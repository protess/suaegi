//! M4 retention oracle — ported from Orca's `src/shared/automation-run-retention.test.ts`
//! in full (prune cap / in-flight survival / stable tie-break / backfill / nextRunNumber),
//! plus `isFinalAutomationRunStatus` and the run-identity helpers.
//!
//! Every test here is mutation-verifiable: it FAILS if the specific logic it guards is broken
//! (no hollow tests — this repo has shipped ≥5).

use suaegi_automation::{
    backfill_automation_run_numbers, get_automation_legacy_repo_id,
    get_automation_run_project_id, get_automation_run_repo_id, is_final_automation_run_status,
    next_automation_run_number, prune_automation_runs, AutomationIdentity, AutomationRun,
    AutomationRunContext, AutomationRunStatus, MAX_AUTOMATION_RUNS_PER_AUTOMATION,
};

/// Mirrors the oracle `run()` factory: final status by default (`skipped_precheck`),
/// `created_at`/`scheduled_for` 0, no `run_number`.
fn run(id: &str, automation_id: &str) -> AutomationRun {
    AutomationRun {
        id: id.to_string(),
        automation_id: automation_id.to_string(),
        status: AutomationRunStatus::SkippedPrecheck,
        created_at: 0,
        scheduled_for: 0,
        run_number: None,
    }
}

fn with_created(mut r: AutomationRun, created_at: i64) -> AutomationRun {
    r.created_at = created_at;
    r
}

/// Mirrors the oracle `makeRuns(automationId, count, from=0)`: `id = {automationId}-{from+i}`,
/// `created_at = from + i`.
fn make_runs(automation_id: &str, count: i64, from: i64) -> Vec<AutomationRun> {
    (0..count)
        .map(|i| with_created(run(&format!("{automation_id}-{}", from + i), automation_id), from + i))
        .collect()
}

fn ids(runs: &[AutomationRun]) -> Vec<String> {
    runs.iter().map(|r| r.id.clone()).collect()
}

fn numbers(runs: &[AutomationRun]) -> Vec<Option<i64>> {
    runs.iter().map(|r| r.run_number).collect()
}

// ---------------------------------------------------------------------------------------
// isFinalAutomationRunStatus (`automations-types.ts:21-30`).
// ---------------------------------------------------------------------------------------

#[test]
fn final_status_covers_completed_dispatch_failed_and_all_skipped() {
    for status in [
        AutomationRunStatus::Completed,
        AutomationRunStatus::DispatchFailed,
        AutomationRunStatus::SkippedPrecheck,
        AutomationRunStatus::SkippedMissed,
        AutomationRunStatus::SkippedUnavailable,
        AutomationRunStatus::SkippedNeedsInteractiveAuth,
    ] {
        assert!(is_final_automation_run_status(status), "{status:?} is final");
    }
    for status in [
        AutomationRunStatus::Pending,
        AutomationRunStatus::Dispatching,
        AutomationRunStatus::Dispatched,
    ] {
        assert!(
            !is_final_automation_run_status(status),
            "{status:?} is in-flight"
        );
    }
}

// ---------------------------------------------------------------------------------------
// pruneAutomationRuns.
// ---------------------------------------------------------------------------------------

#[test]
fn keeps_everything_below_the_cap() {
    let runs = make_runs("a", 5, 0);
    assert_eq!(prune_automation_runs(&runs, 10), runs);
}

#[test]
fn keeps_only_the_newest_n_per_automation() {
    let kept = prune_automation_runs(&make_runs("a", 10, 0), 3);
    assert_eq!(ids(&kept), ["a-7", "a-8", "a-9"]);
}

#[test]
fn caps_each_automation_independently() {
    let mut runs = make_runs("a", 6, 0);
    runs.extend(make_runs("b", 2, 0));
    let kept = prune_automation_runs(&runs, 3);
    assert_eq!(kept.iter().filter(|r| r.automation_id == "a").count(), 3);
    // 'b' is under the cap and must survive intact.
    assert_eq!(kept.iter().filter(|r| r.automation_id == "b").count(), 2);
}

#[test]
fn preserves_original_append_order_among_survivors() {
    let mut runs = make_runs("a", 4, 0);
    runs.extend(make_runs("b", 4, 0));
    let kept = prune_automation_runs(&runs, 2);
    assert_eq!(ids(&kept), ["a-2", "a-3", "b-2", "b-3"]);
}

#[test]
fn breaks_created_at_ties_on_scheduled_for() {
    let runs = vec![
        AutomationRun {
            created_at: 5,
            scheduled_for: 1,
            ..run("x", "a")
        },
        AutomationRun {
            created_at: 5,
            scheduled_for: 2,
            ..run("y", "a")
        },
    ];
    // Higher scheduledFor wins the tie deterministically.
    assert_eq!(ids(&prune_automation_runs(&runs, 1)), ["y"]);
}

#[test]
fn returns_nothing_when_the_cap_is_zero_or_negative() {
    assert_eq!(prune_automation_runs(&make_runs("a", 3, 0), 0), []);
    // Negative cap must clamp to 0 (keep nothing), NOT drop from the tail.
    assert_eq!(prune_automation_runs(&make_runs("a", 3, 0), -1), []);
}

#[test]
fn never_evicts_in_flight_runs_even_far_past_the_cap() {
    let mut runs = vec![
        with_created(
            AutomationRun {
                status: AutomationRunStatus::Pending,
                ..run("old-pending", "a")
            },
            -3,
        ),
        with_created(
            AutomationRun {
                status: AutomationRunStatus::Dispatching,
                ..run("old-dispatching", "a")
            },
            -2,
        ),
        with_created(
            AutomationRun {
                status: AutomationRunStatus::Dispatched,
                ..run("old-dispatched", "a")
            },
            -1,
        ),
    ];
    runs.extend(make_runs("a", 10, 0));
    let kept = prune_automation_runs(&runs, 3);
    assert_eq!(
        ids(&kept),
        [
            "old-pending",
            "old-dispatching",
            "old-dispatched",
            "a-7",
            "a-8",
            "a-9"
        ]
    );
}

#[test]
fn shrinks_a_realistic_runaway_history_to_the_cap() {
    let mut runaway = Vec::new();
    for a in ["a", "b", "c", "d"] {
        runaway.extend(make_runs(a, 2796, 0));
    }
    assert_eq!(runaway.len(), 11_184);
    assert_eq!(
        prune_automation_runs(&runaway, MAX_AUTOMATION_RUNS_PER_AUTOMATION).len(),
        4 * MAX_AUTOMATION_RUNS_PER_AUTOMATION as usize
    );
}

// ---------------------------------------------------------------------------------------
// backfillAutomationRunNumbers.
// ---------------------------------------------------------------------------------------

#[test]
fn numbers_legacy_runs_by_append_position_within_their_automation() {
    let runs = vec![run("a-0", "a"), run("b-0", "b"), run("a-1", "a")];
    assert_eq!(
        numbers(&backfill_automation_run_numbers(&runs)),
        [Some(1), Some(1), Some(2)]
    );
}

#[test]
fn leaves_an_existing_run_number_untouched() {
    let runs = vec![AutomationRun {
        run_number: Some(42),
        ..run("a-0", "a")
    }];
    assert_eq!(backfill_automation_run_numbers(&runs)[0].run_number, Some(42));
}

#[test]
fn never_reissues_a_number_a_numbered_run_already_holds() {
    let runs = vec![
        AutomationRun {
            run_number: Some(2),
            ..run("a-0", "a")
        },
        run("a-1", "a"),
    ];
    let out = numbers(&backfill_automation_run_numbers(&runs));
    assert_eq!(out, [Some(2), Some(3)]);
    let unique: std::collections::HashSet<_> = out.iter().collect();
    assert_eq!(unique.len(), out.len());
}

#[test]
fn numbers_unnumbered_runs_above_the_highest_survivor_per_automation() {
    let runs = vec![
        AutomationRun {
            run_number: Some(200),
            ..run("a-0", "a")
        },
        AutomationRun {
            run_number: Some(7),
            ..run("b-0", "b")
        },
        run("a-1", "a"),
        run("b-1", "b"),
    ];
    assert_eq!(
        numbers(&backfill_automation_run_numbers(&runs)),
        [Some(200), Some(7), Some(201), Some(8)]
    );
}

// ---------------------------------------------------------------------------------------
// nextAutomationRunNumber.
// ---------------------------------------------------------------------------------------

#[test]
fn continues_from_the_highest_surviving_run_number() {
    let retained = vec![
        AutomationRun {
            run_number: Some(2795),
            ..run("a-0", "a")
        },
        AutomationRun {
            run_number: Some(2796),
            ..run("a-1", "a")
        },
    ];
    assert_eq!(next_automation_run_number(&retained), 2797);
}

#[test]
fn falls_back_to_the_count_for_legacy_runs_with_no_numbers() {
    assert_eq!(next_automation_run_number(&make_runs("a", 100, 0)), 101);
}

#[test]
fn starts_at_1_for_a_brand_new_automation() {
    assert_eq!(next_automation_run_number(&[]), 1);
}

#[test]
fn never_repeats_a_number_across_a_prune_cycle() {
    let runs = backfill_automation_run_numbers(&make_runs("a", 250, 0));
    let runs = prune_automation_runs(&runs, 100);
    let next = next_automation_run_number(&runs);
    assert_eq!(next, 251);
    assert!(!runs.iter().any(|r| r.run_number == Some(next)));
}

// ---------------------------------------------------------------------------------------
// Run identity (`automation-run-identity.ts:1-16`).
// ---------------------------------------------------------------------------------------

#[test]
fn legacy_repo_id_is_the_project_id() {
    assert_eq!(get_automation_legacy_repo_id("proj-1"), "proj-1");
}

#[test]
fn run_repo_and_project_ids_prefer_run_context() {
    let with_ctx = AutomationIdentity {
        project_id: "legacy".to_string(),
        run_context: Some(AutomationRunContext {
            repo_id: Some("ctx-repo".to_string()),
            project_id: Some("ctx-proj".to_string()),
        }),
    };
    assert_eq!(get_automation_run_repo_id(&with_ctx), "ctx-repo");
    assert_eq!(get_automation_run_project_id(&with_ctx), "ctx-proj");
}

#[test]
fn run_repo_and_project_ids_fall_back_to_legacy() {
    // No run_context at all → legacy projectId.
    let no_ctx = AutomationIdentity {
        project_id: "legacy".to_string(),
        run_context: None,
    };
    assert_eq!(get_automation_run_repo_id(&no_ctx), "legacy");
    assert_eq!(get_automation_run_project_id(&no_ctx), "legacy");

    // run_context present but the specific field missing → legacy projectId.
    let partial = AutomationIdentity {
        project_id: "legacy".to_string(),
        run_context: Some(AutomationRunContext {
            repo_id: None,
            project_id: None,
        }),
    };
    assert_eq!(get_automation_run_repo_id(&partial), "legacy");
    assert_eq!(get_automation_run_project_id(&partial), "legacy");
}
