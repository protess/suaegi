//! VERBATIM port of Orca's `src/shared/nested-repo-telemetry.ts` (233 lines)
//! @ v1.4.146-rc.0. Line citations below (`O:N`) refer to that source file.
//!
//! Ported: `O:7` [`NESTED_REPO_TELEMETRY_MAX_REPO_COUNT`], `O:9`
//! [`NestedRepoTelemetrySurface`], `O:12` [`NestedRepoTelemetryRuntimeKind`],
//! `O:15-20` [`NestedRepoScanTelemetryResult`], `O:23-28`
//! [`NestedRepoImportTelemetryAction`], `O:31`
//! [`NestedRepoImportTelemetryOutcome`], `O:34` [`NestedRepoCountBucket`],
//! `O:37-50` [`NestedRepoScanTelemetry`] (+ base fields), `O:52-59`
//! [`NestedRepoImportActionTelemetry`], `O:61-75`
//! [`NestedRepoImportResultTelemetry`], `O:77-82`
//! [`cap_nested_repo_telemetry_count`], `O:84-89`
//! (`normalize_nested_repo_telemetry_count`, private), `O:91-109`
//! [`bucket_nested_repo_telemetry_count`], `O:111-117`
//! [`should_emit_nested_repo_import_submit_telemetry`], `O:119-139`
//! [`create_nested_repo_telemetry_attempt_id`], `O:141-168`
//! [`build_nested_repo_scan_telemetry`], `O:170-193`
//! [`build_nested_repo_import_action_telemetry`], `O:195-233`
//! [`build_nested_repo_import_result_telemetry`].
//!
//! The three type-only imports (`O:1-5`, from `./types`) are modeled
//! NARROWLY, not as full ports of `NestedRepoScanResult` /
//! `ProjectGroupImportMode` / `ProjectGroupImportResult` — only the fields
//! this module actually reads are represented:
//! [`NestedRepoScanInput`] carries `scan.repos.length` (as
//! [`NestedRepoScanInput::repo_count`]), `scan.selectedPathKind`,
//! `scan.truncated`, `scan.timedOut`; [`ProjectGroupImportMode`] is the two-
//! member `'group' | 'separate'` union; [`NestedRepoImportResultCounts`]
//! carries `result.importedCount`, `result.alreadyKnownCount`,
//! `result.failedCount`.
//!
//! # Traps (see the plan's §1 for the full rationale; `M<N>` numbering
//! # matches `docs/superpowers/plans/2026-07-27-nested-repo-telemetry.md`)
//!
//! - **M1**: [`cap_nested_repo_telemetry_count`]'s finiteness gate (`O:78-80`,
//!   `!Number.isFinite(count)`) returns `0` for `NaN` AND both infinities —
//!   NOT the bucket ceiling. [`bucket_nested_repo_telemetry_count`] applies
//!   this cap FIRST (`O:92`), so `bucket(Infinity) == "0"` and
//!   `bucket(-5) == "0"`. Bucketing before capping, or saturating infinity
//!   to the cap instead of `0`, both yield `"16+"` for `Infinity`. The
//!   oracle never passes an infinity anywhere, and never passes a negative
//!   or fractional value to `bucket()` either — see the `m1_*` pins.
//! - **M2**: every rung of the bucket ladder (`O:93-108`) is `<=`
//!   (`capped == 0`, `capped == 1`, `capped <= 3`, `capped <= 7`,
//!   `capped <= 15`, else `"16+"`), so a boundary value lands in the LOWER
//!   bucket. The oracle (`T:40-45`) pins only the upper edges
//!   (`0,1,3,7,15,16`); the lower edges `4` and `8`, and `500`/`501`, have no
//!   bucket assertion at all — see `m2_full_bucket_table_zero_through_sixteen`
//!   and the `500`/`501` pins.
//! - **M3**: every counting argument at the API boundary
//!   (`found_count`/`selected_count`/`imported_count`/`already_known_count`/
//!   `failed_count`, plus `repo_count` on [`NestedRepoScanInput`]) is `f64`,
//!   matching the TS `number` parameters; only OUTPUT payload fields are
//!   `u32`. An unsigned-integer PARAMETER type would make the
//!   `!Number.isFinite` branches (`O:78-80`, `O:85-87`) dead code and make a
//!   negative count inexpressible, silently changing
//!   [`should_emit_nested_repo_import_submit_telemetry`]'s `> 0` check into
//!   something that can never observe zero-vs-negative.
//! - **M4**: `failedCount ?? selectedCount` (`O:210`) is nullish
//!   coalescing, not `||`. When `result` is present with `failedCount: 0`,
//!   `??` keeps `0`; a falsy (`||`) fallback would substitute
//!   `selected_count` instead, flipping `outcome` from `success` to
//!   `partial_failure` and changing `failed_count`/its bucket. Modeled as
//!   [`NestedRepoImportResultCounts::failed_count`] living inside
//!   `Option<NestedRepoImportResultCounts>` (`args.result`), so the derived
//!   `Option<f64>` is `None` ONLY when the whole `result` is absent, and
//!   `Some(0.0)` survives untouched by `.unwrap_or(selected_count)` when a
//!   `result` IS present with a genuine zero. The oracle produces a zero
//!   failed-count twice (`T:108`, `T:189`) and asserts neither `failed_count`
//!   nor `outcome` in either case — this module's biggest silent-divergence
//!   risk. See `m4_zero_failed_count_keeps_success_outcome`.
//! - **M5**: the oracle's result-payload assertion (`T:144`,
//!   `toMatchObject`) is a SUBSET match, so all five `*_bucket` fields on
//!   [`NestedRepoImportResultTelemetry`] are entirely unpinned by it — a
//!   builder that emits none of them still passes. All five are emitted and
//!   pinned explicitly below (`m5_*`). (The two `toEqual` — full-equality —
//!   oracle assertions are the scan and action payloads, `T:56`, `T:79`.)
//! - **M6**: `outcome` (`O:213`) checks `acceptedCount === 0` FIRST, so
//!   `{imported: 0, alreadyKnown: 0, failed: 0}` is `failed`, never
//!   `success`. Reordering this check is invisible to the oracle — `'failed'`
//!   and `'success'` string literals are never asserted anywhere in it. See
//!   `m6_full_outcome_table`.
//! - **M7**: `all_selected` (`O:191`, `O:231`) is computed from
//!   `normalize_nested_repo_telemetry_count` (floored, NOT capped) values,
//!   BEFORE the cap is applied, and the `rawFoundCount > 0` guard makes
//!   `0/0` -> `false`. The oracle's `T:92-115` (600 vs 500) distinguishes raw
//!   from capped, but never distinguishes "normalized" from "passed through
//!   uncapped" — a comparison on the raw un-floored `f64` inputs diverges at
//!   e.g. `foundCount: 3.5, selectedCount: 3` (JS floors both to `3`, so
//!   `true`; a naive un-floored compare gives `false`). See `m7_*`.
//! - **M8**: six wire vocabularies (`O:9`, `:12`, `:15-20`, `:23-28`, `:31`,
//!   `:34`) have ZERO oracle contact for import-action's `open_as_folder`/
//!   `back`, scan-result's `git_repo`/`no_nested_repos`/`scan_failed`, and
//!   outcome's `success`/`failed` — the `.test.ts` file never even imports
//!   the raw constant arrays, only the builder functions, and every builder
//!   call site in the oracle happens to land on a different member. A port
//!   using `"partial-failure"` or `"openAsFolder"` would be entirely green
//!   against this oracle and entirely broken against the downstream
//!   `z.enum` schema (`telemetry-events.ts:890-896`). All literals are
//!   pinned explicitly via each enum's `as_str()` — see `m8_*`. Member
//!   ORDER is NOT load-bearing here (a `z.enum` is set membership, not a
//!   sequence) — preserved for readability only, unlike a previously ported
//!   module where order mattered.
//! - **M9**: [`NESTED_REPO_TELEMETRY_MAX_REPO_COUNT`] is pinned by a literal
//!   in a DIFFERENT oracle test (`T:111-112`, inside "computes all_selected
//!   from raw counts before caps are applied") than the one that looks like
//!   it pins it (`T:38`, `capNestedRepoTelemetryCount(999)).toBe(
//!   NESTED_REPO_TELEMETRY_MAX_REPO_COUNT)` — symbolic, admits any value
//!   `<= 999`). See `m9_max_is_five_hundred`. The cap is unreachable in
//!   production (upstream already clamps repo counts to 500,
//!   `nested-repo-discovery.ts:71-74`), so the oracle (and now this pin) is
//!   its only protection.
//! - **M10**: the conditional spread `...(args.scan ? { selected_path_kind:
//!   ... } : {})` (`O:162`) OMITS the key entirely when there is no scan —
//!   modeled as `Option<NestedRepoScanPathKind>` where `None` means the
//!   FIELD IS ABSENT on the wire, not present-as-null (relevant once/if a
//!   serde boundary is added — see the `Cargo.toml` charter comment). There
//!   is NO `scan: null` test in the oracle at all, so the whole
//!   `'scan_failed'` branch (`O:150`) is completely uncovered by it despite
//!   being a live production path
//!   (`useAddRepoNestedImportFlow.ts:211-219`). See `m10_*`.
//! - **M11**: all three conjuncts of `shouldEmitNestedRepoImportSubmitTelemetry`
//!   (`O:116`, `Boolean(args.attemptId && args.selectedCount > 0 &&
//!   !args.isBusy)`) are JS truthiness, not `Option::is_some()` /
//!   `> 0.0` alone: an empty-string `attemptId` is falsy (`Boolean('') ===
//!   false`), so `Some(String::new())` must yield `false`, same as `None`;
//!   `isBusy` is negated-truthy, so both `Some(false)` and `None` pass;
//!   `selectedCount` is the RAW (un-normalized) argument, so `0.5` is `true`
//!   and `NaN` is `false` (`NaN > 0` is always `false`). The oracle only
//!   exercises `is_busy: true` and `is_busy` absent, always with a
//!   non-empty id — see `m11_*`.
//! - **M12**: `createNestedRepoTelemetryAttemptId`'s oracle regex
//!   (`/^[0-9a-f-]{36}$/`, `T:163`) is far weaker than the downstream wire
//!   contract (`z.string().uuid()`, `telemetry-events.ts:899`) — it doesn't
//!   check dash position, group sizes, version, or variant at all (36 bare
//!   `-` characters would pass it). Under Node >= 19 only the
//!   `globalThis.crypto.randomUUID()` early return (`O:121-123`) ever
//!   executes in CI, so the JS fallback body is dead code there. This port
//!   has no `globalThis.crypto` equivalent, so ONLY the fallback's *shape*
//!   is reproduced: version nibble (`O:135`), variant nibble (`O:136`), and
//!   the 8-4-4-4-12 grouping (`O:138`), built on the `suaegi-setupseq`
//!   precedent (`std::collections::hash_map::RandomState` +
//!   `std::time::SystemTime` — never the forbidden `rand`/`uuid` crates).
//!   Pinned directly (not via the oracle's weak regex): length 36, dashes at
//!   indices 8/13/18/23, lowercase hex elsewhere, `s[14] == '4'`, `s[19] in
//!   {8,9,a,b}`. See `m12_*`.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// O:7 NESTED_REPO_TELEMETRY_MAX_REPO_COUNT
// ---------------------------------------------------------------------------

/// `O:7`. M9: pinned directly by `m9_max_is_five_hundred` — the oracle test
/// that *looks* like it pins this (`T:38`) is symbolic and admits any value
/// `<= 999`; the literal `500` actually appears in a different test
/// (`T:111-112`).
pub const NESTED_REPO_TELEMETRY_MAX_REPO_COUNT: u32 = 500;

// ---------------------------------------------------------------------------
// O:9 NestedRepoTelemetrySurface
// ---------------------------------------------------------------------------

/// `O:9`, `'onboarding' | 'sidebar'`. M8: both literals have oracle contact
/// (`T:52`, `T:73`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoTelemetrySurface {
    Onboarding,
    Sidebar,
}

impl NestedRepoTelemetrySurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::Sidebar => "sidebar",
        }
    }
}

// ---------------------------------------------------------------------------
// O:12 NestedRepoTelemetryRuntimeKind
// ---------------------------------------------------------------------------

/// `O:12`, `'local' | 'runtime' | 'ssh'`. M8: all three have oracle contact
/// (`T:53`, `T:74`, `T:138`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoTelemetryRuntimeKind {
    Local,
    Runtime,
    Ssh,
}

impl NestedRepoTelemetryRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Runtime => "runtime",
            Self::Ssh => "ssh",
        }
    }
}

// ---------------------------------------------------------------------------
// O:15-20 NestedRepoScanTelemetryResult
// ---------------------------------------------------------------------------

/// `O:15-20`, `'review_shown' | 'git_repo' | 'no_nested_repos' |
/// 'scan_failed'`. M8: ONLY `review_shown` has oracle contact (`T:60`); the
/// other three are entirely unexercised by the `.test.ts` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoScanTelemetryResult {
    ReviewShown,
    GitRepo,
    NoNestedRepos,
    ScanFailed,
}

impl NestedRepoScanTelemetryResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReviewShown => "review_shown",
            Self::GitRepo => "git_repo",
            Self::NoNestedRepos => "no_nested_repos",
            Self::ScanFailed => "scan_failed",
        }
    }
}

// ---------------------------------------------------------------------------
// O:23-28 NestedRepoImportTelemetryAction
// ---------------------------------------------------------------------------

/// `O:23-28`, `'import_group' | 'import_separate' | 'open_as_folder' |
/// 'back'`. M8: only `import_group`/`import_separate` have oracle contact
/// (`T:75`, `T:178`); `open_as_folder`/`back` are entirely unexercised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoImportTelemetryAction {
    ImportGroup,
    ImportSeparate,
    OpenAsFolder,
    Back,
}

impl NestedRepoImportTelemetryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImportGroup => "import_group",
            Self::ImportSeparate => "import_separate",
            Self::OpenAsFolder => "open_as_folder",
            Self::Back => "back",
        }
    }
}

// ---------------------------------------------------------------------------
// O:31 NestedRepoImportTelemetryOutcome
// ---------------------------------------------------------------------------

/// `O:31`, `'success' | 'partial_failure' | 'failed'`. M8: only
/// `partial_failure` has oracle contact (`T:149`); `success`/`failed` are
/// entirely unexercised. M6: `outcome` computation checks `accepted == 0`
/// first — see [`build_nested_repo_import_result_telemetry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoImportTelemetryOutcome {
    Success,
    PartialFailure,
    Failed,
}

impl NestedRepoImportTelemetryOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::PartialFailure => "partial_failure",
            Self::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// O:34 NestedRepoCountBucket
// ---------------------------------------------------------------------------

/// `O:34`, `'0' | '1' | '2-3' | '4-7' | '8-15' | '16+'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoCountBucket {
    Zero,
    One,
    TwoToThree,
    FourToSeven,
    EightToFifteen,
    SixteenPlus,
}

impl NestedRepoCountBucket {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "0",
            Self::One => "1",
            Self::TwoToThree => "2-3",
            Self::FourToSeven => "4-7",
            Self::EightToFifteen => "8-15",
            Self::SixteenPlus => "16+",
        }
    }
}

// ---------------------------------------------------------------------------
// types.ts (type-only import) — NestedRepoScanPathKind, ProjectGroupImportMode
// ---------------------------------------------------------------------------

/// `NestedRepoScanResult['selectedPathKind']`, `'git_repo' | 'non_git_folder'`
/// (from `types.ts`). Distinct from [`NestedRepoScanTelemetryResult::GitRepo`]
/// — same wire string, different vocabulary (this one is a SCAN outcome
/// classifying the user's selected path, not a telemetry `result` value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestedRepoScanPathKind {
    GitRepo,
    NonGitFolder,
}

impl NestedRepoScanPathKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GitRepo => "git_repo",
            Self::NonGitFolder => "non_git_folder",
        }
    }
}

/// `ProjectGroupImportMode` (from `types.ts`), `'group' | 'separate'`.
/// Type-only import — modeled here in full since it's a two-member closed
/// union with no other fields to narrow away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectGroupImportMode {
    Group,
    Separate,
}

impl ProjectGroupImportMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Separate => "separate",
        }
    }
}

// ---------------------------------------------------------------------------
// Narrow input models (only the fields this module reads)
// ---------------------------------------------------------------------------

/// The narrow slice of `NestedRepoScanResult` this module reads: `repos`
/// (as its `.length`, `O:147`), `selectedPathKind` (`O:151`, `O:162`),
/// `truncated` (`O:165`), `timedOut` (`O:166`). M3: `repo_count` is `f64` at
/// this boundary (it feeds directly into
/// [`cap_nested_repo_telemetry_count`]), even though a real array length is
/// always a non-negative integer in practice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NestedRepoScanInput {
    pub repo_count: f64,
    pub selected_path_kind: NestedRepoScanPathKind,
    pub truncated: bool,
    pub timed_out: bool,
}

/// The narrow slice of `ProjectGroupImportResult` this module reads:
/// `importedCount`, `alreadyKnownCount`, `failedCount` (`O:208-210`). M3:
/// all three are `f64` at this boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NestedRepoImportResultCounts {
    pub imported_count: f64,
    pub already_known_count: f64,
    pub failed_count: f64,
}

// ---------------------------------------------------------------------------
// O:37-50 NestedRepoScanTelemetry (+ shared base fields inlined per struct)
// ---------------------------------------------------------------------------

/// `O:43-50`. M10: `selected_path_kind` is `None` when the source key is
/// ABSENT on the wire (no scan), not present-as-null.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedRepoScanTelemetry {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub result: NestedRepoScanTelemetryResult,
    pub selected_path_kind: Option<NestedRepoScanPathKind>,
    pub found_count: u32,
    pub found_count_bucket: NestedRepoCountBucket,
    pub truncated: bool,
    pub timed_out: bool,
}

/// Args for [`build_nested_repo_scan_telemetry`], mirroring the TS `args`
/// object parameter (`O:141-146`). `scan: None` mirrors `scan: null`.
pub struct BuildNestedRepoScanTelemetryArgs {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub scan: Option<NestedRepoScanInput>,
}

/// `buildNestedRepoScanTelemetry` (`O:141-168`). M10: the `scan_failed`
/// branch and the omitted `selected_path_kind` key both fire when `scan` is
/// `None` — completely uncovered by the oracle (see module docs).
pub fn build_nested_repo_scan_telemetry(
    args: BuildNestedRepoScanTelemetryArgs,
) -> NestedRepoScanTelemetry {
    let found_count = cap_nested_repo_telemetry_count(
        args.scan.map(|scan| scan.repo_count).unwrap_or(0.0),
    );

    let result = match args.scan {
        None => NestedRepoScanTelemetryResult::ScanFailed,
        Some(scan) if scan.selected_path_kind == NestedRepoScanPathKind::GitRepo => {
            NestedRepoScanTelemetryResult::GitRepo
        }
        Some(_) if found_count > 0 => NestedRepoScanTelemetryResult::ReviewShown,
        Some(_) => NestedRepoScanTelemetryResult::NoNestedRepos,
    };

    NestedRepoScanTelemetry {
        attempt_id: args.attempt_id,
        surface: args.surface,
        runtime_kind: args.runtime_kind,
        result,
        // M10: `None` here means the FIELD IS ABSENT, mirroring the TS
        // conditional spread `...(args.scan ? { selected_path_kind: ... } :
        // {})` (`O:162`) — never a present-as-null value.
        selected_path_kind: args.scan.map(|scan| scan.selected_path_kind),
        found_count,
        found_count_bucket: bucket_nested_repo_telemetry_count(f64::from(found_count)),
        truncated: args.scan.map(|scan| scan.truncated).unwrap_or(false),
        timed_out: args.scan.map(|scan| scan.timed_out).unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// O:52-59 NestedRepoImportActionTelemetry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct NestedRepoImportActionTelemetry {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub action: NestedRepoImportTelemetryAction,
    pub found_count: u32,
    pub found_count_bucket: NestedRepoCountBucket,
    pub selected_count: u32,
    pub selected_count_bucket: NestedRepoCountBucket,
    pub all_selected: bool,
}

/// Args for [`build_nested_repo_import_action_telemetry`], mirroring the TS
/// `args` object parameter (`O:170-176`).
pub struct BuildNestedRepoImportActionTelemetryArgs {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub action: NestedRepoImportTelemetryAction,
    pub found_count: f64,
    pub selected_count: f64,
}

/// `buildNestedRepoImportActionTelemetry` (`O:170-193`). M7: `all_selected`
/// is computed from the NORMALIZED (floored, uncapped) raw counts, before
/// the cap is applied to the emitted `found_count`/`selected_count` fields.
pub fn build_nested_repo_import_action_telemetry(
    args: BuildNestedRepoImportActionTelemetryArgs,
) -> NestedRepoImportActionTelemetry {
    let raw_found_count = normalize_nested_repo_telemetry_count(args.found_count);
    let raw_selected_count = normalize_nested_repo_telemetry_count(args.selected_count);
    let found_count = cap_nested_repo_telemetry_count(args.found_count);
    let selected_count = cap_nested_repo_telemetry_count(args.selected_count);

    NestedRepoImportActionTelemetry {
        attempt_id: args.attempt_id,
        surface: args.surface,
        runtime_kind: args.runtime_kind,
        action: args.action,
        found_count,
        found_count_bucket: bucket_nested_repo_telemetry_count(f64::from(found_count)),
        selected_count,
        selected_count_bucket: bucket_nested_repo_telemetry_count(f64::from(selected_count)),
        // M7: `rawFoundCount > 0` guard makes 0/0 -> `false`.
        all_selected: raw_found_count > 0.0 && raw_selected_count == raw_found_count,
    }
}

// ---------------------------------------------------------------------------
// O:61-75 NestedRepoImportResultTelemetry
// ---------------------------------------------------------------------------

/// `O:61-75`. M5: all five `*_bucket` fields are unpinned by the oracle
/// (subset match) — pinned explicitly in this crate's tests instead.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedRepoImportResultTelemetry {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub mode: ProjectGroupImportMode,
    pub outcome: NestedRepoImportTelemetryOutcome,
    pub found_count: u32,
    pub found_count_bucket: NestedRepoCountBucket,
    pub selected_count: u32,
    pub selected_count_bucket: NestedRepoCountBucket,
    pub imported_count: u32,
    pub imported_count_bucket: NestedRepoCountBucket,
    pub already_known_count: u32,
    pub already_known_count_bucket: NestedRepoCountBucket,
    pub failed_count: u32,
    pub failed_count_bucket: NestedRepoCountBucket,
    pub all_selected: bool,
}

/// Args for [`build_nested_repo_import_result_telemetry`], mirroring the TS
/// `args` object parameter (`O:195-203`). `result: None` mirrors
/// `result: null`.
pub struct BuildNestedRepoImportResultTelemetryArgs {
    pub attempt_id: String,
    pub surface: NestedRepoTelemetrySurface,
    pub runtime_kind: NestedRepoTelemetryRuntimeKind,
    pub mode: ProjectGroupImportMode,
    pub found_count: f64,
    pub selected_count: f64,
    pub result: Option<NestedRepoImportResultCounts>,
}

/// `buildNestedRepoImportResultTelemetry` (`O:195-233`).
///
/// M4: `failed_count`'s fallback is derived via `Option<f64>` +
/// `.unwrap_or(selected_count)` — `None` occurs ONLY when `args.result` is
/// absent (mirrors `args.result?.failedCount ?? selectedCount`, `O:210`);
/// when `result` IS present, its `failed_count` field (even `0.0`) is used
/// as-is, never re-substituted. M6: `outcome` checks `accepted_count == 0`
/// FIRST. M7: `all_selected` uses pre-cap normalized values, guarded against
/// `0/0`.
pub fn build_nested_repo_import_result_telemetry(
    args: BuildNestedRepoImportResultTelemetryArgs,
) -> NestedRepoImportResultTelemetry {
    let raw_found_count = normalize_nested_repo_telemetry_count(args.found_count);
    let raw_selected_count = normalize_nested_repo_telemetry_count(args.selected_count);
    let found_count = cap_nested_repo_telemetry_count(args.found_count);
    let selected_count = cap_nested_repo_telemetry_count(args.selected_count);

    let imported_count = cap_nested_repo_telemetry_count(
        args.result.map(|result| result.imported_count).unwrap_or(0.0),
    );
    let already_known_count = cap_nested_repo_telemetry_count(
        args.result
            .map(|result| result.already_known_count)
            .unwrap_or(0.0),
    );
    // M4: nullish, not falsy — `Some(0.0)` (a present result with a genuine
    // zero failed count) must NOT fall back to `selected_count`.
    let failed_count_input: Option<f64> = args.result.map(|result| result.failed_count);
    let failed_count =
        cap_nested_repo_telemetry_count(failed_count_input.unwrap_or(f64::from(selected_count)));

    // M6: `accepted_count == 0` is checked FIRST — an all-zero result is
    // `failed`, never `success`, regardless of `failed_count`.
    let accepted_count = imported_count + already_known_count;
    let outcome = if accepted_count == 0 {
        NestedRepoImportTelemetryOutcome::Failed
    } else if failed_count > 0 {
        NestedRepoImportTelemetryOutcome::PartialFailure
    } else {
        NestedRepoImportTelemetryOutcome::Success
    };

    NestedRepoImportResultTelemetry {
        attempt_id: args.attempt_id,
        surface: args.surface,
        runtime_kind: args.runtime_kind,
        mode: args.mode,
        outcome,
        found_count,
        found_count_bucket: bucket_nested_repo_telemetry_count(f64::from(found_count)),
        selected_count,
        selected_count_bucket: bucket_nested_repo_telemetry_count(f64::from(selected_count)),
        imported_count,
        imported_count_bucket: bucket_nested_repo_telemetry_count(f64::from(imported_count)),
        already_known_count,
        already_known_count_bucket: bucket_nested_repo_telemetry_count(f64::from(
            already_known_count,
        )),
        failed_count,
        failed_count_bucket: bucket_nested_repo_telemetry_count(f64::from(failed_count)),
        // M7: pre-cap normalized values; `0/0` -> `false` via the guard.
        all_selected: raw_found_count > 0.0 && raw_selected_count == raw_found_count,
    }
}

// ---------------------------------------------------------------------------
// O:77-82 capNestedRepoTelemetryCount
// ---------------------------------------------------------------------------

/// `capNestedRepoTelemetryCount` (`O:77-82`). M1: `!count.is_finite()` (NaN
/// or either infinity) returns `0`, NOT [`NESTED_REPO_TELEMETRY_MAX_REPO_COUNT`].
/// Otherwise: floor, then clamp into `[0, MAX]`.
pub fn cap_nested_repo_telemetry_count(count: f64) -> u32 {
    if !count.is_finite() {
        return 0;
    }
    let floored = count.floor();
    let clamped = floored.max(0.0).min(f64::from(NESTED_REPO_TELEMETRY_MAX_REPO_COUNT));
    // Safety: `clamped` is finite and lies in `[0.0, MAX as f64]`, so the
    // cast is exact and in-range for `u32`.
    clamped as u32
}

// ---------------------------------------------------------------------------
// O:84-89 normalizeNestedRepoTelemetryCount (private upstream, private here)
// ---------------------------------------------------------------------------

/// `normalizeNestedRepoTelemetryCount` (`O:84-89`). NOT exported in the TS
/// source (no `export` keyword) — kept private here too. M1: same
/// finiteness gate as [`cap_nested_repo_telemetry_count`], but no upper
/// bound — only `max(0, floor(count))`. Feeds M7's `all_selected`.
fn normalize_nested_repo_telemetry_count(count: f64) -> f64 {
    if !count.is_finite() {
        return 0.0;
    }
    count.floor().max(0.0)
}

// ---------------------------------------------------------------------------
// O:91-109 bucketNestedRepoTelemetryCount
// ---------------------------------------------------------------------------

/// `bucketNestedRepoTelemetryCount` (`O:91-109`). M1: caps BEFORE bucketing
/// (`O:92`). M2: every rung below is `<=`, so boundary values fall into the
/// LOWER bucket (e.g. capped `3` -> `"2-3"`, capped `4` -> `"4-7"`).
pub fn bucket_nested_repo_telemetry_count(count: f64) -> NestedRepoCountBucket {
    let capped = cap_nested_repo_telemetry_count(count);
    match capped {
        0 => NestedRepoCountBucket::Zero,
        1 => NestedRepoCountBucket::One,
        2..=3 => NestedRepoCountBucket::TwoToThree,
        4..=7 => NestedRepoCountBucket::FourToSeven,
        8..=15 => NestedRepoCountBucket::EightToFifteen,
        _ => NestedRepoCountBucket::SixteenPlus,
    }
}

// ---------------------------------------------------------------------------
// O:111-117 shouldEmitNestedRepoImportSubmitTelemetry
// ---------------------------------------------------------------------------

/// Args for [`should_emit_nested_repo_import_submit_telemetry`], mirroring
/// the TS `args` object parameter (`O:111-115`). `attempt_id: None` mirrors
/// `attemptId: null`; `Some(String::new())` mirrors `attemptId: ''`
/// (JS-falsy, distinct from `None`, but both must yield `false` here).
pub struct ShouldEmitNestedRepoImportSubmitTelemetryArgs {
    pub attempt_id: Option<String>,
    pub selected_count: f64,
    pub is_busy: Option<bool>,
}

/// `shouldEmitNestedRepoImportSubmitTelemetry` (`O:111-117`,
/// `Boolean(args.attemptId && args.selectedCount > 0 && !args.isBusy)`).
/// M11: all three conjuncts are JS truthiness: an absent OR empty-string
/// `attempt_id` is falsy; `is_busy` is negated-truthy (`None` and
/// `Some(false)` both pass); `selected_count` is compared RAW (not
/// normalized), so `0.5` is `true` and `NaN` is `false` (`NaN > 0.0` is
/// always `false` in both languages).
pub fn should_emit_nested_repo_import_submit_telemetry(
    args: &ShouldEmitNestedRepoImportSubmitTelemetryArgs,
) -> bool {
    let attempt_id_truthy = args.attempt_id.as_deref().is_some_and(|id| !id.is_empty());
    attempt_id_truthy && args.selected_count > 0.0 && !args.is_busy.unwrap_or(false)
}

// ---------------------------------------------------------------------------
// O:119-139 createNestedRepoTelemetryAttemptId
// ---------------------------------------------------------------------------

/// `createNestedRepoTelemetryAttemptId` (`O:119-139`). M12: the
/// `globalThis.crypto.randomUUID()` preference branch (`O:121-123`) has NO
/// Rust std counterpart — this function reproduces only the JS *fallback*
/// body's shape (version nibble `O:135`, variant nibble `O:136`, 8-4-4-4-12
/// grouping `O:138`), sourced from `std::collections::hash_map::RandomState`
/// combined with `std::time::SystemTime` (never `rand`/`uuid`), per the
/// `suaegi-setupseq` precedent.
pub fn create_nested_repo_telemetry_attempt_id() -> String {
    let mut bytes = random_16_bytes();
    // O:135: version nibble -> `4` (UUID v4).
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    // O:136: variant nibble -> RFC 4122 (`10xx`).
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format_uuid_bytes(&bytes)
}

/// 16 pseudo-random bytes. A monotonic per-process call counter combined
/// with the current time guarantees two calls in the same process never
/// collide, independent of clock resolution; `RandomState`'s per-instance
/// seed adds non-determinism on top (never asserted exactly by any test —
/// only shape/uniqueness is pinned, matching the setupseq nonce precedent).
fn random_16_bytes() -> [u8; 16] {
    static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);
    let call_id = CALL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u64(call_id);
        hasher.write_usize(index);
        *byte = (hasher.finish() & 0xff) as u8;
    }
    bytes
}

/// O:138: `8-4-4-4-12` hex grouping joined by `-`.
fn format_uuid_bytes(bytes: &[u8; 16]) -> String {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Oracle — T:34-46 (cap + bucket table, upper edges only)
    // =========================================================================

    #[test]
    fn oracle_caps_and_buckets_repo_counts_for_low_cardinality_breakdowns() {
        assert_eq!(cap_nested_repo_telemetry_count(-1.0), 0);
        assert_eq!(cap_nested_repo_telemetry_count(2.9), 2);
        assert_eq!(cap_nested_repo_telemetry_count(f64::NAN), 0);
        assert_eq!(
            cap_nested_repo_telemetry_count(999.0),
            NESTED_REPO_TELEMETRY_MAX_REPO_COUNT
        );

        assert_eq!(bucket_nested_repo_telemetry_count(0.0).as_str(), "0");
        assert_eq!(bucket_nested_repo_telemetry_count(1.0).as_str(), "1");
        assert_eq!(bucket_nested_repo_telemetry_count(3.0).as_str(), "2-3");
        assert_eq!(bucket_nested_repo_telemetry_count(7.0).as_str(), "4-7");
        assert_eq!(bucket_nested_repo_telemetry_count(15.0).as_str(), "8-15");
        assert_eq!(bucket_nested_repo_telemetry_count(16.0).as_str(), "16+");
    }

    // =========================================================================
    // Oracle — T:48-67 (scan telemetry, full equality)
    // =========================================================================

    #[test]
    fn oracle_classifies_a_scan_that_should_show_nested_repo_review() {
        let telemetry = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
            surface: NestedRepoTelemetrySurface::Onboarding,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(NestedRepoScanInput {
                repo_count: 3.0,
                selected_path_kind: NestedRepoScanPathKind::NonGitFolder,
                truncated: false,
                timed_out: false,
            }),
        });

        assert_eq!(telemetry.attempt_id, "2fbac1e3-5094-45b4-80a6-90281e6e9e09");
        assert_eq!(telemetry.surface, NestedRepoTelemetrySurface::Onboarding);
        assert_eq!(telemetry.runtime_kind, NestedRepoTelemetryRuntimeKind::Local);
        assert_eq!(telemetry.result, NestedRepoScanTelemetryResult::ReviewShown);
        assert_eq!(
            telemetry.selected_path_kind,
            Some(NestedRepoScanPathKind::NonGitFolder)
        );
        assert_eq!(telemetry.found_count, 3);
        assert_eq!(telemetry.found_count_bucket, NestedRepoCountBucket::TwoToThree);
        assert!(!telemetry.truncated);
        assert!(!telemetry.timed_out);
    }

    // =========================================================================
    // Oracle — T:69-90 (import action telemetry, full equality)
    // =========================================================================

    #[test]
    fn oracle_records_import_action_selection_without_raw_path_details() {
        let telemetry =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Ssh,
                action: NestedRepoImportTelemetryAction::ImportGroup,
                found_count: 3.0,
                selected_count: 2.0,
            });

        assert_eq!(telemetry.attempt_id, "2fbac1e3-5094-45b4-80a6-90281e6e9e09");
        assert_eq!(telemetry.surface, NestedRepoTelemetrySurface::Sidebar);
        assert_eq!(telemetry.runtime_kind, NestedRepoTelemetryRuntimeKind::Ssh);
        assert_eq!(telemetry.action, NestedRepoImportTelemetryAction::ImportGroup);
        assert_eq!(telemetry.found_count, 3);
        assert_eq!(telemetry.found_count_bucket, NestedRepoCountBucket::TwoToThree);
        assert_eq!(telemetry.selected_count, 2);
        assert_eq!(telemetry.selected_count_bucket, NestedRepoCountBucket::TwoToThree);
        assert!(!telemetry.all_selected);
    }

    // =========================================================================
    // Oracle — T:92-115 (all_selected from raw counts, before caps)
    // =========================================================================

    #[test]
    fn oracle_computes_all_selected_from_raw_counts_before_caps_are_applied() {
        let action =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::ImportGroup,
                found_count: 600.0,
                selected_count: 500.0,
            });
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Group,
                found_count: 600.0,
                selected_count: 500.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 500.0,
                    already_known_count: 0.0,
                    failed_count: 0.0,
                }),
            });

        assert_eq!(action.found_count, 500);
        assert_eq!(action.selected_count, 500);
        assert!(!action.all_selected);
        assert!(!result.all_selected);
    }

    // =========================================================================
    // Oracle — T:117-157 (import result telemetry, subset match — M5's gap)
    // =========================================================================

    #[test]
    fn oracle_keeps_exact_imported_counts_on_import_result_payloads() {
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Onboarding,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Runtime,
                mode: ProjectGroupImportMode::Group,
                found_count: 4.0,
                selected_count: 4.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 2.0,
                    already_known_count: 1.0,
                    failed_count: 1.0,
                }),
            });

        assert_eq!(result.attempt_id, "2fbac1e3-5094-45b4-80a6-90281e6e9e09");
        assert_eq!(result.surface, NestedRepoTelemetrySurface::Onboarding);
        assert_eq!(result.runtime_kind, NestedRepoTelemetryRuntimeKind::Runtime);
        assert_eq!(result.mode, ProjectGroupImportMode::Group);
        assert_eq!(result.outcome, NestedRepoImportTelemetryOutcome::PartialFailure);
        assert_eq!(result.found_count, 4);
        assert_eq!(result.selected_count, 4);
        assert_eq!(result.imported_count, 2);
        assert_eq!(result.already_known_count, 1);
        assert_eq!(result.failed_count, 1);
        assert!(result.all_selected);
    }

    // =========================================================================
    // Oracle — T:159-165 (attempt id shape + uniqueness)
    // =========================================================================

    #[test]
    fn oracle_generates_non_persistent_random_attempt_ids() {
        let first = create_nested_repo_telemetry_attempt_id();
        let second = create_nested_repo_telemetry_attempt_id();

        assert_eq!(first.len(), 36);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert_ne!(second, first);
    }

    // =========================================================================
    // Oracle — T:167-201 (one attempt id threaded across scan/action/result)
    // =========================================================================

    #[test]
    fn oracle_threads_one_attempt_id_across_scan_action_and_result_and_allows_a_new_scan_id() {
        let scan_input = NestedRepoScanInput {
            repo_count: 3.0,
            selected_path_kind: NestedRepoScanPathKind::NonGitFolder,
            truncated: false,
            timed_out: false,
        };

        let scan = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
            surface: NestedRepoTelemetrySurface::Sidebar,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(scan_input),
        });
        let action =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::ImportSeparate,
                found_count: 3.0,
                selected_count: 3.0,
            });
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Separate,
                found_count: 3.0,
                selected_count: 3.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 3.0,
                    already_known_count: 0.0,
                    failed_count: 0.0,
                }),
            });
        let next_scan = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "d22bb9e0-b7f8-480a-8a2a-9b34f84f2c42".to_string(),
            surface: NestedRepoTelemetrySurface::Sidebar,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(scan_input),
        });

        assert_eq!(action.attempt_id, scan.attempt_id);
        assert_eq!(result.attempt_id, scan.attempt_id);
        assert_ne!(next_scan.attempt_id, scan.attempt_id);
    }

    // =========================================================================
    // Oracle — T:203-223 (shouldEmit: zero selection, busy, plain accept)
    // =========================================================================

    #[test]
    fn oracle_prevents_zero_selection_submit_telemetry() {
        assert!(!should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string()),
                selected_count: 0.0,
                is_busy: None,
            }
        ));
        assert!(!should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string()),
                selected_count: 1.0,
                is_busy: Some(true),
            }
        ));
        assert!(should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("2fbac1e3-5094-45b4-80a6-90281e6e9e09".to_string()),
                selected_count: 1.0,
                is_busy: None,
            }
        ));
    }

    // =========================================================================
    // M1 — Infinity/-Infinity/NaN/negative/fractional all bucket to "0"
    // (proves cap-runs-before-bucket; the oracle never exercises any of
    // these on `bucket()`).
    // =========================================================================

    #[test]
    fn m1_infinity_caps_to_zero_not_the_bucket_ceiling() {
        assert_eq!(cap_nested_repo_telemetry_count(f64::INFINITY), 0);
        assert_eq!(
            bucket_nested_repo_telemetry_count(f64::INFINITY).as_str(),
            "0"
        );
    }

    #[test]
    fn m1_negative_infinity_caps_to_zero() {
        assert_eq!(cap_nested_repo_telemetry_count(f64::NEG_INFINITY), 0);
        assert_eq!(
            bucket_nested_repo_telemetry_count(f64::NEG_INFINITY).as_str(),
            "0"
        );
    }

    #[test]
    fn m1_nan_caps_to_zero() {
        assert_eq!(cap_nested_repo_telemetry_count(f64::NAN), 0);
        assert_eq!(bucket_nested_repo_telemetry_count(f64::NAN).as_str(), "0");
    }

    #[test]
    fn m1_negative_finite_value_buckets_to_zero() {
        assert_eq!(bucket_nested_repo_telemetry_count(-5.0).as_str(), "0");
    }

    #[test]
    fn m1_negative_fractional_value_buckets_to_zero() {
        assert_eq!(bucket_nested_repo_telemetry_count(-0.5).as_str(), "0");
    }

    // =========================================================================
    // M2 — full 0..=16 bucket table, plus 500 and 501 (lower edges + max
    // saturation, none of which the oracle asserts).
    // =========================================================================

    #[test]
    fn m2_full_bucket_table_zero_through_sixteen() {
        let expected: &[(u32, &str)] = &[
            (0, "0"),
            (1, "1"),
            (2, "2-3"),
            (3, "2-3"),
            (4, "4-7"),
            (5, "4-7"),
            (6, "4-7"),
            (7, "4-7"),
            (8, "8-15"),
            (9, "8-15"),
            (10, "8-15"),
            (11, "8-15"),
            (12, "8-15"),
            (13, "8-15"),
            (14, "8-15"),
            (15, "8-15"),
            (16, "16+"),
        ];
        for &(count, bucket) in expected {
            assert_eq!(
                bucket_nested_repo_telemetry_count(f64::from(count)).as_str(),
                bucket,
                "count {count} expected bucket {bucket}"
            );
        }
    }

    #[test]
    fn m2_five_hundred_buckets_to_sixteen_plus() {
        assert_eq!(bucket_nested_repo_telemetry_count(500.0).as_str(), "16+");
    }

    #[test]
    fn m2_five_hundred_one_saturates_to_max_and_still_buckets_sixteen_plus() {
        assert_eq!(cap_nested_repo_telemetry_count(501.0), 500);
        assert_eq!(bucket_nested_repo_telemetry_count(501.0).as_str(), "16+");
    }

    // =========================================================================
    // M4 — a present result with a genuine zero failed count keeps
    // `outcome == success` (kills the `??` -> `||` mutation).
    // =========================================================================

    #[test]
    fn m4_zero_failed_count_keeps_success_outcome() {
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Onboarding,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Group,
                found_count: 3.0,
                selected_count: 3.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 2.0,
                    already_known_count: 1.0,
                    failed_count: 0.0,
                }),
            });

        assert_eq!(result.failed_count, 0);
        assert_eq!(result.failed_count_bucket, NestedRepoCountBucket::Zero);
        assert_eq!(result.outcome, NestedRepoImportTelemetryOutcome::Success);
    }

    #[test]
    fn m4_absent_result_falls_back_failed_count_to_selected_count() {
        // `result: None` is the ONLY path where the `?? selectedCount`
        // fallback fires — distinct from "a present result with failedCount
        // 0", which must NOT fall back (see the test above).
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Onboarding,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Group,
                found_count: 3.0,
                selected_count: 3.0,
                result: None,
            });

        assert_eq!(result.imported_count, 0);
        assert_eq!(result.already_known_count, 0);
        assert_eq!(result.failed_count, 3);
        assert_eq!(result.outcome, NestedRepoImportTelemetryOutcome::Failed);
    }

    // =========================================================================
    // M5 — all five `*_bucket` fields on the result payload, pinned
    // individually (the oracle's `toMatchObject` never checks any of them).
    // =========================================================================

    #[test]
    fn m5_all_five_bucket_fields_on_the_result_payload() {
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Onboarding,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Runtime,
                mode: ProjectGroupImportMode::Group,
                found_count: 16.0,
                selected_count: 8.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 4.0,
                    already_known_count: 1.0,
                    failed_count: 3.0,
                }),
            });

        assert_eq!(result.found_count_bucket, NestedRepoCountBucket::SixteenPlus);
        assert_eq!(result.selected_count_bucket, NestedRepoCountBucket::EightToFifteen);
        assert_eq!(result.imported_count_bucket, NestedRepoCountBucket::FourToSeven);
        assert_eq!(result.already_known_count_bucket, NestedRepoCountBucket::One);
        assert_eq!(result.failed_count_bucket, NestedRepoCountBucket::TwoToThree);
    }

    // =========================================================================
    // M6 — full outcome table: accepted == 0 wins first; `failed` and
    // `success` are asserted directly (the oracle never asserts either).
    // =========================================================================

    #[test]
    fn m6_full_outcome_table() {
        struct Case {
            imported: f64,
            already_known: f64,
            failed: f64,
            expected: NestedRepoImportTelemetryOutcome,
        }
        let cases = [
            Case {
                imported: 0.0,
                already_known: 0.0,
                failed: 0.0,
                expected: NestedRepoImportTelemetryOutcome::Failed,
            },
            Case {
                imported: 0.0,
                already_known: 0.0,
                failed: 5.0,
                expected: NestedRepoImportTelemetryOutcome::Failed,
            },
            Case {
                imported: 5.0,
                already_known: 0.0,
                failed: 0.0,
                expected: NestedRepoImportTelemetryOutcome::Success,
            },
            Case {
                imported: 0.0,
                already_known: 5.0,
                failed: 0.0,
                expected: NestedRepoImportTelemetryOutcome::Success,
            },
            Case {
                imported: 5.0,
                already_known: 5.0,
                failed: 0.0,
                expected: NestedRepoImportTelemetryOutcome::Success,
            },
            Case {
                imported: 5.0,
                already_known: 0.0,
                failed: 5.0,
                expected: NestedRepoImportTelemetryOutcome::PartialFailure,
            },
            Case {
                imported: 0.0,
                already_known: 5.0,
                failed: 5.0,
                expected: NestedRepoImportTelemetryOutcome::PartialFailure,
            },
            Case {
                imported: 5.0,
                already_known: 5.0,
                failed: 5.0,
                expected: NestedRepoImportTelemetryOutcome::PartialFailure,
            },
        ];

        for case in cases {
            let result = build_nested_repo_import_result_telemetry(
                BuildNestedRepoImportResultTelemetryArgs {
                    attempt_id: "id".to_string(),
                    surface: NestedRepoTelemetrySurface::Onboarding,
                    runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                    mode: ProjectGroupImportMode::Group,
                    found_count: 10.0,
                    selected_count: 10.0,
                    result: Some(NestedRepoImportResultCounts {
                        imported_count: case.imported,
                        already_known_count: case.already_known,
                        failed_count: case.failed,
                    }),
                },
            );
            assert_eq!(
                result.outcome, case.expected,
                "imported={} already_known={} failed={} expected {:?}",
                case.imported, case.already_known, case.failed, case.expected
            );
        }

        // Direct literal assertions — the oracle never asserts either string.
        assert_eq!(NestedRepoImportTelemetryOutcome::Failed.as_str(), "failed");
        assert_eq!(NestedRepoImportTelemetryOutcome::Success.as_str(), "success");
    }

    // =========================================================================
    // M7 — 3.5 vs 3 yields all_selected == true (both floor to 3); 0/0 is
    // false via the `rawFoundCount > 0` guard.
    // =========================================================================

    #[test]
    fn m7_fractional_found_and_integral_selected_normalize_to_equal_and_all_selected() {
        let action =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::ImportGroup,
                found_count: 3.5,
                selected_count: 3.0,
            });
        assert!(action.all_selected);

        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Group,
                found_count: 3.5,
                selected_count: 3.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 3.0,
                    already_known_count: 0.0,
                    failed_count: 0.0,
                }),
            });
        assert!(result.all_selected);
    }

    #[test]
    fn m7_zero_found_and_zero_selected_is_not_all_selected() {
        let action =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::ImportGroup,
                found_count: 0.0,
                selected_count: 0.0,
            });
        assert!(!action.all_selected);
    }

    // =========================================================================
    // M8 — all wire vocabulary literals, pinned explicitly (order is NOT
    // load-bearing; only set membership matters downstream).
    // =========================================================================

    #[test]
    fn m8_surface_literals() {
        assert_eq!(NestedRepoTelemetrySurface::Onboarding.as_str(), "onboarding");
        assert_eq!(NestedRepoTelemetrySurface::Sidebar.as_str(), "sidebar");
    }

    #[test]
    fn m8_runtime_kind_literals() {
        assert_eq!(NestedRepoTelemetryRuntimeKind::Local.as_str(), "local");
        assert_eq!(NestedRepoTelemetryRuntimeKind::Runtime.as_str(), "runtime");
        assert_eq!(NestedRepoTelemetryRuntimeKind::Ssh.as_str(), "ssh");
    }

    #[test]
    fn m8_scan_result_literals() {
        assert_eq!(NestedRepoScanTelemetryResult::ReviewShown.as_str(), "review_shown");
        assert_eq!(NestedRepoScanTelemetryResult::GitRepo.as_str(), "git_repo");
        assert_eq!(
            NestedRepoScanTelemetryResult::NoNestedRepos.as_str(),
            "no_nested_repos"
        );
        assert_eq!(NestedRepoScanTelemetryResult::ScanFailed.as_str(), "scan_failed");
    }

    #[test]
    fn m8_import_action_literals() {
        assert_eq!(NestedRepoImportTelemetryAction::ImportGroup.as_str(), "import_group");
        assert_eq!(
            NestedRepoImportTelemetryAction::ImportSeparate.as_str(),
            "import_separate"
        );
        assert_eq!(
            NestedRepoImportTelemetryAction::OpenAsFolder.as_str(),
            "open_as_folder"
        );
        assert_eq!(NestedRepoImportTelemetryAction::Back.as_str(), "back");
    }

    #[test]
    fn m8_import_outcome_literals() {
        assert_eq!(NestedRepoImportTelemetryOutcome::Success.as_str(), "success");
        assert_eq!(
            NestedRepoImportTelemetryOutcome::PartialFailure.as_str(),
            "partial_failure"
        );
        assert_eq!(NestedRepoImportTelemetryOutcome::Failed.as_str(), "failed");
    }

    #[test]
    fn m8_scan_path_kind_literals() {
        assert_eq!(NestedRepoScanPathKind::GitRepo.as_str(), "git_repo");
        assert_eq!(NestedRepoScanPathKind::NonGitFolder.as_str(), "non_git_folder");
    }

    #[test]
    fn m8_import_mode_literals() {
        assert_eq!(ProjectGroupImportMode::Group.as_str(), "group");
        assert_eq!(ProjectGroupImportMode::Separate.as_str(), "separate");
    }

    // =========================================================================
    // M9 — direct literal pin (the oracle test that looks like it pins this
    // is symbolic and admits any value <= 999; the literal 500 lives in a
    // different test, T:111-112).
    // =========================================================================

    #[test]
    fn m9_max_is_five_hundred() {
        assert_eq!(NESTED_REPO_TELEMETRY_MAX_REPO_COUNT, 500);
    }

    // =========================================================================
    // M10 — no scan: scan_failed result, selected_path_kind key ABSENT,
    // truncated/timed_out default true-when-present-only-if-scan-present
    // (here: default false since there's no scan at all is already covered
    // above; this pin exercises a scan carrying truncated/timed_out true).
    // =========================================================================

    #[test]
    fn m10_no_scan_yields_scan_failed_and_omits_selected_path_kind() {
        let telemetry = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "id".to_string(),
            surface: NestedRepoTelemetrySurface::Onboarding,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: None,
        });

        assert_eq!(telemetry.result, NestedRepoScanTelemetryResult::ScanFailed);
        assert_eq!(
            telemetry.selected_path_kind, None,
            "selected_path_kind key must be ABSENT, not present-as-null"
        );
        assert_eq!(telemetry.found_count, 0);
        assert_eq!(telemetry.found_count_bucket, NestedRepoCountBucket::Zero);
        assert!(!telemetry.truncated);
        assert!(!telemetry.timed_out);
    }

    #[test]
    fn m10_scan_present_with_truncated_and_timed_out_true() {
        let telemetry = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "id".to_string(),
            surface: NestedRepoTelemetrySurface::Onboarding,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(NestedRepoScanInput {
                repo_count: 2.0,
                selected_path_kind: NestedRepoScanPathKind::NonGitFolder,
                truncated: true,
                timed_out: true,
            }),
        });

        assert!(telemetry.truncated);
        assert!(telemetry.timed_out);
    }

    // =========================================================================
    // M11 — all three truthiness conjuncts, exercised individually.
    // =========================================================================

    #[test]
    fn m11_empty_string_attempt_id_is_falsy() {
        assert!(!should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some(String::new()),
                selected_count: 1.0,
                is_busy: None,
            }
        ));
    }

    #[test]
    fn m11_absent_attempt_id_is_falsy() {
        assert!(!should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: None,
                selected_count: 1.0,
                is_busy: None,
            }
        ));
    }

    #[test]
    fn m11_is_busy_explicit_false_passes() {
        assert!(should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("id".to_string()),
                selected_count: 1.0,
                is_busy: Some(false),
            }
        ));
    }

    #[test]
    fn m11_raw_fractional_selected_count_is_truthy() {
        assert!(should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("id".to_string()),
                selected_count: 0.5,
                is_busy: None,
            }
        ));
    }

    #[test]
    fn m11_nan_selected_count_is_falsy() {
        assert!(!should_emit_nested_repo_import_submit_telemetry(
            &ShouldEmitNestedRepoImportSubmitTelemetryArgs {
                attempt_id: Some("id".to_string()),
                selected_count: f64::NAN,
                is_busy: None,
            }
        ));
    }

    // =========================================================================
    // M12 — full UUID shape: length 36, dash positions, hex elsewhere,
    // version nibble, variant nibble.
    // =========================================================================

    #[test]
    fn m12_generated_attempt_id_has_the_full_uuid_v4_shape() {
        let id = create_nested_repo_telemetry_attempt_id();
        let chars: Vec<char> = id.chars().collect();

        assert_eq!(chars.len(), 36);
        for &dash_index in &[8usize, 13, 18, 23] {
            assert_eq!(chars[dash_index], '-', "expected dash at index {dash_index}");
        }
        for (index, &ch) in chars.iter().enumerate() {
            if [8, 13, 18, 23].contains(&index) {
                continue;
            }
            assert!(
                ch.is_ascii_hexdigit() && (ch.is_ascii_digit() || ch.is_lowercase()),
                "expected lowercase hex at index {index}, got {ch}"
            );
        }
        assert_eq!(chars[14], '4', "version nibble must be 4");
        assert!(
            matches!(chars[19], '8' | '9' | 'a' | 'b'),
            "variant nibble must be in {{8,9,a,b}}, got {}",
            chars[19]
        );
    }

    // =========================================================================
    // Recon-flagged uncovered branches
    // =========================================================================

    #[test]
    fn recon_scan_selected_path_kind_git_repo_yields_git_repo_result() {
        let telemetry = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "id".to_string(),
            surface: NestedRepoTelemetrySurface::Onboarding,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(NestedRepoScanInput {
                repo_count: 5.0,
                selected_path_kind: NestedRepoScanPathKind::GitRepo,
                truncated: false,
                timed_out: false,
            }),
        });
        assert_eq!(telemetry.result, NestedRepoScanTelemetryResult::GitRepo);
        assert_eq!(
            telemetry.selected_path_kind,
            Some(NestedRepoScanPathKind::GitRepo)
        );
    }

    #[test]
    fn recon_scan_with_zero_repos_yields_no_nested_repos_result() {
        let telemetry = build_nested_repo_scan_telemetry(BuildNestedRepoScanTelemetryArgs {
            attempt_id: "id".to_string(),
            surface: NestedRepoTelemetrySurface::Onboarding,
            runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
            scan: Some(NestedRepoScanInput {
                repo_count: 0.0,
                selected_path_kind: NestedRepoScanPathKind::NonGitFolder,
                truncated: false,
                timed_out: false,
            }),
        });
        assert_eq!(telemetry.result, NestedRepoScanTelemetryResult::NoNestedRepos);
        assert_eq!(telemetry.found_count, 0);
    }

    #[test]
    fn recon_open_as_folder_and_back_actions_round_trip() {
        let open_as_folder =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::OpenAsFolder,
                found_count: 1.0,
                selected_count: 0.0,
            });
        assert_eq!(open_as_folder.action, NestedRepoImportTelemetryAction::OpenAsFolder);

        let back =
            build_nested_repo_import_action_telemetry(BuildNestedRepoImportActionTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Sidebar,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                action: NestedRepoImportTelemetryAction::Back,
                found_count: 1.0,
                selected_count: 0.0,
            });
        assert_eq!(back.action, NestedRepoImportTelemetryAction::Back);
    }

    #[test]
    fn recon_imported_and_already_known_exceeding_the_cap_do_not_clamp_the_sum() {
        // Each of imported/already_known is individually capped at 500, but
        // `accepted_count` (their sum) is never re-clamped — mirrors O:211
        // exactly, which has no upper bound on the addition.
        let result =
            build_nested_repo_import_result_telemetry(BuildNestedRepoImportResultTelemetryArgs {
                attempt_id: "id".to_string(),
                surface: NestedRepoTelemetrySurface::Onboarding,
                runtime_kind: NestedRepoTelemetryRuntimeKind::Local,
                mode: ProjectGroupImportMode::Group,
                found_count: 999.0,
                selected_count: 999.0,
                result: Some(NestedRepoImportResultCounts {
                    imported_count: 999.0,
                    already_known_count: 999.0,
                    failed_count: 0.0,
                }),
            });

        assert_eq!(result.imported_count, 500);
        assert_eq!(result.already_known_count, 500);
        assert_eq!(result.outcome, NestedRepoImportTelemetryOutcome::Success);
    }
}
