//! Hosted-review queue classifier — verbatim port of Orca's
//! `src/shared/hosted-review-queue.ts` (@ v1.4.150-rc.0), **excluding**
//! `hostedReviewIdentityKey` (`:14-16`), which was already ported in M1
//! (see [`crate::hosted_review_identity_key`]).
//!
//! This module is **M2** of a 4-milestone port (see
//! `docs/superpowers/plans/2026-07-25-hosted-review-m2.md`): the pure
//! classification logic that turns a [`crate::HostedReviewQueueSummary`]
//! into a queue bucket / needs-response / ready-to-merge verdict. It
//! consumes M1's vocabulary (`crate::hosted_review::*`) and adds exactly one
//! new dependency, `chrono`, for RFC3339 timestamp parsing (H1). The
//! GitHub/GitLab normalizers and any `pr_actions.rs` extension are **out of
//! scope** here (M3/M4).
//!
//! # H1 — `Date.parse` becomes strict RFC3339, not general ECMAScript date parsing
//!
//! `queue.ts:88` calls `Date.parse(summary.updatedAt)`, which is JS's
//! famously loose date parser: besides full ISO 8601 / RFC3339 strings, it
//! also accepts offset-less date-times (interpreted as **local** time) and
//! date-only strings like `"2026-05-10"` (interpreted as **UTC** midnight).
//! [`parse_updated_at_ms`] instead uses
//! [`chrono::DateTime::parse_from_rfc3339`], which is strict RFC3339 only —
//! it rejects both of those looser forms.
//!
//! This is a **narrow, documented divergence**: every real producer of
//! `updatedAt` in this cluster is a GitHub or GitLab API timestamp, and both
//! APIs always emit full `Z`-suffixed (or explicit-offset) RFC3339 strings.
//! So the loose-vs-strict distinction is unobservable for any input this
//! function actually receives in production; it would only matter for
//! synthetic test input specifically crafted to be offset-less or
//! date-only, which the oracle test suite does not do.
//!
//! A parse failure — including empty string or any non-date garbage —
//! yields `None`, and [`review_needs_response`] treats that exactly like
//! JS's `Date.parse` returning `NaN`: `Number.isFinite(NaN)` is `false`, so
//! the whole expression short-circuits to `false` without ever reaching the
//! comparison. `parse_updated_at_ms` returning `None` reproduces that via
//! `Option`'s combinators (`and_then`/`map`) instead of a `NaN` sentinel.
//!
//! # H2 (CRITICAL) — timestamp comparison happens in signed (`i64`) space
//!
//! `queue.ts:89`: `updatedAt > summary.lastViewedAt`. `Date.parse` returns a
//! **signed** number (negative for any instant before the 1970-01-01 epoch).
//! M1's `HostedReviewQueueSummary::last_viewed_at` is `Option<u64>` (epoch
//! milliseconds, always non-negative by construction — no real "last
//! viewed" timestamp predates 1970).
//!
//! The comparison **must** happen in `i64` space:
//! `parsed_ms > last_viewed_at as i64`. Casting the *parsed* value to `u64`
//! instead (`parsed_ms as u64 > last_viewed_at`) would be a silent
//! correctness bug: a negative `parsed_ms` (pre-1970 `updatedAt`) cast to
//! `u64` wraps around to a huge positive number, which would then compare
//! greater than almost any real `last_viewed_at` — a **false positive**
//! "needs response" verdict for data that should never trigger one. This
//! module never performs that cast; see [`review_needs_response`]'s pinned
//! regression test `pin_h2_negative_epoch_does_not_wrap_to_false_positive`.
//!
//! The comparison is strict `>` (not `>=`): an `updatedAt` exactly equal to
//! `lastViewedAt` is "already seen," not "newer."
//!
//! # H3 — the `unresolvedCount` asymmetry between the two public functions is intentional
//!
//! `threadSummary` (and its `unresolvedCount` field) may be entirely absent
//! — the caller might not have fetched thread data at all. The two public
//! functions read that absence in **opposite** directions, and this is a
//! deliberate safety posture, not an oversight:
//!
//! - [`review_needs_response`] (`queue.ts:76`): absent/`null` count is
//!   treated as `0` (`.unwrap_or(0) > 0`) — "unknown thread state: don't
//!   nag." If we don't know whether there are unresolved threads, we don't
//!   surface a needs-response signal from that alone.
//! - [`review_ready_to_merge`] (`queue.ts:117`): absent/`null` count fails
//!   the gate (`!= Some(0)` → blocked) — "unknown thread state: don't merge
//!   either." Merging is a much less reversible action than "not nagging,"
//!   so the same missing information is treated conservatively here.
//!
//! In short: **"unknown thread state: don't nag, but don't merge either."**
//! Both directions are pinned by dedicated regression tests below.
//!
//! # H4 — `review_needs_response`'s `viewer` parameter is deliberately unused
//!
//! `queue.ts:72`: `void viewer` — the TS source explicitly discards the
//! `viewer` parameter inside `reviewNeedsResponse` (it plays no role in that
//! function's logic), yet `classifyHostedReview` (`:131`) still passes it
//! through. This Rust port preserves that exact signature shape —
//! `review_needs_response(summary, _viewer: Option<&HostedReviewUser>)` —
//! rather than dropping the parameter, so that [`classify_hosted_review`]'s
//! call site continues to match the source 1:1. The leading underscore
//! (plus a `#[allow(unused_variables)]` would be redundant given the
//! underscore, so only the underscore is used) documents the non-use as
//! **intentional upstream signature preservation**, not a mistake to "fix"
//! by deleting the parameter.
//!
//! # H5 — unknown/absent `review_decision` and `neutral` checks both pass the merge gate
//!
//! `queue.ts:108-113` blocks the merge gate only for the two named
//! decisions `'review_required'` and `'changes_requested'` — `'approved'`,
//! absent, and `null` (all collapsed to `None` per M1's G1) **all pass**.
//! Likewise `queue.ts:114` accepts `checksStatus` values of either
//! `'success'` **or** `'neutral'`; only `'pending'`/`'failure'` block.
//!
//! This is a **lenient** stance on unknown/missing review state, which
//! diverges from this crate's usual conservative-on-unknown convention seen
//! elsewhere (e.g. `pr_actions.rs:267,281` treat unknown states as
//! blocking). That divergence is **intentional and preserved verbatim**
//! here because it is the source's contract, not a bug to fix — an absent
//! `reviewDecision` most often means "no review requested/required for this
//! PR at all," which is a legitimately mergeable state in the source
//! product's model.
//!
//! # H6 — `contains("bot")` subsumes and overmatches `ends_with("[bot]")`
//!
//! `queue.ts:47`: `author.endsWith('[bot]') || author.includes('bot')`. The
//! second condition is a strict superset of the first (any string ending in
//! `"[bot]"` also contains `"bot"`), making the `endsWith` check a **dead
//! condition** in practice — verbatim-preserved anyway, matching the
//! source's exact boolean expression rather than simplifying it away.
//! `.includes('bot')` additionally **overmatches** logins that merely
//! contain the substring `"bot"` without being bots at all — e.g.
//! `"robot"`, `"abbot"`, `"botvinnik"` all classify as agent-authored. This
//! is intentional (if surprising) upstream behavior, pinned by
//! `pin_h6_contains_bot_overmatches_robot` below.
//!
//! # H7 — every lowercase fold is full-Unicode, never ASCII-only
//!
//! Every `.toLowerCase()` call in the source (`:29,30,40,44,54,55`) becomes
//! Rust's [`str::to_lowercase`] (full Unicode case folding), **never**
//! [`str::to_ascii_lowercase`] or [`str::eq_ignore_ascii_case`]. This module
//! is built on JS `String.prototype.toLowerCase()`, which folds non-ASCII
//! letters (e.g. `'Ä'.toLowerCase() === 'ä'`) — an ASCII-only fold would
//! silently diverge for any non-ASCII login, which real GitHub/GitLab
//! usernames can be for internationalized instances.
//!
//! # H8 — `get_queue_state` priority is mine → requested → agent → teammate
//!
//! `queue.ts:56,59,62,65`: the checks run in that exact order and the first
//! match wins. In particular, an author who is *also* a requested reviewer
//! of their own PR (an edge case, but representable) still classifies as
//! `Mine` — the `mine` check runs first and short-circuits before the
//! `requested` check is ever reached. Separately, [`classify_hosted_review`]
//! computes its `requested` field **independently** of `get_queue_state`
//! (`queue.ts:130` calls `hasRequestedReviewerSignal` directly, not through
//! `getQueueState`), so `requested: true` can — and, per the above edge
//! case, does — **coexist** with `state: Mine` in the same classification
//! result. This is not a contradiction to reconcile; it is the source's
//! contract, pinned by `pin_h8_mine_and_requested_coexist` below.
//!
//! # H9 — an empty-string login counts as absent, exactly like `None`
//!
//! `queue.ts:22` (`!viewer?.login`), `:41` (`!author`), and `:54-56`
//! (`?? null` then truthiness-gated) all use JS truthiness, under which
//! `""` is falsy — indistinguishable from `undefined`/`null`. So `None` and
//! `Some(String::new())` (i.e. `Some("")`) **both** fail every
//! viewer/author identity check in this module. This is checked explicitly
//! via `.filter(|s| !s.is_empty())`-style guards (or equivalent) at each
//! call site below rather than relying on `Option::is_some()` alone.
//!
//! # H10 — `review_ready_to_merge` keeps all 8 gates, in source order
//!
//! `queue.ts:93,96,99,102-107,108-113,114,117,120` — eight sequential
//! early-return gates, checked in this exact order:
//!
//! 1. `state != Open` → blocked (this is why `Draft` **fails** here, unlike
//!    [`review_needs_response`], which explicitly **allows** `Draft` at its
//!    own gate 1 — the two functions read `state` differently on purpose).
//! 2. `draft == Some(true)` → blocked.
//! 3. `mergeable != Mergeable` → blocked.
//! 4. `provider == Github && merge_state_status is "BEHIND" or "BLOCKED"` →
//!    blocked. This gate applies **only** when the provider is GitHub — a
//!    GitLab summary with `merge_state_status == Some("BLOCKED")` sails
//!    straight through this gate (pinned by
//!    `pin_h10_gitlab_merge_state_status_gate_is_github_only` below,
//!    mirroring the oracle test at `queue.test.ts:108-123`). The string
//!    comparison is exact-case UPPERCASE (`"BEHIND"`/`"BLOCKED"`), matching
//!    M1's `merge_state_status: Option<String>` raw-string storage.
//! 5. `review_decision` is `ReviewRequired` or `ChangesRequested` → blocked
//!    (see H5 for the leniency on every other value).
//! 6. `checks_status` is neither `Success` nor `Neutral` → blocked (see H5).
//! 7. `thread_summary.unresolved_count != Some(0)` → blocked (see H3).
//! 8. Otherwise: ready to merge.

use chrono::DateTime;

use crate::hosted_review::{
    CheckStatus, HostedReviewDecision, HostedReviewProvider, HostedReviewQueueClassification,
    HostedReviewQueueState, HostedReviewQueueSummary, HostedReviewState, HostedReviewUser,
    PrMergeableState,
};

/// `HostedReviewClassificationOptions` (`hosted-review-queue.ts:9-12`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostedReviewClassificationOptions {
    /// TS `viewer?: HostedReviewUser | null` — optional-and-nullable,
    /// collapses to `Option` (M1's G1).
    pub viewer: Option<HostedReviewUser>,
    /// TS `agentAuthorLogins?: string[]` — optional-non-null (G1).
    pub agent_author_logins: Option<Vec<String>>,
}

/// H9 helper: a TS-truthy login. `None` and `Some("")` both fail (empty
/// string is JS-falsy), matching every `!x?.login` / `!author` guard in the
/// source (`:22`, `:41`, `:54-56`).
fn nonempty_login(login: Option<&str>) -> Option<&str> {
    login.filter(|s| !s.is_empty())
}

/// Verbatim port of private `hasRequestedReviewerSignal` (`queue.ts:18-31`).
fn has_requested_reviewer_signal(
    summary: &HostedReviewQueueSummary,
    viewer: Option<&HostedReviewUser>,
) -> bool {
    // `:22` `if (!viewer?.login) return false` — H9: absent or empty login
    // both fail.
    let Some(viewer_login) = nonempty_login(viewer.and_then(|v| v.login.as_deref())) else {
        return false;
    };
    // `:25-27` `if (!requested || requested.length === 0) return false`.
    let Some(requested) = summary.requested_reviewer_logins.as_ref() else {
        return false;
    };
    if requested.is_empty() {
        return false;
    }
    // `:29-30` full-Unicode lowercase compare (H7).
    let viewer_login_lower = viewer_login.to_lowercase();
    requested
        .iter()
        .any(|login| login.to_lowercase() == viewer_login_lower)
}

/// Verbatim port of private `isAgentAuthored` (`queue.ts:33-48`).
fn is_agent_authored(
    summary: &HostedReviewQueueSummary,
    options: Option<&HostedReviewClassificationOptions>,
) -> bool {
    // `:37-39` `if (summary.author?.isBot) return true` — wins regardless of
    // login (pinned below).
    if summary.author.as_ref().and_then(|a| a.is_bot) == Some(true) {
        return true;
    }
    // `:40` `const author = summary.author?.login?.toLowerCase()` — H7/H9.
    let Some(author_login) = summary
        .author
        .as_ref()
        .and_then(|a| nonempty_login(a.login.as_deref()))
    else {
        // `:41-43` `if (!author) return false`.
        return false;
    };
    let author = author_login.to_lowercase();
    // `:44-46` explicit agent-author allowlist, full-Unicode compare.
    if let Some(true) = options
        .and_then(|o| o.agent_author_logins.as_ref())
        .map(|logins| logins.iter().any(|login| login.to_lowercase() == author))
    {
        return true;
    }
    // `:47` H6 — `contains("bot")` subsumes `ends_with("[bot]")`; both kept
    // verbatim, overmatch is intentional.
    author.ends_with("[bot]") || author.contains("bot")
}

/// Verbatim port of private `getQueueState` (`queue.ts:50-66`). H8 priority:
/// mine → requested → agent → teammate.
fn get_queue_state(
    summary: &HostedReviewQueueSummary,
    options: Option<&HostedReviewClassificationOptions>,
) -> HostedReviewQueueState {
    // `:54` H7/H9 lowercase + truthiness on the viewer login.
    let viewer_login = options
        .and_then(|o| o.viewer.as_ref())
        .and_then(|v| nonempty_login(v.login.as_deref()))
        .map(str::to_lowercase);
    // `:55` same for the author login.
    let author_login = summary
        .author
        .as_ref()
        .and_then(|a| nonempty_login(a.login.as_deref()))
        .map(str::to_lowercase);
    // `:56-58` mine: both present and equal.
    if let (Some(v), Some(a)) = (&viewer_login, &author_login) {
        if v == a {
            return HostedReviewQueueState::Mine;
        }
    }
    // `:59-61` requested.
    if has_requested_reviewer_signal(summary, options.and_then(|o| o.viewer.as_ref())) {
        return HostedReviewQueueState::Requested;
    }
    // `:62-64` agent.
    if is_agent_authored(summary, options) {
        return HostedReviewQueueState::Agent;
    }
    // `:65` teammate (default).
    HostedReviewQueueState::Teammate
}

/// H1: strict-RFC3339 port of `Date.parse(summary.updatedAt)`, returning
/// epoch milliseconds. `None` on parse failure — mirrors JS's `NaN`
/// (`Number.isFinite(NaN) === false`). See the module doc comment for the
/// narrow, unobservable-in-practice divergence from full ECMAScript date
/// parsing (offset-less and date-only forms, which GitHub/GitLab never
/// emit).
fn parse_updated_at_ms(updated_at: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(updated_at)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Verbatim port of `reviewNeedsResponse` (`queue.ts:68-90`).
///
/// H4: `viewer` is intentionally unused here (`:72` `void viewer`) —
/// preserved in the signature only because `classifyHostedReview` passes it
/// through; see the module doc comment.
pub fn review_needs_response(
    summary: &HostedReviewQueueSummary,
    _viewer: Option<&HostedReviewUser>,
) -> bool {
    // `:73-75` — Draft is explicitly ALLOWED here (contrast H10 gate 1 in
    // `review_ready_to_merge`, which blocks Draft).
    if summary.state != HostedReviewState::Open && summary.state != HostedReviewState::Draft {
        return false;
    }
    // `:76-78` H3: absent/null unresolved count reads as 0 — "don't nag."
    if summary
        .thread_summary
        .as_ref()
        .and_then(|t| t.unresolved_count)
        .unwrap_or(0)
        > 0
    {
        return true;
    }
    // `:79-81`.
    if summary.checks_status == CheckStatus::Failure {
        return true;
    }
    // `:82-84`.
    if summary.mergeable == PrMergeableState::Conflicting {
        return true;
    }
    // `:85-87` — no `lastViewedAt` at all: no remote-update signal possible.
    let Some(last_viewed_at) = summary.last_viewed_at else {
        return false;
    };
    // `:88-89` H1 (parse) + H2 (CRITICAL: i64-space, strict `>`, never `as u64`).
    match parse_updated_at_ms(&summary.updated_at) {
        Some(parsed_ms) => parsed_ms > last_viewed_at as i64,
        None => false,
    }
}

/// Verbatim port of `reviewReadyToMerge` (`queue.ts:92-121`). H10: all 8
/// gates, in source order.
pub fn review_ready_to_merge(summary: &HostedReviewQueueSummary) -> bool {
    // Gate 1 (`:93-95`) — Draft is BLOCKED here via `state != Open` (contrast
    // `review_needs_response`, which allows Draft).
    if summary.state != HostedReviewState::Open {
        return false;
    }
    // Gate 2 (`:96-98`).
    if summary.draft == Some(true) {
        return false;
    }
    // Gate 3 (`:99-101`).
    if summary.mergeable != PrMergeableState::Mergeable {
        return false;
    }
    // Gate 4 (`:102-107`) H10: GitHub-only merge-state-status blocker.
    if summary.identity.provider == HostedReviewProvider::Github {
        if let Some(status) = summary.merge_state_status.as_deref() {
            if status == "BEHIND" || status == "BLOCKED" {
                return false;
            }
        }
    }
    // Gate 5 (`:108-113`) H5: only these two decisions block; everything
    // else (including `None`) passes.
    if matches!(
        summary.review_decision,
        Some(HostedReviewDecision::ReviewRequired) | Some(HostedReviewDecision::ChangesRequested)
    ) {
        return false;
    }
    // Gate 6 (`:114-116`) H5: `Success` or `Neutral` both pass.
    if summary.checks_status != CheckStatus::Success
        && summary.checks_status != CheckStatus::Neutral
    {
        return false;
    }
    // Gate 7 (`:117-119`) H3: absent/null unresolved count BLOCKS here
    // (opposite of `review_needs_response`'s gate).
    if summary
        .thread_summary
        .as_ref()
        .and_then(|t| t.unresolved_count)
        != Some(0)
    {
        return false;
    }
    // Gate 8 (`:120`).
    true
}

/// Verbatim port of `classifyHostedReview` (`queue.ts:123-134`).
pub fn classify_hosted_review(
    summary: &HostedReviewQueueSummary,
    options: Option<&HostedReviewClassificationOptions>,
) -> HostedReviewQueueClassification {
    let state = get_queue_state(summary, options);
    let viewer = options.and_then(|o| o.viewer.as_ref());
    HostedReviewQueueClassification {
        state,
        // H8: computed independently of `state` — can coexist with `Mine`.
        requested: has_requested_reviewer_signal(summary, viewer),
        needs_response: review_needs_response(summary, viewer),
        ready_to_merge: review_ready_to_merge(summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosted_review::{HostedReviewIdentity, HostedReviewThreadSummary};

    /// `baseSummary()` fixture (`queue.test.ts:10-23`). Deliberately has NO
    /// `last_viewed_at`/`draft`/`review_decision`/`merge_state_status`/
    /// `requested_reviewer_logins` — matching the TS fixture's omissions —
    /// and is `ready_to_merge == true` as-is.
    fn base_summary() -> HostedReviewQueueSummary {
        HostedReviewQueueSummary {
            identity: HostedReviewIdentity {
                provider: HostedReviewProvider::Github,
                host: "github.com".to_string(),
                owner: "acme".to_string(),
                repo: "orca".to_string(),
                number: 42,
            },
            title: "Improve checks panel".to_string(),
            url: "https://github.com/acme/orca/pull/42".to_string(),
            state: HostedReviewState::Open,
            author: Some(HostedReviewUser {
                login: Some("teammate".to_string()),
                is_bot: None,
            }),
            updated_at: "2026-05-10T00:00:00.000Z".to_string(),
            last_viewed_at: None,
            mergeable: PrMergeableState::Mergeable,
            merge_state_status: None,
            checks_status: CheckStatus::Success,
            review_decision: None,
            thread_summary: Some(HostedReviewThreadSummary {
                unresolved_count: Some(0),
                data_completeness: None,
            }),
            requested_reviewer_logins: None,
            draft: None,
        }
    }

    fn user(login: &str) -> HostedReviewUser {
        HostedReviewUser {
            login: Some(login.to_string()),
            is_bot: None,
        }
    }

    #[test]
    fn base_summary_is_ready_to_merge() {
        assert!(review_ready_to_merge(&base_summary()));
    }

    // ── classifyHostedReview: mine/requested/agent/teammate (queue.test.ts:46-66) ──

    #[test]
    fn classify_mine() {
        let mut summary = base_summary();
        summary.author = Some(user("me"));
        let options = HostedReviewClassificationOptions {
            viewer: Some(user("me")),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Mine);
    }

    #[test]
    fn classify_requested() {
        let mut summary = base_summary();
        summary.requested_reviewer_logins = Some(vec!["me".to_string()]);
        let options = HostedReviewClassificationOptions {
            viewer: Some(user("me")),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Requested);
    }

    #[test]
    fn classify_agent() {
        let mut summary = base_summary();
        summary.author = Some(user("orca-ci"));
        let options = HostedReviewClassificationOptions {
            viewer: None,
            agent_author_logins: Some(vec!["orca-ci".to_string()]),
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Agent);
    }

    #[test]
    fn classify_teammate() {
        let result = classify_hosted_review(&base_summary(), None);
        assert_eq!(result.state, HostedReviewQueueState::Teammate);
    }

    // ── reviewNeedsResponse (queue.test.ts:69-87) ──

    #[test]
    fn needs_response_true_for_unresolved_threads() {
        let mut summary = base_summary();
        summary.thread_summary = Some(HostedReviewThreadSummary {
            unresolved_count: Some(1),
            data_completeness: None,
        });
        assert!(review_needs_response(&summary, None));
    }

    #[test]
    fn needs_response_true_for_failed_checks() {
        let mut summary = base_summary();
        summary.checks_status = CheckStatus::Failure;
        assert!(review_needs_response(&summary, None));
    }

    #[test]
    fn needs_response_true_for_conflicts() {
        let mut summary = base_summary();
        summary.mergeable = PrMergeableState::Conflicting;
        assert!(review_needs_response(&summary, None));
    }

    #[test]
    fn needs_response_true_for_newer_remote_update() {
        let mut summary = base_summary();
        summary.updated_at = "2026-05-11T00:00:00.000Z".to_string();
        summary.last_viewed_at =
            Some(parse_updated_at_ms("2026-05-10T00:00:00.000Z").expect("valid rfc3339") as u64);
        assert!(review_needs_response(&summary, None));
    }

    #[test]
    fn needs_response_false_when_last_viewed_at_missing() {
        let mut summary = base_summary();
        summary.updated_at = "2026-05-11T00:00:00.000Z".to_string();
        // last_viewed_at stays None (base_summary default).
        assert!(!review_needs_response(&summary, None));
    }

    // ── reviewReadyToMerge (queue.test.ts:90-124) ──

    #[test]
    fn ready_to_merge_false_for_draft() {
        let mut summary = base_summary();
        summary.state = HostedReviewState::Draft;
        summary.draft = Some(true);
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_conflicting() {
        let mut summary = base_summary();
        summary.mergeable = PrMergeableState::Conflicting;
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_failed_checks() {
        let mut summary = base_summary();
        summary.checks_status = CheckStatus::Failure;
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_pending_checks() {
        let mut summary = base_summary();
        summary.checks_status = CheckStatus::Pending;
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_unresolved_threads() {
        let mut summary = base_summary();
        summary.thread_summary = Some(HostedReviewThreadSummary {
            unresolved_count: Some(2),
            data_completeness: None,
        });
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_absent_thread_summary() {
        let mut summary = base_summary();
        summary.thread_summary = None;
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_unknown_mergeability() {
        let mut summary = base_summary();
        summary.mergeable = PrMergeableState::Unknown;
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_review_required() {
        let mut summary = base_summary();
        summary.review_decision = Some(HostedReviewDecision::ReviewRequired);
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_changes_requested() {
        let mut summary = base_summary();
        summary.review_decision = Some(HostedReviewDecision::ChangesRequested);
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_behind() {
        let mut summary = base_summary();
        summary.merge_state_status = Some("BEHIND".to_string());
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_false_for_blocked() {
        let mut summary = base_summary();
        summary.merge_state_status = Some("BLOCKED".to_string());
        assert!(!review_ready_to_merge(&summary));
    }

    #[test]
    fn ready_to_merge_true_for_neutral_checks() {
        let mut summary = base_summary();
        summary.checks_status = CheckStatus::Neutral;
        assert!(review_ready_to_merge(&summary));
    }

    /// H10 gate 4: GitHub-only. A GitLab summary with `BLOCKED` sails
    /// through — mirrors `queue.test.ts:108-123`.
    #[test]
    fn pin_h10_gitlab_merge_state_status_gate_is_github_only() {
        let mut summary = base_summary();
        summary.identity = HostedReviewIdentity {
            provider: HostedReviewProvider::Gitlab,
            host: "gitlab.com".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 42,
        };
        summary.merge_state_status = Some("BLOCKED".to_string());
        assert!(review_ready_to_merge(&summary));
    }

    // ── Mandatory extra pins (oracle-silent) ────────────────────────────

    /// H2 CRITICAL: a pre-1970 `updated_at` yields a negative epoch-ms
    /// value from `Date.parse`. Comparing in `i64` space must NOT wrap: if
    /// this were mistakenly cast to `u64` before comparison, the negative
    /// value would become a huge positive number and falsely compare
    /// greater than `last_viewed_at: Some(0)`, producing a false-positive
    /// "needs response." This must be `false`.
    #[test]
    fn pin_h2_negative_epoch_does_not_wrap_to_false_positive() {
        let mut summary = base_summary();
        summary.updated_at = "1969-06-01T00:00:00Z".to_string();
        summary.last_viewed_at = Some(0);
        assert!(!review_needs_response(&summary, None));
    }

    /// Sanity check backing the H2 pin: confirm the parsed value truly is
    /// negative, so the pin above is exercising the wrap scenario and not
    /// something else.
    #[test]
    fn pin_h2_parsed_pre_1970_timestamp_is_negative() {
        let parsed = parse_updated_at_ms("1969-06-01T00:00:00Z").expect("valid rfc3339");
        assert!(parsed < 0);
    }

    /// H3, direction 1: absent `thread_summary` → `review_needs_response`
    /// is `false` ("don't nag" on unknown thread state).
    #[test]
    fn pin_h3_absent_thread_summary_does_not_need_response() {
        let mut summary = base_summary();
        summary.thread_summary = None;
        assert!(!review_needs_response(&summary, None));
    }

    /// H3, direction 2: absent `thread_summary` → `review_ready_to_merge`
    /// is also `false` ("don't merge either" on the same unknown state).
    /// Together with the previous test this locks the intentional
    /// asymmetry: same missing data, opposite functions, both `false` but
    /// for opposite reasons (permissive vs. conservative).
    #[test]
    fn pin_h3_absent_thread_summary_is_not_ready_to_merge() {
        let mut summary = base_summary();
        summary.thread_summary = None;
        assert!(!review_ready_to_merge(&summary));
    }

    /// H6: `"robot"` contains `"bot"` and is NOT authored by `"[bot]"`
    /// suffix — proves the `contains("bot")` overmatch is live, not just
    /// the `ends_with` branch.
    #[test]
    fn pin_h6_contains_bot_overmatches_robot() {
        let mut summary = base_summary();
        summary.author = Some(user("robot"));
        let result = classify_hosted_review(&summary, None);
        assert_eq!(result.state, HostedReviewQueueState::Agent);
    }

    /// H7: full-Unicode case folding for login comparisons. Viewer `"ÄNNA"`
    /// and author `"änna"` fold to the same lowercase string only under
    /// `str::to_lowercase()` (full Unicode) — `to_ascii_lowercase` would
    /// leave `Ä` unfolded and this would classify as `Teammate` instead.
    #[test]
    fn pin_h7_non_ascii_login_folds_to_mine() {
        // Sanity: ASCII-only lowercasing would NOT fold Ä to ä.
        assert_ne!("ÄNNA".to_ascii_lowercase(), "änna");
        let mut summary = base_summary();
        summary.author = Some(user("änna"));
        let options = HostedReviewClassificationOptions {
            viewer: Some(user("ÄNNA")),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Mine);
    }

    /// H8: viewer == author AND viewer is also in `requested_reviewer_logins`
    /// → `state` is `Mine` (mine-check wins the priority race) while
    /// `requested` is independently computed as `true` — the two coexist.
    #[test]
    fn pin_h8_mine_and_requested_coexist() {
        let mut summary = base_summary();
        summary.author = Some(user("me"));
        summary.requested_reviewer_logins = Some(vec!["me".to_string()]);
        let options = HostedReviewClassificationOptions {
            viewer: Some(user("me")),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Mine);
        assert!(result.requested);
    }

    /// H9: `Some("")` login for the viewer is treated identically to an
    /// absent viewer — classification falls through to `Teammate`, not
    /// `Mine`, even though the author's login is also non-matching-empty.
    #[test]
    fn pin_h9_empty_string_viewer_login_is_absent() {
        let mut summary = base_summary();
        summary.author = Some(user("teammate"));
        let options = HostedReviewClassificationOptions {
            viewer: Some(HostedReviewUser {
                login: Some(String::new()),
                is_bot: None,
            }),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Teammate);
    }

    /// H9: `Some("")` login for the author is likewise treated as absent —
    /// `is_agent_authored`'s `!author` branch fires, and (with no matching
    /// viewer) classification is `Teammate`.
    #[test]
    fn pin_h9_empty_string_author_login_is_absent() {
        let mut summary = base_summary();
        summary.author = Some(HostedReviewUser {
            login: Some(String::new()),
            is_bot: None,
        });
        let result = classify_hosted_review(&summary, None);
        assert_eq!(result.state, HostedReviewQueueState::Teammate);
    }

    /// H5: `review_decision: None` passes the merge gate (only the two
    /// named blocking decisions block).
    #[test]
    fn pin_h5_none_review_decision_passes_merge_gate() {
        let mut summary = base_summary();
        summary.review_decision = None;
        assert!(review_ready_to_merge(&summary));
    }

    /// H5: `checks_status: Neutral` passes the merge gate (duplicate of
    /// `ready_to_merge_true_for_neutral_checks` above, named to match the
    /// H5 contract-decision label explicitly).
    #[test]
    fn pin_h5_neutral_checks_status_passes_merge_gate() {
        let mut summary = base_summary();
        summary.checks_status = CheckStatus::Neutral;
        assert!(review_ready_to_merge(&summary));
    }

    /// `is_bot: Some(true)` wins regardless of login content — a login that
    /// would otherwise classify as `Teammate` (no "bot" substring) still
    /// becomes `Agent` once `is_bot` is set.
    #[test]
    fn pin_is_bot_true_wins_regardless_of_login() {
        let mut summary = base_summary();
        summary.author = Some(HostedReviewUser {
            login: Some("perfectly-normal-name".to_string()),
            is_bot: Some(true),
        });
        let result = classify_hosted_review(&summary, None);
        assert_eq!(result.state, HostedReviewQueueState::Agent);
    }

    /// H1: an empty `updated_at` string fails to parse as RFC3339 → `None`
    /// → `review_needs_response` is `false` (matching JS `NaN` →
    /// `Number.isFinite` false), even with a `last_viewed_at` present.
    #[test]
    fn pin_h1_empty_updated_at_fails_to_parse() {
        let mut summary = base_summary();
        summary.updated_at = String::new();
        summary.last_viewed_at = Some(0);
        assert!(!review_needs_response(&summary, None));
    }

    /// H1: a non-date garbage string also fails to parse as RFC3339 →
    /// `false`.
    #[test]
    fn pin_h1_garbage_updated_at_fails_to_parse() {
        let mut summary = base_summary();
        summary.updated_at = "not-a-date".to_string();
        summary.last_viewed_at = Some(0);
        assert!(!review_needs_response(&summary, None));
    }

    /// `last_viewed_at: Some(0)` is NOT an early return — with a genuinely
    /// newer `updated_at`, the comparison still proceeds and returns `true`.
    /// This proves gate 4 (`:85-87`) only early-returns on `None`, not on
    /// any falsy-looking `Some(0)`.
    #[test]
    fn pin_last_viewed_at_zero_does_not_early_return() {
        let mut summary = base_summary();
        summary.updated_at = "2026-05-11T00:00:00.000Z".to_string();
        summary.last_viewed_at = Some(0);
        assert!(review_needs_response(&summary, None));
    }

    // ── Mutation-testing follow-up pins (H2/H7/H10 gaps) ────────────────

    /// H7, requested path: `has_requested_reviewer_signal`'s own
    /// full-Unicode fold is unpinned by `pin_h7_non_ascii_login_folds_to_mine`
    /// (which only exercises the `mine` comparison in `get_queue_state`).
    /// Why: viewer `"ÄNNA"` and requested-reviewer entry `"änna"` fold equal
    /// only under `str::to_lowercase()`; with `to_ascii_lowercase()` the `Ä`
    /// stays unfolded, the requested check would miss, and (with an author
    /// who is someone else entirely, so `mine` cannot win instead) the
    /// result would fall through to `Teammate`.
    #[test]
    fn pin_h7_non_ascii_login_folds_to_requested() {
        // Sanity: ASCII-only lowercasing would NOT fold Ä to ä.
        assert_ne!("ÄNNA".to_ascii_lowercase(), "änna");
        let mut summary = base_summary();
        summary.author = Some(user("someone-else"));
        summary.requested_reviewer_logins = Some(vec!["änna".to_string()]);
        let options = HostedReviewClassificationOptions {
            viewer: Some(user("ÄNNA")),
            agent_author_logins: None,
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Requested);
        assert!(result.requested);
    }

    /// H7, agent path: `is_agent_authored`'s `agent_author_logins` fold is
    /// likewise unpinned elsewhere. Why: author `"MÜLLER"` and allowlist
    /// entry `"müller"` fold equal only under full-Unicode lowercasing;
    /// under `to_ascii_lowercase()` the `Ü`/`ü` pair stays mismatched and
    /// the allowlist match would miss. Note `"müller"` has no `"bot"`
    /// substring, so this cannot pass via the H6 `contains("bot")`
    /// heuristic instead — it isolates the allowlist fold specifically.
    #[test]
    fn pin_h7_non_ascii_login_folds_to_agent() {
        // Sanity: ASCII-only lowercasing would NOT fold Ü/ü to match.
        assert_ne!("MÜLLER".to_ascii_lowercase(), "müller");
        let mut summary = base_summary();
        summary.author = Some(user("MÜLLER"));
        let options = HostedReviewClassificationOptions {
            viewer: None,
            agent_author_logins: Some(vec!["müller".to_string()]),
        };
        let result = classify_hosted_review(&summary, Some(&options));
        assert_eq!(result.state, HostedReviewQueueState::Agent);
    }

    /// H2: the comparison is strict `>`, not `>=`. Why: an `updated_at` that
    /// parses to EXACTLY the same epoch-ms as `last_viewed_at` is "already
    /// seen," not "newer" — no test elsewhere constructs an exact tie, so
    /// `>=` silently passes the whole suite. The expected ms is derived from
    /// `chrono` parsing the same RFC3339 string, not hardcoded.
    #[test]
    fn pin_h2_equal_timestamps_do_not_need_response() {
        let mut summary = base_summary();
        let ts = "2026-05-10T00:00:00.000Z";
        summary.updated_at = ts.to_string();
        let ms = DateTime::parse_from_rfc3339(ts)
            .expect("valid rfc3339")
            .timestamp_millis();
        summary.last_viewed_at = Some(ms as u64);
        // Other gates must not fire first: base_summary() is Open, has no
        // unresolved threads, checks are Success, and mergeable is
        // Mergeable — so the comparison at the end is actually reached.
        assert!(!review_needs_response(&summary, None));
    }

    /// H10 gate 1, isolated from gate 2: `draft: None` means gate 2
    /// (`draft == Some(true)`) cannot fire, so only `state != Open` can be
    /// responsible for blocking. Why: the oracle's draft case sets both
    /// `state: Draft` and `draft: Some(true)`, so deleting gate 1 entirely
    /// would not fail any existing test.
    #[test]
    fn pin_h10_draft_state_without_draft_flag_blocks_gate_one() {
        let mut summary = base_summary();
        summary.state = HostedReviewState::Draft;
        summary.draft = None;
        assert!(!review_ready_to_merge(&summary));
    }

    /// H10 gate 1: a `Closed` PR is blocked purely by `state != Open`, with
    /// every other gate left in its ready-to-merge configuration. Why: this
    /// is a state value gate 2 (`draft`) cannot possibly catch, so it
    /// isolates gate 1 from the rest of the pipeline.
    #[test]
    fn pin_h10_closed_state_blocks_gate_one() {
        let mut summary = base_summary();
        summary.state = HostedReviewState::Closed;
        assert!(!review_ready_to_merge(&summary));
    }

    /// H10 gate 1: same reasoning as the `Closed` pin above, for `Merged`.
    #[test]
    fn pin_h10_merged_state_blocks_gate_one() {
        let mut summary = base_summary();
        summary.state = HostedReviewState::Merged;
        assert!(!review_ready_to_merge(&summary));
    }
}
