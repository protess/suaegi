//! Hosted-review vocabulary — verbatim port of Orca's `src/shared/hosted-review.ts`
//! (215L, @ v1.4.150-rc.0) plus the `hostedReviewIdentityKey` function from
//! `src/shared/hosted-review-queue.ts:14-16`.
//!
//! This module is **M1** of a 4-milestone port (see
//! `docs/superpowers/plans/2026-07-25-hosted-review-m1.md`): vocabulary +
//! identity key only. No new dependency, no existing file touched besides
//! `lib.rs`. The queue classifier (`hosted-review-queue.ts`'s other three
//! functions), the GitHub/GitLab normalizers, and any `pr_actions.rs`
//! extension are **out of scope** here (M2–M4).
//!
//! # G6 — this module defines its OWN vocabulary
//!
//! Do not reach for this crate's existing [`crate::MergeabilityState`],
//! [`crate::ChecksSummary`], [`crate::PrReviewState`], [`crate::AnyForge`],
//! or [`crate::RepoCoords`] when working with these types — they encode
//! *different* axes (e.g. `MergeabilityState` folds `mergeStateStatus`
//! staleness into a 4-state mergeability axis that Orca keeps separate;
//! `ChecksSummary` is a count and cannot represent `'neutral'`). Reusing them
//! here would silently change semantics. This module's types
//! ([`PrMergeableState`], [`CheckStatus`], [`PrReviewDecisionAggregate`],
//! [`PrConflictSummary`], ...) are local, faithful ports of the TS
//! equivalents and must stay that way.
//!
//! # G1 — three-way `?: T | null` collapses to `Option<T>`
//!
//! TypeScript's three states (property absent / present-but-`null` /
//! present-with-value) collapse to Rust's two-state `Option<T>` throughout
//! this module — **never** `Option<Option<T>>`. This is verified lossless
//! *for this cluster's read paths*: every consumer in `hosted-review-queue.ts`
//! treats `null` and `undefined` identically (e.g. `mergeStateStatus` is only
//! ever compared with `===` against literal strings, `reviewDecision` the
//! same, `requestedReviewerLogins` is guarded by `!x || x.length === 0`, and
//! `unresolvedCount` only ever appears behind `??` or `!== 0`). If a JSON
//! round-trip or a null-distinguishing consumer is ever added on top of these
//! types, this collapse decision **must be revisited** — that is also the
//! reason `serde` derives are deliberately withheld in this milestone (see
//! below).
//!
//! One field is *required*-but-nullable with no "absent" state at all:
//! `HostedReviewCreationEligibility.review: HostedReviewSummary | null`. It
//! still becomes `Option<HostedReviewSummary>` (there is no third state to
//! model), just always structurally present as a field on the Rust struct.
//!
//! # G5 — no `serde` derives yet
//!
//! There is no wire consumer for these types in this milestone. When one is
//! added, whoever adds `#[derive(Serialize, Deserialize)]` here **must**
//! re-examine the G1 collapse above — `serde`'s default `Option` handling
//! conflates absent and `null` on deserialize by default, which matches this
//! module's collapse, but explicit `#[serde(default)]` / `skip_serializing_if`
//! choices can reintroduce the distinction and must be made deliberately.
//!
//! # G3 — this module's identity key is NOT `github_repo_identity_key`
//!
//! [`hosted_review_identity_key`] and [`crate::github_repo_identity_key`] are
//! two **different** keys that must never be substituted for one another:
//! `github_repo_identity_key` omits `github.com` (so pre-Enterprise keys stay
//! stable) and is scoped to GitHub only. `hosted_review_identity_key` is
//! provider-agnostic, **always** includes the host (`github.com` included),
//! and is `::`-delimited with no trimming. Using the wrong one silently
//! collides GitHub Enterprise Server hosts with github.com repos of the same
//! owner/repo, which is exactly the bug `hosted-review-queue.test.ts:26-42`
//! guards against.

use std::fmt;

// ─── Enums ported from `hosted-review.ts` (10) ──────────────────────────

/// `HostedReviewProvider` (`hosted-review.ts:3-9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewProvider {
    Github,
    Gitlab,
    Bitbucket,
    /// TS literal `'azure-devops'` — hyphen preserved verbatim (G5).
    AzureDevops,
    Gitea,
    Unsupported,
}

impl HostedReviewProvider {
    /// Exact TS string-literal spelling for this variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Bitbucket => "bitbucket",
            Self::AzureDevops => "azure-devops",
            Self::Gitea => "gitea",
            Self::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for HostedReviewProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `HostedReviewState` (`hosted-review.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewState {
    Open,
    Closed,
    Merged,
    Draft,
}

impl HostedReviewState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::Draft => "draft",
        }
    }
}

/// `CreateHostedReviewErrorCode` (`hosted-review.ts:77-85`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateHostedReviewErrorCode {
    AuthRequired,
    UnsupportedProvider,
    AlreadyExists,
    Validation,
    Timeout,
    UnknownCompletion,
    PushFailed,
    Unknown,
}

impl CreateHostedReviewErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AuthRequired => "auth_required",
            Self::UnsupportedProvider => "unsupported_provider",
            Self::AlreadyExists => "already_exists",
            Self::Validation => "validation",
            Self::Timeout => "timeout",
            Self::UnknownCompletion => "unknown_completion",
            Self::PushFailed => "push_failed",
            Self::Unknown => "unknown",
        }
    }
}

/// `HostedReviewCreationBlockedReason` (`hosted-review.ts:96-111`). The TS
/// type is `... | null`; the `null` arm is modeled by wrapping this enum in
/// `Option` at every use site (G1) rather than as a variant here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewCreationBlockedReason {
    Dirty,
    DetachedHead,
    DefaultBranch,
    NoUpstream,
    NeedsPush,
    NeedsSync,
    AuthRequired,
    ForkHeadUnsupported,
    UnsupportedProvider,
    ExistingReview,
    /// Why (from source comment): a stacked worktree's local-only parent base
    /// is unresolvable on the remote; blocked at create-time so the submit
    /// fails with actionable copy instead of the provider's opaque error.
    BaseNotOnRemote,
}

impl HostedReviewCreationBlockedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dirty => "dirty",
            Self::DetachedHead => "detached_head",
            Self::DefaultBranch => "default_branch",
            Self::NoUpstream => "no_upstream",
            Self::NeedsPush => "needs_push",
            Self::NeedsSync => "needs_sync",
            Self::AuthRequired => "auth_required",
            Self::ForkHeadUnsupported => "fork_head_unsupported",
            Self::UnsupportedProvider => "unsupported_provider",
            Self::ExistingReview => "existing_review",
            Self::BaseNotOnRemote => "base_not_on_remote",
        }
    }
}

/// `HostedReviewCreationNextAction` (`hosted-review.ts:113-120`). The TS
/// type is `... | null`; modeled via `Option` at use sites (G1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewCreationNextAction {
    Commit,
    Publish,
    Push,
    Sync,
    Authenticate,
    OpenExistingReview,
}

impl HostedReviewCreationNextAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Publish => "publish",
            Self::Push => "push",
            Self::Sync => "sync",
            Self::Authenticate => "authenticate",
            Self::OpenExistingReview => "open_existing_review",
        }
    }
}

/// `HostedReviewLookupOutcome` (`hosted-review.ts:128`).
///
/// Records whether the eligibility result observed an authoritative
/// existing-review lookup. `Found` / `NotFound` come only from an accepted
/// provider lookup; `Unavailable` marks a local-blocker fallback returned
/// after a swallowed or skipped lookup, so it can never masquerade as
/// authoritative no-review evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewLookupOutcome {
    Found,
    NotFound,
    Unavailable,
}

impl HostedReviewLookupOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Found => "found",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
        }
    }
}

/// `HostedReviewDecision` (`hosted-review.ts:175`). The TS type already
/// includes `| null` in its own definition, and every use site additionally
/// makes the field optional (`reviewDecision?: HostedReviewDecision`) —
/// i.e. a 4-way union in practice (absent / null / one of 3 strings). Per G1
/// this collapses to `Option<HostedReviewDecision>` with only the 3 real
/// variants represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl HostedReviewDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::ReviewRequired => "review_required",
        }
    }
}

/// `HostedReviewThreadSummary.dataCompleteness` (`hosted-review.ts:179`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewThreadDataCompleteness {
    /// T24: no producer in the source ever constructs `'full'` — both
    /// GitHub and GitLab normalizers hard-code `'partial'`. Kept here
    /// because the TS type still admits it; a future normalizer may.
    Full,
    Partial,
}

impl HostedReviewThreadDataCompleteness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }
}

/// `HostedReviewQueueKey` (`hosted-review.ts:199-206`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewQueueKey {
    Mine,
    Requested,
    Agent,
    Teammate,
    /// TS literal `'needs-response'` — hyphen preserved verbatim (G5).
    NeedsResponse,
    /// TS literal `'ready-to-merge'` — hyphen preserved verbatim (G5).
    ReadyToMerge,
}

impl HostedReviewQueueKey {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::Requested => "requested",
            Self::Agent => "agent",
            Self::Teammate => "teammate",
            Self::NeedsResponse => "needs-response",
            Self::ReadyToMerge => "ready-to-merge",
        }
    }
}

/// `HostedReviewQueueState` (`hosted-review.ts:207`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedReviewQueueState {
    Mine,
    Requested,
    Agent,
    Teammate,
}

impl HostedReviewQueueState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mine => "mine",
            Self::Requested => "requested",
            Self::Agent => "agent",
            Self::Teammate => "teammate",
        }
    }
}

// ─── Enums ported from `types.ts` (4) ───────────────────────────────────
// Named with the crate's existing `Pr`-not-`PR` convention (see
// `pr_actions.rs`'s `PrActions`/`PrComment`/`PrReview`/`PrReviewState`), and
// deliberately NOT the same names/shapes as this crate's existing
// `MergeabilityState` / `ChecksSummary` / `PrReviewState` (G6).

/// `PRState` (`types.ts:1144`). Note this has the exact same variants as
/// [`HostedReviewState`] above but is a **separate** type, ported verbatim
/// from its own source location per G6 (no cross-reuse, even when identical).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Closed,
    Merged,
    Draft,
}

impl PrState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
            Self::Draft => "draft",
        }
    }
}

/// `CheckStatus` (`types.ts:1146`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckStatus {
    Pending,
    Success,
    Failure,
    Neutral,
}

impl CheckStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Neutral => "neutral",
        }
    }
}

/// `PRMergeableState` (`types.ts:1148`). Variants are **UPPERCASE**,
/// matching the TS string literals verbatim (G5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrMergeableState {
    Mergeable,
    Conflicting,
    Unknown,
}

impl PrMergeableState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mergeable => "MERGEABLE",
            Self::Conflicting => "CONFLICTING",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// `PRReviewDecision` (`types.ts:1149`). Variants are **UPPERCASE**,
/// matching the TS string literals verbatim (G5). Named `...Aggregate`
/// (per the plan) to distinguish it from [`HostedReviewDecision`], which is
/// a differently-spelled, differently-cased 3-way vocabulary of its own
/// (`hosted-review.ts:175`) used only by [`HostedReviewQueueSummary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrReviewDecisionAggregate {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

impl PrReviewDecisionAggregate {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::ChangesRequested => "CHANGES_REQUESTED",
            Self::ReviewRequired => "REVIEW_REQUIRED",
        }
    }
}

/// `PRConflictSummary.localMergeState` (`types.ts:1156`) is an inline
/// single-literal type (`'clean'`), not a separately exported TS type, so it
/// is not among the "10 + 4" named enums in the plan — it is defined here
/// purely so [`PrConflictSummary`] (needed as a field of [`HostedReviewInfo`])
/// can be ported faithfully. See the module-level report for this deviation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrConflictLocalMergeState {
    Clean,
}

impl PrConflictLocalMergeState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Clean => "clean",
        }
    }
}

// ─── Structs ─────────────────────────────────────────────────────────────

/// `PRConflictSummary` (`types.ts:1151-1157`). Not one of the plan's
/// enumerated 12 structs (those are all from `hosted-review.ts` itself),
/// but required as a field type of [`HostedReviewInfo::conflict_summary`] —
/// defined locally here rather than reused from elsewhere, per G6.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrConflictSummary {
    pub base_ref: String,
    pub base_commit: String,
    /// TS `number`; a non-negative commit count, `u32` per this crate's
    /// existing count-field convention (see `ChecksSummary::passing` etc.
    /// in `provider.rs`).
    pub commits_behind: u32,
    pub files: Vec<String>,
    /// TS `localMergeState?: 'clean'` — optional-non-null (G1) single-literal
    /// field.
    pub local_merge_state: Option<PrConflictLocalMergeState>,
}

/// `HostedReviewIdentity` (`hosted-review.ts:170-176`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewIdentity {
    pub provider: HostedReviewProvider,
    pub host: String,
    pub owner: String,
    pub repo: String,
    /// TS `number` — review/PR/MR numbers are `u64` (G4).
    pub number: u64,
}

/// `HostedReviewUser` (`hosted-review.ts:172-175`).
///
/// G7: `login` is TS `string | null` — **required-but-nullable** (the key
/// always exists on the object; the value may be `null`). `isBot` is TS
/// `isBot?: boolean` — **optional-non-null** (the key may be entirely
/// absent; when present it is always a real boolean, never `null`). Both
/// become `Option<...>` in Rust (G1), but the TS meanings differ and later
/// milestones (the queue classifier, M2) depend on that distinction being
/// remembered even though the Rust *type* can't express it: `login: None`
/// always means "no login value", while `is_bot: None` means "the producer
/// didn't say" (treated as "not a bot" by the one M2 read path, but that is
/// a classifier decision, not a vocabulary one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewUser {
    pub login: Option<String>,
    pub is_bot: Option<bool>,
}

/// `HostedReviewThreadSummary` (`hosted-review.ts:177-180`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewThreadSummary {
    /// TS `unresolvedCount: number | null` — required-but-nullable (G1).
    /// `u32` per this crate's count-field convention.
    pub unresolved_count: Option<u32>,
    /// TS `dataCompleteness?: 'full' | 'partial'` — optional-non-null (G1).
    pub data_completeness: Option<HostedReviewThreadDataCompleteness>,
}

/// `HostedReviewQueueSummary` (`hosted-review.ts:182-196`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewQueueSummary {
    pub identity: HostedReviewIdentity,
    pub title: String,
    pub url: String,
    pub state: HostedReviewState,
    /// TS `author: HostedReviewUser | null` — required-but-nullable (G1).
    pub author: Option<HostedReviewUser>,
    pub updated_at: String,
    /// TS `lastViewedAt?: number` — optional-non-null (G1); epoch
    /// milliseconds, always non-negative, hence `u64`.
    pub last_viewed_at: Option<u64>,
    pub mergeable: PrMergeableState,
    /// TS `mergeStateStatus?: string | null` — optional-and-nullable (G1).
    pub merge_state_status: Option<String>,
    pub checks_status: CheckStatus,
    /// TS `reviewDecision?: HostedReviewDecision` (which is itself `... |
    /// null`) — collapses to a single `Option` per G1.
    pub review_decision: Option<HostedReviewDecision>,
    /// TS `threadSummary?: HostedReviewThreadSummary` — optional-non-null (G1).
    pub thread_summary: Option<HostedReviewThreadSummary>,
    /// TS `requestedReviewerLogins?: string[] | null` — optional-and-nullable
    /// (G1).
    pub requested_reviewer_logins: Option<Vec<String>>,
    /// TS `draft?: boolean` — optional-non-null (G1).
    pub draft: Option<bool>,
}

/// `HostedReviewQueueClassification` (`hosted-review.ts:209-214`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewQueueClassification {
    pub state: HostedReviewQueueState,
    pub needs_response: bool,
    pub ready_to_merge: bool,
    pub requested: bool,
}

/// `HostedReviewInfo` (`hosted-review.ts:18-38`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewInfo {
    pub provider: HostedReviewProvider,
    /// TS `number` — `u64` (G4).
    pub number: u64,
    pub title: String,
    pub state: HostedReviewState,
    pub url: String,
    pub status: CheckStatus,
    pub updated_at: String,
    pub mergeable: PrMergeableState,
    /// TS `reviewDecision?: PRReviewDecision | null` — optional-and-nullable
    /// (G1).
    pub review_decision: Option<PrReviewDecisionAggregate>,
    /// TS `autoMergeEnabled?: boolean` — optional-non-null (G1).
    pub auto_merge_enabled: Option<bool>,
    /// TS `autoMergeAllowed?: boolean | null` — optional-and-nullable (G1).
    pub auto_merge_allowed: Option<bool>,
    /// TS `mergeQueueRequired?: boolean | null` — optional-and-nullable (G1).
    pub merge_queue_required: Option<bool>,
    /// TS `mergeStateStatus?: string | null` — optional-and-nullable (G1).
    pub merge_state_status: Option<String>,
    /// TS `headSha?: string` — optional-non-null (G1).
    pub head_sha: Option<String>,
    /// TS `confirmedContainedHeadOid?: string` — optional-non-null (G1).
    /// Why (source comment): mirrors `PRInfo.confirmedContainedHeadOid` so
    /// merged-review staleness checks accept a worktree head confirmed to be
    /// part of the merged PR.
    pub confirmed_contained_head_oid: Option<String>,
    /// TS `baseRefName?: string` — optional-non-null (G1). Target branch
    /// name for review-created worktree compare-base repair.
    pub base_ref_name: Option<String>,
    /// TS `conflictSummary?: PRConflictSummary` — optional-non-null (G1).
    pub conflict_summary: Option<PrConflictSummary>,
}

/// `HostedReviewForBranchArgs` (`hosted-review.ts:40-53`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewForBranchArgs {
    pub repo_path: String,
    /// TS `repoId?: string` — optional-non-null (G1).
    pub repo_id: Option<String>,
    pub branch: String,
    /// TS `linkedGitHubPR?: number | null` — optional-and-nullable (G1);
    /// review number, `u64` (G4).
    pub linked_github_pr: Option<u64>,
    pub fallback_github_pr: Option<u64>,
    pub linked_gitlab_mr: Option<u64>,
    pub linked_bitbucket_pr: Option<u64>,
    pub linked_azure_devops_pr: Option<u64>,
    pub linked_gitea_pr: Option<u64>,
    /// TS `currentHeadOid?: string | null` — optional-and-nullable (G1). The
    /// worktree's checked-out HEAD oid (GitHub merged-at-head visibility).
    pub current_head_oid: Option<String>,
}

/// `HostedReviewSummary` (`hosted-review.ts:55-58`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewSummary {
    /// TS `number?: number` — optional-non-null (G1); review number, `u64`
    /// (G4).
    pub number: Option<u64>,
    pub url: String,
}

/// `CreateHostedReviewInput` (`hosted-review.ts:60-68`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateHostedReviewInput {
    pub provider: HostedReviewProvider,
    pub base: String,
    /// TS `head?: string` — optional-non-null (G1).
    pub head: Option<String>,
    pub title: String,
    /// TS `body?: string` — optional-non-null (G1).
    pub body: Option<String>,
    /// TS `draft?: boolean` — optional-non-null (G1).
    pub draft: Option<bool>,
    /// TS `worktreePath?: string` — optional-non-null (G1).
    pub worktree_path: Option<String>,
    /// TS `useTemplate?: boolean` — optional-non-null (G1).
    pub use_template: Option<bool>,
}

/// `CreateHostedReviewArgs` (`hosted-review.ts:70-74`).
///
/// TS defines this as an intersection type, `CreateHostedReviewInput & {
/// repoPath: string; repoId?: string; connectionId?: string | null }` — the
/// runtime shape is a single flat object, so this Rust struct flattens all
/// of `CreateHostedReviewInput`'s fields plus the three extra ones, rather
/// than nesting a `CreateHostedReviewInput` value inside, to preserve
/// faithful field-access shape (`args.provider`, not `args.input.provider`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateHostedReviewArgs {
    pub provider: HostedReviewProvider,
    pub base: String,
    pub head: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub draft: Option<bool>,
    pub worktree_path: Option<String>,
    pub use_template: Option<bool>,
    pub repo_path: String,
    /// TS `repoId?: string` — optional-non-null (G1).
    pub repo_id: Option<String>,
    /// TS `connectionId?: string | null` — optional-and-nullable (G1).
    pub connection_id: Option<String>,
}

/// `CreateHostedReviewResult` (`hosted-review.ts:87-95`).
///
/// TS is a discriminated union on a boolean literal (`ok: true` / `ok:
/// false`), ported as a 2-variant Rust enum rather than a struct with a
/// `bool` + all-fields-optional shape, so illegal states (e.g. `ok: true`
/// with a `code` set) are unrepresentable — a strict improvement in
/// faithfulness over a flattened struct, not a semantic change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateHostedReviewResult {
    Ok {
        /// Review number — `u64` (G4).
        number: u64,
        url: String,
    },
    Err {
        code: CreateHostedReviewErrorCode,
        error: String,
        /// TS `existingReview?: HostedReviewSummary` — optional-non-null (G1).
        existing_review: Option<HostedReviewSummary>,
    },
}

/// `HostedReviewCreationEligibility` (`hosted-review.ts:124-138`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedReviewCreationEligibility {
    pub provider: HostedReviewProvider,
    /// TS `review: HostedReviewSummary | null` — required-but-nullable, no
    /// "absent" state exists for this field at all (G1 special case: the
    /// field is always structurally present on the Rust struct).
    pub review: Option<HostedReviewSummary>,
    pub can_create: bool,
    /// TS `blockedReason: HostedReviewCreationBlockedReason` (a type that
    /// itself includes `| null`) — required-but-nullable, collapses to
    /// `Option` per G1.
    pub blocked_reason: Option<HostedReviewCreationBlockedReason>,
    /// TS `nextAction: HostedReviewCreationNextAction` (a type that itself
    /// includes `| null`) — required-but-nullable, collapses to `Option`
    /// per G1.
    pub next_action: Option<HostedReviewCreationNextAction>,
    pub review_lookup_outcome: HostedReviewLookupOutcome,
    /// TS `defaultBaseRef?: string | null` — optional-and-nullable (G1).
    pub default_base_ref: Option<String>,
    /// TS `head?: string | null` — optional-and-nullable (G1).
    pub head: Option<String>,
    /// TS `title?: string | null` — optional-and-nullable (G1).
    pub title: Option<String>,
    /// TS `body?: string | null` — optional-and-nullable (G1).
    pub body: Option<String>,
}

// ─── Functions ───────────────────────────────────────────────────────────

/// Verbatim port of `isPositiveHostedReviewNumber` (`hosted-review.ts:14-16`):
/// `typeof value === 'number' && Number.isInteger(value) && value > 0`.
///
/// A linked review is identified by a positive integer PR/MR number.
///
/// `value: None` models `typeof value !== 'number'` (the JS predicate's
/// `unknown` input not being a number at all) → `false`. For `Some(v)`,
/// `Number.isInteger` rejects `NaN`/`±Infinity` and non-integers, which is
/// exactly `v.is_finite() && v.fract() == 0.0`; combined with the `v > 0`
/// check this reproduces the JS predicate exactly.
///
/// Note: JS `Number.isInteger` also accepts values like `1e21` — a JS
/// "integer" (no fractional part, per IEEE-754) far beyond `u64::MAX`
/// (≈1.8e19) — as a positive integer. That is outside the real
/// review-number domain (no GitHub/GitLab/Bitbucket/Azure DevOps/Gitea PR or
/// MR number is anywhere near that large), so this predicate does not
/// special-case it: for such an input this function still returns `true`,
/// matching JS, even though no `u64` could actually hold the value. Do not
/// "fix" this by adding a `u64::MAX` bound — that would be a behavior change
/// the source does not make (G2).
pub fn is_positive_hosted_review_number(value: Option<f64>) -> bool {
    match value {
        None => false,
        Some(v) => v.is_finite() && v.fract() == 0.0 && v > 0.0,
    }
}

/// Verbatim port of `hostedReviewIdentityKey` (`hosted-review-queue.ts:14-16`):
///
/// ```ts
/// return `${identity.provider}::${identity.host.toLowerCase()}::${identity.owner.toLowerCase()}::${identity.repo.toLowerCase()}::${identity.number}`
/// ```
///
/// - `provider` is interpolated as-is — **not** lowercased (G3); it is
///   already one of [`HostedReviewProvider`]'s fixed lowercase literals, so
///   this is a no-op either way, exactly as in the TS source (`identity`'s
///   TS type is a closed literal union too, so this is equally unobservable
///   there — see this module's test suite for the honest limitation this
///   implies for mutation testing).
/// - `host`, `owner`, `repo` use full-Unicode `str::to_lowercase()` — **not**
///   `to_ascii_lowercase()` (G3; [[js-lowercase-two-mechanisms]]), matching
///   JS `String.prototype.toLowerCase()`.
/// - **No trimming** anywhere — the source has zero `.trim()` calls in this
///   cluster, so leading/trailing whitespace in `host`/`owner`/`repo`
///   survives into the key verbatim.
/// - The separator is exactly `::` (two characters), and the host segment is
///   **always** included — `github.com` is never special-cased or omitted,
///   unlike [`crate::github_repo_identity_key`], which is a **different**
///   key that deliberately omits `github.com` (see the module doc comment
///   above; the two keys must never be substituted for one another).
/// - `number` is `u64` (G4); its `Display` decimal rendering matches JS's
///   `` `${42}` `` for every value this predicate's domain actually produces.
pub fn hosted_review_identity_key(identity: &HostedReviewIdentity) -> String {
    format!(
        "{}::{}::{}::{}::{}",
        identity.provider,
        identity.host.to_lowercase(),
        identity.owner.to_lowercase(),
        identity.repo.to_lowercase(),
        identity.number
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Oracle: hosted-review-queue.test.ts:26-42 ──────────────────────

    /// A github.com host key must NOT equal an otherwise-identical
    /// github.acme.internal (GHES) host key — the core invariant this
    /// function exists to protect (GHES/dotcom key collision would silently
    /// merge two different repositories' review queues).
    #[test]
    fn dotcom_and_ghes_keys_are_never_equal() {
        let dotcom = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 7,
        });
        let ghe = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            host: "github.acme.internal".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 7,
        });
        assert_ne!(dotcom, ghe);
    }

    // ── Mandatory extra pins (oracle-silent) ───────────────────────────

    /// `github.com` is present in the key, not omitted — the opposite
    /// choice ([`crate::github_repo_identity_key`]'s behavior) would make
    /// this assertion fail.
    #[test]
    fn pin_github_com_host_is_present_not_omitted() {
        let key = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 7,
        });
        assert_eq!(key, "github::github.com::acme::orca::7");
    }

    /// Exact `::` separator and full key string for a known identity —
    /// mutation-kills a separator change (e.g. `::` -> `:`) and a segment
    /// reordering.
    #[test]
    fn pin_exact_separator_and_full_key_string() {
        let key = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Gitlab,
            host: "gitlab.example.com".to_string(),
            owner: "Team".to_string(),
            repo: "Widgets".to_string(),
            number: 123,
        });
        assert_eq!(key, "gitlab::gitlab.example.com::team::widgets::123");
    }

    /// Full-Unicode lowercasing of host/owner/repo: `to_ascii_lowercase`
    /// would leave a non-ASCII uppercase letter (`É`) unfolded, so this
    /// proves `str::to_lowercase()` (full Unicode) is used, not
    /// `to_ascii_lowercase()`.
    #[test]
    fn pin_full_unicode_lowercasing_not_ascii_only() {
        let host = "GHE.\u{c9}XAMPLE"; // "GHE.ÉXAMPLE"
                                       // Sanity: ASCII-only lowercasing would NOT suffice here — it leaves
                                       // the non-ASCII É unfolded.
        assert_ne!(host.to_ascii_lowercase(), "ghe.\u{e9}xample");
        let key = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            host: host.to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 1,
        });
        assert_eq!(key, "github::ghe.\u{e9}xample::acme::orca::1"); // "...ghe.éxample..."
    }

    /// The provider segment is NOT lowercased. Every [`HostedReviewProvider`]
    /// literal is already all-lowercase (matching the closed TS literal
    /// union `identity.provider` is typed as), so this is, honestly,
    /// unobservable through this enum alone — exactly as it is in the TS
    /// source, where `identity.provider`'s type is equally a closed
    /// lowercase-only literal union. This test pins the *value* that does
    /// reach the key (asserting it is not somehow re-cased or dropped), not
    /// a case-folding mutation that has no observable effect on this type.
    #[test]
    fn pin_provider_segment_is_not_recased_or_altered() {
        let key = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::AzureDevops,
            host: "dev.azure.com".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 1,
        });
        assert!(key.starts_with("azure-devops::"));
    }

    /// No trimming: leading/trailing spaces in host/owner/repo survive into
    /// the key verbatim (the source has zero `.trim()` calls in this
    /// cluster).
    #[test]
    fn pin_no_trimming_survives_into_key() {
        let key = hosted_review_identity_key(&HostedReviewIdentity {
            provider: HostedReviewProvider::Github,
            host: " github.com ".to_string(),
            owner: " acme ".to_string(),
            repo: " orca ".to_string(),
            number: 7,
        });
        assert_eq!(key, "github:: github.com :: acme :: orca ::7");
    }

    /// Determinism: same input produces the same key every time (no hidden
    /// state, no `Date.now()`-like nondeterminism anywhere in this cluster).
    #[test]
    fn pin_deterministic_same_input_same_key() {
        let identity = HostedReviewIdentity {
            provider: HostedReviewProvider::Bitbucket,
            host: "bitbucket.org".to_string(),
            owner: "acme".to_string(),
            repo: "orca".to_string(),
            number: 99,
        };
        let a = hosted_review_identity_key(&identity);
        let b = hosted_review_identity_key(&identity);
        assert_eq!(a, b);
    }

    // ── G5 spelling pins (exact string spellings preserved) ────────────

    #[test]
    fn pin_azure_devops_literal_keeps_hyphen() {
        assert_eq!(HostedReviewProvider::AzureDevops.as_str(), "azure-devops");
    }

    #[test]
    fn pin_queue_key_hyphenated_literals() {
        assert_eq!(
            HostedReviewQueueKey::NeedsResponse.as_str(),
            "needs-response"
        );
        assert_eq!(
            HostedReviewQueueKey::ReadyToMerge.as_str(),
            "ready-to-merge"
        );
    }

    #[test]
    fn pin_pr_mergeable_state_is_uppercase() {
        assert_eq!(PrMergeableState::Mergeable.as_str(), "MERGEABLE");
        assert_eq!(PrMergeableState::Conflicting.as_str(), "CONFLICTING");
        assert_eq!(PrMergeableState::Unknown.as_str(), "UNKNOWN");
    }

    #[test]
    fn pin_pr_review_decision_aggregate_is_uppercase() {
        assert_eq!(PrReviewDecisionAggregate::Approved.as_str(), "APPROVED");
        assert_eq!(
            PrReviewDecisionAggregate::ChangesRequested.as_str(),
            "CHANGES_REQUESTED"
        );
        assert_eq!(
            PrReviewDecisionAggregate::ReviewRequired.as_str(),
            "REVIEW_REQUIRED"
        );
    }

    // ── G2: is_positive_hosted_review_number — all 8 cases ─────────────

    #[test]
    fn is_positive_hosted_review_number_none_is_false() {
        assert!(!is_positive_hosted_review_number(None));
    }

    #[test]
    fn is_positive_hosted_review_number_zero_is_false() {
        assert!(!is_positive_hosted_review_number(Some(0.0)));
    }

    #[test]
    fn is_positive_hosted_review_number_negative_is_false() {
        assert!(!is_positive_hosted_review_number(Some(-1.0)));
    }

    #[test]
    fn is_positive_hosted_review_number_fractional_is_false() {
        assert!(!is_positive_hosted_review_number(Some(1.5)));
    }

    #[test]
    fn is_positive_hosted_review_number_nan_is_false() {
        assert!(!is_positive_hosted_review_number(Some(f64::NAN)));
    }

    #[test]
    fn is_positive_hosted_review_number_infinity_is_false() {
        assert!(!is_positive_hosted_review_number(Some(f64::INFINITY)));
    }

    #[test]
    fn is_positive_hosted_review_number_one_is_true() {
        assert!(is_positive_hosted_review_number(Some(1.0)));
    }

    #[test]
    fn is_positive_hosted_review_number_forty_two_is_true() {
        assert!(is_positive_hosted_review_number(Some(42.0)));
    }
}
