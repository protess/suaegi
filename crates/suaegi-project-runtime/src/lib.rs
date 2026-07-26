//! VERBATIM port of Orca's `src/shared/project-execution-runtime.ts` (294
//! lines, ZERO imports).
//!
//! Ported: `O:1-4` [`LocalWindowsRuntimePreference`], `O:6-8`
//! [`GlobalWindowsRuntimeDefault`], `O:16-38`
//! [`ResolvedProjectExecutionRuntime`], `O:40-43`
//! [`ProjectExecutionRuntimeRepairReason`], `O:45-51`
//! [`ProjectExecutionRuntimeRepair`], `O:53-55`
//! [`ProjectExecutionRuntimeResolution`], `O:57-62`
//! [`LegacyWindowsRuntimeSettings`], `O:64-67`
//! [`LegacyWindowsRuntimeMigrationContext`], `O:69-71`
//! [`LegacyWindowsRuntimeFallbackReason`], `O:73-76`
//! [`LegacyWindowsRuntimeDefaultMigration`], `O:78-85`
//! [`ResolveProjectExecutionRuntimeArgs`], `O:87` [`RuntimeSource`], `O:89-108`
//! [`normalize_project_runtime_preference`], `O:110-120`
//! [`normalize_global_windows_runtime_default`], `O:122-143`
//! [`derive_global_windows_runtime_default_from_legacy_settings`], `O:145-176`
//! [`resolve_project_execution_runtime`], `O:178-225` `resolveWslRuntime`
//! (private, see `resolve_wsl_runtime`), `O:227-241` `resolvedWindowsHost`
//! (private, see `resolved_windows_host`), `O:243-254`
//! `getLegacyWslFallbackReason` (private, see `get_legacy_wsl_fallback_reason`),
//! `O:256-267` `getWslRepairReason` (private, see `get_wsl_repair_reason`),
//! `O:269-274` `isKnownMissingDistro` (private, see `is_known_missing_distro`),
//! `O:276-278` `isRecord` (private, see [`RawValue`] / `is_record`),
//! `O:280-286` `normalizeDistro` (private, see `normalize_distro`), `O:288-294`
//! `isWslShell` (private, see `is_wsl_shell`).
//!
//! # Traps (see the plan's §1 for the full rationale)
//! - **E1**: the TS `unknown` input surface is modeled by the hand-rolled
//!   [`RawValue`] enum instead of `serde_json::Value` — the entire observable
//!   surface is "is it object-like" (`typeof === 'object' && !== null`;
//!   arrays count, functions don't), "is a given field a string and which
//!   one". `distro: null` / `distro: 42` / an absent `distro` all normalize
//!   identically (via `normalize_distro`'s `None` branch), so they collapse
//!   to the same `Option::None` in the field representation. Every field of
//!   [`LegacyWindowsRuntimeSettings`] is likewise `unknown` in the TS source
//!   and is modeled the same way (`Option<&str>`, where `None` means "not a
//!   string"). Note: because `RawValue` is *constructed* directly by Rust
//!   callers (there is no JS-value deserializer here), "an array with no
//!   `kind`/`distro` string properties" and "a bare non-object scalar" are
//!   both represented as inputs that produce identical output through every
//!   public function in this module — there is no test that can observe a
//!   difference between them through this API, and none is invented below.
//! - **E2**: FOUR separate enums, never merged: [`LocalWindowsRuntimePreference::Wsl`]
//!   carries `distro: String` (always non-empty, `O:104`);
//!   [`GlobalWindowsRuntimeDefault::Wsl`] carries `distro: Option<String>`
//!   (`O:116`). A corrupt *preference* distro collapses to `InheritGlobal`
//!   (`O:104`); a *global default* with no distro stays `Wsl` and later
//!   produces a `wsl-distro-required` repair (`O:183`). `windows-host` also
//!   appears in [`ResolvedProjectExecutionRuntime::WindowsHost`] (payload:
//!   `project_id`/`reason`/`cache_key`) in addition to the two payload-free
//!   `WindowsHost` variants above.
//! - **E3**: `context.wslAvailable === false` is a STRICT comparison
//!   (`O:196-199`, `O:260`), and `undefined`/absent is the common production
//!   path (the WSL probe hasn't cached yet) — it must NOT produce a repair.
//!   Written as `wsl_available == Some(false)`, never `unwrap_or(...)`.
//! - **E4**: `Array.isArray([])` is `true` (`O:273`), so `Some(&[])` (an
//!   empty available-distros list) means "every distro is missing", while
//!   `None` means "unknown, assume present". Never collapse `Some(&[])` to
//!   `None`.
//! - **E5**: the haystack (`availableWslDistros`) is NOT normalized — no
//!   trim, no case fold (`O:273`, `.includes` is exact/case-sensitive). Only
//!   the needle (`distro`) is trimmed, by `normalize_distro`. Implemented as
//!   `list.contains(&distro)` (exact/case-sensitive, no trim, no fold).
//! - **E6**: `O:292`,
//!   `value.trim().split(/[\\/]/).pop()?.toLowerCase()`, mixes THREE
//!   mechanisms that must not be conflated: `.trim()` is ECMAScript
//!   whitespace (-> [`suaegi_misc::js_trim`], same as `O:284`'s
//!   `normalizeDistro`); `.split(/[\\/]/)` is a regex but an unflagged
//!   2-ASCII-char class (-> a plain `char` predicate, no `regex` crate);
//!   `.toLowerCase()` is full-Unicode (-> `str::to_lowercase()`, NOT
//!   `to_ascii_lowercase()` — there is no `/i` regex anywhere in this file).
//! - **E7**: the two repair `cacheKey`s are DIFFERENT string shapes, kept as
//!   two separate `format!`s: `O:191`'s `wsl-distro-required` cacheKey
//!   hardcodes the literal `:default` and never interpolates the distro
//!   (because there IS no distro at that call site); `O:208`'s cacheKey
//!   interpolates `${distro ?? 'default'}`, whose `?? 'default'` fallback is
//!   unreachable dead code (`O:183` already guarantees `distro` is non-empty
//!   by the time `O:208` runs).
//! - **E8**: the `windows-host` cacheKey deliberately OMITS the reason
//!   (`O:238`, `${projectId}:windows-host`), so `project-override` and
//!   `global-default` windows-host results share a cache key. NOT
//!   "improved" here. The resolved `wsl` cacheKey (`O:222`) likewise omits
//!   the reason but includes the distro.
//! - **E9**: `reason: 'migration-fallback'` on [`WindowsHostReason`] is
//!   UNREACHABLE. It is part of `resolvedWindowsHost`'s parameter type
//!   (`O:229`) but no call site (`O:163`, `O:175`) ever passes it — only
//!   `project-override`/`global-default` are constructed. The variant is
//!   kept, never constructed by this crate's code, and `deriveGlobal...`'s
//!   `fallbackReason` is deliberately NOT wired into it (that would invent
//!   behavior the TS source doesn't have).
//! - **E10**: [`ProjectExecutionRuntimeResolution`] is a 2-variant sum type
//!   (`O:53-55`) — a repair result has no `runtime` field at all, and vice
//!   versa. Not a struct with two `Option` fields (which could represent
//!   "both Some" or "both None", neither of which the TS source can produce).
//! - **E11**: two check orders are contractual and both are oracle-silent.
//!   (a) inside `resolve_wsl_runtime`, the missing-distro check (`O:183`,
//!   `!distro`) runs BEFORE the availability check (`O:196`), so
//!   `distro: None` + `wsl_available: Some(false)` yields
//!   `wsl-distro-required`, not `wsl-unavailable`. (b) inside both
//!   `get_wsl_repair_reason` and `get_legacy_wsl_fallback_reason`,
//!   `wsl_available == Some(false)` is checked BEFORE the missing-distro
//!   check (`O:260`->`O:263`, `O:247`->`O:250`).
//! - **E12**: a corrupt *preference* distro (`{kind:'wsl', distro:'   '}`)
//!   yields `InheritGlobal` (`O:104`), NOT `WindowsHost` — the project
//!   defers to the global default, which can still resolve to WSL. Not
//!   "made safe" by forcing a host fallback.
//! - **E13**: `normalizeGlobalWindowsRuntimeDefault` is not even imported by
//!   the `.test.ts` oracle — its non-record branch (`O:111-112`,
//!   -> `windows-host`) has zero oracle coverage, pinned directly below. It
//!   also has NO `inherit-global` case at all (`O:119`): an `inherit-global`
//!   `kind` value maps to `windows-host`, same as any other non-`wsl` kind.
//! - **E14**: [`ProjectExecutionRuntimeRepairReason`] (3 values:
//!   `wsl-unavailable` / `wsl-distro-missing` / `wsl-distro-required`) and
//!   [`LegacyWindowsRuntimeFallbackReason`] (2 values, `legacy-`-prefixed;
//!   there is no legacy `distro-required` because a `null` legacy distro is
//!   passed straight through, `O:139`) are kept as two separate enums.
//! - **E15**: `is_wsl_shell` has ZERO oracle coverage (the only structural
//!   candidate, `T:41-52`, returns early on `localAgentRuntime === 'host'`
//!   before reaching it). All pins are hand-written below: split on BOTH
//!   `\` and `/`; exact-match the last segment against `wsl.exe` or `wsl`;
//!   `wsl.exe.bak` and `mywsl` are `false`; `" wsl.exe "` is `true` because
//!   of the leading `.trim()`.
//! - **E16**: `localAgentRuntime === 'host'` (`O:127`) beats everything,
//!   including a WSL terminal-shell sniff; the distro is
//!   `localAgentWslDistro` then `terminalWindowsWslDistro` (`O:133-134`),
//!   both through `normalize_distro`, so a whitespace-only agent distro
//!   falls through to the terminal distro; `fallbackReason` is ALWAYS
//!   present in the return value (as `Option::None`, never omitted).

use suaegi_misc::js_trim;

// ---------------------------------------------------------------------------
// O:1-4 LocalWindowsRuntimePreference
// ---------------------------------------------------------------------------

/// `O:1-4`. E2: the `Wsl` distro is always non-empty (`normalize_distro`
/// guarantees this at every construction site, `O:104`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalWindowsRuntimePreference {
    InheritGlobal,
    WindowsHost,
    Wsl { distro: String },
}

// ---------------------------------------------------------------------------
// O:6-8 GlobalWindowsRuntimeDefault
// ---------------------------------------------------------------------------

/// `O:6-8`. E2: the `Wsl` distro MAY be absent — that's the whole reason
/// this is a separate enum from [`LocalWindowsRuntimePreference`], not a
/// shared "Wsl { distro: Option<String> }" merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobalWindowsRuntimeDefault {
    WindowsHost,
    Wsl { distro: Option<String> },
}

// ---------------------------------------------------------------------------
// O:87 RuntimeSource (module-private type alias in the TS source)
// ---------------------------------------------------------------------------

/// `O:87`, `type RuntimeSource = 'project-override' | 'global-default'`.
/// Shared by a resolved WSL runtime's `reason` and a repair's `source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeSource {
    ProjectOverride,
    GlobalDefault,
}

// ---------------------------------------------------------------------------
// O:10-14 / O:16-38 ResolvedProjectExecutionRuntime (+ its reason union)
// ---------------------------------------------------------------------------

/// `Exclude<ProjectExecutionRuntimeReason, 'non-windows'>` (`O:28`) — the
/// reason attached to a `windows-host` resolution. E9: `MigrationFallback`
/// is UNREACHABLE — part of `resolvedWindowsHost`'s parameter type (`O:229`)
/// but no call site in this module ever constructs it. Kept, unconstructed,
/// for type fidelity with the TS union.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsHostReason {
    ProjectOverride,
    GlobalDefault,
    MigrationFallback,
}

/// `O:16-38`. `hostPlatform` is a fixed literal for the `WindowsHost`
/// (`'win32'`) and `Wsl` (`'wsl'`) variants in the TS source, so it is
/// implied by the Rust variant tag rather than stored redundantly; for
/// `LocalHost` it varies (`args.appPlatform`, `O:153`) and IS stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedProjectExecutionRuntime {
    /// `reason` is always the fixed literal `'non-windows'` (`O:21`), so it
    /// is implied by the variant tag rather than stored as a field.
    LocalHost {
        host_platform: String,
        project_id: String,
        cache_key: String,
    },
    WindowsHost {
        project_id: String,
        reason: WindowsHostReason,
        cache_key: String,
    },
    Wsl {
        project_id: String,
        distro: String,
        reason: RuntimeSource,
        cache_key: String,
    },
}

// ---------------------------------------------------------------------------
// O:40-43 / O:45-51 repair types
// ---------------------------------------------------------------------------

/// `O:40-43`. E14: 3 values, distinct from [`LegacyWindowsRuntimeFallbackReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectExecutionRuntimeRepairReason {
    WslUnavailable,
    WslDistroRequired,
    WslDistroMissing,
}

impl ProjectExecutionRuntimeRepairReason {
    /// The exact string used inside a repair `cacheKey` (`O:208`).
    fn cache_key_fragment(self) -> &'static str {
        match self {
            Self::WslUnavailable => "wsl-unavailable",
            Self::WslDistroRequired => "wsl-distro-required",
            Self::WslDistroMissing => "wsl-distro-missing",
        }
    }
}

/// `O:45-51`. `preferredRuntime: { kind: 'wsl'; distro: string | null }`
/// (`O:47`) is flattened to `preferred_distro` here since `kind` has only
/// the one possible literal value `'wsl'` in this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectExecutionRuntimeRepair {
    pub project_id: String,
    pub preferred_distro: Option<String>,
    pub reason: ProjectExecutionRuntimeRepairReason,
    pub source: RuntimeSource,
    pub cache_key: String,
}

// ---------------------------------------------------------------------------
// O:53-55 ProjectExecutionRuntimeResolution
// ---------------------------------------------------------------------------

/// `O:53-55`. E10: a 2-variant sum type, not a struct with two `Option`
/// fields — a repair result has no `runtime` field at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectExecutionRuntimeResolution {
    Resolved(ResolvedProjectExecutionRuntime),
    RepairRequired(ProjectExecutionRuntimeRepair),
}

// ---------------------------------------------------------------------------
// O:57-62 / O:64-67 / O:69-71 / O:73-76 legacy migration types
// ---------------------------------------------------------------------------

/// `O:57-62`. Every field is `unknown` in the TS source; modeled per E1 as
/// `Option<&str>` where `None` covers `null`/absent/any non-string value —
/// each field is only ever used via `=== 'literal'` comparisons or passed to
/// a `typeof value !== 'string'` check (`normalize_distro`, `is_wsl_shell`),
/// so collapsing every non-string case to `None` loses no observable
/// behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct LegacyWindowsRuntimeSettings<'a> {
    pub local_agent_runtime: Option<&'a str>,
    pub local_agent_wsl_distro: Option<&'a str>,
    pub terminal_windows_shell: Option<&'a str>,
    pub terminal_windows_wsl_distro: Option<&'a str>,
}

/// `O:64-67`. E4: `available_wsl_distros: Some(&[])` ("every distro is
/// missing") and `None` ("unknown, assume present") are distinct and must
/// never be collapsed into each other.
#[derive(Debug, Clone, Copy, Default)]
pub struct LegacyWindowsRuntimeMigrationContext<'a> {
    pub wsl_available: Option<bool>,
    pub available_wsl_distros: Option<&'a [&'a str]>,
}

/// `O:69-71`. E14: 2 values (`legacy-` prefixed) — there is no
/// `legacy-wsl-distro-required` because `O:139` passes a possibly-`None`
/// distro straight through without a required-distro check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyWindowsRuntimeFallbackReason {
    LegacyWslUnavailable,
    LegacyWslDistroMissing,
}

/// `O:73-76`. E16: `fallback_reason` is always present as a field (its value
/// is `None` when there's no fallback), never omitted from the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyWindowsRuntimeDefaultMigration {
    pub default_runtime: GlobalWindowsRuntimeDefault,
    pub fallback_reason: Option<LegacyWindowsRuntimeFallbackReason>,
}

// ---------------------------------------------------------------------------
// O:78-85 ResolveProjectExecutionRuntimeArgs
// ---------------------------------------------------------------------------

/// `O:78-85`.
#[derive(Debug, Clone, Copy)]
pub struct ResolveProjectExecutionRuntimeArgs<'a> {
    pub app_platform: &'a str,
    pub project_id: &'a str,
    pub project_runtime_preference: RawValue<'a>,
    pub global_windows_runtime_default: RawValue<'a>,
    pub wsl_available: Option<bool>,
    pub available_wsl_distros: Option<&'a [&'a str]>,
}

// ---------------------------------------------------------------------------
// E1 — hand-rolled `unknown` input model
// ---------------------------------------------------------------------------

/// Hand-rolled model of a TS `unknown` value as consumed by
/// `normalizeProjectRuntimePreference` (`O:89`) and
/// `normalizeGlobalWindowsRuntimeDefault` (`O:110`). See the crate-level E1
/// doc for what this deliberately does and does not distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawValue<'a> {
    /// `typeof value !== 'object' || value === null` — not object-like.
    /// Covers primitives, `null`, `undefined`, and functions (`typeof fn`
    /// is `'function'`, never `'object'`).
    NotObject,
    /// `typeof value === 'object' && value !== null` — object-like (arrays
    /// included, per `Array.isArray([]) === true`; functions excluded).
    /// Only the two fields this module ever reads (`kind`, `distro`) are
    /// modeled; each is `None` when absent or not a string.
    Object {
        kind: Option<&'a str>,
        distro: Option<&'a str>,
    },
}

/// `O:276-278`, `isRecord`. `typeof value === 'object' && value !== null`.
fn is_record(value: &RawValue) -> bool {
    matches!(value, RawValue::Object { .. })
}

// ---------------------------------------------------------------------------
// O:280-286 normalizeDistro (private)
// ---------------------------------------------------------------------------

/// `O:280-286`. `value` is `None` when the source field was not a string
/// (covers `null`/absent/any non-string per E1). E6: the trim is
/// [`js_trim`] (ECMAScript whitespace), never `str::trim`.
fn normalize_distro(value: Option<&str>) -> Option<String> {
    let value = value?;
    let trimmed = js_trim(value);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ---------------------------------------------------------------------------
// O:288-294 isWslShell (private)
// ---------------------------------------------------------------------------

/// `O:288-294`, `value.trim().split(/[\\/]/).pop()?.toLowerCase()`. E6: three
/// mechanisms, none of which may be conflated — `.trim()` is [`js_trim`]
/// (ECMAScript whitespace); `.split(/[\\/]/)` is an unflagged 2-ASCII-char
/// class, implemented as a plain `char` predicate (no `regex` crate);
/// `.toLowerCase()` is full-Unicode `str::to_lowercase()`, NOT
/// `to_ascii_lowercase()` — there is no `/i` flag anywhere in this file.
fn is_wsl_shell(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let trimmed = js_trim(value);
    let shell_name = trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_lowercase();
    shell_name == "wsl.exe" || shell_name == "wsl"
}

// ---------------------------------------------------------------------------
// O:269-274 isKnownMissingDistro (private)
// ---------------------------------------------------------------------------

/// `O:269-274`. E4: `Some(&[])` ("it IS an array, just empty") means every
/// distro is missing; `None` means "not an array" (`null`/`undefined` in the
/// TS source), i.e. "unknown, assume present" -> `false`. E5: the haystack
/// is compared exactly, with no trim and no case fold.
fn is_known_missing_distro(distro: &str, available_wsl_distros: Option<&[&str]>) -> bool {
    match available_wsl_distros {
        Some(list) => !list.contains(&distro),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// O:256-267 getWslRepairReason (private)
// ---------------------------------------------------------------------------

/// `O:256-267`. E11(b): the `wsl_available == Some(false)` check runs BEFORE
/// the missing-distro check.
fn get_wsl_repair_reason(
    distro: &str,
    context: &LegacyWindowsRuntimeMigrationContext,
) -> Option<ProjectExecutionRuntimeRepairReason> {
    if context.wsl_available == Some(false) {
        return Some(ProjectExecutionRuntimeRepairReason::WslUnavailable);
    }
    if is_known_missing_distro(distro, context.available_wsl_distros) {
        return Some(ProjectExecutionRuntimeRepairReason::WslDistroMissing);
    }
    None
}

// ---------------------------------------------------------------------------
// O:243-254 getLegacyWslFallbackReason (private)
// ---------------------------------------------------------------------------

/// `O:243-254`. E11(b): same check order as `get_wsl_repair_reason` —
/// unavailable before distro-missing.
fn get_legacy_wsl_fallback_reason(
    distro: Option<&str>,
    context: &LegacyWindowsRuntimeMigrationContext,
) -> Option<LegacyWindowsRuntimeFallbackReason> {
    if context.wsl_available == Some(false) {
        return Some(LegacyWindowsRuntimeFallbackReason::LegacyWslUnavailable);
    }
    if let Some(distro) = distro {
        if is_known_missing_distro(distro, context.available_wsl_distros) {
            return Some(LegacyWindowsRuntimeFallbackReason::LegacyWslDistroMissing);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// O:227-241 resolvedWindowsHost (private)
// ---------------------------------------------------------------------------

/// `O:227-241`. E8: the cacheKey deliberately omits `reason`, so a
/// `ProjectOverride` and a `GlobalDefault` result for the same `project_id`
/// share a cache key.
fn resolved_windows_host(
    project_id: &str,
    reason: WindowsHostReason,
) -> ProjectExecutionRuntimeResolution {
    ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::WindowsHost {
        project_id: project_id.to_string(),
        reason,
        cache_key: format!("{project_id}:windows-host"),
    })
}

// ---------------------------------------------------------------------------
// O:178-225 resolveWslRuntime (private)
// ---------------------------------------------------------------------------

/// `O:178-225`. E11(a): the missing-distro check (`!distro`, `O:183`) runs
/// BEFORE the availability check (`O:196`) — `distro: None` always yields
/// `wsl-distro-required` regardless of `wsl_available`. E7: this repair's
/// cacheKey hardcodes the literal `:default` and never interpolates a
/// distro (`O:191`) — a SEPARATE `format!` from the one below.
fn resolve_wsl_runtime(
    args: &ResolveProjectExecutionRuntimeArgs,
    distro: Option<String>,
    source: RuntimeSource,
) -> ProjectExecutionRuntimeResolution {
    let Some(distro) = distro else {
        return ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
            project_id: args.project_id.to_string(),
            preferred_distro: None,
            reason: ProjectExecutionRuntimeRepairReason::WslDistroRequired,
            source,
            cache_key: format!("{}:repair:wsl-distro-required:default", args.project_id),
        });
    };

    let context = LegacyWindowsRuntimeMigrationContext {
        wsl_available: args.wsl_available,
        available_wsl_distros: args.available_wsl_distros,
    };
    if let Some(repair_reason) = get_wsl_repair_reason(&distro, &context) {
        // E7: this cacheKey DOES interpolate the distro — a distinct string
        // shape from the `wsl-distro-required` cacheKey above; `?? 'default'`
        // in the TS source (`O:208`) is unreachable dead code here since
        // `distro` is guaranteed non-empty at this point, so it is not
        // reproduced.
        return ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
            project_id: args.project_id.to_string(),
            preferred_distro: Some(distro.clone()),
            reason: repair_reason,
            source,
            cache_key: format!(
                "{}:repair:{}:{}",
                args.project_id,
                repair_reason.cache_key_fragment(),
                distro
            ),
        });
    }

    ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
        project_id: args.project_id.to_string(),
        cache_key: format!("{}:wsl:{}", args.project_id, distro),
        distro,
        reason: source,
    })
}

// ---------------------------------------------------------------------------
// O:89-108 normalizeProjectRuntimePreference (exported)
// ---------------------------------------------------------------------------

/// `O:89-108`. E12: a corrupt `wsl` distro (blank after trim) collapses to
/// `InheritGlobal`, NOT `WindowsHost` — the project defers to the global
/// default.
pub fn normalize_project_runtime_preference(value: &RawValue) -> LocalWindowsRuntimePreference {
    if !is_record(value) {
        return LocalWindowsRuntimePreference::InheritGlobal;
    }
    let RawValue::Object { kind, distro } = value else {
        unreachable!("is_record guard above ensures Object")
    };

    match *kind {
        Some("inherit-global") => LocalWindowsRuntimePreference::InheritGlobal,
        Some("windows-host") => LocalWindowsRuntimePreference::WindowsHost,
        Some("wsl") => match normalize_distro(*distro) {
            Some(distro) => LocalWindowsRuntimePreference::Wsl { distro },
            None => LocalWindowsRuntimePreference::InheritGlobal,
        },
        _ => LocalWindowsRuntimePreference::InheritGlobal,
    }
}

// ---------------------------------------------------------------------------
// O:110-120 normalizeGlobalWindowsRuntimeDefault (exported)
// ---------------------------------------------------------------------------

/// `O:110-120`. E13: NOT imported by the `.test.ts` oracle at all — the
/// non-record branch (-> `WindowsHost`) has zero indirect coverage either.
/// Also has NO `inherit-global` case: any `kind` other than `'wsl'` maps to
/// `WindowsHost`.
pub fn normalize_global_windows_runtime_default(value: &RawValue) -> GlobalWindowsRuntimeDefault {
    if !is_record(value) {
        return GlobalWindowsRuntimeDefault::WindowsHost;
    }
    let RawValue::Object { kind, distro } = value else {
        unreachable!("is_record guard above ensures Object")
    };

    if *kind == Some("wsl") {
        return GlobalWindowsRuntimeDefault::Wsl {
            distro: normalize_distro(*distro),
        };
    }
    GlobalWindowsRuntimeDefault::WindowsHost
}

// ---------------------------------------------------------------------------
// O:122-143 deriveGlobalWindowsRuntimeDefaultFromLegacySettings (exported)
// ---------------------------------------------------------------------------

/// `O:122-143`. E16: `localAgentRuntime === 'host'` beats everything,
/// including a WSL terminal-shell sniff; the distro preference order is
/// `localAgentWslDistro` then `terminalWindowsWslDistro`, both through
/// [`normalize_distro`]; `fallback_reason` is always present as a field.
pub fn derive_global_windows_runtime_default_from_legacy_settings(
    settings: Option<&LegacyWindowsRuntimeSettings>,
    context: &LegacyWindowsRuntimeMigrationContext,
) -> LegacyWindowsRuntimeDefaultMigration {
    let selected_runtime = settings.and_then(|s| s.local_agent_runtime);
    if selected_runtime == Some("host") {
        return LegacyWindowsRuntimeDefaultMigration {
            default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
            fallback_reason: None,
        };
    }

    let terminal_shell = settings.and_then(|s| s.terminal_windows_shell);
    if selected_runtime == Some("wsl") || is_wsl_shell(terminal_shell) {
        let local_distro = settings.and_then(|s| s.local_agent_wsl_distro);
        let terminal_distro = settings.and_then(|s| s.terminal_windows_wsl_distro);
        let distro = normalize_distro(local_distro).or_else(|| normalize_distro(terminal_distro));

        let fallback_reason = get_legacy_wsl_fallback_reason(distro.as_deref(), context);
        if let Some(fallback_reason) = fallback_reason {
            return LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
                fallback_reason: Some(fallback_reason),
            };
        }
        return LegacyWindowsRuntimeDefaultMigration {
            default_runtime: GlobalWindowsRuntimeDefault::Wsl { distro },
            fallback_reason: None,
        };
    }

    LegacyWindowsRuntimeDefaultMigration {
        default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
        fallback_reason: None,
    }
}

// ---------------------------------------------------------------------------
// O:145-176 resolveProjectExecutionRuntime (exported)
// ---------------------------------------------------------------------------

/// `O:145-176`.
pub fn resolve_project_execution_runtime(
    args: &ResolveProjectExecutionRuntimeArgs,
) -> ProjectExecutionRuntimeResolution {
    if args.app_platform != "win32" {
        return ProjectExecutionRuntimeResolution::Resolved(
            ResolvedProjectExecutionRuntime::LocalHost {
                host_platform: args.app_platform.to_string(),
                cache_key: format!("{}:local-host:{}", args.project_id, args.app_platform),
                project_id: args.project_id.to_string(),
            },
        );
    }

    let project_preference = normalize_project_runtime_preference(&args.project_runtime_preference);
    match project_preference {
        LocalWindowsRuntimePreference::WindowsHost => {
            return resolved_windows_host(args.project_id, WindowsHostReason::ProjectOverride);
        }
        LocalWindowsRuntimePreference::Wsl { distro } => {
            return resolve_wsl_runtime(args, Some(distro), RuntimeSource::ProjectOverride);
        }
        LocalWindowsRuntimePreference::InheritGlobal => {}
    }

    let global_default =
        normalize_global_windows_runtime_default(&args.global_windows_runtime_default);
    match global_default {
        GlobalWindowsRuntimeDefault::Wsl { distro } => {
            resolve_wsl_runtime(args, distro, RuntimeSource::GlobalDefault)
        }
        GlobalWindowsRuntimeDefault::WindowsHost => {
            resolved_windows_host(args.project_id, WindowsHostReason::GlobalDefault)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj<'a>(kind: Option<&'a str>, distro: Option<&'a str>) -> RawValue<'a> {
        RawValue::Object { kind, distro }
    }

    // =======================================================================
    // Oracle — normalizeProjectRuntimePreference (T:9-30)
    // =======================================================================

    #[test]
    fn oracle_preserves_valid_project_runtime_preferences() {
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("inherit-global"), None)),
            LocalWindowsRuntimePreference::InheritGlobal
        );
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("windows-host"), None)),
            LocalWindowsRuntimePreference::WindowsHost
        );
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("wsl"), Some("Ubuntu-24.04"))),
            LocalWindowsRuntimePreference::Wsl {
                distro: "Ubuntu-24.04".to_string()
            }
        );
    }

    #[test]
    fn oracle_falls_back_malformed_project_runtime_preferences_to_inherit_global() {
        assert_eq!(
            normalize_project_runtime_preference(&RawValue::NotObject),
            LocalWindowsRuntimePreference::InheritGlobal
        );
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("wsl"), Some("   "))),
            LocalWindowsRuntimePreference::InheritGlobal
        );
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("bogus"), Some("Ubuntu"))),
            LocalWindowsRuntimePreference::InheritGlobal
        );
    }

    // =======================================================================
    // Oracle — deriveGlobalWindowsRuntimeDefaultFromLegacySettings (T:34-101)
    // =======================================================================

    #[test]
    fn oracle_defaults_malformed_legacy_settings_to_the_host_global_default() {
        let result = derive_global_windows_runtime_default_from_legacy_settings(
            None,
            &LegacyWindowsRuntimeMigrationContext::default(),
        );
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
                fallback_reason: None,
            }
        );
    }

    #[test]
    fn oracle_migrates_existing_host_settings_to_the_host_global_default() {
        let settings = LegacyWindowsRuntimeSettings {
            local_agent_runtime: Some("host"),
            terminal_windows_shell: Some("wsl.exe"),
            terminal_windows_wsl_distro: Some("Ubuntu"),
            ..Default::default()
        };
        let result = derive_global_windows_runtime_default_from_legacy_settings(
            Some(&settings),
            &LegacyWindowsRuntimeMigrationContext::default(),
        );
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
                fallback_reason: None,
            }
        );
    }

    #[test]
    fn oracle_migrates_existing_wsl_agent_settings_with_their_selected_distro() {
        let settings = LegacyWindowsRuntimeSettings {
            local_agent_runtime: Some("wsl"),
            local_agent_wsl_distro: Some("Ubuntu-24.04"),
            terminal_windows_wsl_distro: Some("Debian"),
            ..Default::default()
        };
        let result = derive_global_windows_runtime_default_from_legacy_settings(
            Some(&settings),
            &LegacyWindowsRuntimeMigrationContext::default(),
        );
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::Wsl {
                    distro: Some("Ubuntu-24.04".to_string())
                },
                fallback_reason: None,
            }
        );
    }

    #[test]
    fn oracle_uses_the_terminal_wsl_distro_when_the_agent_setting_only_selected_wsl() {
        let settings = LegacyWindowsRuntimeSettings {
            local_agent_runtime: Some("wsl"),
            terminal_windows_wsl_distro: Some("Debian"),
            ..Default::default()
        };
        let result = derive_global_windows_runtime_default_from_legacy_settings(
            Some(&settings),
            &LegacyWindowsRuntimeMigrationContext::default(),
        );
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::Wsl {
                    distro: Some("Debian".to_string())
                },
                fallback_reason: None,
            }
        );
    }

    #[test]
    fn oracle_turns_stale_legacy_wsl_state_into_a_migration_host_fallback_when_wsl_is_unavailable()
    {
        let settings = LegacyWindowsRuntimeSettings {
            local_agent_runtime: Some("wsl"),
            local_agent_wsl_distro: Some("Ubuntu"),
            ..Default::default()
        };
        let context = LegacyWindowsRuntimeMigrationContext {
            wsl_available: Some(false),
            available_wsl_distros: Some(&[]),
        };
        let result =
            derive_global_windows_runtime_default_from_legacy_settings(Some(&settings), &context);
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
                fallback_reason: Some(LegacyWindowsRuntimeFallbackReason::LegacyWslUnavailable),
            }
        );
    }

    #[test]
    fn oracle_turns_stale_legacy_wsl_distro_state_into_a_migration_host_fallback() {
        let settings = LegacyWindowsRuntimeSettings {
            local_agent_runtime: Some("wsl"),
            local_agent_wsl_distro: Some("Ubuntu"),
            ..Default::default()
        };
        let context = LegacyWindowsRuntimeMigrationContext {
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Debian"]),
        };
        let result =
            derive_global_windows_runtime_default_from_legacy_settings(Some(&settings), &context);
        assert_eq!(
            result,
            LegacyWindowsRuntimeDefaultMigration {
                default_runtime: GlobalWindowsRuntimeDefault::WindowsHost,
                fallback_reason: Some(LegacyWindowsRuntimeFallbackReason::LegacyWslDistroMissing),
            }
        );
    }

    // =======================================================================
    // Oracle — resolveProjectExecutionRuntime (T:105-331)
    // =======================================================================

    #[test]
    fn oracle_ignores_local_windows_wsl_preferences_on_non_windows_platforms() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "darwin",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(
                ResolvedProjectExecutionRuntime::LocalHost {
                    host_platform: "darwin".to_string(),
                    project_id: "project-1".to_string(),
                    cache_key: "project-1:local-host:darwin".to_string(),
                }
            )
        );
    }

    #[test]
    fn oracle_resolves_inherited_windows_projects_to_the_host_global_default() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(
                ResolvedProjectExecutionRuntime::WindowsHost {
                    project_id: "project-1".to_string(),
                    reason: WindowsHostReason::GlobalDefault,
                    cache_key: "project-1:windows-host".to_string(),
                }
            )
        );
    }

    #[test]
    fn oracle_falls_back_malformed_global_defaults_to_windows_host() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("bogus"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(
                ResolvedProjectExecutionRuntime::WindowsHost {
                    project_id: "project-1".to_string(),
                    reason: WindowsHostReason::GlobalDefault,
                    cache_key: "project-1:windows-host".to_string(),
                }
            )
        );
    }

    #[test]
    fn oracle_resolves_inherited_windows_projects_to_the_wsl_global_default() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu", "Debian"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "project-1".to_string(),
                distro: "Ubuntu".to_string(),
                reason: RuntimeSource::GlobalDefault,
                cache_key: "project-1:wsl:Ubuntu".to_string(),
            })
        );
    }

    #[test]
    fn oracle_lets_a_project_force_host_when_the_global_default_is_wsl() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("windows-host"), None),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(
                ResolvedProjectExecutionRuntime::WindowsHost {
                    project_id: "project-1".to_string(),
                    reason: WindowsHostReason::ProjectOverride,
                    cache_key: "project-1:windows-host".to_string(),
                }
            )
        );
    }

    #[test]
    fn oracle_lets_a_project_force_wsl_when_the_global_default_is_host() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Debian")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu", "Debian"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "project-1".to_string(),
                distro: "Debian".to_string(),
                reason: RuntimeSource::ProjectOverride,
                cache_key: "project-1:wsl:Debian".to_string(),
            })
        );
    }

    #[test]
    fn oracle_returns_repair_state_instead_of_silently_falling_back_when_wsl_is_unavailable() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: Some(false),
            available_wsl_distros: Some(&[]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: Some("Ubuntu".to_string()),
                reason: ProjectExecutionRuntimeRepairReason::WslUnavailable,
                source: RuntimeSource::ProjectOverride,
                cache_key: "project-1:repair:wsl-unavailable:Ubuntu".to_string(),
            })
        );
    }

    #[test]
    fn oracle_returns_repair_state_when_wsl_is_selected_without_a_distro() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("wsl"), None),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: None,
                reason: ProjectExecutionRuntimeRepairReason::WslDistroRequired,
                source: RuntimeSource::GlobalDefault,
                cache_key: "project-1:repair:wsl-distro-required:default".to_string(),
            })
        );
    }

    #[test]
    fn oracle_keeps_two_projects_with_different_runtime_preferences_isolated() {
        let host_args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "host-project",
            project_runtime_preference: obj(Some("windows-host"), None),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        let wsl_args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "wsl-project",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };

        assert_eq!(
            resolve_project_execution_runtime(&host_args),
            ProjectExecutionRuntimeResolution::Resolved(
                ResolvedProjectExecutionRuntime::WindowsHost {
                    project_id: "host-project".to_string(),
                    reason: WindowsHostReason::ProjectOverride,
                    cache_key: "host-project:windows-host".to_string(),
                }
            )
        );
        assert_eq!(
            resolve_project_execution_runtime(&wsl_args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "wsl-project".to_string(),
                distro: "Ubuntu".to_string(),
                reason: RuntimeSource::GlobalDefault,
                cache_key: "wsl-project:wsl:Ubuntu".to_string(),
            })
        );
    }

    #[test]
    fn oracle_returns_repair_state_when_the_selected_distro_is_missing() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Debian"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: Some("Ubuntu".to_string()),
                reason: ProjectExecutionRuntimeRepairReason::WslDistroMissing,
                source: RuntimeSource::ProjectOverride,
                cache_key: "project-1:repair:wsl-distro-missing:Ubuntu".to_string(),
            })
        );
    }

    // =======================================================================
    // E1 — hand-rolled input enum: array-shaped / undefined / string / number
    // all collapse identically through the public normalizers.
    // =======================================================================

    #[test]
    fn e1_non_string_scalars_and_object_like_inputs_without_a_matching_kind_all_collapse() {
        // `undefined`, a bare number, and a bare string are all `NotObject`.
        assert_eq!(
            normalize_project_runtime_preference(&RawValue::NotObject),
            LocalWindowsRuntimePreference::InheritGlobal
        );
        assert_eq!(
            normalize_global_windows_runtime_default(&RawValue::NotObject),
            GlobalWindowsRuntimeDefault::WindowsHost
        );
        // An array-shaped value (`Array.isArray([]) === true`) is
        // object-like but has no string `kind`/`distro` properties — produces
        // the SAME output as `NotObject` through both normalizers. This
        // crate cannot construct an operationally distinguishable "array"
        // input beyond this (see the crate-level E1 doc); this pin documents
        // that fact rather than asserting a difference that doesn't exist.
        let array_shaped = obj(None, None);
        assert_eq!(
            normalize_project_runtime_preference(&array_shaped),
            LocalWindowsRuntimePreference::InheritGlobal
        );
        assert_eq!(
            normalize_global_windows_runtime_default(&array_shaped),
            GlobalWindowsRuntimeDefault::WindowsHost
        );
    }

    // =======================================================================
    // E3 — absent `wsl_available` (the common pre-probe production path)
    // must resolve, not repair. The single most important pin in this file.
    // =======================================================================

    #[test]
    fn e3_absent_wsl_available_resolves_instead_of_repairing() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: None, // NOT Some(false) — the probe hasn't run yet.
            available_wsl_distros: None,
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "project-1".to_string(),
                distro: "Ubuntu".to_string(),
                reason: RuntimeSource::ProjectOverride,
                cache_key: "project-1:wsl:Ubuntu".to_string(),
            })
        );
    }

    // =======================================================================
    // E4 — `Some(&[])` vs `None` for available_wsl_distros.
    // =======================================================================

    #[test]
    fn e4_empty_distro_list_means_missing_but_absent_list_means_present() {
        let mut args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: None,
            available_wsl_distros: Some(&[]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: Some("Ubuntu".to_string()),
                reason: ProjectExecutionRuntimeRepairReason::WslDistroMissing,
                source: RuntimeSource::ProjectOverride,
                cache_key: "project-1:repair:wsl-distro-missing:Ubuntu".to_string(),
            })
        );

        args.available_wsl_distros = None;
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "project-1".to_string(),
                distro: "Ubuntu".to_string(),
                reason: RuntimeSource::ProjectOverride,
                cache_key: "project-1:wsl:Ubuntu".to_string(),
            })
        );
    }

    // =======================================================================
    // E5 — the haystack is never trimmed or case-folded.
    // =======================================================================

    #[test]
    fn e5_available_distro_membership_is_exact_and_case_sensitive() {
        assert!(!is_known_missing_distro("Ubuntu", Some(&["Ubuntu"])));
        assert!(is_known_missing_distro("ubuntu", Some(&["Ubuntu"])));
        assert!(is_known_missing_distro(" Ubuntu", Some(&["Ubuntu"])));
        assert!(is_known_missing_distro("Ubuntu", Some(&[" Ubuntu"])));
    }

    // =======================================================================
    // E6 — js_trim (not str::trim) on both normalize_distro and is_wsl_shell.
    // =======================================================================

    #[test]
    fn e6_feff_is_trimmed_but_nel_is_not_in_normalize_distro() {
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("wsl"), Some("\u{FEFF}Ubuntu\u{FEFF}"))),
            LocalWindowsRuntimePreference::Wsl {
                distro: "Ubuntu".to_string()
            }
        );
        // U+0085 (NEL) is NOT ECMAScript whitespace, so it survives the trim
        // and stays part of the (non-empty) distro string.
        assert_eq!(
            normalize_project_runtime_preference(&obj(Some("wsl"), Some("\u{0085}Ubuntu"))),
            LocalWindowsRuntimePreference::Wsl {
                distro: "\u{0085}Ubuntu".to_string()
            }
        );
    }

    #[test]
    fn e6_feff_is_trimmed_but_nel_is_not_in_is_wsl_shell() {
        // str::trim would strip this differently in both directions: it does
        // NOT strip U+FEFF (so this would wrongly stay non-matching) and DOES
        // strip U+0085 (so the next pin would wrongly become a match).
        assert!(is_wsl_shell(Some("\u{FEFF}wsl.exe")));
        assert!(!is_wsl_shell(Some("\u{0085}wsl.exe")));
    }

    #[test]
    fn e6_to_lowercase_has_no_ascii_vs_unicode_distinguishing_input_for_is_wsl_shell() {
        // Documented rather than faked: neither "wsl" nor "wsl.exe" contains
        // a letter with a special Unicode lowercase mapping from a distinct
        // non-ASCII codepoint (the closest, U+212A KELVIN SIGN -> 'k', maps
        // to a letter absent from both target strings). No input can make
        // `to_lowercase()` and `to_ascii_lowercase()` disagree on the final
        // `shell_name == "wsl.exe" || shell_name == "wsl"` comparison here.
        // This pin only records ordinary ASCII case-insensitivity, which
        // both mechanisms already agree on.
        assert!(is_wsl_shell(Some("WSL.EXE")));
    }

    // =======================================================================
    // E7 — the two repair cacheKey shapes never unify.
    // =======================================================================

    #[test]
    fn e7_repair_cache_keys_have_different_shapes() {
        // `wsl-distro-required`: literal `:default`, no distro interpolated.
        let required = ProjectExecutionRuntimeRepairReason::WslDistroRequired;
        assert_eq!(required.cache_key_fragment(), "wsl-distro-required");
        let args_no_distro = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "p",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("wsl"), None),
            wsl_available: None,
            available_wsl_distros: None,
        };
        let ProjectExecutionRuntimeResolution::RepairRequired(repair) =
            resolve_project_execution_runtime(&args_no_distro)
        else {
            panic!("expected repair-required");
        };
        assert_eq!(repair.cache_key, "p:repair:wsl-distro-required:default");

        // `wsl-distro-missing`/`wsl-unavailable`: distro IS interpolated.
        let args_missing = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "p",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: None,
            available_wsl_distros: Some(&["Debian"]),
        };
        let ProjectExecutionRuntimeResolution::RepairRequired(repair) =
            resolve_project_execution_runtime(&args_missing)
        else {
            panic!("expected repair-required");
        };
        assert_eq!(repair.cache_key, "p:repair:wsl-distro-missing:Ubuntu");
    }

    // =======================================================================
    // E8 — the windows-host cacheKey collision is explicit and intentional.
    // =======================================================================

    #[test]
    fn e8_project_override_and_global_default_host_results_share_a_cache_key() {
        let ProjectExecutionRuntimeResolution::Resolved(
            ResolvedProjectExecutionRuntime::WindowsHost {
                cache_key: project_override_key,
                reason: project_override_reason,
                ..
            },
        ) = resolved_windows_host("project-1", WindowsHostReason::ProjectOverride)
        else {
            panic!("expected windows-host");
        };
        let ProjectExecutionRuntimeResolution::Resolved(
            ResolvedProjectExecutionRuntime::WindowsHost {
                cache_key: global_default_key,
                reason: global_default_reason,
                ..
            },
        ) = resolved_windows_host("project-1", WindowsHostReason::GlobalDefault)
        else {
            panic!("expected windows-host");
        };

        assert_eq!(project_override_key, "project-1:windows-host");
        assert_eq!(global_default_key, "project-1:windows-host");
        assert_eq!(project_override_key, global_default_key);
        assert_ne!(project_override_reason, global_default_reason);
    }

    // =======================================================================
    // E11(a) — the missing-distro check runs before the availability check.
    // =======================================================================

    #[test]
    fn e11a_missing_distro_beats_unavailable_inside_resolve_wsl_runtime() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("inherit-global"), None),
            global_windows_runtime_default: obj(Some("wsl"), None),
            wsl_available: Some(false),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: None,
                reason: ProjectExecutionRuntimeRepairReason::WslDistroRequired,
                source: RuntimeSource::GlobalDefault,
                cache_key: "project-1:repair:wsl-distro-required:default".to_string(),
            })
        );
    }

    // =======================================================================
    // E11(b) — unavailable beats distro-missing inside the resolve path.
    // =======================================================================

    #[test]
    fn e11b_unavailable_beats_distro_missing_inside_resolve_wsl_runtime() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            project_runtime_preference: obj(Some("wsl"), Some("Ubuntu")),
            global_windows_runtime_default: obj(Some("windows-host"), None),
            wsl_available: Some(false),
            available_wsl_distros: Some(&["Debian"]), // Ubuntu is ALSO missing.
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::RepairRequired(ProjectExecutionRuntimeRepair {
                project_id: "project-1".to_string(),
                preferred_distro: Some("Ubuntu".to_string()),
                reason: ProjectExecutionRuntimeRepairReason::WslUnavailable,
                source: RuntimeSource::ProjectOverride,
                cache_key: "project-1:repair:wsl-unavailable:Ubuntu".to_string(),
            })
        );
    }

    // =======================================================================
    // E12 — end-to-end: a corrupt project distro defers to the global
    // default, which can still resolve to WSL.
    // =======================================================================

    #[test]
    fn e12_corrupt_project_distro_defers_to_a_wsl_global_default() {
        let args = ResolveProjectExecutionRuntimeArgs {
            app_platform: "win32",
            project_id: "project-1",
            // Whitespace-only distro -> normalize_distro -> None ->
            // LocalWindowsRuntimePreference::InheritGlobal (E12), NOT
            // WindowsHost.
            project_runtime_preference: obj(Some("wsl"), Some("   ")),
            global_windows_runtime_default: obj(Some("wsl"), Some("Ubuntu")),
            wsl_available: Some(true),
            available_wsl_distros: Some(&["Ubuntu"]),
        };
        assert_eq!(
            resolve_project_execution_runtime(&args),
            ProjectExecutionRuntimeResolution::Resolved(ResolvedProjectExecutionRuntime::Wsl {
                project_id: "project-1".to_string(),
                distro: "Ubuntu".to_string(),
                reason: RuntimeSource::GlobalDefault,
                cache_key: "project-1:wsl:Ubuntu".to_string(),
            })
        );
    }

    // =======================================================================
    // E13 — normalizeGlobalWindowsRuntimeDefault has zero oracle coverage.
    // =======================================================================

    #[test]
    fn e13_non_record_global_default_becomes_windows_host() {
        assert_eq!(
            normalize_global_windows_runtime_default(&RawValue::NotObject),
            GlobalWindowsRuntimeDefault::WindowsHost
        );
    }

    #[test]
    fn e13_inherit_global_kind_has_no_case_and_becomes_windows_host() {
        assert_eq!(
            normalize_global_windows_runtime_default(&obj(Some("inherit-global"), None)),
            GlobalWindowsRuntimeDefault::WindowsHost
        );
    }

    // =======================================================================
    // E15 — is_wsl_shell has zero oracle coverage; every pin hand-written.
    // =======================================================================

    #[test]
    fn e15_is_wsl_shell_full_coverage() {
        assert!(is_wsl_shell(Some(r"C:\Users\foo\wsl.exe"))); // backslash separator
        assert!(is_wsl_shell(Some("C:/Users/foo/wsl.exe"))); // forward-slash separator
        assert!(is_wsl_shell(Some("WSL.EXE"))); // uppercase, exact segment
        assert!(is_wsl_shell(Some(" wsl.exe "))); // trimmed
        assert!(is_wsl_shell(Some("wsl"))); // bare "wsl", no extension
        assert!(!is_wsl_shell(Some("wsl.exe.bak"))); // not an exact segment match
        assert!(!is_wsl_shell(Some("mywsl"))); // not an exact segment match
        assert!(!is_wsl_shell(None)); // non-string input
    }
}
