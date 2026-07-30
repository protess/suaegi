//! VERBATIM port of Orca's `src/shared/native-file-drop.ts` (269 lines, @
//! v1.4.146-rc.0).
//!
//! Ported: `O:3` [`ORCA_INTERNAL_FILE_DRAG_TYPE`], `O:5-6`
//! [`NATIVE_FILE_DROP_MAX_PATHS`] / [`NATIVE_FILE_DROP_MAX_PATH_BYTES`],
//! `O:8-14` (`NATIVE_FILE_DROP_TARGET`, kept as five string constants +
//! [`NATIVE_FILE_DROP_TARGET_VALUES`]), `O:16-22` [`NativeDropResolution`],
//! `O:24-39` [`NativeFileDropPayload`], `O:41-46`
//! [`NativeFileDropRejectedPayload`], `O:48-53` [`NativeFileDropPathEntry`],
//! `O:55-62` [`NativeFileDropPathValidation`], `O:64-68` (`isNativeFileDropRejectedReason`,
//! private), `O:70-72` (`isNativeFileDropTarget`, private), `O:74-76`
//! (`isOptionalNativeFileDropString`, private), `O:78-80`
//! (`isNativeFileDropPathList`, private), `O:82-84`
//! (`isNonNegativeFiniteNumber`, private), `O:92-97`
//! [`has_native_file_drag_types`], `O:99-136` [`resolve_native_file_drop_path`],
//! `O:138-144` [`measure_native_file_drop_path_bytes`], `O:146-182`
//! [`validate_native_file_drop_paths`], `O:184-193`
//! [`create_rejected_native_file_drop_payload`], `O:195-227`
//! [`create_native_file_drop_payload`], `O:229-269`
//! [`is_native_file_drop_payload`].
//!
//! `O:86-90` (`getDataTransferTypes`) is folded into
//! [`has_native_file_drag_types`] rather than kept as its own function: this
//! crate has no `DataTransfer`/iterable-vs-array-like distinction to bridge,
//! so the caller just passes an already-collected `Option<&[&str]>`.
//!
//! # Traps (see the plan's §1 for full rationale; `L<N>` numbering matches
//! # `docs/superpowers/plans/2026-07-27-native-file-drop.md`)
//!
//! - **L1**: [`NATIVE_FILE_DROP_MAX_PATH_BYTES`] is a **total** cap across the
//!   whole path list, not a per-path cap, despite its name reading as
//!   per-path. [`validate_native_file_drop_paths`] keeps one `byte_length`
//!   accumulator **outside** the loop (`O:165`); each path's `stop_after_bytes`
//!   budget is the **remaining** total (`max_path_bytes - byte_length`,
//!   `O:168`), not the full cap re-applied per path. A per-path
//!   reimplementation (checking each path's own length against the cap
//!   independently) passes almost the entire oracle and fails exactly one
//!   fixture (`test:158-171`, ported below as
//!   `oracle_rejects_native_drops_whose_path_list_is_too_large_without_exposing_paths`) —
//!   see `l1_total_cap_vs_per_path_cap_diverge` for a case built specifically
//!   to separate the two readings.
//! - **L2**: the `byteLength` reported on a `paths-too-large` rejection is a
//!   **truncated total that includes the overshooting character**, not
//!   `paths.iter().map(String::len).sum()`. [`validate_native_file_drop_paths`]
//!   gets this by calling the already-ported
//!   [`suaegi_misc::measure_clipboard_text_byte_length`] with the remaining
//!   budget as `stop_after_bytes` and adding its returned `byte_length` to the
//!   accumulator (`O:167-170`) — never "simplifying" to a plain length sum.
//!   The two are arithmetically identical on the **accepted** path (nothing
//!   ever stops early, so the measured length equals the true length), which
//!   is why an incorrect `path.len()`-sum port still passes the oracle's
//!   *accepted* cases; it silently diverges only on **every rejection**. See
//!   `l2_size_rejection_reports_the_truncated_overshoot_inclusive_total`.
//! - **L3**: `terminalPaneLeafId ??= entry.terminalPaneLeafId` (`O:107`) is
//!   **nullish** assignment, not falsy (`||=`) — they diverge only when a
//!   candidate is an **empty string**: `??=` locks it in (an empty string is
//!   not nullish), `||=` would let a later entry's value overwrite it. The
//!   oracle's only `terminalPaneLeafId` fixture is truthy (`'leaf-1'`), so it
//!   cannot tell the two apart. [`resolve_native_file_drop_path`] reproduces
//!   `??=` with `Option::get_or_insert_with`, which only inserts while the
//!   accumulator is still `None` (nullish) and is a no-op when the entry
//!   itself has no leaf id (mirrors an undefined RHS being a no-op `??=`).
//!   See `l3_empty_string_pane_leaf_id_latches_like_nullish_not_falsy`.
//! - **L4**: the destination-dir guard (`O:123`) is a **mixed predicate**:
//!   "not yet set (`destinationDir === undefined`) AND the candidate is
//!   truthy (`entry.nativeFileDropDir`)". An empty-string dir is skipped
//!   **without latching** the slot, so a later non-empty dir still wins. Both
//!   plausible simplifications are wrong: latching on `is_some()` regardless
//!   of emptiness would let an empty-string dir permanently block a later
//!   good one; last-wins (dropping the "not yet set" half) fails the oracle's
//!   nearest-marker fixture (`test:52-64`, ported as
//!   `oracle_uses_the_nearest_file_explorer_destination_and_fails_closed_without_one`,
//!   which requires the *first* non-empty dir encountered to win). See
//!   `l4_empty_string_dir_does_not_latch_a_later_non_empty_dir_wins`.
//! - **L5**: there is **no decoding anywhere** in this module — no
//!   percent-decoding, no `file://` handling, no platform path munging,
//!   despite the "native file drop" name. `destination_dir` and every path
//!   are opaque, byte-preserving strings; nothing is added here.
//! - **L6**: both caps are modeled as `Option<f64>` (mirroring `S2`/`S6` in
//!   `suaegi-misc::clipboard_text`), not `Option<u64>`. JS `NaN` caps accept
//!   everything (`x > NaN` is always `false`, and
//!   [`suaegi_misc::measure_clipboard_text_byte_length`]'s own finiteness
//!   guard rejects `NaN`, so it never stops early either); `Infinity` accepts
//!   everything the same way. A negative `max_path_bytes` rejects at the
//!   first character of the first non-empty path — **but an empty `paths`
//!   list is still accepted**, because the size check lives *inside* the
//!   `for` loop (`O:171`) and the success return is unconditional (`O:181`).
//!   See `l6_nan_infinity_and_negative_caps_accept_everything_they_should` and
//!   `l6_negative_byte_cap_with_empty_path_list_is_still_accepted`.
//! - **L7**: the oracle references every constant **symbolically**
//!   (`NATIVE_FILE_DROP_MAX_PATHS`, never `256`), so a port with wrong wire
//!   literals — `MAX_PATHS = 512`, `"file_explorer"` instead of
//!   `"file-explorer"`, a different MIME string — is green across the *whole*
//!   suite. These cross an IPC boundary (the payload's `target`/cap values)
//!   and a DOM dataset (the drag-type MIME string), so every literal is
//!   pinned explicitly below: [`ORCA_INTERNAL_FILE_DRAG_TYPE`], all five
//!   target values (`O:8-14`; note **two keys differ from their values**:
//!   `fileExplorer` → `"file-explorer"`, `projectSidebar` →
//!   `"project-sidebar"`), both numeric caps, and
//!   [`NATIVE_FILE_DROP_TARGET_VALUES`]'s `Object.values` insertion order
//!   (`O:71`), which is load-bearing for [`is_native_file_drop_payload`]'s
//!   membership check.
//! - **L8**: the too-many-paths rejection **hard-codes `byteLength: 0`**
//!   (`O:157`) and never attempts byte accounting — a *different* rule from
//!   the size rejection's truncated total (L2). The two rejection reasons
//!   are NOT unified in [`validate_native_file_drop_paths`].
//! - **L9**: both caps use strict `>` (`O:155`, `O:171`); landing exactly on
//!   a cap is **accepted**, one past it is rejected.
//! - **L10**: [`is_native_file_drop_payload`]'s `unknown` input is modeled as
//!   a hand-rolled [`JsValue`]/[`JsRecord`] tree, not `serde_json::Value`:
//!   (a) `serde_json::Number` cannot represent `NaN`/`±Infinity`, which would
//!   make [`is_non_negative_finite_number`]'s finiteness guard unreachable
//!   even though the real IPC transport (structured clone) delivers those
//!   values; (b) `#[serde(deny_unknown_fields)]` would reject extra keys the
//!   guard tolerates at **every** branch (it only ever checks for the
//!   presence/type of specific expected keys, never the *absence* of
//!   others); (c) the `'rejected'` arm (`O:239-245`) never inspects `paths`
//!   at all — a derive-based struct can't express "this variant doesn't even
//!   look at that field."
//!
//! # Other zero-oracle-coverage branches (recon-flagged, pinned below)
//!
//! The **editor** resolution branch, [`resolve_native_file_drop_path`]
//! returning `None` (the common production path — a drop with no marker
//! anywhere), two entries both carrying a pane-leaf id (first wins, via
//! `L3`), a `terminal` resolution with a missing `tab_id`, the
//! conditional-spread **omission** arm in [`create_native_file_drop_payload`]
//! (`O:221-222`: an empty-string `tab_id`/`pane_leaf_id` is dropped from the
//! payload, not carried through as `Some("")`), a too-many-paths rejection
//! reaching [`create_native_file_drop_payload`] (not just
//! [`validate_native_file_drop_paths`] directly), the ordering between the
//! size check and the `rejected`-target check (`O:199` runs before `O:204` —
//! a case where both apply must report the **size** rejection, not `None`),
//! and the `composer`/`project-sidebar` targets. [`measure_native_file_drop_path_bytes`]
//! (`O:138-144`) has neither an oracle test nor a production caller upstream,
//! but is ported and pinned anyway.

use suaegi_misc::measure_clipboard_text_byte_length;

// ---------------------------------------------------------------------------
// Wire constants (`O:3-14`; L7)
// ---------------------------------------------------------------------------

/// `O:3`. The DOM drag-data MIME type Orca stamps on its own internal
/// (in-app) file moves, so native OS file drags can be told apart from them
/// (`O:96`).
pub const ORCA_INTERNAL_FILE_DRAG_TYPE: &str = "text/x-orca-file-path";

/// `O:5`. L1: this is a **total** cap across the whole path list count, not
/// a per-path anything (there is no per-path count to cap).
pub const NATIVE_FILE_DROP_MAX_PATHS: u64 = 256;

/// `O:6`. L1: despite the name reading as "max bytes of a single path", this
/// is the **total** byte budget across every path in the list — see the
/// module doc comment's L1 entry and [`validate_native_file_drop_paths`].
pub const NATIVE_FILE_DROP_MAX_PATH_BYTES: u64 = 256 * 1024;

/// `O:9`.
pub const NATIVE_FILE_DROP_TARGET_EDITOR: &str = "editor";
/// `O:10`.
pub const NATIVE_FILE_DROP_TARGET_TERMINAL: &str = "terminal";
/// `O:11`.
pub const NATIVE_FILE_DROP_TARGET_COMPOSER: &str = "composer";
/// `O:12`. L7: the value is kebab-case and differs from the JS key
/// (`fileExplorer`).
pub const NATIVE_FILE_DROP_TARGET_FILE_EXPLORER: &str = "file-explorer";
/// `O:13`. L7: the value is kebab-case and differs from the JS key
/// (`projectSidebar`).
pub const NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR: &str = "project-sidebar";
/// The `'rejected'` string literal target (`O:22`, `O:45`, `O:71`) — not a
/// member of the `NATIVE_FILE_DROP_TARGET` object upstream, checked
/// separately everywhere it appears.
pub const NATIVE_FILE_DROP_TARGET_REJECTED: &str = "rejected";

/// `Object.values(NATIVE_FILE_DROP_TARGET)` (`O:71`), in the object
/// literal's declaration order (`O:8-14`: editor, terminal, composer,
/// fileExplorer, projectSidebar). L7: this order is load-bearing for
/// [`is_native_file_drop_target`]'s membership check, which is why it is
/// pinned as its own array rather than inlined as five independent
/// comparisons.
pub const NATIVE_FILE_DROP_TARGET_VALUES: [&str; 5] = [
    NATIVE_FILE_DROP_TARGET_EDITOR,
    NATIVE_FILE_DROP_TARGET_TERMINAL,
    NATIVE_FILE_DROP_TARGET_COMPOSER,
    NATIVE_FILE_DROP_TARGET_FILE_EXPLORER,
    NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR,
];

const NATIVE_FILE_DROP_REASON_PATHS_TOO_LARGE: &str = "paths-too-large";
const NATIVE_FILE_DROP_REASON_TOO_MANY_PATHS: &str = "too-many-paths";

// ---------------------------------------------------------------------------
// Domain types (`O:16-62`)
// ---------------------------------------------------------------------------

/// `NativeDropResolution` (`O:16-22`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeDropResolution {
    Editor,
    Terminal {
        tab_id: Option<String>,
        pane_leaf_id: Option<String>,
    },
    Composer,
    FileExplorer {
        destination_dir: String,
    },
    ProjectSidebar,
    Rejected,
}

/// `NativeFileDropRejectedPayload['reason']` (`O:44`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFileDropRejectedReason {
    PathsTooLarge,
    TooManyPaths,
}

/// `NativeFileDropRejectedPayload` (`O:41-46`). `target` is always
/// `'rejected'` and is carried by the wrapping [`NativeFileDropPayload::Rejected`]
/// variant rather than duplicated as a field here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileDropRejectedPayload {
    pub byte_length: u64,
    pub path_count: u64,
    pub reason: NativeFileDropRejectedReason,
}

/// `NativeFileDropPayload` (`O:24-39`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeFileDropPayload {
    Editor {
        paths: Vec<String>,
    },
    Terminal {
        paths: Vec<String>,
        tab_id: Option<String>,
        pane_leaf_id: Option<String>,
    },
    Composer {
        paths: Vec<String>,
    },
    FileExplorer {
        paths: Vec<String>,
        destination_dir: String,
    },
    ProjectSidebar {
        paths: Vec<String>,
    },
    Rejected(NativeFileDropRejectedPayload),
}

/// `NativeFileDropPathEntry` (`O:48-53`). Every field is `unknown`-ish in the
/// TS source (an optional bare string) — L5: never decoded, never munged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeFileDropPathEntry {
    pub native_file_drop_target: Option<String>,
    pub native_file_drop_dir: Option<String>,
    pub terminal_tab_id: Option<String>,
    pub terminal_pane_leaf_id: Option<String>,
}

/// A rejected [`NativeFileDropPathValidation`]. Kept as its own type (rather
/// than folded into the enum inline) so
/// [`create_rejected_native_file_drop_payload`]'s parameter type mirrors the
/// TS `Extract<NativeFileDropPathValidation, { status: 'rejected' }>`
/// narrowing at compile time — an accepted validation cannot even be passed
/// to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeFileDropRejectedValidation {
    pub byte_length: u64,
    pub path_count: u64,
    pub reason: NativeFileDropRejectedReason,
}

/// `NativeFileDropPathValidation` (`O:55-62`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeFileDropPathValidation {
    Accepted { byte_length: u64, path_count: u64 },
    Rejected(NativeFileDropRejectedValidation),
}

/// `validateNativeFileDropPaths`'s `options` parameter (`O:148-151`). L6:
/// both fields are `Option<f64>`, not `Option<u64>`, to reproduce JS numeric
/// coercion (`NaN`/`±Infinity`/negative all have specific, different
/// meanings — see the module doc comment's L6 entry).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NativeFileDropValidationOptions {
    pub max_path_bytes: Option<f64>,
    pub max_paths: Option<f64>,
}

// ---------------------------------------------------------------------------
// Untyped input model (L10)
// ---------------------------------------------------------------------------

/// A JS-like untyped value tree — mirrors TS `unknown`, the input type of
/// [`is_native_file_drop_payload`] (`O:229`). L10: a hand-rolled tree rather
/// than `serde_json::Value`, because `serde_json::Number` cannot represent
/// `NaN`/`±Infinity` (which the real IPC transport delivers and
/// [`is_non_negative_finite_number`]'s guard must reject), and because a
/// derive-based struct can express neither "extra keys are tolerated" nor
/// "the `rejected` arm never looks at `paths`".
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Null,
    Undefined,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<JsValue>),
    Object(JsRecord),
}

impl JsValue {
    /// Convenience constructor for a string value.
    pub fn str(s: impl Into<String>) -> JsValue {
        JsValue::Str(s.into())
    }

    /// Convenience constructor for a number value.
    pub fn number(n: f64) -> JsValue {
        JsValue::Number(n)
    }

    /// Convenience constructor for an array value.
    pub fn array<I>(items: I) -> JsValue
    where
        I: IntoIterator<Item = JsValue>,
    {
        JsValue::Array(items.into_iter().collect())
    }

    /// Convenience constructor for an object value from `(key, value)` pairs.
    pub fn object<I>(pairs: I) -> JsValue
    where
        I: IntoIterator<Item = (&'static str, JsValue)>,
    {
        JsValue::Object(JsRecord::from_pairs(pairs))
    }
}

/// An untyped object record: an ordered list of own `(key, value)` pairs.
/// This module's guard never needs an exact-key-count check (unlike
/// `suaegi-quickcmd`'s `hasExactKeys`), so a simple `Vec` with linear lookup
/// is enough — no `HashMap` needed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JsRecord(Vec<(String, JsValue)>);

impl JsRecord {
    pub fn new() -> Self {
        JsRecord(Vec::new())
    }

    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, JsValue)>,
    {
        JsRecord(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// Builder-style single-key append (own-key semantics: a repeated key
    /// shadows, matching a JS object literal's last-write-wins).
    pub fn with(mut self, key: &str, value: JsValue) -> Self {
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            self.0.push((key.to_string(), value));
        }
        self
    }

    fn get(&self, key: &str) -> Option<&JsValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// `value === literal` for a JS string field (`O:64-72`'s comparisons):
/// tolerates any non-string input (numbers, booleans, `null`/absent all
/// compare unequal), exactly like JS strict equality against a string
/// literal.
fn str_eq(value: Option<&JsValue>, literal: &str) -> bool {
    matches!(value, Some(JsValue::Str(s)) if s == literal)
}

/// `isNonNegativeFiniteNumber` (`O:82-84`).
fn is_non_negative_finite_number(value: Option<&JsValue>) -> bool {
    matches!(value, Some(JsValue::Number(n)) if n.is_finite() && *n >= 0.0)
}

/// `isNativeFileDropRejectedReason` (`O:64-68`).
fn is_native_file_drop_rejected_reason(value: Option<&JsValue>) -> bool {
    str_eq(value, NATIVE_FILE_DROP_REASON_PATHS_TOO_LARGE)
        || str_eq(value, NATIVE_FILE_DROP_REASON_TOO_MANY_PATHS)
}

/// `isNativeFileDropTarget` (`O:70-72`). L7: iterates
/// [`NATIVE_FILE_DROP_TARGET_VALUES`] in its pinned declaration order (moot
/// for `.includes()`-style membership correctness, but the order is still
/// pinned explicitly per L7 since the oracle can never observe it any other
/// way).
fn is_native_file_drop_target(value: Option<&JsValue>) -> bool {
    NATIVE_FILE_DROP_TARGET_VALUES
        .iter()
        .any(|literal| str_eq(value, literal))
        || str_eq(value, NATIVE_FILE_DROP_TARGET_REJECTED)
}

/// `isOptionalNativeFileDropString` (`O:74-76`): `value === undefined ||
/// typeof value === 'string'`. A key absent from the record (`None`) and a
/// key explicitly present with an `undefined` value both satisfy `===
/// undefined` in JS, so both map to this returning `true` here.
fn is_optional_native_file_drop_string(value: Option<&JsValue>) -> bool {
    matches!(
        value,
        None | Some(JsValue::Undefined) | Some(JsValue::Str(_))
    )
}

/// `isNativeFileDropPathList` (`O:78-80`): `Array.isArray(value) &&
/// value.every((path) => typeof path === 'string')`. Returns the extracted
/// `Vec<String>` on success (rather than a bare `bool`) since every caller
/// immediately needs the paths themselves for
/// [`validate_native_file_drop_paths`] — a pragmatic Rust-side fusion of the
/// type guard and its payload extraction, not a behavioral deviation.
fn is_native_file_drop_path_list(value: Option<&JsValue>) -> Option<Vec<String>> {
    match value {
        Some(JsValue::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    JsValue::Str(s) => out.push(s.clone()),
                    _ => return None,
                }
            }
            Some(out)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Drag-type detection (`O:86-97`)
// ---------------------------------------------------------------------------

/// `hasNativeFileDragTypes` (`O:92-97`, folding in `getDataTransferTypes`
/// `O:86-90`). `None` mirrors a `null`/`undefined` `DataTransfer.types`.
pub fn has_native_file_drag_types(types: Option<&[&str]>) -> bool {
    let values = types.unwrap_or(&[]);
    values.contains(&"Files") && !values.contains(&ORCA_INTERNAL_FILE_DRAG_TYPE)
}

// ---------------------------------------------------------------------------
// Resolution (`O:99-136`)
// ---------------------------------------------------------------------------

/// `resolveNativeFileDropPath` (`O:99-136`).
pub fn resolve_native_file_drop_path(
    entries: &[NativeFileDropPathEntry],
) -> Option<NativeDropResolution> {
    let mut found_explorer = false;
    let mut destination_dir: Option<String> = None;
    let mut terminal_pane_leaf_id: Option<String> = None;

    for entry in entries {
        // L3: `terminalPaneLeafId ??= entry.terminalPaneLeafId` (`O:107`) —
        // nullish, not falsy. `get_or_insert_with` only fires while the
        // accumulator is still `None`; if this entry has no leaf id at all,
        // we skip the call entirely (a no-op, matching an `undefined` RHS).
        if let Some(id) = &entry.terminal_pane_leaf_id {
            terminal_pane_leaf_id.get_or_insert_with(|| id.clone());
        }

        let target = entry.native_file_drop_target.as_deref();

        if target == Some(NATIVE_FILE_DROP_TARGET_TERMINAL) {
            return Some(NativeDropResolution::Terminal {
                tab_id: entry.terminal_tab_id.clone(),
                pane_leaf_id: terminal_pane_leaf_id,
            });
        }
        if target == Some(NATIVE_FILE_DROP_TARGET_EDITOR) {
            return Some(NativeDropResolution::Editor);
        }
        if target == Some(NATIVE_FILE_DROP_TARGET_COMPOSER) {
            return Some(NativeDropResolution::Composer);
        }
        if target == Some(NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR) {
            return Some(NativeDropResolution::ProjectSidebar);
        }
        if target == Some(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER) {
            found_explorer = true;
        }

        // L4: mixed predicate — "not yet set AND the candidate is truthy"
        // (`O:123`). An empty-string dir is skipped WITHOUT latching the
        // slot, so a later non-empty dir can still win.
        if destination_dir.is_none() {
            if let Some(dir) = &entry.native_file_drop_dir {
                if !dir.is_empty() {
                    destination_dir = Some(dir.clone());
                }
            }
        }
    }

    if found_explorer {
        return match destination_dir {
            // `!destinationDir` (`O:129`) is equivalent to `=== undefined`
            // here: L4's guard above only ever assigns a non-empty string,
            // so a `Some` value is never falsy.
            None => Some(NativeDropResolution::Rejected),
            Some(dir) => Some(NativeDropResolution::FileExplorer {
                destination_dir: dir,
            }),
        };
    }

    None
}

// ---------------------------------------------------------------------------
// Byte measurement (`O:138-144`)
// ---------------------------------------------------------------------------

/// `measureNativeFileDropPathBytes` (`O:138-144`). Note: this function has
/// neither an oracle test nor a production caller upstream (recon-flagged);
/// ported and pinned anyway for wire-format fidelity.
pub fn measure_native_file_drop_path_bytes(paths: &[String]) -> u64 {
    let mut byte_length: u64 = 0;
    for path in paths {
        byte_length += measure_clipboard_text_byte_length(path, None).byte_length;
    }
    byte_length
}

// ---------------------------------------------------------------------------
// Validation (`O:146-193`)
// ---------------------------------------------------------------------------

/// `validateNativeFileDropPaths` (`O:146-182`).
///
/// L1: `byte_length` accumulates OUTSIDE the loop across the whole path
/// list — this is a TOTAL cap, not a per-path one, despite
/// [`NATIVE_FILE_DROP_MAX_PATH_BYTES`]'s name. L2: each path's measurement
/// comes from [`measure_clipboard_text_byte_length`] (with the REMAINING
/// budget as `stop_after_bytes`), and its returned `byte_length` — a
/// possibly-truncated, overshoot-inclusive partial sum — is what gets added
/// to the accumulator, never a plain `path.len()`. L8: the too-many-paths
/// branch hard-codes `byte_length: 0` and returns before any byte
/// accounting happens at all — a different, unrelated rule. L9: both caps
/// compare with strict `>`. L6: both caps are `Option<f64>`; see the module
/// doc comment.
pub fn validate_native_file_drop_paths(
    paths: &[String],
    options: NativeFileDropValidationOptions,
) -> NativeFileDropPathValidation {
    let path_count = paths.len() as u64;
    let max_paths = options
        .max_paths
        .unwrap_or(NATIVE_FILE_DROP_MAX_PATHS as f64);
    // `pathCount > maxPaths` (`O:155`): a plain JS `>`, no separate
    // `Number.isFinite` guard needed here — Rust's `f64` comparison already
    // returns `false` against `NaN` exactly like JS, so `NaN` accepts any
    // path count for free (L6).
    if (path_count as f64) > max_paths {
        return NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
            byte_length: 0,
            path_count,
            reason: NativeFileDropRejectedReason::TooManyPaths,
        });
    }

    let max_path_bytes = options
        .max_path_bytes
        .unwrap_or(NATIVE_FILE_DROP_MAX_PATH_BYTES as f64);
    let mut byte_length: u64 = 0; // L1: lives OUTSIDE the loop — a total, not per-path.
    for path in paths {
        let remaining_budget = max_path_bytes - byte_length as f64; // L1: remaining, not the full cap.
        let measurement = measure_clipboard_text_byte_length(path, Some(remaining_budget));
        byte_length += measurement.byte_length; // L2: truncated overshoot-inclusive sum, not path.len().
        if (byte_length as f64) > max_path_bytes {
            return NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length,
                path_count,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            });
        }
    }

    // L6: this return is unconditional — an empty `paths` list never enters
    // the loop above, so it reaches here (accepted) even under a negative
    // `max_path_bytes` that would reject any non-empty path.
    NativeFileDropPathValidation::Accepted {
        byte_length,
        path_count,
    }
}

/// `createRejectedNativeFileDropPayload` (`O:184-193`). The parameter type
/// (`NativeFileDropRejectedValidation`, not the full
/// `NativeFileDropPathValidation` enum) mirrors the TS
/// `Extract<NativeFileDropPathValidation, { status: 'rejected' }>` narrowing.
pub fn create_rejected_native_file_drop_payload(
    validation: &NativeFileDropRejectedValidation,
) -> NativeFileDropRejectedPayload {
    NativeFileDropRejectedPayload {
        byte_length: validation.byte_length,
        path_count: validation.path_count,
        reason: validation.reason,
    }
}

// ---------------------------------------------------------------------------
// Payload creation (`O:195-227`)
// ---------------------------------------------------------------------------

/// Falsy-string filter for the terminal payload's conditional spread
/// (`O:221-222`: `...(resolution.tabId ? { tabId: resolution.tabId } : {})`).
/// An empty string is falsy in JS, so it is OMITTED from the payload here
/// too, not carried through as `Some("")`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

/// `createNativeFileDropPayload` (`O:195-227`).
///
/// Order matters and is preserved verbatim: the size/count validation
/// (`O:199`) is checked BEFORE the `resolution?.target === 'rejected'` check
/// (`O:204`) — a path list that is simultaneously oversized AND routed to a
/// rejected target reports the SIZE rejection, never `None`.
pub fn create_native_file_drop_payload(
    resolution: Option<NativeDropResolution>,
    paths: &[String],
) -> Option<NativeFileDropPayload> {
    let validation =
        validate_native_file_drop_paths(paths, NativeFileDropValidationOptions::default());
    if let NativeFileDropPathValidation::Rejected(rejected) = validation {
        return Some(NativeFileDropPayload::Rejected(
            create_rejected_native_file_drop_payload(&rejected),
        ));
    }

    if matches!(resolution, Some(NativeDropResolution::Rejected)) {
        return None;
    }

    if let Some(NativeDropResolution::FileExplorer { destination_dir }) = resolution.clone() {
        return Some(NativeFileDropPayload::FileExplorer {
            paths: paths.to_vec(),
            destination_dir,
        });
    }

    if let Some(NativeDropResolution::Terminal {
        tab_id,
        pane_leaf_id,
    }) = resolution.clone()
    {
        return Some(NativeFileDropPayload::Terminal {
            paths: paths.to_vec(),
            tab_id: non_empty(tab_id),
            pane_leaf_id: non_empty(pane_leaf_id),
        });
    }

    match resolution {
        Some(NativeDropResolution::Composer) => Some(NativeFileDropPayload::Composer {
            paths: paths.to_vec(),
        }),
        Some(NativeDropResolution::ProjectSidebar) => Some(NativeFileDropPayload::ProjectSidebar {
            paths: paths.to_vec(),
        }),
        // `resolution?.target ?? NATIVE_FILE_DROP_TARGET.editor` (`O:216`):
        // `None` (no resolution at all) and `Some(Editor)` both land here.
        Some(NativeDropResolution::Editor) | None => Some(NativeFileDropPayload::Editor {
            paths: paths.to_vec(),
        }),
        // Unreachable: both handled by early returns above.
        Some(NativeDropResolution::FileExplorer { .. })
        | Some(NativeDropResolution::Terminal { .. }) => {
            unreachable!(
                "FileExplorer and Terminal resolutions are handled by earlier early-returns"
            )
        }
        Some(NativeDropResolution::Rejected) => {
            unreachable!("Rejected resolutions are handled by the earlier early-return")
        }
    }
}

// ---------------------------------------------------------------------------
// Protocol-boundary guard (`O:229-269`; L10)
// ---------------------------------------------------------------------------

/// `isNativeFileDropPayload` (`O:229-269`).
pub fn is_native_file_drop_payload(value: &JsValue) -> bool {
    // `!value || typeof value !== 'object'` (`O:230`): every non-object
    // `JsValue` variant (including `Null`/`Undefined`, both falsy in JS)
    // falls through to `_ => false` below.
    let record = match value {
        JsValue::Object(record) => record,
        _ => return false,
    };

    let target = record.get("target");
    if !is_native_file_drop_target(target) {
        return false;
    }

    if str_eq(target, NATIVE_FILE_DROP_TARGET_REJECTED) {
        return is_non_negative_finite_number(record.get("byteLength"))
            && is_non_negative_finite_number(record.get("pathCount"))
            && is_native_file_drop_rejected_reason(record.get("reason"));
    }

    let paths = match is_native_file_drop_path_list(record.get("paths")) {
        Some(paths) => paths,
        None => return false,
    };
    if !matches!(
        validate_native_file_drop_paths(&paths, NativeFileDropValidationOptions::default()),
        NativeFileDropPathValidation::Accepted { .. }
    ) {
        return false;
    }

    if str_eq(target, NATIVE_FILE_DROP_TARGET_TERMINAL) {
        return is_optional_native_file_drop_string(record.get("tabId"))
            && is_optional_native_file_drop_string(record.get("paneLeafId"));
    }
    if str_eq(target, NATIVE_FILE_DROP_TARGET_FILE_EXPLORER) {
        return matches!(record.get("destinationDir"), Some(JsValue::Str(_)));
    }

    str_eq(target, NATIVE_FILE_DROP_TARGET_EDITOR)
        || str_eq(target, NATIVE_FILE_DROP_TARGET_COMPOSER)
        || str_eq(target, NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> NativeFileDropPathEntry {
        NativeFileDropPathEntry::default()
    }

    fn s(v: &str) -> String {
        v.to_string()
    }

    // -----------------------------------------------------------------
    // Oracle: `native-file-drop.test.ts` (`test:1-277`)
    // -----------------------------------------------------------------

    #[test]
    fn oracle_accepts_native_os_file_drags() {
        assert!(has_native_file_drag_types(Some(&["Files"])));
    }

    #[test]
    fn oracle_rejects_internal_orca_file_moves_and_url_text_drags() {
        assert!(!has_native_file_drag_types(Some(&[
            "Files",
            ORCA_INTERNAL_FILE_DRAG_TYPE
        ])));
        assert!(!has_native_file_drag_types(Some(&["text/uri-list"])));
        assert!(!has_native_file_drag_types(Some(&["text/plain"])));
    }

    #[test]
    fn oracle_routes_drops_on_the_left_sidebar_to_the_add_project_surface() {
        let entries = [NativeFileDropPathEntry {
            native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR)),
            ..entry()
        }];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::ProjectSidebar)
        );
    }

    #[test]
    fn oracle_preserves_terminal_tab_and_pane_routing_for_native_file_drops() {
        let entries = [
            NativeFileDropPathEntry {
                terminal_pane_leaf_id: Some(s("leaf-1")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_TERMINAL)),
                terminal_tab_id: Some(s("tab-1")),
                ..entry()
            },
        ];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Terminal {
                tab_id: Some(s("tab-1")),
                pane_leaf_id: Some(s("leaf-1")),
            })
        );
    }

    #[test]
    fn oracle_uses_the_nearest_file_explorer_destination_and_fails_closed_without_one() {
        let entries = [
            NativeFileDropPathEntry {
                native_file_drop_dir: Some(s("/repo/src")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)),
                native_file_drop_dir: Some(s("/repo")),
                ..entry()
            },
        ];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::FileExplorer {
                destination_dir: s("/repo/src"),
            })
        );

        let entries2 = [NativeFileDropPathEntry {
            native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)),
            ..entry()
        }];
        assert_eq!(
            resolve_native_file_drop_path(&entries2),
            Some(NativeDropResolution::Rejected)
        );
    }

    #[test]
    fn oracle_rejects_native_drops_by_file_count_before_path_byte_accounting_is_needed() {
        let paths: Vec<String> = (0..=NATIVE_FILE_DROP_MAX_PATHS)
            .map(|i| format!("/tmp/file-{i}"))
            .collect();
        assert_eq!(
            validate_native_file_drop_paths(&paths, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 0,
                path_count: NATIVE_FILE_DROP_MAX_PATHS + 1,
                reason: NativeFileDropRejectedReason::TooManyPaths,
            })
        );
    }

    #[test]
    fn oracle_rejects_native_drops_whose_path_list_is_too_large_without_exposing_paths() {
        let paths = vec![s("C:\\Users\\alice\\secret-token.txt")];
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(4.0),
                max_paths: None,
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 5,
                path_count: 1,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            })
        );
        if let NativeFileDropPathValidation::Rejected(rejected) = validation {
            let payload = create_rejected_native_file_drop_payload(&rejected);
            assert!(!format!("{payload:?}").contains("secret"));
            assert!(!format!("{payload:?}").contains("alice"));
        }
    }

    #[test]
    fn oracle_accepts_path_payloads_within_the_configured_limits() {
        let paths = vec![s("/tmp/a"), s("/tmp/b")];
        assert_eq!(
            validate_native_file_drop_paths(&paths, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Accepted {
                byte_length: 12,
                path_count: 2,
            }
        );
    }

    #[test]
    fn oracle_rejects_multibyte_native_path_lists_with_bounded_byte_accounting() {
        let paths = vec!["\u{1F600}".repeat(3)];
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(5.0),
                max_paths: None,
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 8,
                path_count: 1,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            })
        );
    }

    #[test]
    fn oracle_preserves_terminal_tab_and_pane_routing_in_accepted_payloads() {
        let resolution = Some(NativeDropResolution::Terminal {
            tab_id: Some(s("tab-1")),
            pane_leaf_id: Some(s("leaf-1")),
        });
        let payload = create_native_file_drop_payload(resolution, &[s("/tmp/a")]);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Terminal {
                paths: vec![s("/tmp/a")],
                tab_id: Some(s("tab-1")),
                pane_leaf_id: Some(s("leaf-1")),
            })
        );
    }

    #[test]
    fn oracle_preserves_file_explorer_destination_routing_in_accepted_payloads() {
        let resolution = Some(NativeDropResolution::FileExplorer {
            destination_dir: s("/repo/src"),
        });
        let payload = create_native_file_drop_payload(resolution, &[s("/tmp/a")]);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::FileExplorer {
                paths: vec![s("/tmp/a")],
                destination_dir: s("/repo/src"),
            })
        );
    }

    #[test]
    fn oracle_falls_back_to_editor_for_unmarked_drops_and_fails_closed_for_rejected_targets() {
        assert_eq!(
            create_native_file_drop_payload(None, &[s("/tmp/a")]),
            Some(NativeFileDropPayload::Editor {
                paths: vec![s("/tmp/a")],
            })
        );
        assert_eq!(
            create_native_file_drop_payload(Some(NativeDropResolution::Rejected), &[s("/tmp/a")]),
            None
        );
    }

    #[test]
    fn oracle_returns_metadata_only_rejected_payloads_for_oversized_path_lists() {
        let paths = vec![
            s("C:\\Users\\alice\\"),
            "a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize),
        ];
        let payload = create_native_file_drop_payload(None, &paths);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Rejected(
                NativeFileDropRejectedPayload {
                    byte_length: NATIVE_FILE_DROP_MAX_PATH_BYTES + 1,
                    path_count: 2,
                    reason: NativeFileDropRejectedReason::PathsTooLarge,
                }
            ))
        );
        assert!(!format!("{payload:?}").contains("alice"));
    }

    #[test]
    fn oracle_accepts_bounded_native_file_drop_payload_shapes() {
        assert!(is_native_file_drop_payload(&JsValue::object([
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(is_native_file_drop_payload(&JsValue::object([
            ("destinationDir", JsValue::str("/repo/src")),
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            (
                "target",
                JsValue::str(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)
            ),
        ])));
        assert!(is_native_file_drop_payload(&JsValue::object([
            ("paneLeafId", JsValue::str("leaf-1")),
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            ("tabId", JsValue::str("tab-1")),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_TERMINAL)),
        ])));
        assert!(is_native_file_drop_payload(&JsValue::object([
            ("byteLength", JsValue::number(0.0)),
            (
                "pathCount",
                JsValue::number((NATIVE_FILE_DROP_MAX_PATHS + 1) as f64)
            ),
            ("reason", JsValue::str("too-many-paths")),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_REJECTED)),
        ])));
    }

    #[test]
    fn oracle_rejects_malformed_or_unbounded_native_file_drop_payloads() {
        assert!(!is_native_file_drop_payload(&JsValue::Null));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            ("target", JsValue::str("browser")),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            (
                "target",
                JsValue::str(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)
            ),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            ("paths", JsValue::array([JsValue::str("/tmp/a")])),
            ("tabId", JsValue::number(42.0)),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_TERMINAL)),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array((0..=NATIVE_FILE_DROP_MAX_PATHS).map(|_| JsValue::str("/tmp/a")))
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array([JsValue::str(
                    "a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize + 1)
                )])
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            ("byteLength", JsValue::number(0.0)),
            ("pathCount", JsValue::number(1.0)),
            ("reason", JsValue::str("contains-secret-path")),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_REJECTED)),
        ])));
    }

    #[test]
    fn oracle_enforces_native_file_drop_count_and_byte_limits_at_their_boundaries() {
        assert!(is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array(
                    (0..NATIVE_FILE_DROP_MAX_PATHS).map(|i| JsValue::str(format!("/tmp/{i}")))
                )
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array(
                    (0..=NATIVE_FILE_DROP_MAX_PATHS).map(|i| JsValue::str(format!("/tmp/{i}")))
                )
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array([JsValue::str(
                    "a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize)
                )])
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
        assert!(!is_native_file_drop_payload(&JsValue::object([
            (
                "paths",
                JsValue::array([JsValue::str(
                    "a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize + 1)
                )])
            ),
            ("target", JsValue::str(NATIVE_FILE_DROP_TARGET_EDITOR)),
        ])));
    }

    // -----------------------------------------------------------------
    // Mandatory extra pins (oracle-silent)
    // -----------------------------------------------------------------

    /// L7: every wire literal, pinned against exact values, plus the
    /// `Object.values` insertion order.
    #[test]
    fn l7_pins_every_wire_literal_exactly() {
        assert_eq!(ORCA_INTERNAL_FILE_DRAG_TYPE, "text/x-orca-file-path");
        assert_eq!(NATIVE_FILE_DROP_MAX_PATHS, 256);
        assert_eq!(NATIVE_FILE_DROP_MAX_PATH_BYTES, 256 * 1024);
        assert_eq!(NATIVE_FILE_DROP_TARGET_EDITOR, "editor");
        assert_eq!(NATIVE_FILE_DROP_TARGET_TERMINAL, "terminal");
        assert_eq!(NATIVE_FILE_DROP_TARGET_COMPOSER, "composer");
        assert_eq!(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER, "file-explorer");
        assert_eq!(NATIVE_FILE_DROP_TARGET_PROJECT_SIDEBAR, "project-sidebar");
        assert_eq!(NATIVE_FILE_DROP_TARGET_REJECTED, "rejected");
        assert_eq!(
            NATIVE_FILE_DROP_TARGET_VALUES,
            [
                "editor",
                "terminal",
                "composer",
                "file-explorer",
                "project-sidebar"
            ]
        );
    }

    /// L1: separates the TOTAL-cap reading from a (wrong) per-path reading.
    /// Two paths of 6 bytes each with `max_path_bytes: 10`: under a per-path
    /// cap, each path individually is under 10 bytes, so a buggy per-path
    /// implementation would ACCEPT this. Under the correct total-cap
    /// reading, the accumulator reaches 6 after path 1 (fine, `<= 10`), then
    /// the second path's own measurement stops early once the REMAINING
    /// budget (`10 - 6 = 4`) is exceeded — at its 5th byte — so the final
    /// total is `6 + 5 = 11`, which exceeds the 10-byte TOTAL — REJECTED.
    #[test]
    fn l1_total_cap_vs_per_path_cap_diverge() {
        let paths = vec![s("aaaaaa"), s("bbbbbb")]; // 6 bytes each
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(10.0),
                max_paths: None,
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 11,
                path_count: 2,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            })
        );
    }

    /// L2: the size rejection's `byteLength` is the exact truncated,
    /// overshoot-inclusive total — not `paths.iter().map(String::len).sum()`
    /// (which would report `15 + 262144 = 262159` here, not `262145`).
    #[test]
    fn l2_size_rejection_reports_the_truncated_overshoot_inclusive_total() {
        let paths = vec![
            s("C:\\Users\\alice\\"),                              // 15 ASCII bytes
            "a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize), // 262144 bytes
        ];
        let validation =
            validate_native_file_drop_paths(&paths, NativeFileDropValidationOptions::default());
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: NATIVE_FILE_DROP_MAX_PATH_BYTES + 1, // 262145, not 262159/262144/262160
                path_count: 2,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            })
        );
    }

    /// L3: an empty-string pane-leaf id LATCHES (nullish semantics), unlike
    /// `||=` which would let a later truthy candidate overwrite it.
    #[test]
    fn l3_empty_string_pane_leaf_id_latches_like_nullish_not_falsy() {
        let entries = [
            NativeFileDropPathEntry {
                terminal_pane_leaf_id: Some(s("")),
                ..entry()
            },
            NativeFileDropPathEntry {
                terminal_pane_leaf_id: Some(s("leaf-2")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_TERMINAL)),
                ..entry()
            },
        ];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Terminal {
                tab_id: None,
                pane_leaf_id: Some(s("")),
            })
        );
    }

    /// L4: an empty-string dir does NOT latch, so a later non-empty dir
    /// still wins (the "last-wins" simplification would coincidentally get
    /// this one right too, but see the oracle's nearest-marker case above
    /// for where last-wins actually fails).
    #[test]
    fn l4_empty_string_dir_does_not_latch_a_later_non_empty_dir_wins() {
        let entries = [
            NativeFileDropPathEntry {
                native_file_drop_dir: Some(s("")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)),
                native_file_drop_dir: Some(s("/repo")),
                ..entry()
            },
        ];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::FileExplorer {
                destination_dir: s("/repo"),
            })
        );
    }

    /// L4 (is_some-latch simplification): if the empty-string dir HAD
    /// latched, the outcome above would instead be `Rejected` (`!""` is
    /// true in JS), diverging from the correct `FileExplorer` result. This
    /// test exists as documentation for why `l4_*` above is
    /// mutation-killable against that specific wrong simplification.
    #[test]
    fn l4_is_some_latch_simplification_would_reject_instead_of_resolving() {
        let entries = [
            NativeFileDropPathEntry {
                native_file_drop_dir: Some(s("")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_FILE_EXPLORER)),
                native_file_drop_dir: Some(s("/repo")),
                ..entry()
            },
        ];
        // The correct (faithful) result is FileExplorer, not Rejected.
        assert_ne!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Rejected)
        );
    }

    /// L6: `NaN`, `Infinity`, and negative caps all accept everything they
    /// should.
    #[test]
    fn l6_nan_infinity_and_negative_caps_accept_everything_they_should() {
        let paths = vec![s("/tmp/a"), s("/tmp/b"), s("/tmp/c")];

        // NaN max_paths: `pathCount > NaN` is always false, so it accepts
        // any path count.
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: None,
                max_paths: Some(f64::NAN),
            },
        );
        assert!(matches!(
            validation,
            NativeFileDropPathValidation::Accepted { .. }
        ));

        // NaN max_path_bytes: never stops early, never exceeds -> accepted.
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(f64::NAN),
                max_paths: None,
            },
        );
        assert!(matches!(
            validation,
            NativeFileDropPathValidation::Accepted { .. }
        ));

        // Infinity accepts everything too (both caps).
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(f64::INFINITY),
                max_paths: Some(f64::INFINITY),
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Accepted {
                byte_length: 18,
                path_count: 3,
            }
        );

        // A negative max_path_bytes rejects at the first character of the
        // first non-empty path.
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(-1.0),
                max_paths: None,
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 1,
                path_count: 3,
                reason: NativeFileDropRejectedReason::PathsTooLarge,
            })
        );
    }

    /// L6: a negative `max_path_bytes` still ACCEPTS an empty path list —
    /// the size check lives inside the loop, and the loop never runs.
    #[test]
    fn l6_negative_byte_cap_with_empty_path_list_is_still_accepted() {
        let paths: Vec<String> = vec![];
        let validation = validate_native_file_drop_paths(
            &paths,
            NativeFileDropValidationOptions {
                max_path_bytes: Some(-1.0),
                max_paths: None,
            },
        );
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Accepted {
                byte_length: 0,
                path_count: 0,
            }
        );
    }

    /// L8: the too-many-paths rejection reports `byteLength == 0` even
    /// though the paths themselves have plenty of bytes — no accounting is
    /// attempted at all.
    #[test]
    fn l8_too_many_paths_rejection_reports_byte_length_zero() {
        let paths: Vec<String> = (0..=NATIVE_FILE_DROP_MAX_PATHS)
            .map(|_| "a".repeat(1000))
            .collect();
        let validation =
            validate_native_file_drop_paths(&paths, NativeFileDropValidationOptions::default());
        assert_eq!(
            validation,
            NativeFileDropPathValidation::Rejected(NativeFileDropRejectedValidation {
                byte_length: 0,
                path_count: NATIVE_FILE_DROP_MAX_PATHS + 1,
                reason: NativeFileDropRejectedReason::TooManyPaths,
            })
        );
    }

    /// L9: exactly at the path-count cap is accepted; one over is rejected.
    #[test]
    fn l9_exactly_at_max_paths_is_accepted_one_over_is_rejected() {
        let at_cap: Vec<String> = (0..NATIVE_FILE_DROP_MAX_PATHS)
            .map(|i| format!("/{i}"))
            .collect();
        assert!(matches!(
            validate_native_file_drop_paths(&at_cap, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Accepted { .. }
        ));

        let over_cap: Vec<String> = (0..=NATIVE_FILE_DROP_MAX_PATHS)
            .map(|i| format!("/{i}"))
            .collect();
        assert!(matches!(
            validate_native_file_drop_paths(&over_cap, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Rejected(_)
        ));
    }

    /// L9: exactly at the byte-total cap is accepted; one over is rejected.
    #[test]
    fn l9_exactly_at_max_path_bytes_is_accepted_one_over_is_rejected() {
        let at_cap = vec!["a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize)];
        assert_eq!(
            validate_native_file_drop_paths(&at_cap, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Accepted {
                byte_length: NATIVE_FILE_DROP_MAX_PATH_BYTES,
                path_count: 1,
            }
        );

        let over_cap = vec!["a".repeat(NATIVE_FILE_DROP_MAX_PATH_BYTES as usize + 1)];
        assert!(matches!(
            validate_native_file_drop_paths(&over_cap, NativeFileDropValidationOptions::default()),
            NativeFileDropPathValidation::Rejected(_)
        ));
    }

    /// Zero-coverage: the `editor` resolution branch.
    #[test]
    fn pin_editor_resolution_branch() {
        let entries = [NativeFileDropPathEntry {
            native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_EDITOR)),
            ..entry()
        }];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Editor)
        );
    }

    /// Zero-coverage: the `composer` resolution branch.
    #[test]
    fn pin_composer_resolution_branch() {
        let entries = [NativeFileDropPathEntry {
            native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_COMPOSER)),
            ..entry()
        }];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Composer)
        );
        let payload = create_native_file_drop_payload(
            resolve_native_file_drop_path(&entries),
            &[s("/tmp/a")],
        );
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Composer {
                paths: vec![s("/tmp/a")],
            })
        );
    }

    /// Zero-coverage: the `project-sidebar` target end-to-end through
    /// `create_native_file_drop_payload`.
    #[test]
    fn pin_project_sidebar_payload_creation() {
        let payload = create_native_file_drop_payload(
            Some(NativeDropResolution::ProjectSidebar),
            &[s("/tmp/a")],
        );
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::ProjectSidebar {
                paths: vec![s("/tmp/a")],
            })
        );
    }

    /// Zero-coverage: `resolveNativeFileDropPath` returning `None` — the
    /// common production path (a drop with no marker anywhere in the
    /// ancestry chain).
    #[test]
    fn pin_resolve_returns_none_for_unmarked_ancestry() {
        assert_eq!(resolve_native_file_drop_path(&[]), None);
        let entries = [entry(), entry()];
        assert_eq!(resolve_native_file_drop_path(&entries), None);
    }

    /// Zero-coverage: a terminal resolution with a missing `tabId`.
    #[test]
    fn pin_terminal_resolution_with_missing_tab_id() {
        let entries = [NativeFileDropPathEntry {
            native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_TERMINAL)),
            terminal_pane_leaf_id: Some(s("leaf-1")),
            ..entry()
        }];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Terminal {
                tab_id: None,
                pane_leaf_id: Some(s("leaf-1")),
            })
        );
    }

    /// Zero-coverage: two entries both carrying a pane-leaf id — first
    /// wins (L3's nullish latch).
    #[test]
    fn pin_two_entries_with_pane_leaf_id_first_wins() {
        let entries = [
            NativeFileDropPathEntry {
                terminal_pane_leaf_id: Some(s("leaf-first")),
                ..entry()
            },
            NativeFileDropPathEntry {
                terminal_pane_leaf_id: Some(s("leaf-second")),
                ..entry()
            },
            NativeFileDropPathEntry {
                native_file_drop_target: Some(s(NATIVE_FILE_DROP_TARGET_TERMINAL)),
                ..entry()
            },
        ];
        assert_eq!(
            resolve_native_file_drop_path(&entries),
            Some(NativeDropResolution::Terminal {
                tab_id: None,
                pane_leaf_id: Some(s("leaf-first")),
            })
        );
    }

    /// Zero-coverage: the conditional-spread OMISSION arm — an empty-string
    /// `tabId`/`paneLeafId` on the resolution is dropped from the payload
    /// entirely (`None`), not carried through as `Some("")`.
    #[test]
    fn pin_conditional_spread_omission_arm_drops_empty_strings() {
        let resolution = Some(NativeDropResolution::Terminal {
            tab_id: Some(s("")),
            pane_leaf_id: Some(s("")),
        });
        let payload = create_native_file_drop_payload(resolution, &[s("/tmp/a")]);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Terminal {
                paths: vec![s("/tmp/a")],
                tab_id: None,
                pane_leaf_id: None,
            })
        );
    }

    /// Zero-coverage: a too-many-paths rejection reaching
    /// `create_native_file_drop_payload`, not just
    /// `validate_native_file_drop_paths` directly.
    #[test]
    fn pin_too_many_paths_reaching_payload_creation() {
        let paths: Vec<String> = (0..=NATIVE_FILE_DROP_MAX_PATHS)
            .map(|i| format!("/tmp/{i}"))
            .collect();
        let payload = create_native_file_drop_payload(None, &paths);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Rejected(
                NativeFileDropRejectedPayload {
                    byte_length: 0,
                    path_count: NATIVE_FILE_DROP_MAX_PATHS + 1,
                    reason: NativeFileDropRejectedReason::TooManyPaths,
                }
            ))
        );
    }

    /// Zero-coverage: the ordering between the size check and the
    /// rejected-target check. Both apply here (an oversized path list AND a
    /// `Rejected` resolution) — the SIZE rejection must win, never `None`.
    #[test]
    fn pin_size_check_runs_before_rejected_target_check() {
        let paths: Vec<String> = (0..=NATIVE_FILE_DROP_MAX_PATHS)
            .map(|i| format!("/tmp/{i}"))
            .collect();
        let payload = create_native_file_drop_payload(Some(NativeDropResolution::Rejected), &paths);
        assert_eq!(
            payload,
            Some(NativeFileDropPayload::Rejected(
                NativeFileDropRejectedPayload {
                    byte_length: 0,
                    path_count: NATIVE_FILE_DROP_MAX_PATHS + 1,
                    reason: NativeFileDropRejectedReason::TooManyPaths,
                }
            ))
        );
    }

    /// `measureNativeFileDropPathBytes` — no oracle test and no production
    /// caller upstream, pinned anyway.
    #[test]
    fn pin_measure_native_file_drop_path_bytes() {
        assert_eq!(
            measure_native_file_drop_path_bytes(&[s("/tmp/a"), s("/tmp/bb")]),
            13
        );
        assert_eq!(measure_native_file_drop_path_bytes(&[]), 0);
        assert_eq!(measure_native_file_drop_path_bytes(&[s("\u{1F600}")]), 4);
    }
}
