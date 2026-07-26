//! VERBATIM port of Orca's `src/shared/workspace-cleanup.ts` (240 lines,
//! ZERO imports).
//!
//! Ported: `O:1` [`WORKSPACE_CLEANUP_CLASSIFIER_VERSION`], `O:2`
//! [`WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS`], `O:3`
//! [`WORKSPACE_CLEANUP_IDLE_MS`], `O:5` [`WorkspaceCleanupTier`], `O:7`
//! [`WorkspaceCleanupReason`], `O:9-12` [`WorkspaceCleanupInactivityInput`],
//! `O:14-30` [`WorkspaceCleanupBlocker`], `O:32-37`
//! [`WorkspaceCleanupDismissal`], `O:39-41` [`WorkspaceCleanupUIState`],
//! `O:43-72` [`WorkspaceCleanupCandidate`] (+ [`WorkspaceCleanupLocalContext`],
//! [`WorkspaceCleanupGitState`]), `O:74-78` [`WorkspaceCleanupScanArgs`],
//! `O:80-84` [`WorkspaceCleanupLocalProcessArgs`], `O:86-90`
//! [`WorkspaceCleanupScanError`], `O:92-96` [`WorkspaceCleanupScanResult`],
//! `O:98-103` [`WorkspaceCleanupScanProgress`] (+
//! [`WorkspaceCleanupCandidateMode`]), `O:105-107`
//! [`WorkspaceCleanupLocalProcessResult`], `O:109-111`
//! [`WorkspaceCleanupDismissArgs`], `O:113-130`
//! [`WORKSPACE_CLEANUP_HARD_BLOCKERS`], `O:132-136`
//! `WORKSPACE_CLEANUP_QUEUE_BLOCKERS` (private, see G2 below), `O:138-139`
//! [`WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS`], `O:141-143`
//! [`is_workspace_cleanup_hard_blocker`], `O:145-152`
//! [`can_queue_workspace_cleanup_candidate`], `O:154-162`
//! [`should_force_workspace_cleanup_removal`], `O:164-173`
//! [`can_select_workspace_cleanup_candidate`], `O:175-187`
//! [`apply_workspace_cleanup_policy`], `O:189-205`
//! [`create_workspace_cleanup_fingerprint`], `O:207-222`
//! [`get_workspace_cleanup_inactivity_reasons`], `O:224-229`
//! [`is_workspace_old_for_cleanup`], `O:231-240`
//! [`should_hide_workspace_cleanup_candidate`].
//!
//! # Traps (see the plan's §1 for the full rationale; `G<N>` numbering
//! # matches `docs/superpowers/plans/2026-07-26-workspace-cleanup.md`)
//! - **G1**: [`WORKSPACE_CLEANUP_HARD_BLOCKERS`] contains ALL 16
//!   [`WorkspaceCleanupBlocker`] variants, so [`is_workspace_cleanup_hard_blocker`]
//!   is effectively constant-`true` today. All 16 members are still written
//!   out explicitly (as a `const` array, mirroring the TS `Set` literal,
//!   `O:113-130`) rather than collapsing the tier computation to
//!   `blockers.is_empty()` — that would be behaviorally identical right now
//!   but would silently change policy the moment a 17th
//!   [`WorkspaceCleanupBlocker`] variant is added without a matching update
//!   here. The three blocker sets are kept as three separate consts —
//!   [`WORKSPACE_CLEANUP_QUEUE_BLOCKERS`] (3 members) and
//!   [`WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS`] (4 members) are genuinely
//!   different, and `UnpushedCommits`' membership in the latter but not the
//!   former (queueable YET force-removed) is the point of the oracle case at
//!   `T:74-82`.
//! - **G2**: `WORKSPACE_CLEANUP_QUEUE_BLOCKERS` is NOT exported in the TS
//!   source (`O:132`) — the only one of the three sets without `export`.
//!   Kept `const` (module-private, no `pub`) here; making it `pub` would
//!   give the Rust port a wider API surface than the source.
//! - **G3**: both idle comparisons in
//!   [`get_workspace_cleanup_inactivity_reasons`] are `>=` (inclusive,
//!   `O:214`, `O:218`), and NEITHER `WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS` nor
//!   `WORKSPACE_CLEANUP_IDLE_MS` is imported by the `.test.ts` oracle — an
//!   elapsed time exactly equal to a threshold DOES trigger. The archived
//!   check is guarded by `workspace.is_archived` (`O:213`); the idle check
//!   is NOT guarded by anything. The two conditions are independent, so an
//!   archived workspace idle for >= 30 days gets BOTH reasons, `archived`
//!   first (`O:216` runs before `O:219`).
//! - **G4**: the elapsed-time subtraction is signed `i64` arithmetic, never
//!   `saturating_sub`. A future-dated `last_activity_at` (clock skew, bad
//!   data) makes `scanned_at - last_activity_at` negative, and
//!   `negative >= threshold` is `false` -> not idle. A `u64` +
//!   `checked_sub().unwrap_or(u64::MAX)` "safety" reflex would invert this
//!   into "maximally idle" instead.
//! - **G5**: the fingerprint's day bucket (`O:197`,
//!   `Math.floor(lastActivityAt / 86_400_000)`) floors toward negative
//!   infinity; Rust's `/` truncates toward zero. Implemented with
//!   [`i64::div_euclid`], which floors the same way `Math.floor` does for a
//!   positive divisor. For `last_activity_at = -1`, JS gives bucket `-1`;
//!   plain Rust `/` would (wrongly) give `0`. The `.test.ts` oracle never
//!   inspects the fingerprint's return value at all (see G7), so this has
//!   zero oracle coverage — hand-written pins below are the only guard.
//! - **G6**: `??` (`O:196`, classifier version) and `||` (`O:197`,
//!   `lastActivityAt`) are six lines apart and deliberately different.
//!   `classifier_version: Some(0)` is honored AS `0` (`??` only falls back
//!   on `null`/`undefined`); only an absent/`None` version falls back to
//!   [`WORKSPACE_CLEANUP_CLASSIFIER_VERSION`]. Modeled as
//!   `Option<i64>::unwrap_or(..)`, which has exactly `??` semantics for a
//!   non-nullable numeric type. `last_activity_at` uses `||`, which for a
//!   non-`Option` `i64` model (no `NaN`, no `0`-is-falsy-but-still-`i64`
//!   ambiguity) is a no-op — NOT unified with the version fallback, and NOT
//!   "if zero, use the default".
//! - **G7**: the fingerprint format is completely unpinned by the oracle —
//!   `T:104-109` calls [`create_workspace_cleanup_fingerprint`] but never
//!   inspects the return value (only compares it to itself via
//!   [`should_hide_workspace_cleanup_candidate`]); an implementation
//!   returning just the head would still pass. Format:
//!   `{version}|{branch}|{head}|{clean|dirty|unknown}|{bucket}`, joined by
//!   `|` (`O:198-204`). The git tri-state maps `None` -> `"unknown"`,
//!   `Some(true)` -> `"clean"`, `Some(false)` -> `"dirty"` (`O:202`). There
//!   is NO separator escaping — a branch containing `|` collides with the
//!   field boundary. Reproduced faithfully; NOT fixed.
//! - **G8**: the classifier-version clause (`O:238`) can be deleted entirely
//!   and the oracle still passes — its sole exercise (`T:103-128`) passes
//!   [`WORKSPACE_CLEANUP_CLASSIFIER_VERSION`] straight through on both the
//!   matching and non-matching branch, so neither assertion depends on that
//!   clause firing. [`should_hide_workspace_cleanup_candidate`] reads the
//!   module constant directly — NOT parameterized for testability. The
//!   comparison is `==` (not `>=`), so a version *downgrade* invalidates a
//!   dismissal exactly as much as an upgrade does.
//! - **G9**: [`should_force_workspace_cleanup_removal`] passes the oracle
//!   even if implemented as ONLY the blocker-membership check — its sole
//!   exercise (`T:74-82`) has `clean: true, checked_at: Some(_)`, so the
//!   first two disjuncts never fire, and the function is never asserted to
//!   return `false` anywhere in the oracle. Three disjuncts (`O:157-161`):
//!   `git.clean != Some(true)` OR `git.checked_at.is_none()` OR any
//!   [`WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS`] member present.
//! - **G10**: `git.clean !== true` (`O:158`) is NOT `=== false` — `None`
//!   (git status unknown) forces removal exactly like `Some(false)` (dirty).
//!   Written as `!= Some(true)`; `unwrap_or` is deliberately never used here
//!   (that reflex — collapsing "unknown" into "known-good" or
//!   "known-bad" — is what broke `suaegi-project-runtime`'s E3 in an earlier
//!   port).
//! - **G11**: `git.checkedAt === null` (`O:159`, `O:170`) is a strict
//!   comparison, so `checked_at: Some(0)` is a VALID/fresh check, not a
//!   sentinel for "never checked". `checked_at`, `upstream_ahead`,
//!   `upstream_behind`, and `newest_diff_comment_at` are all modeled as
//!   `Option<i64>` — never a magic `0`/`-1`. The oracle's one exercise of
//!   this (`T:92-101`) changes `clean` and `checked_at` together, so the two
//!   `O:169`/`O:170` conjuncts are never independently exercised by it.
//! - **G12**: [`apply_workspace_cleanup_policy`] must preserve every field
//!   other than `tier`/`selected_by_default` (`O:182-186`'s spread only
//!   overwrites those two), including an ABSENT `created_at` (the producer
//!   deliberately omits that key — modeled as `Option::None` staying
//!   `None`). The input's `tier` and `selected_by_default` are dead values
//!   (the producer passes placeholders) and must NOT be read by this
//!   function, only overwritten. The function is idempotent.
//! - **G13**: the tier lattice has three mutually-supporting redundancies.
//!   [`can_select_workspace_cleanup_candidate`] re-checks hard blockers as
//!   its LAST conjunct (`O:171`), so `can_select ⟹ !has_hard_blocker`, which
//!   makes the ternary order at `O:180` and the `&& can_select` at `O:185`
//!   unobservably redundant TODAY. Kept verbatim, on purpose: the moment the
//!   hard-blocker conjunct is removed from
//!   [`can_select_workspace_cleanup_candidate`], "simplifying"
//!   `selected_by_default` to just `can_select` would silently start
//!   selecting protected candidates.
//! - **G14**: `reasons` order is fixed and meaningful — `archived` before
//!   `idle-clean` (`O:216`, `O:219`); returned as a `Vec`, never a set, and
//!   the two `if`s are never reordered.
//!   [`apply_workspace_cleanup_policy`] passes `blockers` through in input
//!   order (via struct-update syntax, so nothing reorders it).
//! - **G15**: [`should_hide_workspace_cleanup_candidate`]'s three-clause
//!   conjunction lives entirely inside the "dismissal present" arm — the TS
//!   optional-chain `dismissal?.worktreeId` (`O:236`) makes clauses 2 and 3
//!   unreachable when `dismissal` is absent, i.e. `undefined?.x === y` is
//!   `false`, never a thrown error, so absence short-circuits the whole
//!   expression to `false`. Implemented with `Option::is_some_and`.
//!   `dismissed_at` is never read anywhere in this module — there is no TTL.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// O:1-3 module constants
// ---------------------------------------------------------------------------

/// `O:1`. G8: compared with `==`, never `>=` — see
/// [`should_hide_workspace_cleanup_candidate`].
pub const WORKSPACE_CLEANUP_CLASSIFIER_VERSION: i64 = 2;

/// `O:2`, `7 * 24 * 60 * 60 * 1000`.
pub const WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// `O:3`, `30 * 24 * 60 * 60 * 1000`.
pub const WORKSPACE_CLEANUP_IDLE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

// ---------------------------------------------------------------------------
// O:5 WorkspaceCleanupTier
// ---------------------------------------------------------------------------

/// `O:5`, `'ready' | 'review' | 'protected'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanupTier {
    Ready,
    Review,
    Protected,
}

// ---------------------------------------------------------------------------
// O:7 WorkspaceCleanupReason
// ---------------------------------------------------------------------------

/// `O:7`, `'archived' | 'idle-clean'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanupReason {
    Archived,
    IdleClean,
}

// ---------------------------------------------------------------------------
// O:9-12 WorkspaceCleanupInactivityInput
// ---------------------------------------------------------------------------

/// `O:9-12`.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceCleanupInactivityInput {
    pub is_archived: bool,
    pub last_activity_at: i64,
}

// ---------------------------------------------------------------------------
// O:14-30 WorkspaceCleanupBlocker
// ---------------------------------------------------------------------------

/// `O:14-30`, all 16 members of the union, in source declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanupBlocker {
    MainWorktree,
    FolderRepo,
    Pinned,
    ActiveWorkspace,
    RunningTerminal,
    TerminalLivenessUnknown,
    DirtyEditorBuffer,
    VolatileLocalContext,
    RecentVisibleContext,
    LiveAgent,
    SshDisconnected,
    GitStatusError,
    DirtyFiles,
    UnpushedCommits,
    UnknownBase,
    Dismissed,
}

// ---------------------------------------------------------------------------
// O:32-37 WorkspaceCleanupDismissal
// ---------------------------------------------------------------------------

/// `O:32-37`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCleanupDismissal {
    pub worktree_id: String,
    pub dismissed_at: i64,
    pub fingerprint: String,
    pub classifier_version: i64,
}

// ---------------------------------------------------------------------------
// O:39-41 WorkspaceCleanupUIState
// ---------------------------------------------------------------------------

/// `O:39-41`, `Record<string, WorkspaceCleanupDismissal>`.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceCleanupUIState {
    pub dismissals: HashMap<String, WorkspaceCleanupDismissal>,
}

// ---------------------------------------------------------------------------
// O:43-72 WorkspaceCleanupCandidate (+ nested localContext / git)
// ---------------------------------------------------------------------------

/// `O:57-63`, the `localContext` field of [`WorkspaceCleanupCandidate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceCleanupLocalContext {
    pub terminal_tab_count: i64,
    pub clean_editor_tab_count: i64,
    pub browser_tab_count: i64,
    pub diff_comment_count: i64,
    pub newest_diff_comment_at: Option<i64>,
    pub retained_done_agent_count: i64,
}

/// `O:65-70`, the `git` field of [`WorkspaceCleanupCandidate`]. G10/G11:
/// `clean` and `checked_at` are both tri-state `Option`, never collapsed to
/// a `bool`/magic-number sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceCleanupGitState {
    pub clean: Option<bool>,
    pub upstream_ahead: Option<i64>,
    pub upstream_behind: Option<i64>,
    pub checked_at: Option<i64>,
}

/// `O:43-72`. G12: `created_at` is `Option<i64>` — `None` models the
/// producer deliberately omitting the TS `createdAt?: number` key, not a
/// numeric sentinel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCleanupCandidate {
    pub worktree_id: String,
    pub repo_id: String,
    pub repo_name: String,
    pub connection_id: Option<String>,
    pub display_name: String,
    pub branch: String,
    pub path: String,
    pub tier: WorkspaceCleanupTier,
    pub selected_by_default: bool,
    pub reasons: Vec<WorkspaceCleanupReason>,
    pub blockers: Vec<WorkspaceCleanupBlocker>,
    pub last_activity_at: i64,
    pub created_at: Option<i64>,
    pub local_context: WorkspaceCleanupLocalContext,
    pub git: WorkspaceCleanupGitState,
    pub fingerprint: String,
}

// ---------------------------------------------------------------------------
// O:74-111 remaining record types (IPC-shaped in Orca; ported for type
// fidelity even though this module's 9 functions don't consume them).
// ---------------------------------------------------------------------------

/// `O:74-78`. All three fields are optional in the TS source.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceCleanupScanArgs {
    pub worktree_id: Option<String>,
    pub skip_git_worktree_ids: Option<Vec<String>>,
    pub scan_id: Option<String>,
}

/// `O:80-84`. `worktree_id` is required; `connection_id` is `string | null`
/// and optional in the TS source — collapsed to a single `Option<String>`
/// since nothing in this module distinguishes an absent key from an
/// explicit `null` here (no serialization boundary in this crate).
#[derive(Debug, Clone)]
pub struct WorkspaceCleanupLocalProcessArgs {
    pub worktree_id: String,
    pub connection_id: Option<String>,
    pub worktree_path: Option<String>,
}

/// `O:86-90`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCleanupScanError {
    pub repo_id: String,
    pub repo_name: String,
    pub message: String,
}

/// `O:92-96`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCleanupScanResult {
    pub scanned_at: i64,
    pub candidates: Vec<WorkspaceCleanupCandidate>,
    pub errors: Vec<WorkspaceCleanupScanError>,
}

/// `O:102`, `'append' | 'snapshot'` — one of the plan's "4 string-literal
/// unions" (with [`WorkspaceCleanupTier`], [`WorkspaceCleanupReason`],
/// [`WorkspaceCleanupBlocker`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCleanupCandidateMode {
    Append,
    Snapshot,
}

/// `O:98-103`, `WorkspaceCleanupScanResult & { ... }`. TS `&`-intersection is
/// flattened into one struct since Rust has no structural intersection
/// types; `candidate_mode` stays optional (`O:102`'s `?`).
#[derive(Debug, Clone)]
pub struct WorkspaceCleanupScanProgress {
    pub scanned_at: i64,
    pub candidates: Vec<WorkspaceCleanupCandidate>,
    pub errors: Vec<WorkspaceCleanupScanError>,
    pub scan_id: String,
    pub scanned_worktree_count: i64,
    pub total_worktree_count: i64,
    pub candidate_mode: Option<WorkspaceCleanupCandidateMode>,
}

/// `O:105-107`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceCleanupLocalProcessResult {
    pub has_killable_processes: Option<bool>,
}

/// `O:109-111`.
#[derive(Debug, Clone)]
pub struct WorkspaceCleanupDismissArgs {
    pub dismissals: Vec<WorkspaceCleanupDismissal>,
}

// ---------------------------------------------------------------------------
// O:113-130 WORKSPACE_CLEANUP_HARD_BLOCKERS (exported)
// ---------------------------------------------------------------------------

/// `O:113-130`. G1: ALL 16 [`WorkspaceCleanupBlocker`] variants, written out
/// explicitly rather than derived from "is the blocker list non-empty" —
/// see the crate-level G1 doc.
pub const WORKSPACE_CLEANUP_HARD_BLOCKERS: [WorkspaceCleanupBlocker; 16] = [
    WorkspaceCleanupBlocker::MainWorktree,
    WorkspaceCleanupBlocker::FolderRepo,
    WorkspaceCleanupBlocker::Pinned,
    WorkspaceCleanupBlocker::ActiveWorkspace,
    WorkspaceCleanupBlocker::RunningTerminal,
    WorkspaceCleanupBlocker::TerminalLivenessUnknown,
    WorkspaceCleanupBlocker::DirtyEditorBuffer,
    WorkspaceCleanupBlocker::VolatileLocalContext,
    WorkspaceCleanupBlocker::LiveAgent,
    WorkspaceCleanupBlocker::RecentVisibleContext,
    WorkspaceCleanupBlocker::SshDisconnected,
    WorkspaceCleanupBlocker::GitStatusError,
    WorkspaceCleanupBlocker::DirtyFiles,
    WorkspaceCleanupBlocker::UnpushedCommits,
    WorkspaceCleanupBlocker::UnknownBase,
    WorkspaceCleanupBlocker::Dismissed,
];

// ---------------------------------------------------------------------------
// O:132-136 WORKSPACE_CLEANUP_QUEUE_BLOCKERS (module-private; G2)
// ---------------------------------------------------------------------------

/// `O:132-136`. G2: NOT `export`ed in the TS source (the only one of the
/// three sets without `export`) — kept module-private here too.
const WORKSPACE_CLEANUP_QUEUE_BLOCKERS: [WorkspaceCleanupBlocker; 3] = [
    WorkspaceCleanupBlocker::MainWorktree,
    WorkspaceCleanupBlocker::FolderRepo,
    WorkspaceCleanupBlocker::Dismissed,
];

// ---------------------------------------------------------------------------
// O:138-139 WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS (exported)
// ---------------------------------------------------------------------------

/// `O:138-139`. G1: genuinely different from
/// [`WORKSPACE_CLEANUP_QUEUE_BLOCKERS`] — `UnpushedCommits` is a member here
/// but NOT of the queue set, which is the asymmetry `T:74-82` exercises.
pub const WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS: [WorkspaceCleanupBlocker; 4] = [
    WorkspaceCleanupBlocker::DirtyFiles,
    WorkspaceCleanupBlocker::UnpushedCommits,
    WorkspaceCleanupBlocker::UnknownBase,
    WorkspaceCleanupBlocker::GitStatusError,
];

// ---------------------------------------------------------------------------
// O:141-143 isWorkspaceCleanupHardBlocker (exported)
// ---------------------------------------------------------------------------

/// `O:141-143`.
pub fn is_workspace_cleanup_hard_blocker(blocker: WorkspaceCleanupBlocker) -> bool {
    WORKSPACE_CLEANUP_HARD_BLOCKERS.contains(&blocker)
}

// ---------------------------------------------------------------------------
// O:145-152 canQueueWorkspaceCleanupCandidate (exported)
// ---------------------------------------------------------------------------

/// `O:145-152`. TS `Pick<Candidate, 'blockers' | 'reasons'>` is collapsed to
/// a full [`WorkspaceCleanupCandidate`] reference — the only caller in the
/// oracle (and in Orca's UI layer) already has the whole candidate in hand.
pub fn can_queue_workspace_cleanup_candidate(candidate: &WorkspaceCleanupCandidate) -> bool {
    !candidate.reasons.is_empty()
        && !candidate
            .blockers
            .iter()
            .any(|blocker| WORKSPACE_CLEANUP_QUEUE_BLOCKERS.contains(blocker))
}

// ---------------------------------------------------------------------------
// O:154-162 shouldForceWorkspaceCleanupRemoval (exported)
// ---------------------------------------------------------------------------

/// `O:154-162`. G9: three independent disjuncts. G10: clause 1 is
/// `!= Some(true)`, not `== Some(false)` — `None` (unknown) forces removal
/// too. G11: clause 2 is `is_none()`, so `checked_at: Some(0)` does NOT
/// trigger it.
pub fn should_force_workspace_cleanup_removal(candidate: &WorkspaceCleanupCandidate) -> bool {
    candidate.git.clean != Some(true)
        || candidate.git.checked_at.is_none()
        || candidate
            .blockers
            .iter()
            .any(|blocker| WORKSPACE_CLEANUP_FORCE_REMOVE_BLOCKERS.contains(blocker))
}

// ---------------------------------------------------------------------------
// O:164-173 canSelectWorkspaceCleanupCandidate (exported)
// ---------------------------------------------------------------------------

/// `O:164-173`. G13: the last conjunct re-checks hard blockers, which makes
/// `can_select ⟹ !has_hard_blocker` — load-bearing for the redundancies in
/// [`apply_workspace_cleanup_policy`] (see G13 crate doc).
pub fn can_select_workspace_cleanup_candidate(candidate: &WorkspaceCleanupCandidate) -> bool {
    !candidate.reasons.is_empty()
        && candidate.git.clean == Some(true)
        && candidate.git.checked_at.is_some()
        && !candidate
            .blockers
            .iter()
            .any(|&blocker| is_workspace_cleanup_hard_blocker(blocker))
}

// ---------------------------------------------------------------------------
// O:175-187 applyWorkspaceCleanupPolicy (exported)
// ---------------------------------------------------------------------------

/// `O:175-187`. G12: preserves every field but `tier`/`selected_by_default`
/// via struct-update syntax (never reconstructs the struct field-by-field,
/// so nothing can be silently dropped); the input's `tier` and
/// `selected_by_default` are dead values and are never read here, only
/// overwritten. G13: the ternary order (hard-blocker first, then
/// `can_select`) and the `&& can_select` in `selected_by_default` are kept
/// verbatim even though `can_select ⟹ !has_hard_blocker` makes both
/// unobservably redundant today — see the crate-level G13 doc for why
/// removing either is NOT a safe "simplification". G14: `blockers` (and
/// every other field) pass through in input order, unmodified.
pub fn apply_workspace_cleanup_policy(
    candidate: WorkspaceCleanupCandidate,
) -> WorkspaceCleanupCandidate {
    let can_select = can_select_workspace_cleanup_candidate(&candidate);
    let has_hard_blocker = candidate
        .blockers
        .iter()
        .any(|&blocker| is_workspace_cleanup_hard_blocker(blocker));
    // G13: `has_hard_blocker` in this ternary and the re-check inside
    // `can_select` are mutually redundant (`can_select` already implies
    // `!has_hard_blocker`) — kept verbatim, load-bearing on each other.
    let tier = if has_hard_blocker {
        WorkspaceCleanupTier::Protected
    } else if can_select {
        WorkspaceCleanupTier::Ready
    } else {
        WorkspaceCleanupTier::Review
    };
    // G13: `&& can_select` is likewise redundant with `tier ==
    // WorkspaceCleanupTier::Ready` today (Ready is only reachable when
    // `can_select` was true), but removing it is NOT safe once the
    // hard-blocker re-check above is ever removed from `can_select`.
    let selected_by_default = tier == WorkspaceCleanupTier::Ready && can_select;

    WorkspaceCleanupCandidate {
        tier,
        selected_by_default,
        ..candidate
    }
}

// ---------------------------------------------------------------------------
// O:189-205 createWorkspaceCleanupFingerprint (exported)
// ---------------------------------------------------------------------------

/// `O:189-194`, the args of [`create_workspace_cleanup_fingerprint`].
/// `classifier_version` mirrors the TS optional param (`O:194`, `?:
/// number`) — `None` means "argument omitted", falling back to
/// [`WORKSPACE_CLEANUP_CLASSIFIER_VERSION`] (G6, `??` semantics).
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceCleanupFingerprintArgs<'a> {
    pub branch: &'a str,
    pub head: &'a str,
    pub git_clean: Option<bool>,
    pub last_activity_at: i64,
    pub classifier_version: Option<i64>,
}

/// `O:189-205`. G5: the day bucket uses [`i64::div_euclid`], NOT `/` —
/// floors toward negative infinity like `Math.floor`, not toward zero. G6:
/// `classifier_version` falls back via `Option::unwrap_or` (`??`
/// semantics — an explicit `Some(0)` survives as `0`); `last_activity_at`
/// has no `Option` fallback at all, since the TS `|| 0` is a no-op once
/// modeled as a non-`Option` `i64`. G7: format is
/// `{version}|{branch}|{head}|{clean|dirty|unknown}|{bucket}`, joined by
/// `|`, with NO separator escaping — a `branch` containing `|` collides
/// with the field boundary, reproduced faithfully.
pub fn create_workspace_cleanup_fingerprint(args: &WorkspaceCleanupFingerprintArgs) -> String {
    let version = args
        .classifier_version
        .unwrap_or(WORKSPACE_CLEANUP_CLASSIFIER_VERSION);
    let last_activity_bucket = args.last_activity_at.div_euclid(24 * 60 * 60 * 1000);
    let git_state = match args.git_clean {
        None => "unknown",
        Some(true) => "clean",
        Some(false) => "dirty",
    };
    [
        version.to_string(),
        args.branch.to_string(),
        args.head.to_string(),
        git_state.to_string(),
        last_activity_bucket.to_string(),
    ]
    .join("|")
}

// ---------------------------------------------------------------------------
// O:207-222 getWorkspaceCleanupInactivityReasons (exported)
// ---------------------------------------------------------------------------

/// `O:207-222`. G3: both comparisons are `>=` (inclusive); the archived
/// check is guarded by `workspace.is_archived`, the idle check is NOT; the
/// two are independent, so both reasons can fire together (archived first).
/// G4: signed `i64` subtraction — a future `last_activity_at` yields a
/// negative elapsed time, which never satisfies either `>=` threshold.
pub fn get_workspace_cleanup_inactivity_reasons(
    workspace: &WorkspaceCleanupInactivityInput,
    scanned_at: i64,
) -> Vec<WorkspaceCleanupReason> {
    let mut reasons = Vec::new();
    let elapsed = scanned_at - workspace.last_activity_at;
    if workspace.is_archived && elapsed >= WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS {
        reasons.push(WorkspaceCleanupReason::Archived);
    }
    if elapsed >= WORKSPACE_CLEANUP_IDLE_MS {
        reasons.push(WorkspaceCleanupReason::IdleClean);
    }
    reasons
}

// ---------------------------------------------------------------------------
// O:224-229 isWorkspaceOldForCleanup (exported)
// ---------------------------------------------------------------------------

/// `O:224-229`. A one-line wrapper around
/// [`get_workspace_cleanup_inactivity_reasons`] — no independent seam.
pub fn is_workspace_old_for_cleanup(
    workspace: &WorkspaceCleanupInactivityInput,
    scanned_at: i64,
) -> bool {
    !get_workspace_cleanup_inactivity_reasons(workspace, scanned_at).is_empty()
}

// ---------------------------------------------------------------------------
// O:231-240 shouldHideWorkspaceCleanupCandidate (exported)
// ---------------------------------------------------------------------------

/// `O:231-240`. TS `Pick<Candidate, 'worktreeId' | 'fingerprint'>` is
/// collapsed to a full [`WorkspaceCleanupCandidate`] reference, same
/// rationale as [`can_queue_workspace_cleanup_candidate`]. G15: the
/// three-clause conjunction (`worktree_id` match, `fingerprint` match,
/// `classifier_version` match) lives entirely inside `Option::is_some_and`
/// — when `dismissal` is `None`, the whole expression is `false` without
/// ever evaluating clauses 2/3, mirroring the TS optional chain (`O:236`,
/// `dismissal?.worktreeId === ...`) short-circuiting on `undefined`. G8: the
/// version clause compares against the module constant directly (never a
/// parameter), with `==`, so either a downgrade or an upgrade invalidates a
/// dismissal. `dismissed_at` is never read — there is no TTL.
pub fn should_hide_workspace_cleanup_candidate(
    candidate: &WorkspaceCleanupCandidate,
    dismissal: Option<&WorkspaceCleanupDismissal>,
) -> bool {
    dismissal.is_some_and(|dismissal| {
        dismissal.worktree_id == candidate.worktree_id
            && dismissal.fingerprint == candidate.fingerprint
            && dismissal.classifier_version == WORKSPACE_CLEANUP_CLASSIFIER_VERSION
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate() -> WorkspaceCleanupCandidate {
        WorkspaceCleanupCandidate {
            worktree_id: "repo-1::/tmp/feature".to_string(),
            repo_id: "repo-1".to_string(),
            repo_name: "Repo".to_string(),
            connection_id: None,
            display_name: "feature".to_string(),
            branch: "feature".to_string(),
            path: "/tmp/feature".to_string(),
            tier: WorkspaceCleanupTier::Review,
            selected_by_default: false,
            reasons: vec![WorkspaceCleanupReason::IdleClean],
            blockers: vec![],
            last_activity_at: 1_700_000_000_000,
            created_at: None,
            local_context: WorkspaceCleanupLocalContext {
                terminal_tab_count: 0,
                clean_editor_tab_count: 0,
                browser_tab_count: 0,
                diff_comment_count: 0,
                newest_diff_comment_at: None,
                retained_done_agent_count: 0,
            },
            git: WorkspaceCleanupGitState {
                clean: Some(true),
                upstream_ahead: Some(0),
                upstream_behind: Some(0),
                checked_at: Some(1_700_000_000_000),
            },
            fingerprint: "fingerprint".to_string(),
        }
    }

    // =======================================================================
    // Oracle — T:58-64
    // =======================================================================

    #[test]
    fn oracle_marks_clean_inactive_workspaces_as_ready_and_selected() {
        let candidate = apply_workspace_cleanup_policy(make_candidate());

        assert_eq!(candidate.tier, WorkspaceCleanupTier::Ready);
        assert!(candidate.selected_by_default);
        assert!(can_select_workspace_cleanup_candidate(&candidate));
    }

    // =======================================================================
    // Oracle — T:66-72
    // =======================================================================

    #[test]
    fn oracle_requires_an_inactivity_reason_before_selecting_a_workspace() {
        let mut input = make_candidate();
        input.reasons = vec![];
        let candidate = apply_workspace_cleanup_policy(input);

        assert!(!can_select_workspace_cleanup_candidate(&candidate));
        assert_eq!(candidate.tier, WorkspaceCleanupTier::Review);
        assert!(!candidate.selected_by_default);
    }

    // =======================================================================
    // Oracle — T:74-82 (the G1 asymmetry: unpushed-commits is queueable YET
    // forces removal)
    // =======================================================================

    #[test]
    fn oracle_keeps_not_suggested_candidates_queueable_when_git_evidence_is_clean() {
        let mut input = make_candidate();
        input.blockers = vec![WorkspaceCleanupBlocker::UnpushedCommits];
        let candidate = apply_workspace_cleanup_policy(input);

        assert_eq!(candidate.tier, WorkspaceCleanupTier::Protected);
        assert!(!candidate.selected_by_default);
        assert!(!can_select_workspace_cleanup_candidate(&candidate));
        assert!(can_queue_workspace_cleanup_candidate(&candidate));
        assert!(should_force_workspace_cleanup_removal(&candidate));
    }

    // =======================================================================
    // Oracle — T:84-90
    // =======================================================================

    #[test]
    fn oracle_does_not_queue_main_worktrees_or_folder_projects_for_cleanup_removal() {
        let mut main_worktree_input = make_candidate();
        main_worktree_input.blockers = vec![WorkspaceCleanupBlocker::MainWorktree];
        let main_worktree = apply_workspace_cleanup_policy(main_worktree_input);

        let mut folder_project_input = make_candidate();
        folder_project_input.blockers = vec![WorkspaceCleanupBlocker::FolderRepo];
        let folder_project = apply_workspace_cleanup_policy(folder_project_input);

        assert!(!can_queue_workspace_cleanup_candidate(&main_worktree));
        assert!(!can_queue_workspace_cleanup_candidate(&folder_project));
    }

    // =======================================================================
    // Oracle — T:92-101
    // =======================================================================

    #[test]
    fn oracle_requires_current_git_status_before_selecting_a_workspace() {
        let mut input = make_candidate();
        input.git.clean = None;
        input.git.checked_at = None;
        let candidate = apply_workspace_cleanup_policy(input);

        assert_eq!(candidate.tier, WorkspaceCleanupTier::Review);
        assert!(!can_select_workspace_cleanup_candidate(&candidate));
    }

    // =======================================================================
    // Oracle — T:103-128
    // =======================================================================

    #[test]
    fn oracle_matches_dismissals_only_for_the_current_classifier_fingerprint() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "feature",
            head: "abc123",
            git_clean: Some(true),
            last_activity_at: 1_700_000_000_000,
            classifier_version: None,
        });
        let mut candidate = make_candidate();
        candidate.fingerprint = fingerprint.clone();

        let matching_dismissal = WorkspaceCleanupDismissal {
            worktree_id: candidate.worktree_id.clone(),
            dismissed_at: 1_700_000_000_000,
            fingerprint: fingerprint.clone(),
            classifier_version: WORKSPACE_CLEANUP_CLASSIFIER_VERSION,
        };
        assert!(should_hide_workspace_cleanup_candidate(
            &candidate,
            Some(&matching_dismissal)
        ));

        let changed_dismissal = WorkspaceCleanupDismissal {
            worktree_id: candidate.worktree_id.clone(),
            dismissed_at: 1_700_000_000_000,
            fingerprint: format!("{fingerprint}|changed"),
            classifier_version: WORKSPACE_CLEANUP_CLASSIFIER_VERSION,
        };
        assert!(!should_hide_workspace_cleanup_candidate(
            &candidate,
            Some(&changed_dismissal)
        ));
    }

    // =======================================================================
    // G1 — all 16 blockers are hard; the queue/force-remove asymmetry.
    // =======================================================================

    #[test]
    fn g1_all_sixteen_blockers_are_hard() {
        let all = [
            WorkspaceCleanupBlocker::MainWorktree,
            WorkspaceCleanupBlocker::FolderRepo,
            WorkspaceCleanupBlocker::Pinned,
            WorkspaceCleanupBlocker::ActiveWorkspace,
            WorkspaceCleanupBlocker::RunningTerminal,
            WorkspaceCleanupBlocker::TerminalLivenessUnknown,
            WorkspaceCleanupBlocker::DirtyEditorBuffer,
            WorkspaceCleanupBlocker::VolatileLocalContext,
            WorkspaceCleanupBlocker::RecentVisibleContext,
            WorkspaceCleanupBlocker::LiveAgent,
            WorkspaceCleanupBlocker::SshDisconnected,
            WorkspaceCleanupBlocker::GitStatusError,
            WorkspaceCleanupBlocker::DirtyFiles,
            WorkspaceCleanupBlocker::UnpushedCommits,
            WorkspaceCleanupBlocker::UnknownBase,
            WorkspaceCleanupBlocker::Dismissed,
        ];
        assert_eq!(all.len(), 16);
        for blocker in all {
            assert!(
                is_workspace_cleanup_hard_blocker(blocker),
                "{blocker:?} must be a hard blocker"
            );
        }
    }

    #[test]
    fn g1_unpushed_commits_is_queueable_but_forces_removal() {
        let mut candidate = make_candidate();
        candidate.blockers = vec![WorkspaceCleanupBlocker::UnpushedCommits];

        assert!(can_queue_workspace_cleanup_candidate(&candidate));
        assert!(should_force_workspace_cleanup_removal(&candidate));
        assert!(is_workspace_cleanup_hard_blocker(
            WorkspaceCleanupBlocker::UnpushedCommits
        ));
    }

    // =======================================================================
    // G2 — the queue set is private (accessible here only because `tests` is
    // a child module) and has exactly these 3 members, `dismissed` included.
    // =======================================================================

    #[test]
    fn g2_queue_blocker_set_has_exactly_these_three_members() {
        assert_eq!(WORKSPACE_CLEANUP_QUEUE_BLOCKERS.len(), 3);
        assert_eq!(
            WORKSPACE_CLEANUP_QUEUE_BLOCKERS,
            [
                WorkspaceCleanupBlocker::MainWorktree,
                WorkspaceCleanupBlocker::FolderRepo,
                WorkspaceCleanupBlocker::Dismissed,
            ]
        );
    }

    // =======================================================================
    // G3 — both idle thresholds are `>=` boundaries; `is_archived` guard;
    // both reasons firing together.
    // =======================================================================

    #[test]
    fn g3_archived_threshold_minus_one_produces_no_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: true,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS - 1;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![]
        );
    }

    #[test]
    fn g3_archived_threshold_exact_produces_archived_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: true,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![WorkspaceCleanupReason::Archived]
        );
    }

    #[test]
    fn g3_archived_threshold_plus_one_produces_archived_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: true,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_ARCHIVED_IDLE_MS + 1;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![WorkspaceCleanupReason::Archived]
        );
    }

    #[test]
    fn g3_unarchived_guard_suppresses_archived_reason_at_eight_days() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: false,
            last_activity_at: 0,
        };
        // 8 days: past the 7-day archived threshold, nowhere near the
        // 30-day idle threshold.
        let scanned_at = 8 * 24 * 60 * 60 * 1000;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![]
        );
    }

    #[test]
    fn g3_idle_threshold_minus_one_produces_no_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: false,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_IDLE_MS - 1;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![]
        );
    }

    #[test]
    fn g3_idle_threshold_exact_produces_idle_clean_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: false,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_IDLE_MS;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![WorkspaceCleanupReason::IdleClean]
        );
    }

    #[test]
    fn g3_idle_threshold_plus_one_produces_idle_clean_reason() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: false,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_IDLE_MS + 1;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![WorkspaceCleanupReason::IdleClean]
        );
    }

    #[test]
    fn g3_archived_and_idle_clean_both_fire_in_order_when_idle_thirty_days_and_archived() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: true,
            last_activity_at: 0,
        };
        let scanned_at = WORKSPACE_CLEANUP_IDLE_MS;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![
                WorkspaceCleanupReason::Archived,
                WorkspaceCleanupReason::IdleClean
            ]
        );
        assert!(is_workspace_old_for_cleanup(&workspace, scanned_at));
    }

    // =======================================================================
    // G4 — signed subtraction: a future last_activity_at yields no reasons.
    // =======================================================================

    #[test]
    fn g4_future_last_activity_at_produces_no_reasons() {
        let workspace = WorkspaceCleanupInactivityInput {
            is_archived: true,
            // last_activity_at is AFTER scanned_at (clock skew / bad data):
            // elapsed = 1_000 - 1_000_000_000_000 is deeply negative.
            last_activity_at: 1_000_000_000_000,
        };
        let scanned_at = 1_000;
        assert_eq!(
            get_workspace_cleanup_inactivity_reasons(&workspace, scanned_at),
            vec![]
        );
        assert!(!is_workspace_old_for_cleanup(&workspace, scanned_at));
    }

    // =======================================================================
    // G5 — the fingerprint bucket floors toward negative infinity.
    // =======================================================================

    #[test]
    fn g5_negative_last_activity_at_produces_bucket_negative_one() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "b",
            head: "h",
            git_clean: None,
            last_activity_at: -1,
            classifier_version: None,
        });
        assert_eq!(fingerprint, "2|b|h|unknown|-1");
    }

    // =======================================================================
    // G6 — classifier_version: Some(0) survives as 0 (`??`, not `||`).
    // =======================================================================

    #[test]
    fn g6_explicit_zero_classifier_version_is_honored_not_defaulted() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "b",
            head: "h",
            git_clean: Some(true),
            last_activity_at: 0,
            classifier_version: Some(0),
        });
        assert_eq!(fingerprint, "0|b|h|clean|0");
    }

    // =======================================================================
    // G7 — full fingerprint string, all three git-state arms, and the
    // unescaped `|`-collision in `branch`.
    // =======================================================================

    #[test]
    fn g7_full_fingerprint_string_for_a_known_input() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "feature",
            head: "abc123",
            git_clean: Some(true),
            last_activity_at: 1_700_000_000_000,
            classifier_version: None,
        });
        assert_eq!(fingerprint, "2|feature|abc123|clean|19675");
    }

    #[test]
    fn g7_unknown_git_state_arm() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "feature",
            head: "abc123",
            git_clean: None,
            last_activity_at: 0,
            classifier_version: Some(1),
        });
        assert_eq!(fingerprint, "1|feature|abc123|unknown|0");
    }

    #[test]
    fn g7_dirty_git_state_arm() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "feature",
            head: "abc123",
            git_clean: Some(false),
            last_activity_at: 0,
            classifier_version: Some(1),
        });
        assert_eq!(fingerprint, "1|feature|abc123|dirty|0");
    }

    #[test]
    fn g7_branch_containing_pipe_collides_with_the_field_boundary_unescaped() {
        let fingerprint = create_workspace_cleanup_fingerprint(&WorkspaceCleanupFingerprintArgs {
            branch: "a|b",
            head: "h",
            git_clean: Some(true),
            last_activity_at: 0,
            classifier_version: Some(1),
        });
        // The unescaped `|` inside `branch` produces a 6-field-looking join
        // instead of 5 — reproduced faithfully, not fixed.
        assert_eq!(fingerprint, "1|a|b|h|clean|0");
        assert_eq!(fingerprint.split('|').count(), 6);
    }

    // =======================================================================
    // G8 — classifier-version mismatch (both directions), absent dismissal,
    // mismatched worktree_id.
    // =======================================================================

    #[test]
    fn g8_higher_classifier_version_is_not_hidden() {
        let candidate = make_candidate();
        let dismissal = WorkspaceCleanupDismissal {
            worktree_id: candidate.worktree_id.clone(),
            dismissed_at: 0,
            fingerprint: candidate.fingerprint.clone(),
            classifier_version: WORKSPACE_CLEANUP_CLASSIFIER_VERSION + 1,
        };
        assert!(!should_hide_workspace_cleanup_candidate(
            &candidate,
            Some(&dismissal)
        ));
    }

    #[test]
    fn g8_lower_classifier_version_is_not_hidden() {
        let candidate = make_candidate();
        let dismissal = WorkspaceCleanupDismissal {
            worktree_id: candidate.worktree_id.clone(),
            dismissed_at: 0,
            fingerprint: candidate.fingerprint.clone(),
            classifier_version: WORKSPACE_CLEANUP_CLASSIFIER_VERSION - 1,
        };
        assert!(!should_hide_workspace_cleanup_candidate(
            &candidate,
            Some(&dismissal)
        ));
    }

    #[test]
    fn g8_absent_dismissal_is_not_hidden() {
        let candidate = make_candidate();
        assert!(!should_hide_workspace_cleanup_candidate(&candidate, None));
    }

    #[test]
    fn g8_mismatched_worktree_id_is_not_hidden() {
        let candidate = make_candidate();
        let dismissal = WorkspaceCleanupDismissal {
            worktree_id: "some-other-worktree".to_string(),
            dismissed_at: 0,
            fingerprint: candidate.fingerprint.clone(),
            classifier_version: WORKSPACE_CLEANUP_CLASSIFIER_VERSION,
        };
        assert!(!should_hide_workspace_cleanup_candidate(
            &candidate,
            Some(&dismissal)
        ));
    }

    // =======================================================================
    // G9 — the three should_force_removal disjuncts, each firing alone, plus
    // a case that returns false.
    // =======================================================================

    #[test]
    fn g9_dirty_git_alone_forces_removal() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(false);
        candidate.git.checked_at = Some(1_700_000_000_000);
        candidate.blockers = vec![];
        assert!(should_force_workspace_cleanup_removal(&candidate));
    }

    #[test]
    fn g9_missing_checked_at_alone_forces_removal() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(true);
        candidate.git.checked_at = None;
        candidate.blockers = vec![];
        assert!(should_force_workspace_cleanup_removal(&candidate));
    }

    #[test]
    fn g9_force_remove_blocker_alone_forces_removal() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(true);
        candidate.git.checked_at = Some(1_700_000_000_000);
        candidate.blockers = vec![WorkspaceCleanupBlocker::DirtyFiles];
        assert!(should_force_workspace_cleanup_removal(&candidate));
    }

    #[test]
    fn g9_clean_checked_and_no_force_blockers_does_not_force_removal() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(true);
        candidate.git.checked_at = Some(1_700_000_000_000);
        candidate.blockers = vec![];
        assert!(!should_force_workspace_cleanup_removal(&candidate));
    }

    // =======================================================================
    // G10 — clean: None (unknown) forces removal, same as Some(false).
    // =======================================================================

    #[test]
    fn g10_unknown_git_clean_forces_removal() {
        let mut candidate = make_candidate();
        candidate.git.clean = None;
        candidate.git.checked_at = Some(1_700_000_000_000);
        candidate.blockers = vec![];
        assert!(should_force_workspace_cleanup_removal(&candidate));
    }

    // =======================================================================
    // G11 — checked_at: Some(0) is a valid check; clean/checked_at varied on
    // independent axes.
    // =======================================================================

    #[test]
    fn g11_checked_at_zero_is_treated_as_checked() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(true);
        candidate.git.checked_at = Some(0);
        candidate.blockers = vec![];
        assert!(!should_force_workspace_cleanup_removal(&candidate));
        assert!(can_select_workspace_cleanup_candidate(&candidate));
    }

    #[test]
    fn g11_clean_true_but_checked_at_none_is_independent_axis() {
        let mut candidate = make_candidate();
        candidate.git.clean = Some(true);
        candidate.git.checked_at = None;
        candidate.blockers = vec![];
        assert!(should_force_workspace_cleanup_removal(&candidate));
        assert!(!can_select_workspace_cleanup_candidate(&candidate));
    }

    #[test]
    fn g11_checked_at_zero_but_clean_none_is_independent_axis() {
        let mut candidate = make_candidate();
        candidate.git.clean = None;
        candidate.git.checked_at = Some(0);
        candidate.blockers = vec![];
        assert!(should_force_workspace_cleanup_removal(&candidate));
        assert!(!can_select_workspace_cleanup_candidate(&candidate));
    }

    // =======================================================================
    // G12 — every field survives apply_policy, including absent created_at;
    // idempotence; garbage input tier/selected_by_default are ignored.
    // =======================================================================

    #[test]
    fn g12_every_field_is_preserved_including_absent_created_at() {
        let mut input = make_candidate();
        // Deliberately garbage placeholder tier/selected_by_default — the
        // producer's dead values, per G12 — plus a distinctive marker in
        // every other field.
        input.tier = WorkspaceCleanupTier::Protected;
        input.selected_by_default = true;
        input.worktree_id = "marker-worktree".to_string();
        input.repo_id = "marker-repo".to_string();
        input.repo_name = "Marker Repo".to_string();
        input.connection_id = Some("marker-conn".to_string());
        input.display_name = "Marker Display".to_string();
        input.branch = "marker-branch".to_string();
        input.path = "/marker/path".to_string();
        input.blockers = vec![WorkspaceCleanupBlocker::Pinned];
        input.last_activity_at = 42;
        input.created_at = None;
        input.local_context = WorkspaceCleanupLocalContext {
            terminal_tab_count: 1,
            clean_editor_tab_count: 2,
            browser_tab_count: 3,
            diff_comment_count: 4,
            newest_diff_comment_at: Some(5),
            retained_done_agent_count: 6,
        };
        input.git = WorkspaceCleanupGitState {
            clean: Some(true),
            upstream_ahead: Some(7),
            upstream_behind: Some(8),
            checked_at: Some(9),
        };
        input.fingerprint = "marker-fingerprint".to_string();
        let expected_local_context = input.local_context;
        let expected_git = input.git;

        let output = apply_workspace_cleanup_policy(input);

        assert_eq!(output.worktree_id, "marker-worktree");
        assert_eq!(output.repo_id, "marker-repo");
        assert_eq!(output.repo_name, "Marker Repo");
        assert_eq!(output.connection_id, Some("marker-conn".to_string()));
        assert_eq!(output.display_name, "Marker Display");
        assert_eq!(output.branch, "marker-branch");
        assert_eq!(output.path, "/marker/path");
        assert_eq!(output.blockers, vec![WorkspaceCleanupBlocker::Pinned]);
        assert_eq!(output.last_activity_at, 42);
        assert_eq!(
            output.created_at, None,
            "absent created_at must stay absent"
        );
        assert_eq!(output.local_context, expected_local_context);
        assert_eq!(output.git, expected_git);
        assert_eq!(output.fingerprint, "marker-fingerprint");
        // The garbage placeholder tier/selected_by_default were NOT read:
        // `Pinned` is a hard blocker, so the computed tier is `protected`
        // regardless of the placeholder — but `selected_by_default` must be
        // recomputed to `false`, not pass through the placeholder `true`.
        assert_eq!(output.tier, WorkspaceCleanupTier::Protected);
        assert!(!output.selected_by_default);
    }

    #[test]
    fn g12_apply_policy_is_idempotent() {
        let input = make_candidate();
        let once = apply_workspace_cleanup_policy(input);
        let twice = apply_workspace_cleanup_policy(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn g12_input_tier_and_selected_by_default_are_dead_values() {
        // Two inputs differing ONLY in the placeholder tier/selected flags
        // must produce identical output — those input fields are never
        // read.
        let mut a = make_candidate();
        a.tier = WorkspaceCleanupTier::Ready;
        a.selected_by_default = true;

        let mut b = make_candidate();
        b.tier = WorkspaceCleanupTier::Protected;
        b.selected_by_default = false;

        let output_a = apply_workspace_cleanup_policy(a);
        let output_b = apply_workspace_cleanup_policy(b);
        assert_eq!(output_a.tier, output_b.tier);
        assert_eq!(output_a.selected_by_default, output_b.selected_by_default);
    }

    // =======================================================================
    // G13 — a hard blocker present alongside a valid inactivity reason still
    // yields `protected`, never `ready`.
    // =======================================================================

    #[test]
    fn g13_hard_blocker_wins_over_an_otherwise_selectable_candidate() {
        let mut input = make_candidate();
        input.blockers = vec![WorkspaceCleanupBlocker::Pinned];
        let candidate = apply_workspace_cleanup_policy(input);

        assert_eq!(candidate.tier, WorkspaceCleanupTier::Protected);
        assert!(!candidate.selected_by_default);
    }

    // =======================================================================
    // G14 — reasons/blockers order preservation.
    // =======================================================================

    #[test]
    fn g14_apply_policy_preserves_blocker_input_order() {
        let mut input = make_candidate();
        input.blockers = vec![
            WorkspaceCleanupBlocker::UnpushedCommits,
            WorkspaceCleanupBlocker::Pinned,
        ];
        let candidate = apply_workspace_cleanup_policy(input);
        assert_eq!(
            candidate.blockers,
            vec![
                WorkspaceCleanupBlocker::UnpushedCommits,
                WorkspaceCleanupBlocker::Pinned,
            ]
        );
    }

    // =======================================================================
    // G15 — should_hide's conjunction lives entirely inside the Some arm.
    // =======================================================================

    #[test]
    fn g15_none_dismissal_short_circuits_without_touching_other_clauses() {
        let mut candidate = make_candidate();
        // An empty fingerprint would trivially satisfy an (incorrectly
        // implemented) "fingerprint == ''" default-dismissal comparison; the
        // point of this pin is that with `dismissal: None` the function must
        // be `false` regardless of what the candidate's own fields are.
        candidate.fingerprint = String::new();
        candidate.worktree_id = String::new();
        assert!(!should_hide_workspace_cleanup_candidate(&candidate, None));
    }
}
