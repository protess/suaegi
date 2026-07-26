//! GitLab hosted-review normalizer — verbatim port of Orca's
//! `src/shared/hosted-review-gitlab.ts` (125L, @ v1.4.150-rc.0).
//!
//! This module is **M4** of a 4-milestone port (see
//! `docs/superpowers/plans/2026-07-25-hosted-review-m4.md`, decisions
//! K1–K9), completing the cluster started by M1
//! ([`crate::hosted_review`]), M2 ([`crate::hosted_review_queue`]), and M3
//! ([`crate::hosted_review_github`]). It consumes M1's vocabulary and reuses
//! two items from M3 (K8): [`crate::hosted_review_github::unresolved_thread_count`]
//! (now `pub(crate)`, the ONLY edit made to that file) and
//! [`crate::hosted_review_github::HostedReviewCommentInput`] — the TS
//! functions behind both are character-identical between the GitHub and
//! GitLab source files, so duplicating them here would be pure churn.
//!
//! The one new dependency this module needs is the `url` crate (already a
//! workspace dependency, used elsewhere by `suaegi-git` and `suaegi-workref`)
//! for `parse_gitlab_identity`'s URL parsing.
//!
//! # K1 — WHATWG `URL.host` includes the port; Rust `Url::host_str()` doesn't
//!
//! JS `URL.host` returns `"host:port"` when a non-default port is present,
//! and just `"host"` otherwise (including when the port equals the scheme's
//! default, e.g. `:443` on `https:` or `:80` on `http:` — WHATWG omits
//! those). Rust's `url::Url::host_str()` never includes the port at all;
//! the port lives in the separate `Url::port()` accessor, which **already**
//! returns `None` for a scheme-default port. So reassembling
//! `format!("{host}:{port}")` only when `port()` is `Some` reproduces WHATWG
//! `.host` exactly — this was verified empirically against `url` 2.5.8
//! against `https://host:8443/...` (→ `"host:8443"`), `https://host:443/...`
//! (→ `"host"`, port suppressed), and `http://host:80/...` (→ `"host"`).
//! **Do not** reach for `Url::port_or_known_default()` here — it always
//! returns a port (falling back to the scheme default), which would wrongly
//! append `:443`/`:80` where WHATWG (and this reassembly) omits them.
//!
//! # K2 — `path()` is used raw, not percent-decoded
//!
//! `parsed.pathname` in JS is already percent-encoded (e.g. a literal `%2F`
//! in a group path stays `%2F`, it is not decoded to `/`); Rust's
//! `Url::path()` behaves the same way (verified: it returns the raw,
//! percent-encoded path, never decoding `%2F` or any other escape). The path
//! is split on `/` and empty segments are dropped (`filter(Boolean)` in TS,
//! `!s.is_empty()` here) — this naturally absorbs a leading `/` and any
//! doubled `//` without any special-casing.
//!
//! # K3 — the first `-` segment marks the project/route-action boundary
//!
//! GitLab URLs put resource-type markers (`-/merge_requests/12`,
//! `-/issues/3`, ...) after a literal `-` path segment. The **first** such
//! segment (`position`, not `rposition`) marks where the group/subgroup/repo
//! path ends: `project_segments` is `&segments[..marker_index]` when found,
//! or all of `segments` when absent. A marker at index 0 (e.g.
//! `https://host/-/merge_requests/1`) yields an **empty** project-segment
//! list, which falls into the `< 2` branch below and produces owner/repo
//! `"unknown"`/`"unknown"` — but note the **host is still the parsed host**,
//! not reset to `gitlab.com`; that reset only happens in the `catch` arm
//! (K4). Do not special-case marker-at-0 to look like a parse failure — the
//! source doesn't.
//!
//! # K4 — `Url::parse` failure resets the host too (unlike K3)
//!
//! When the URL fails to parse at all (a relative path, or a string that
//! isn't a URL), the source's `catch` arm returns a **fully** degraded
//! `{ host: 'gitlab.com', owner: 'unknown', repo: 'unknown' }` — host
//! included. This is the one place the parsed host is discarded outright;
//! contrast K3, where a marker-at-0 still keeps whatever host *was*
//! successfully parsed. Two different "unknown" shapes for two different
//! failure kinds, and both must stay distinct.
//!
//! # K5 — the two branches, ported literally
//!
//! `>= 2` project segments: `owner` is every segment except the last,
//! `join("/")`'d back together (so nested groups like `a/b/c` become the
//! single string `"a/b/c"`); `repo` is the last segment
//! (`project_segments.at(-1) ?? 'unknown'` in the source — defensive but
//! actually unreachable in this branch, since the branch guard already
//! guarantees at least 2 elements; ported as `unwrap_or("unknown")` anyway
//! for literal fidelity). `< 2` branch: `owner` is `segments.get(0)` (or
//! `"unknown"`), `repo` is `segments.get(1)` (or `"unknown"` — and since the
//! branch guard means there is never a second segment here, `repo` is
//! **always** `"unknown"` in this branch). In **both** branches, the source
//! uses `parsed.host || 'gitlab.com'` — JS `||`, not `??` — so an **empty**
//! reassembled host string (K1's `_ => String::new()` arm) triggers the
//! `"gitlab.com"` fallback exactly like a missing host would.
//!
//! # K6 — a GitLab-specific `derive_checks_status`, deliberately NOT unified with M3's
//!
//! GitLab's failure set is **only** `{failure, timed_out}` — two entries.
//! [`crate::hosted_review_github::derive_checks_status`] (M3) uses a
//! **four**-entry failure set (`+ cancelled + action_required`, with its own
//! documented rationale for folding `action_required` into failure). These
//! are genuinely different source functions in Orca, not one function
//! reused across providers, so this module defines its own copy rather than
//! calling into M3's. If the two are ever merged into one shared classifier,
//! the failure set must become a parameter — until then this duplication is
//! intentional (see the plan's "Deferred" section). Everything else — empty/
//! absent `checks` passing the input status through unchanged, the 3-way
//! pending check, the success check, and the neutral fallthrough — is
//! identical in shape to M3's version, just with the smaller failure set.
//!
//! # K7 — `requested_reviewer_logins` is never set (`None`)
//!
//! The source's return object (`:89-124`) has no `requestedReviewerLogins`
//! key at all — unlike the GitHub normalizer, which threads a
//! `requestedReviewerLogins` argument straight through. This is preserved
//! verbatim: this module's [`hosted_review_summary_from_gitlab_info`] always
//! produces `requested_reviewer_logins: None`, even if a caller conceptually
//! has requested-reviewer data available for a GitLab MR. Downstream, M2's
//! `hasRequestedReviewerSignal` check can therefore never see reviewer data
//! for GitLab, so a GitLab review can never classify into the `Requested`
//! queue state. This looks like a plausible upstream gap in Orca, not a
//! deliberate design choice, but the mandate here is a faithful verbatim
//! port — so it is preserved rather than "fixed".
//!
//! # K9 — field-name and mapping notes
//!
//! Identity is derived from `args.review.url` (unlike GitHub's normalizer,
//! which takes `owner`/`repo`/`host` as separate arguments) via
//! [`parse_gitlab_identity`]. The input field carrying the review's own
//! check-status enrichment baseline is named `status` here
//! ([`GitLabReviewInfo::status`]), not `checksStatus`/`checks_status` as on
//! the GitHub side (M3's J12) — `deriveChecksStatus(args.review.status,
//! args.checks)` in the source. Author truthiness (`args.authorLogin ? {...}
//! : null`, so `Some("")` collapses to `None`), the 3-way `reviewDecision`
//! mapping (`APPROVED`/`CHANGES_REQUESTED`/`REVIEW_REQUIRED`, everything
//! else — in Rust's closed enum, only `None` — mapping to `None`), the
//! thread-summary construction (hardcoded `Partial` completeness whenever an
//! unresolved count exists at all), `lastViewedAt` passthrough, and
//! `draft = state == 'draft'` all follow the exact same rules M3 already
//! documents (J6, J9, J10, J11) — they are not re-derived independently
//! here, since the source itself follows the identical pattern.

use crate::hosted_review::{
    CheckStatus, HostedReviewDecision, HostedReviewIdentity, HostedReviewProvider,
    HostedReviewQueueSummary, HostedReviewState, HostedReviewThreadDataCompleteness,
    HostedReviewThreadSummary, HostedReviewUser, PrMergeableState, PrReviewDecisionAggregate,
};
use crate::hosted_review_github::{
    unresolved_thread_count, HostedReviewCommentInput, PrCheckDetail,
};

/// `GitLabIdentityParts` (`hosted-review-gitlab.ts:13-17`), private to this
/// module (`parse_gitlab_identity`'s return type is not part of the TS
/// module's public surface either).
struct GitLabIdentityParts {
    host: String,
    owner: String,
    repo: String,
}

/// The subset of `HostedReviewInfo` (`types.ts` / `hosted-review.ts:18-38`)
/// that `hostedReviewSummaryFromGitLabInfo` actually reads: `number`,
/// `title`, `url`, `state`, `status`, `updatedAt`, `mergeable`,
/// `mergeStateStatus`, `reviewDecision`. Fields such as `autoMergeEnabled`,
/// `headSha`, `conflictSummary`, etc. exist on the real `HostedReviewInfo`
/// but are never touched by this function, so they are intentionally
/// omitted here (same pattern as M3's `GitHubPrInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitLabReviewInfo {
    /// TS `number` — `u64` per this crate's PR/MR-number convention (M1's G4).
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: HostedReviewState,
    /// K9: the source names this input field `status`, not `checksStatus`.
    pub status: CheckStatus,
    pub updated_at: String,
    pub mergeable: PrMergeableState,
    /// TS `mergeStateStatus?: string | null` — optional-and-nullable (M1's G1).
    pub merge_state_status: Option<String>,
    /// TS `reviewDecision?: PRReviewDecision | null` — optional-and-nullable
    /// (M1's G1).
    pub review_decision: Option<PrReviewDecisionAggregate>,
}

/// `HostedReviewFromGitLabInfoArgs` (`hosted-review-gitlab.ts:4-11`).
pub struct HostedReviewFromGitLabInfoArgs<'a> {
    pub review: GitLabReviewInfo,
    /// TS `authorLogin?: string | null` — optional-and-nullable (M1's G1).
    pub author_login: Option<String>,
    pub author_is_bot: Option<bool>,
    /// TS `comments?: PRComment[]`. Reuses M3's local
    /// [`HostedReviewCommentInput`] input type (K8), not the shared
    /// `pr_actions::PrComment`.
    pub comments: Option<&'a [HostedReviewCommentInput]>,
    /// TS `checks?: PRCheckDetail[]`. Reuses M3's [`PrCheckDetail`] (same
    /// shared source type on both the GitHub and GitLab sides).
    pub checks: Option<&'a [PrCheckDetail]>,
    pub last_viewed_at: Option<u64>,
}

/// Verbatim port of private `parseGitLabIdentity` (`hosted-review-gitlab.ts:19-41`).
/// See the module doc comment's K1–K5 for the full rationale.
fn parse_gitlab_identity(url: &str) -> GitLabIdentityParts {
    let Ok(parsed) = url::Url::parse(url) else {
        // K4: parse failure resets the host too (contrast K3, which keeps
        // the parsed host when only the marker-at-0 case degrades owner/repo).
        return GitLabIdentityParts {
            host: "gitlab.com".to_string(),
            owner: "unknown".to_string(),
            repo: "unknown".to_string(),
        };
    };

    // K1: WHATWG `URL.host` = host_str() + port() reassembled; do NOT use
    // `port_or_known_default()` (it would wrongly re-add scheme-default ports).
    let host = match (parsed.host_str(), parsed.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        _ => String::new(),
    };

    // K2: `path()` preserves percent-encoding (e.g. literal `%2F` stays
    // literal) — do not decode. Drop empty segments (`filter(Boolean)`).
    let segments: Vec<&str> = parsed.path().split('/').filter(|s| !s.is_empty()).collect();

    // K3: FIRST `-` segment (`position`, not `rposition`) marks the
    // group/project boundary.
    let marker_index = segments.iter().position(|s| *s == "-");
    let project_segments: &[&str] = match marker_index {
        Some(i) => &segments[..i],
        None => &segments[..],
    };

    // K5: both branches fall back to "gitlab.com" via `||` semantics — an
    // EMPTY reassembled host (not just a missing one) triggers the fallback.
    let host = if host.is_empty() {
        "gitlab.com".to_string()
    } else {
        host
    };

    if project_segments.len() >= 2 {
        GitLabIdentityParts {
            host,
            owner: project_segments[..project_segments.len() - 1].join("/"),
            repo: project_segments
                .last()
                .copied()
                .unwrap_or("unknown")
                .to_string(),
        }
    } else {
        GitLabIdentityParts {
            host,
            owner: project_segments
                .first()
                .copied()
                .unwrap_or("unknown")
                .to_string(),
            repo: project_segments
                .get(1)
                .copied()
                .unwrap_or("unknown")
                .to_string(),
        }
    }
}

/// GitLab-specific verbatim port of private `deriveChecksStatus`
/// (`hosted-review-gitlab.ts:57-82`).
///
/// K6: the failure set here is deliberately only 2 entries —
/// `{Failure, TimedOut}` — unlike
/// [`crate::hosted_review_github::derive_checks_status`]'s 4-entry set
/// (`+ Cancelled + ActionRequired`). Do not "unify" the two functions; if
/// that is ever done, the failure set must become a parameter. Everything
/// else (empty/absent `checks` pass through, the 3-condition pending check,
/// the success check, neutral fallthrough) is identical in shape to M3's
/// version.
fn derive_checks_status(
    review_status: CheckStatus,
    checks: Option<&[PrCheckDetail]>,
) -> CheckStatus {
    use crate::hosted_review_github::{CheckConclusion, CheckRunStatus};

    let Some(checks) = checks else {
        return review_status;
    };
    if checks.is_empty() {
        return review_status;
    }

    // K6: GitLab's failure set — ONLY failure + timed_out (no cancelled, no
    // action_required — contrast M3's 4-entry set).
    let has_failure = checks.iter().any(|check| {
        matches!(
            check.conclusion,
            Some(CheckConclusion::Failure) | Some(CheckConclusion::TimedOut)
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

/// Verbatim port of `hostedReviewSummaryFromGitLabInfo` (`hosted-review-gitlab.ts:84-125`).
pub fn hosted_review_summary_from_gitlab_info(
    args: HostedReviewFromGitLabInfoArgs<'_>,
) -> HostedReviewQueueSummary {
    let identity = parse_gitlab_identity(&args.review.url);
    // K8: reuse M3's `unresolved_thread_count` verbatim (character-identical
    // TS source functions).
    let unresolved_count = unresolved_thread_count(args.comments);

    HostedReviewQueueSummary {
        identity: HostedReviewIdentity {
            provider: HostedReviewProvider::Gitlab,
            host: identity.host,
            owner: identity.owner,
            repo: identity.repo,
            number: args.review.number,
        },
        title: args.review.title,
        url: args.review.url,
        state: args.review.state,
        // K9: truthiness on `authorLogin` — `Some("")` collapses to `None`
        // (same rule as M3's J6).
        author: args
            .author_login
            .filter(|login| !login.is_empty())
            .map(|login| HostedReviewUser {
                login: Some(login),
                is_bot: args.author_is_bot,
            }),
        updated_at: args.review.updated_at,
        last_viewed_at: args.last_viewed_at,
        mergeable: args.review.mergeable,
        merge_state_status: args.review.merge_state_status,
        // K9: input field is named `status` here, not `checksStatus`.
        checks_status: derive_checks_status(args.review.status, args.checks),
        // K9: same closed 3-way mapping as M3's J9 (every other value — in
        // Rust's closed enum, only `None` — maps to `None`).
        review_decision: match args.review.review_decision {
            Some(PrReviewDecisionAggregate::Approved) => Some(HostedReviewDecision::Approved),
            Some(PrReviewDecisionAggregate::ChangesRequested) => {
                Some(HostedReviewDecision::ChangesRequested)
            }
            Some(PrReviewDecisionAggregate::ReviewRequired) => {
                Some(HostedReviewDecision::ReviewRequired)
            }
            None => None,
        },
        // K9/J10: `None` unresolved count => no thread summary at all
        // ("unknown"); `Some(n)` => a thread summary hardcoding `Partial`.
        thread_summary: unresolved_count.map(|count| HostedReviewThreadSummary {
            unresolved_count: Some(count),
            data_completeness: Some(HostedReviewThreadDataCompleteness::Partial),
        }),
        // K7: the source never sets this field for GitLab — always `None`.
        requested_reviewer_logins: None,
        // K9/J11: derived from `state == 'draft'`, not a separate input field.
        draft: Some(args.review.state == HostedReviewState::Draft),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosted_review::PrMergeableState;
    use crate::hosted_review_github::{CheckConclusion, CheckRunStatus};

    /// `review` fixture (`hosted-review-gitlab.test.ts:5-14`).
    fn base_review() -> GitLabReviewInfo {
        GitLabReviewInfo {
            number: 12,
            title: "Add queue badges".to_string(),
            url: "https://gitlab.acme.internal/group/subgroup/orca/-/merge_requests/12".to_string(),
            state: HostedReviewState::Open,
            status: CheckStatus::Pending,
            updated_at: "2026-05-12T00:00:00.000Z".to_string(),
            mergeable: PrMergeableState::Mergeable,
            merge_state_status: None,
            review_decision: None,
        }
    }

    fn base_args<'a>(review: GitLabReviewInfo) -> HostedReviewFromGitLabInfoArgs<'a> {
        HostedReviewFromGitLabInfoArgs {
            review,
            author_login: None,
            author_is_bot: None,
            comments: None,
            checks: None,
            last_viewed_at: None,
        }
    }

    // ── Oracle: hosted-review-gitlab.test.ts (2/2) ─────────────────────

    /// `:17-29` "maps nested GitLab project URLs into provider-neutral identity".
    #[test]
    fn oracle_maps_nested_project_url_into_identity() {
        let summary = hosted_review_summary_from_gitlab_info(base_args(base_review()));

        assert_eq!(summary.identity.provider.as_str(), "gitlab");
        assert_eq!(summary.identity.host, "gitlab.acme.internal");
        assert_eq!(summary.identity.owner, "group/subgroup");
        assert_eq!(summary.identity.repo, "orca");
        assert_eq!(summary.identity.number, 12);
        assert_eq!(summary.checks_status, CheckStatus::Pending);
        assert!(summary.thread_summary.is_none());
    }

    /// `:31-71` "derives unresolved thread count and failing status from enrichers".
    #[test]
    fn oracle_derives_unresolved_count_and_failing_status() {
        let mut review = base_review();
        review.status = CheckStatus::Success;
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
        let mut args = base_args(review);
        args.comments = Some(&comments);
        args.checks = Some(&checks);
        let summary = hosted_review_summary_from_gitlab_info(args);

        assert_eq!(
            summary.thread_summary,
            Some(HostedReviewThreadSummary {
                unresolved_count: Some(1),
                data_completeness: Some(HostedReviewThreadDataCompleteness::Partial),
            })
        );
        assert_eq!(summary.checks_status, CheckStatus::Failure);
    }

    // ── K1 mandatory extra pins ────────────────────────────────────────

    /// Non-default port is included in the reassembled host, exactly as
    /// WHATWG `URL.host` would render it.
    #[test]
    fn pin_k1_non_default_port_is_included_in_host() {
        let mut review = base_review();
        review.url = "https://host:8443/a/b/-/merge_requests/1".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "host:8443");
    }

    /// Scheme-default port (`:443` on `https:`) is omitted — `url::Url::port()`
    /// already returns `None` for it, matching WHATWG's own omission.
    #[test]
    fn pin_k1_default_port_is_omitted_from_host() {
        let mut review = base_review();
        review.url = "https://host:443/a/b".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "host");
    }

    // ── K2 mandatory extra pins ────────────────────────────────────────

    /// A literal `%2F` in the path stays percent-encoded (not decoded to
    /// `/`), so it is treated as a single opaque path segment rather than
    /// being split into two.
    #[test]
    fn pin_k2_percent_2f_stays_literal_not_decoded() {
        let mut review = base_review();
        review.url = "https://host/a%2Fb/repo".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.owner, "a%2Fb");
        assert_eq!(summary.identity.repo, "repo");
    }

    /// Leading and duplicated slashes produce no empty segments — the
    /// project segments are exactly `["a", "b"]`, not padded with `""`.
    #[test]
    fn pin_k2_leading_and_duplicate_slashes_produce_no_empty_segments() {
        let mut review = base_review();
        review.url = "https://host//a//b//".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.owner, "a");
        assert_eq!(summary.identity.repo, "b");
    }

    // ── K3 mandatory extra pin ─────────────────────────────────────────

    /// A `-` marker as the very FIRST path segment yields an empty project
    /// segment list -> owner/repo both `"unknown"`, but the host is still
    /// the parsed host, NOT reset to `gitlab.com` (contrast K4).
    #[test]
    fn pin_k3_marker_at_index_zero_keeps_parsed_host() {
        let mut review = base_review();
        review.url = "https://host.example.com/-/merge_requests/1".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "host.example.com");
        assert_eq!(summary.identity.owner, "unknown");
        assert_eq!(summary.identity.repo, "unknown");
    }

    /// Two `-` marker segments in the path distinguish `position` (first,
    /// correct) from `rposition` (last, wrong). The FIRST marker sits at
    /// index 1, so `project_segments` is `["group"]` (len 1, `< 2` branch):
    /// owner `"group"`, repo `"unknown"`. Under `rposition` the LAST marker
    /// sits at index 3, giving `["group", "-", "sub"]` (len 3, `>= 2`
    /// branch): owner `"group/-"`, repo `"sub"` — a different pair entirely.
    // Why: mutating `.position(|s| *s == "-")` to `.rposition(...)` in
    // `parse_gitlab_identity` currently passes the whole suite because no
    // existing test has a path with two `-` segments; this test's owner/repo
    // are only reachable when the FIRST marker is used.
    #[test]
    fn pin_k3_multiple_dash_markers_uses_first_not_last() {
        let mut review = base_review();
        review.url = "https://gl.example.com/group/-/sub/-/merge_requests/9".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "gl.example.com");
        assert_eq!(summary.identity.owner, "group");
        assert_eq!(summary.identity.repo, "unknown");
    }

    // ── K4 mandatory extra pins ────────────────────────────────────────

    /// A relative URL fails to parse -> full degradation, host included
    /// (reset to `gitlab.com`), unlike K3's marker-at-0 case.
    #[test]
    fn pin_k4_relative_url_resets_host_too() {
        let mut review = base_review();
        review.url = "/relative/path".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "gitlab.com");
        assert_eq!(summary.identity.owner, "unknown");
        assert_eq!(summary.identity.repo, "unknown");
    }

    /// A non-URL string also fails to parse -> same full degradation.
    #[test]
    fn pin_k4_non_url_string_resets_host_too() {
        let mut review = base_review();
        review.url = "not a url".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "gitlab.com");
        assert_eq!(summary.identity.owner, "unknown");
        assert_eq!(summary.identity.repo, "unknown");
    }

    // ── K5 mandatory extra pins ────────────────────────────────────────

    /// Exactly one project segment -> `< 2` branch: owner is that one
    /// segment, repo is always `"unknown"`.
    #[test]
    fn pin_k5_one_segment_yields_owner_and_unknown_repo() {
        let mut review = base_review();
        review.url = "https://host/onlyrepo".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.owner, "onlyrepo");
        assert_eq!(summary.identity.repo, "unknown");
    }

    /// Zero project segments -> `< 2` branch: owner AND repo both `"unknown"`.
    #[test]
    fn pin_k5_zero_segments_yields_unknown_owner_and_repo() {
        let mut review = base_review();
        review.url = "https://host/".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.owner, "unknown");
        assert_eq!(summary.identity.repo, "unknown");
    }

    /// A deep nested-group path -> `>= 2` branch: owner is every segment
    /// except the last, joined with `/`; repo is the last segment.
    #[test]
    fn pin_k5_deep_group_path_joins_owner_with_slash() {
        let mut review = base_review();
        review.url = "https://host/a/b/c/orca".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.owner, "a/b/c");
        assert_eq!(summary.identity.repo, "orca");
    }

    /// A `file://` URL PARSES SUCCESSFULLY but has no authority, so the
    /// reassembled host is the empty string — this must still trip the `||`
    /// empty-host fallback to `"gitlab.com"`, distinct from K4's
    /// parse-FAILURE fallback (which also forces owner/repo to `"unknown"`).
    /// Asserting owner/repo come from the path (not `"unknown"`) proves this
    /// went through the success path, not the K4 catch arm.
    // Why: replacing the `if host.is_empty()` condition with `if false` in
    // `parse_gitlab_identity` currently passes the whole suite because no
    // existing test produces a successfully-parsed URL with an empty host;
    // this test's host assertion is only satisfied when the `||` fallback
    // actually fires on a success-path empty host.
    #[test]
    fn pin_k5_successfully_parsed_empty_host_falls_back_to_gitlab_com() {
        let mut review = base_review();
        review.url = "file:///group/orca".to_string();
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.identity.host, "gitlab.com");
        assert_eq!(summary.identity.owner, "group");
        assert_eq!(summary.identity.repo, "orca");
    }

    // ── K6 mandatory extra pins ────────────────────────────────────────

    /// `cancelled` is NOT a failure on GitLab (contrast GitHub, where it is)
    /// — with no other checks present it falls to the pending 3-condition
    /// check: `conclusion == Some(Cancelled)` is not `None` and not
    /// `Some(Pending)`, and `status == Completed`, so it lands on neutral,
    /// not failure and not pending.
    #[test]
    fn pin_k6_cancelled_is_not_a_failure() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Cancelled),
        }];
        let status = derive_checks_status(CheckStatus::Success, Some(&checks));
        assert_ne!(status, CheckStatus::Failure);
        assert_eq!(status, CheckStatus::Neutral);
    }

    /// `action_required` is NOT a failure on GitLab either (contrast GitHub).
    #[test]
    fn pin_k6_action_required_is_not_a_failure() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::ActionRequired),
        }];
        let status = derive_checks_status(CheckStatus::Success, Some(&checks));
        assert_ne!(status, CheckStatus::Failure);
        assert_eq!(status, CheckStatus::Neutral);
    }

    /// `timed_out` IS a failure on GitLab (it's one of the 2 entries in the
    /// failure set).
    #[test]
    fn pin_k6_timed_out_is_a_failure() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::TimedOut),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Success, Some(&checks)),
            CheckStatus::Failure
        );
    }

    /// Empty checks slice passes the review's own status through unchanged.
    #[test]
    fn pin_k6_empty_checks_slice_passes_through() {
        let checks: Vec<PrCheckDetail> = Vec::new();
        assert_eq!(
            derive_checks_status(CheckStatus::Pending, Some(&checks)),
            CheckStatus::Pending
        );
    }

    /// `None` checks also passes the review's own status through unchanged.
    #[test]
    fn pin_k6_none_checks_passes_through() {
        assert_eq!(
            derive_checks_status(CheckStatus::Success, None),
            CheckStatus::Success
        );
    }

    /// Neutral fallthrough: no failure, no pending, no success (e.g. a
    /// `Skipped` conclusion).
    #[test]
    fn pin_k6_neutral_fallthrough() {
        let checks = vec![PrCheckDetail {
            status: CheckRunStatus::Completed,
            conclusion: Some(CheckConclusion::Skipped),
        }];
        assert_eq!(
            derive_checks_status(CheckStatus::Pending, Some(&checks)),
            CheckStatus::Neutral
        );
    }

    // ── K7 mandatory extra pin ─────────────────────────────────────────

    /// Even when comments/checks are populated (simulating a "reviewer data
    /// conceptually present" scenario), `requested_reviewer_logins` is
    /// always `None` for GitLab — the source never sets this field.
    #[test]
    fn pin_k7_requested_reviewer_logins_is_always_none() {
        let comments = vec![HostedReviewCommentInput {
            thread_id: Some("t1".to_string()),
            is_resolved: Some(false),
        }];
        let mut args = base_args(base_review());
        args.comments = Some(&comments);
        let summary = hosted_review_summary_from_gitlab_info(args);
        assert_eq!(summary.requested_reviewer_logins, None);
    }

    // ── K9 mandatory extra pins ────────────────────────────────────────

    /// `Some("")` author login is JS-falsy -> `author` collapses to `None`.
    #[test]
    fn pin_k9_empty_string_author_login_is_none() {
        let mut args = base_args(base_review());
        args.author_login = Some(String::new());
        args.author_is_bot = Some(true);
        let summary = hosted_review_summary_from_gitlab_info(args);
        assert!(summary.author.is_none());
    }

    /// `draft` is derived from `state == 'draft'`, not a separate field.
    #[test]
    fn pin_k9_draft_state_yields_draft_true() {
        let mut review = base_review();
        review.state = HostedReviewState::Draft;
        let summary = hosted_review_summary_from_gitlab_info(base_args(review));
        assert_eq!(summary.draft, Some(true));
    }

    #[test]
    fn pin_k9_open_state_yields_draft_false() {
        let summary = hosted_review_summary_from_gitlab_info(base_args(base_review()));
        assert_eq!(summary.draft, Some(false));
    }
}
