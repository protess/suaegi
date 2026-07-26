//! GitHub hosted-review normalizer — verbatim port of Orca's
//! `src/shared/hosted-review-github.ts` (128L, @ v1.4.150-rc.0).
//!
//! This module is **M3** of a 4-milestone port (see
//! `docs/superpowers/plans/2026-07-25-hosted-review-m3.md`, decisions
//! J1–J13): the two public GitHub→provider-neutral mapping functions plus
//! their two private helpers. It consumes M1's vocabulary
//! ([`crate::hosted_review`]) and produces no new dependency. The GitLab
//! normalizer (M4) and any consumer wiring are out of scope here.
//!
//! # J1 — a LOCAL comment input type, not an extension of `pr_actions::PrComment`
//!
//! The research doc recommended extending the crate's existing
//! [`crate::pr_actions::PrComment`] with thread fields. This module
//! deliberately does **not** do that; instead it defines its own
//! [`HostedReviewCommentInput`] with exactly the two fields
//! [`unresolved_thread_count`] reads.
//!
//! Why: `PrComment` (`pr_actions.rs`) is `{ author, body, created_at, url }`
//! — it has **no** thread fields at all — and it is constructed in three
//! places (`pr_actions.rs`'s `From<GhCommentRaw>` impl, plus GitLab's and
//! GitHub-HTTP's forge implementations). If `thread_id`/`is_resolved` were
//! added to the shared `PrComment` instead, those two non-GitHub-normalizer
//! construction sites would have to supply *some* value for `is_resolved`,
//! and the only safe default (`None`) means — per J3's rule below — **every**
//! comment from those two providers would count as resolved forever. That is
//! a silent `unresolvedCount == 0` for providers that were never wired up to
//! report it, which is worse than a compile error. A local type has zero
//! blast radius and matches the two fields this module's function actually
//! reads. If a shared thread-aware comment type is ever needed, that is a
//! separate, deliberate migration — not a side effect of this port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewCommentInput {
    /// TS `threadId?: string` — optional-non-null; also read with JS
    /// truthiness (`!comment.threadId`), so `Some("")` is treated the same
    /// as `None` (J3).
    pub thread_id: Option<String>,
    /// TS `isResolved?: boolean` — optional-non-null; compared with strict
    /// `!== false`, so `None` and `Some(true)` both count as "resolved"
    /// (J3).
    pub is_resolved: Option<bool>,
}

/// `PRCheckDetail['status']` (`types.ts:1329`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckRunStatus {
    Queued,
    InProgress,
    Completed,
}

/// `PRCheckDetail['conclusion']` (`types.ts:1330-1342`), minus its `| null`
/// arm (modeled as `Option<CheckConclusion>` at the field, per M1's G1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Cancelled,
    TimedOut,
    Neutral,
    Skipped,
    Pending,
    /// Why (source comment, `types.ts:1338-1340`): a check suite needing
    /// manual action (e.g. a workflow awaiting "Approve and run") has no
    /// check run and is absent from `statusCheckRollup`, yet blocks
    /// auto-merge (GitHub returns "unstable status"). Surfaced as its own
    /// conclusion so [`derive_checks_status`] can treat it as a failure.
    ActionRequired,
}

/// The subset of `PRCheckDetail` (`types.ts:1327-1346`) that
/// [`derive_checks_status`] reads — only `status` and `conclusion`; `name`,
/// `url`, `checkRunId`, `workflowRunId` are never inspected by this cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrCheckDetail {
    pub status: CheckRunStatus,
    pub conclusion: Option<CheckConclusion>,
}

/// The subset of `PRInfo` (`types.ts:1170-1207`) that
/// `hostedReviewSummaryFromGitHubPRInfo` and `hostedReviewInfoFromGitHubPRInfo`
/// actually read. Fields such as `mergeMethodSettings`, `headRefName`,
/// `headDivergedFromMergedPRAtOid`, `baseRefName`, `prRepo`, `headRepo` exist
/// on the real `PRInfo` but are never touched by these two functions, so they
/// are intentionally omitted here (see also J13 for `baseRefName`
/// specifically).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubPrInfo {
    /// TS `number` — `u64` per this crate's existing PR/MR-number convention
    /// (see `hosted_review.rs`'s G4 note).
    pub number: u64,
    pub title: String,
    pub state: crate::hosted_review::PrState,
    pub url: String,
    pub checks_status: crate::hosted_review::CheckStatus,
    pub updated_at: String,
    pub mergeable: crate::hosted_review::PrMergeableState,
    pub review_decision: Option<crate::hosted_review::PrReviewDecisionAggregate>,
    pub auto_merge_enabled: Option<bool>,
    pub auto_merge_allowed: Option<bool>,
    pub merge_queue_required: Option<bool>,
    pub merge_state_status: Option<String>,
    pub head_sha: Option<String>,
    pub confirmed_contained_head_oid: Option<String>,
    pub conflict_summary: Option<crate::hosted_review::PrConflictSummary>,
}

/// `HostedReviewFromGitHubPRInfoArgs` (`hosted-review-github.ts:4-15`).
///
/// J2: `comments` is `Option<&[HostedReviewCommentInput]>`, not
/// `Option<Vec<...>>` with a separate "was it fetched" flag — `None` means
/// "not fetched" and `Some(&[])` means "fetched, empty," and that absent-vs-
/// empty distinction is the single most load-bearing decision in this
/// cluster (see [`unresolved_thread_count`]).
pub struct HostedReviewFromGitHubPrInfoArgs<'a> {
    pub pr: GitHubPrInfo,
    pub owner: String,
    pub repo: String,
    /// TS `host?: string` — optional-non-null (M1's G1).
    pub host: Option<String>,
    /// TS `authorLogin?: string | null` — optional-and-nullable (M1's G1).
    pub author_login: Option<String>,
    pub author_is_bot: Option<bool>,
    /// TS `requestedReviewerLogins?: string[] | null` — optional-and-nullable
    /// (M1's G1).
    pub requested_reviewer_logins: Option<Vec<String>>,
    /// TS `comments?: PRComment[]`. J1: ported here as
    /// `Option<&[HostedReviewCommentInput]>`, the local input type, not the
    /// shared `PrComment`.
    pub comments: Option<&'a [HostedReviewCommentInput]>,
    /// TS `checks?: PRCheckDetail[]`.
    pub checks: Option<&'a [PrCheckDetail]>,
    pub last_viewed_at: Option<u64>,
}

/// Verbatim port of private `unresolvedThreadCount` (`hosted-review-github.ts:17-29`).
///
/// J2 (**most load-bearing pin in this cluster**): `comments: None` (JS
/// `undefined`) returns `None` immediately (`:18-20`) — "we don't know."
/// `comments: Some(&[])` falls through to the loop (which does nothing) and
/// returns `Some(0)` — "we know, and there are zero." Callers
/// ([`hosted_review_summary_from_github_pr_info`]) turn that distinction into
/// `thread_summary: None` ("unknown," oracle `github.test.ts:105-113`) vs.
/// `thread_summary: Some(HostedReviewThreadSummary { unresolved_count:
/// Some(0), .. })` ("loaded, none," oracle `github.test.ts:114-122`).
///
/// J3 (skip rule, verbatim from `:23-25`): `if (!comment.threadId ||
/// comment.isResolved !== false) continue`. A comment is skipped when
/// `thread_id` is `None` **or** `Some("")` (empty string is JS-falsy, matches
/// `!comment.threadId`), **or** when `is_resolved != Some(false)` — which
/// means both an absent `is_resolved` and an explicit `Some(true)` count as
/// "resolved" and are skipped. Surviving `thread_id`s are deduplicated via a
/// `HashSet` (exact string equality, no case folding) and the set's `len()`
/// is returned — matching JS `Set<string>.size`.
pub(crate) fn unresolved_thread_count(comments: Option<&[HostedReviewCommentInput]>) -> Option<u32> {
    let comments = comments?;
    let mut unresolved: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for comment in comments {
        let Some(thread_id) = comment.thread_id.as_deref() else {
            continue;
        };
        if thread_id.is_empty() {
            continue;
        }
        if comment.is_resolved != Some(false) {
            continue;
        }
        unresolved.insert(thread_id);
    }
    Some(unresolved.len() as u32)
}

/// Verbatim port of private `deriveChecksStatus` (`hosted-review-github.ts:31-62`).
///
/// J4: `checks: None` **or** `Some(&[])` (empty slice) returns
/// `pr_checks_status` unchanged (`:35-37`) — no enrichment data means no
/// override. Otherwise, in strict priority order:
///
/// 1. **failure** — any check's conclusion is `Failure`, `TimedOut`,
///    `Cancelled`, or `ActionRequired` (`:38-46`). The `ActionRequired` arm
///    carries the source's own rationale: a check suite awaiting manual
///    approval has no check run and would otherwise look "clean," so it is
///    folded into failure to keep the review queue honest.
/// 2. **pending** — any check has `status != Completed`, **or**
///    `conclusion.is_none()`, **or** `conclusion == Some(Pending)`
///    (`:50-53`).
/// 3. **success** — any check's conclusion is `Success` (`:57-59`).
/// 4. **neutral** — fallthrough when none of the above matched (`:61`).
///
/// ⚠ M4's GitLab normalizer uses a **different**, 2-entry failure set
/// (`{failure, timed_out}` only — no `cancelled`/`action_required`, per the
/// plan's T31/J4 note). If these two normalizers are ever unified into one
/// shared classifier, the failure set **must** become a parameter; this
/// function stays GitHub-only and hardcodes the GitHub 4-entry set.
fn derive_checks_status(
    pr_checks_status: crate::hosted_review::CheckStatus,
    checks: Option<&[PrCheckDetail]>,
) -> crate::hosted_review::CheckStatus {
    use crate::hosted_review::CheckStatus;

    let Some(checks) = checks else {
        return pr_checks_status;
    };
    if checks.is_empty() {
        return pr_checks_status;
    }

    let has_failure = checks.iter().any(|check| {
        matches!(
            check.conclusion,
            Some(CheckConclusion::Failure)
                | Some(CheckConclusion::TimedOut)
                | Some(CheckConclusion::Cancelled)
                | Some(CheckConclusion::ActionRequired)
        )
    });
    if has_failure {
        return CheckStatus::Failure;
    }

    let has_pending = checks.iter().any(|check| {
        check.status != CheckRunStatus::Completed
            || check.conclusion.is_none()
            || check.conclusion == Some(CheckConclusion::Pending)
    });
    if has_pending {
        return CheckStatus::Pending;
    }

    let has_success = checks
        .iter()
        .any(|check| check.conclusion == Some(CheckConclusion::Success));
    if has_success {
        return CheckStatus::Success;
    }

    CheckStatus::Neutral
}

/// `PRState` and `HostedReviewState` are separate types by design (M1's G6);
/// TS gets away with `state: args.pr.state` (`:78`) directly only because
/// both are structurally-identical string literal unions and TS uses
/// structural typing. Rust's nominal typing needs an explicit, lossless
/// 1:1 mapping — this is a Rust-only necessity, not a behavior change.
fn hosted_review_state_from_pr_state(
    state: crate::hosted_review::PrState,
) -> crate::hosted_review::HostedReviewState {
    use crate::hosted_review::{HostedReviewState, PrState};
    match state {
        PrState::Open => HostedReviewState::Open,
        PrState::Closed => HostedReviewState::Closed,
        PrState::Merged => HostedReviewState::Merged,
        PrState::Draft => HostedReviewState::Draft,
    }
}

/// Verbatim port of `hostedReviewSummaryFromGitHubPRInfo` (`hosted-review-github.ts:64-105`).
///
/// - **J5** (`:71`): `args.host ?? 'github.com'` uses JS's nullish-coalescing
///   `??`, not `||` — only `None`/`null`/`undefined` fall back to the
///   default. `Some("")` (an empty-but-present host) stays `""` verbatim.
///   Do **not** add an is-empty filter here; that would silently change `??`
///   into `||` semantics.
/// - **J6** (`:79`): `args.authorLogin ? { login, isBot } : null` is JS
///   truthiness — `Some("")` is falsy, so `author` becomes `None`. `is_bot`
///   may itself be `None` and the user object is still constructed whenever
///   the login is non-empty.
/// - **J7** (`:82-84`): `mergeStateStatus` passes through as `Option<String>`
///   unchanged (TS's conditional-spread-on-`!== undefined` collapses
///   naturally into M1's `Option<T>`, per G1).
/// - **J9** (`:86-93`): `reviewDecision` maps `APPROVED`→`approved`,
///   `CHANGES_REQUESTED`→`changes_requested`, `REVIEW_REQUIRED`→
///   `review_required`; every other value (in Rust: only `None`, since the
///   enum is closed) maps to `None`. A `None` `review_decision` **passes**
///   M2's `review_ready_to_merge` gate (H5 in `hosted_review_queue.rs`) —
///   this is the lenient contract, not an oversight.
/// - **J10**: `dataCompleteness` is hardcoded `'partial'` (`:99`) whenever a
///   thread summary exists at all.
/// - **J11** (`:103`): `draft` is *derived* from `pr.state == 'draft'`, not a
///   separate input field.
pub fn hosted_review_summary_from_github_pr_info(
    args: HostedReviewFromGitHubPrInfoArgs<'_>,
) -> crate::hosted_review::HostedReviewQueueSummary {
    use crate::hosted_review::{
        HostedReviewDecision, HostedReviewIdentity, HostedReviewProvider, HostedReviewQueueSummary,
        HostedReviewThreadDataCompleteness, HostedReviewThreadSummary, HostedReviewUser,
        PrReviewDecisionAggregate, PrState,
    };

    let unresolved_count = unresolved_thread_count(args.comments);
    let checks_status = derive_checks_status(args.pr.checks_status, args.checks);
    let draft = Some(args.pr.state == PrState::Draft);
    let state = hosted_review_state_from_pr_state(args.pr.state);

    // J6: truthiness on `authorLogin` — `Some("")` collapses to `None`.
    let author = args
        .author_login
        .filter(|login| !login.is_empty())
        .map(|login| HostedReviewUser {
            login: Some(login),
            is_bot: args.author_is_bot,
        });

    // J9: closed mapping; every other value (only `None` is reachable in
    // Rust's closed enum) becomes `None`.
    let review_decision = match args.pr.review_decision {
        Some(PrReviewDecisionAggregate::Approved) => Some(HostedReviewDecision::Approved),
        Some(PrReviewDecisionAggregate::ChangesRequested) => {
            Some(HostedReviewDecision::ChangesRequested)
        }
        Some(PrReviewDecisionAggregate::ReviewRequired) => {
            Some(HostedReviewDecision::ReviewRequired)
        }
        None => None,
    };

    // J2/J10: `None` unresolved count => no thread summary at all ("unknown");
    // `Some(n)` => a thread summary hardcoding `Partial` completeness.
    let thread_summary = unresolved_count.map(|count| HostedReviewThreadSummary {
        unresolved_count: Some(count),
        data_completeness: Some(HostedReviewThreadDataCompleteness::Partial),
    });

    HostedReviewQueueSummary {
        identity: HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            // J5: `??`, not `||` — `Some("")` stays `""`.
            host: args.host.unwrap_or_else(|| "github.com".to_string()),
            owner: args.owner,
            repo: args.repo,
            number: args.pr.number,
        },
        title: args.pr.title,
        url: args.pr.url,
        state,
        author,
        updated_at: args.pr.updated_at,
        last_viewed_at: args.last_viewed_at,
        mergeable: args.pr.mergeable,
        merge_state_status: args.pr.merge_state_status,
        checks_status,
        review_decision,
        thread_summary,
        requested_reviewer_logins: args.requested_reviewer_logins,
        draft,
    }
}

/// Verbatim port of `hostedReviewInfoFromGitHubPRInfo` (`hosted-review-github.ts:107-128`).
///
/// - **J12** (`:114`): `pr.checksStatus` maps to the Rust field named
///   `status` — [`crate::hosted_review::HostedReviewInfo::status`] — a
///   deliberate rename the oracle test (`github.test.ts:124-136`) checks by
///   asserting `status: 'pending'` on the result.
/// - **J7**: `reviewDecision`, `autoMergeEnabled`, `autoMergeAllowed`,
///   `mergeQueueRequired`, `mergeStateStatus` pass through as `Option<T>`
///   unchanged (`:117-121`; conditional-spread-on-`!== undefined` collapses
///   into `Option<T>` per M1's G1). Note `HostedReviewInfo::review_decision`
///   is already `Option<PrReviewDecisionAggregate>` — the *same* type
///   [`GitHubPrInfo::review_decision`] uses — so unlike the summary
///   function's J9 remapping, this one is a plain passthrough with no
///   re-mapping at all.
/// - **J8** (`:122-126`): BUT `headSha`, `confirmedContainedHeadOid`, and
///   `conflictSummary` use a **truthiness** guard in the source
///   (`pr.headSha ? {...} : {}`), not `!== undefined` — an empty string is
///   falsy and gets **dropped** to `None`. This is a different guard than
///   J7's `!== undefined` **within the same source file** — do not
///   normalize the two to look alike. (`conflictSummary` is an object, never
///   an empty string, so its truthiness check is behaviorally a no-op
///   relative to a plain `Option` passthrough — but it is still grouped here
///   because the source's syntax groups it with the two string fields.)
/// - **J13**: `baseRefName` is **not** copied — the TS source declares
///   `HostedReviewInfo.baseRefName` but this function's return object
///   (`:108-128`) never sets it. This is a deliberate upstream gap; the
///   field is always `None` here and must **not** be "fixed" by threading a
///   `base_ref_name` value through [`GitHubPrInfo`] (which correspondingly
///   has no such field — see that struct's doc comment).
pub fn hosted_review_info_from_github_pr_info(
    pr: &GitHubPrInfo,
) -> crate::hosted_review::HostedReviewInfo {
    use crate::hosted_review::HostedReviewInfo;
    use crate::hosted_review::HostedReviewProvider;

    HostedReviewInfo {
        provider: HostedReviewProvider::Github,
        number: pr.number,
        title: pr.title.clone(),
        state: hosted_review_state_from_pr_state(pr.state),
        url: pr.url.clone(),
        // J12: rename `checksStatus` -> `status`.
        status: pr.checks_status,
        updated_at: pr.updated_at.clone(),
        mergeable: pr.mergeable,
        // J7: plain passthrough, no remapping (same enum on both sides).
        review_decision: pr.review_decision,
        auto_merge_enabled: pr.auto_merge_enabled,
        auto_merge_allowed: pr.auto_merge_allowed,
        merge_queue_required: pr.merge_queue_required,
        merge_state_status: pr.merge_state_status.clone(),
        // J8: truthiness guard — empty string dropped to `None`.
        head_sha: pr.head_sha.clone().filter(|s| !s.is_empty()),
        confirmed_contained_head_oid: pr
            .confirmed_contained_head_oid
            .clone()
            .filter(|s| !s.is_empty()),
        // J13: deliberately never populated — upstream gap, do not "fix".
        base_ref_name: None,
        // J8: truthiness on an object is a no-op relative to `Option`
        // passthrough (no object is ever JS-falsy), grouped with the two
        // string fields above only because the source's syntax groups them.
        conflict_summary: pr.conflict_summary.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosted_review::{
        CheckStatus, HostedReviewDecision, HostedReviewState, HostedReviewThreadDataCompleteness,
        PrMergeableState, PrReviewDecisionAggregate, PrState,
    };

    /// `pr` fixture (`hosted-review-github.test.ts:8-17`).
    fn base_pr() -> GitHubPrInfo {
        GitHubPrInfo {
            number: 12,
            title: "Add queue badges".to_string(),
            state: PrState::Open,
            url: "https://github.com/acme/orca/pull/12".to_string(),
            checks_status: CheckStatus::Pending,
            updated_at: "2026-05-12T00:00:00.000Z".to_string(),
            mergeable: PrMergeableState::Mergeable,
            review_decision: None,
            auto_merge_enabled: None,
            auto_merge_allowed: None,
            merge_queue_required: None,
            merge_state_status: None,
            head_sha: Some("abc123".to_string()),
            confirmed_contained_head_oid: None,
            conflict_summary: None,
        }
    }

    fn base_args<'a>(pr: GitHubPrInfo) -> HostedReviewFromGitHubPrInfoArgs<'a> {
        HostedReviewFromGitHubPrInfoArgs {
            pr,
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            host: None,
            author_login: None,
            author_is_bot: None,
            requested_reviewer_logins: None,
            comments: None,
            checks: None,
            last_viewed_at: None,
        }
    }

    // ── Oracle: hosted-review-github.test.ts (6/6) ─────────────────────

    /// `:20-37` "maps PRInfo into provider-neutral summary with host identity".
    #[test]
    fn oracle_maps_pr_info_with_host_identity() {
        let mut args = base_args(base_pr());
        args.host = Some("github.acme.internal".to_string());
        let summary = hosted_review_summary_from_github_pr_info(args);

        assert_eq!(summary.identity.provider.as_str(), "github");
        assert_eq!(summary.identity.host, "github.acme.internal");
        assert_eq!(summary.identity.owner, "acme");
        assert_eq!(summary.identity.repo, "orca");
        assert_eq!(summary.identity.number, 12);
        assert_eq!(summary.checks_status, CheckStatus::Pending);
        assert!(summary.thread_summary.is_none());
    }

    /// `:39-81` "derives unresolved thread count and failing status from enrichers".
    #[test]
    fn oracle_derives_unresolved_count_and_failing_status() {
        let mut pr = base_pr();
        pr.checks_status = CheckStatus::Success;
        let comments = vec![
            HostedReviewCommentInput {
                thread_id: Some("t1".to_string()),
                is_resolved: Some(false),
            },
            HostedReviewCommentInput {
                thread_id: Some("t1".to_string()),
                is_resolved: Some(false),
            },
            HostedReviewCommentInput {
                thread_id: Some("t2".to_string()),
                is_resolved: Some(true),
            },
        ];
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Failure),
        }];
        let mut args = base_args(pr);
        args.comments = Some(&comments);
        args.checks = Some(&checks);
        let summary = hosted_review_summary_from_github_pr_info(args);

        let thread_summary = summary.thread_summary.expect("thread summary present");
        assert_eq!(thread_summary.unresolved_count, Some(1));
        assert_eq!(
            thread_summary.data_completeness,
            Some(HostedReviewThreadDataCompleteness::Partial)
        );
        assert_eq!(summary.checks_status, CheckStatus::Failure);
    }

    /// `:83-92` "treats cancelled checks as failed".
    #[test]
    fn oracle_cancelled_checks_are_failure() {
        let mut pr = base_pr();
        pr.checks_status = CheckStatus::Success;
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Cancelled),
        }];
        let mut args = base_args(pr);
        args.checks = Some(&checks);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.checks_status, CheckStatus::Failure);
    }

    /// `:94-103` "treats action_required checks as failed".
    #[test]
    fn oracle_action_required_checks_are_failure() {
        let mut pr = base_pr();
        pr.checks_status = CheckStatus::Success;
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::ActionRequired),
        }];
        let mut args = base_args(pr);
        args.checks = Some(&checks);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.checks_status, CheckStatus::Failure);
    }

    /// `:105-122` "distinguishes loaded empty comments from unknown comments"
    /// — J2, the single most load-bearing pin in this cluster.
    #[test]
    fn oracle_distinguishes_absent_from_empty_comments() {
        let args_absent = base_args(base_pr());
        let summary_absent = hosted_review_summary_from_github_pr_info(args_absent);
        assert!(summary_absent.thread_summary.is_none());

        let empty: Vec<HostedReviewCommentInput> = Vec::new();
        let mut args_empty = base_args(base_pr());
        args_empty.comments = Some(&empty);
        let summary_empty = hosted_review_summary_from_github_pr_info(args_empty);
        assert_eq!(
            summary_empty.thread_summary,
            Some(crate::hosted_review::HostedReviewThreadSummary {
                unresolved_count: Some(0),
                data_completeness: Some(HostedReviewThreadDataCompleteness::Partial),
            })
        );
    }

    /// `:124-136` "maps PRInfo into sidebar hosted review metadata" — J12 rename pin.
    #[test]
    fn oracle_maps_info_with_status_rename() {
        let pr = base_pr();
        let review = hosted_review_info_from_github_pr_info(&pr);
        assert_eq!(review.provider.as_str(), "github");
        assert_eq!(review.number, 12);
        assert_eq!(review.title, "Add queue badges");
        assert_eq!(review.state, HostedReviewState::Open);
        assert_eq!(review.status, CheckStatus::Pending);
        assert_eq!(review.mergeable, PrMergeableState::Mergeable);
        assert_eq!(review.head_sha, Some("abc123".to_string()));
    }

    // ── J2/J3 mandatory extra pins ──────────────────────────────────────

    #[test]
    fn pin_j3_empty_string_thread_id_is_skipped() {
        let comments = vec![HostedReviewCommentInput {
            thread_id: Some(String::new()),
            is_resolved: Some(false),
        }];
        assert_eq!(unresolved_thread_count(Some(&comments)), Some(0));
    }

    #[test]
    fn pin_j3_absent_is_resolved_is_treated_as_resolved() {
        let comments = vec![HostedReviewCommentInput {
            thread_id: Some("t1".to_string()),
            is_resolved: None,
        }];
        assert_eq!(unresolved_thread_count(Some(&comments)), Some(0));
    }

    #[test]
    fn pin_j3_is_resolved_true_is_skipped() {
        let comments = vec![HostedReviewCommentInput {
            thread_id: Some("t1".to_string()),
            is_resolved: Some(true),
        }];
        assert_eq!(unresolved_thread_count(Some(&comments)), Some(0));
    }

    #[test]
    fn pin_j3_dedupe_two_comments_same_thread_count_once() {
        let comments = vec![
            HostedReviewCommentInput {
                thread_id: Some("shared".to_string()),
                is_resolved: Some(false),
            },
            HostedReviewCommentInput {
                thread_id: Some("shared".to_string()),
                is_resolved: Some(false),
            },
        ];
        assert_eq!(unresolved_thread_count(Some(&comments)), Some(1));
    }

    // ── J4 mandatory extra pins ──────────────────────────────────────────

    #[test]
    fn pin_j4_timed_out_is_failure() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::TimedOut),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Success, Some(&checks)),
            CheckStatus::Failure
        );
    }

    #[test]
    fn pin_j4_empty_checks_slice_passes_through() {
        let checks: Vec<PrCheckDetail> = Vec::new();
        assert_eq!(
            derive_checks_status(CheckStatus::Pending, Some(&checks)),
            CheckStatus::Pending
        );
    }

    #[test]
    fn pin_j4_none_checks_passes_through() {
        assert_eq!(
            derive_checks_status(CheckStatus::Success, None),
            CheckStatus::Success
        );
    }

    #[test]
    fn pin_j4_pending_condition_status_not_completed() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::InProgress,
            conclusion: Some(CheckConclusion::Success),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Success, Some(&checks)),
            CheckStatus::Pending
        );
    }

    #[test]
    fn pin_j4_pending_condition_conclusion_none() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: None,
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Success, Some(&checks)),
            CheckStatus::Pending
        );
    }

    #[test]
    fn pin_j4_pending_condition_conclusion_pending() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Pending),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Success, Some(&checks)),
            CheckStatus::Pending
        );
    }

    #[test]
    fn pin_j4_success_case() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Success),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Pending, Some(&checks)),
            CheckStatus::Success
        );
    }

    /// Neutral fallthrough: no failure, no pending, no success — e.g. every
    /// check is `Skipped`.
    #[test]
    fn pin_j4_neutral_fallthrough() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Skipped),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Pending, Some(&checks)),
            CheckStatus::Neutral
        );
    }

    // ── J5 mandatory extra pins ──────────────────────────────────────────

    #[test]
    fn pin_j5_none_host_defaults_to_github_com() {
        let args = base_args(base_pr());
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.identity.host, "github.com");
    }

    #[test]
    fn pin_j5_empty_string_host_is_not_defaulted() {
        let mut args = base_args(base_pr());
        args.host = Some(String::new());
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.identity.host, "");
    }

    // ── J6 mandatory extra pins ────────────────────────────────────────

    #[test]
    fn pin_j6_empty_string_author_login_is_none() {
        let mut args = base_args(base_pr());
        args.author_login = Some(String::new());
        args.author_is_bot = Some(true);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert!(summary.author.is_none());
    }

    #[test]
    fn pin_j6_is_bot_is_carried_through() {
        let mut args = base_args(base_pr());
        args.author_login = Some("octobot".to_string());
        args.author_is_bot = Some(true);
        let summary = hosted_review_summary_from_github_pr_info(args);
        let author = summary.author.expect("author present");
        assert_eq!(author.login, Some("octobot".to_string()));
        assert_eq!(author.is_bot, Some(true));
    }

    // ── J8 mandatory extra pin ──────────────────────────────────────────

    #[test]
    fn pin_j8_empty_head_sha_is_dropped() {
        let mut pr = base_pr();
        pr.head_sha = Some(String::new());
        let review = hosted_review_info_from_github_pr_info(&pr);
        assert_eq!(review.head_sha, None);
    }

    // ── J9 mandatory extra pins ──────────────────────────────────────────

    #[test]
    fn pin_j9_approved_maps_to_approved() {
        let mut pr = base_pr();
        pr.review_decision = Some(PrReviewDecisionAggregate::Approved);
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(
            summary.review_decision,
            Some(HostedReviewDecision::Approved)
        );
    }

    #[test]
    fn pin_j9_changes_requested_maps_to_changes_requested() {
        let mut pr = base_pr();
        pr.review_decision = Some(PrReviewDecisionAggregate::ChangesRequested);
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(
            summary.review_decision,
            Some(HostedReviewDecision::ChangesRequested)
        );
    }

    #[test]
    fn pin_j9_review_required_maps_to_review_required() {
        let mut pr = base_pr();
        pr.review_decision = Some(PrReviewDecisionAggregate::ReviewRequired);
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(
            summary.review_decision,
            Some(HostedReviewDecision::ReviewRequired)
        );
    }

    #[test]
    fn pin_j9_none_maps_to_none() {
        let pr = base_pr();
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.review_decision, None);
    }

    // ── J11 mandatory extra pins ──────────────────────────────────────────

    #[test]
    fn pin_j11_draft_state_yields_draft_true() {
        let mut pr = base_pr();
        pr.state = PrState::Draft;
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.draft, Some(true));
    }

    #[test]
    fn pin_j11_open_state_yields_draft_false() {
        let pr = base_pr();
        let args = base_args(pr);
        let summary = hosted_review_summary_from_github_pr_info(args);
        assert_eq!(summary.draft, Some(false));
    }

    // ── J13 mandatory extra pin ────────────────────────────────────────

    #[test]
    fn pin_j13_base_ref_name_is_never_populated() {
        let pr = base_pr();
        let review = hosted_review_info_from_github_pr_info(&pr);
        assert_eq!(review.base_ref_name, None);
    }
}
