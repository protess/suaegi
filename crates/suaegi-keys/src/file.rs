//! The on-disk keybindings file layer: read + parse + diagnose a user's
//! `keybindings.json`, and write a single override back atomically.
//!
//! Ported from Orca `src/main/keybindings/keybinding-file.ts` (@ v1.4.150-rc.0):
//!   - the document shape + `FILE_VERSION` (`:21`, `:33-43`)
//!   - `readKeybindingFile` (`:248`) -> [`KeybindingFileSnapshot`]
//!   - `parseBindingSection` / `parsePlatformOverrides` (`:129`, `:176`)
//!   - `removeConflictingOverrides` — the bounded drop-conflicts fixpoint (`:212`)
//!   - `writeKeybindingOverride` (`:426`) + `writeActivePlatformSection` (`:385`)
//!   - the atomic write `writeJsonDocument` (`:68-84`), reimplemented here on
//!     `tempfile` (plan F3: leaf isolation — no `suaegi-core::Store` dependency).
//!
//! ## Design constraints (leaf isolation + testability)
//!
//! Every function takes the file **path** as an argument. This crate never
//! resolves `dirs::config_dir()` — that would add a `dirs` dependency and OS
//! coupling to a pure leaf. The app (M6) resolves
//! `<config_dir>/suaegi/keybindings.json` and passes it in, which keeps the
//! whole layer fully tempdir-testable.
//!
//! ## Intentional divergences from Orca (surfaced for review)
//!
//!   - **Two legacy migrations skipped.** Orca's `migrateLegacyKeybindings`
//!     (`:302`) and `seedLegacyTabSwitchBindings` (`:335-381`) exist only to
//!     upgrade a pre-existing on-disk format from before the tab-switch chord
//!     swap. suaegi is a fresh clone with no prior on-disk state, so neither can
//!     ever trigger and neither is portable to a meaningful mutation test. They
//!     are deliberately omitted. The *legacy flat-root tolerance* they relied on
//!     (`readKeybindingFile :273-276`, `writeActivePlatformSection :399-406`) IS
//!     ported — a hand-authored old-shape file still reads and, on the next
//!     write, gets folded into the `keybindings` section.
//!   - **Typed `action` + `bindings` params.** Orca's `writeKeybindingOverride`
//!     takes `actionId: string` / `bindings: unknown` and guards them at runtime
//!     ("Unknown keybinding action", "Use a string array or null.") because the
//!     value crosses an untyped IPC boundary. Here the caller (M6) is Rust, so
//!     the action is a [`KeybindingActionId`] and the bindings are
//!     `Option<&[String]>` (`None` = reset). Those two runtime string-shape
//!     guards have no Rust analog and are elided by the type system; the binding
//!     *content* is still normalized and can still be rejected
//!     ([`WriteError::InvalidBinding`]).
//!   - **Diagnostic ordering not pinned.** Matching Orca's exact per-section
//!     diagnostic order would require the `preserve_order` serde_json feature,
//!     which — via workspace feature unification — would flip every crate's
//!     `serde_json::Map` from `BTreeMap` to `IndexMap`. That cross-crate side
//!     effect is not worth an unobservable ordering detail, so the tests assert
//!     the diagnostic *set* (severities + messages) rather than the sequence.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::conflicts::find_keybinding_conflicts;
use crate::format::format_keybinding_list;
use crate::normalize::{
    normalize_keybinding_array_for_action, normalize_keybinding_list_for_action,
};
use crate::registry::KeybindingActionId;
use crate::registry::KeybindingPlatform;
use crate::resolve::KeybindingOverrides;

/// The on-disk document version. Mirror of Orca `FILE_VERSION` (`:21`).
const FILE_VERSION: u64 = 1;

/// The three platform section keys. Mirror of Orca `PLATFORM_KEYS` (`:22`).
const PLATFORM_KEYS: [KeybindingPlatform; 3] = [
    KeybindingPlatform::Darwin,
    KeybindingPlatform::Linux,
    KeybindingPlatform::Win32,
];

/// Reserved root keys skipped when tolerating the legacy flat-root shape. Mirror
/// of Orca `ROOT_KEYS` (`:23`).
const ROOT_KEYS: [&str; 4] = ["$schema", "version", "keybindings", "platforms"];

/// The lowercase on-disk key for a platform section (`darwin` / `linux` /
/// `win32`). Matches the serde `rename_all = "lowercase"` on
/// [`KeybindingPlatform`] but is spelled out locally so the file layer never
/// round-trips through serde just to name a section.
fn platform_key(platform: KeybindingPlatform) -> &'static str {
    match platform {
        KeybindingPlatform::Darwin => "darwin",
        KeybindingPlatform::Linux => "linux",
        KeybindingPlatform::Win32 => "win32",
    }
}

/// Parse a platform section key back to its variant, or `None` for an unknown
/// platform (which becomes a diagnostic, not a hard error).
fn platform_from_key(key: &str) -> Option<KeybindingPlatform> {
    PLATFORM_KEYS
        .into_iter()
        .find(|platform| platform_key(*platform) == key)
}

/// Whether a diagnostic is a hard rejection or an advisory. Mirror of Orca's
/// `severity: 'error' | 'warning'` (`keybindings.ts` `KeybindingFileDiagnostic`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The value was rejected (bad shape, unparseable chord, dropped conflict).
    Error,
    /// The value was ignored but is benign (an unknown action / platform key).
    Warning,
}

/// One thing the parser noticed while reading a file. Mirror of Orca
/// `KeybindingFileDiagnostic`: a severity, an optional `section` (`"keybindings"`,
/// `"platforms.linux"`, `"root"`) and optional raw `action_id`, plus a
/// human-readable message. Diagnostics never abort the read — a present-but-
/// malformed file yields diagnostics and an otherwise-usable snapshot, never a
/// panic and never a silently-wrong parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub section: Option<String>,
    /// The raw action-id string. Kept as a `String` because an *unknown* action
    /// (the common diagnostic subject) is by definition not a [`KeybindingActionId`].
    pub action_id: Option<String>,
    pub message: String,
}

/// The parsed result of reading a keybindings file. Mirror of Orca
/// `KeybindingFileSnapshot`: the common (cross-platform) overrides, the
/// per-platform sections, the merged **effective** overrides for the requested
/// platform (common + active platform, with conflicting entries dropped), and
/// the diagnostics gathered along the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingFileSnapshot {
    pub path: PathBuf,
    pub platform: KeybindingPlatform,
    /// Whether the file existed on disk. `false` yields the default (empty)
    /// snapshot with no diagnostics.
    pub exists: bool,
    /// The merged effective overrides for [`Self::platform`]: common overlaid
    /// with the active-platform section, then run through the drop-conflicts
    /// fixpoint. This is what a resolver consumes.
    pub overrides: KeybindingOverrides,
    /// The cross-platform (`keybindings` section, or legacy flat root) overrides.
    pub common_overrides: KeybindingOverrides,
    /// The per-platform sections, keyed by platform.
    pub platform_overrides: HashMap<KeybindingPlatform, KeybindingOverrides>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a [`write_keybinding_override`] call was rejected without touching disk
/// (except [`WriteError::Io`], which can surface a partial-write failure).
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The requested bindings failed normalization (unparseable chord, missing
    /// modifier, non-digit chord for a digit-index action, ...). Nothing is
    /// written. Carries the underlying [`crate::InvalidReason`].
    #[error("Shortcut for \"{}\" is invalid: {reason}", action.as_str())]
    InvalidBinding {
        action: KeybindingActionId,
        reason: crate::InvalidReason,
    },
    /// The bindings would collide with another effective shortcut. Nothing is
    /// written. `binding` is the platform-formatted representative chord.
    #[error("{binding} conflicts with another shortcut.")]
    Conflict { binding: String },
    /// The existing file could not be parsed. A write must never replace a
    /// user-owned file it could not read (Orca `:394-396`); the caller repairs
    /// the file and retries. Nothing is written.
    #[error("Could not read keybindings file: {0}")]
    Unreadable(String),
    /// The atomic write itself failed (create-dir / temp / rename / fsync).
    #[error("Could not write keybindings file: {0}")]
    Io(#[from] std::io::Error),
}

/// The raw JSON document, distinguishing the three states the file layer treats
/// differently: absent (defaults, no error), malformed (a diagnostic on read /
/// a hard `Unreadable` on write — never a clobber), and a well-formed object.
enum RawDocument {
    Missing,
    Malformed(String),
    Object(Map<String, Value>),
}

/// Read + classify the file. Shared by [`read_keybinding_file`] (missing =>
/// empty snapshot, malformed => diagnostic) and the write path (missing =>
/// start from an empty document, malformed => refuse to clobber). Mirror of Orca
/// `readJsonDocument` (`:45-66`).
fn read_json_document(path: &Path) -> RawDocument {
    if !path.exists() {
        return RawDocument::Missing;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return RawDocument::Malformed(error.to_string()),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => RawDocument::Object(map),
        // Parsed, but not a JSON object (array / scalar). Mirror of Orca `:55-56`.
        Ok(_) => RawDocument::Malformed("Keybindings file must contain a JSON object.".to_string()),
        Err(error) => RawDocument::Malformed(error.to_string()),
    }
}

/// The empty default document. Mirror of Orca `createEmptyDocument` (`:33-43`).
fn create_empty_document() -> Map<String, Value> {
    let mut platforms = Map::new();
    platforms.insert("darwin".to_string(), Value::Object(Map::new()));
    platforms.insert("linux".to_string(), Value::Object(Map::new()));
    platforms.insert("win32".to_string(), Value::Object(Map::new()));
    let mut document = Map::new();
    document.insert("version".to_string(), Value::from(FILE_VERSION));
    document.insert("keybindings".to_string(), Value::Object(Map::new()));
    document.insert("platforms".to_string(), Value::Object(platforms));
    document
}

/// Atomically write `document` to `path`: create the parent dir, write into a
/// sibling temp file, fsync, then rename over the target. Own `tempfile`-based
/// implementation (plan F3) — deliberately NOT `suaegi-core::Store`, to keep the
/// crate a leaf. Mirror of Orca `writeJsonDocument` (`:68-84`).
///
/// The temp file is created in the target's own directory (`new_in(parent)`) so
/// the final `persist` is a same-filesystem rename — the atomicity guarantee.
fn write_json_document(path: &Path, document: &Map<String, Value>) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    let mut json = serde_json::to_string_pretty(&Value::Object(document.clone()))
        .map_err(std::io::Error::other)?;
    json.push('\n');

    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(json.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Convert a typed overrides map into its JSON section form (each list becomes a
/// JSON string array). Used to fold the legacy flat-root common overrides into
/// the `keybindings` section on write.
fn overrides_to_json(overrides: &KeybindingOverrides) -> Map<String, Value> {
    overrides
        .iter()
        .map(|(action, bindings)| {
            let array = bindings
                .iter()
                .map(|chord| Value::String(chord.clone()))
                .collect();
            (action.as_str().to_string(), Value::Array(array))
        })
        .collect()
}

/// Normalize one raw binding value from a section into a canonical chord list.
/// Mirror of Orca `normalizeBindingValue` (`:86-113`): `null`/`false` disable the
/// action (empty list); a string is a comma-separated list; an array is merged.
/// The `Err` carries the human message for the diagnostic.
fn normalize_binding_value(
    action: KeybindingActionId,
    value: &Value,
) -> Result<Vec<String>, String> {
    match value {
        Value::Null | Value::Bool(false) => Ok(Vec::new()),
        Value::String(text) => {
            normalize_keybinding_list_for_action(action, text).map_err(|reason| reason.to_string())
        }
        Value::Array(items) => {
            let strings: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
            match strings {
                Some(list) => normalize_keybinding_array_for_action(action, &list)
                    .map_err(|reason| reason.to_string()),
                None => Err("Use a string, string array, null, or false.".to_string()),
            }
        }
        _ => Err("Use a string, string array, null, or false.".to_string()),
    }
}

/// Parse one overrides section (`keybindings`, a `platforms.*` section, or the
/// legacy flat root) into typed overrides, appending a diagnostic for every
/// unknown action or invalid value. Mirror of Orca `parseBindingSection`
/// (`:129-174`). `skip_root_keys` drops the reserved [`ROOT_KEYS`] when the whole
/// document root is being read as the common section.
fn parse_binding_section(
    value: &Value,
    section: &str,
    diagnostics: &mut Vec<Diagnostic>,
    skip_root_keys: bool,
) -> KeybindingOverrides {
    let Some(object) = value.as_object() else {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            section: Some(section.to_string()),
            action_id: None,
            message: format!("{section} must be an object."),
        });
        return KeybindingOverrides::new();
    };

    let mut overrides = KeybindingOverrides::new();
    for (raw_action, raw_binding) in object {
        if skip_root_keys && ROOT_KEYS.contains(&raw_action.as_str()) {
            continue;
        }
        let Some(action) = KeybindingActionId::from_id(raw_action) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                section: Some(section.to_string()),
                action_id: Some(raw_action.clone()),
                message: format!("Unknown keybinding action \"{raw_action}\" was ignored."),
            });
            continue;
        };
        match normalize_binding_value(action, raw_binding) {
            Ok(bindings) => {
                overrides.insert(action, bindings);
            }
            Err(error) => {
                diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    section: Some(section.to_string()),
                    action_id: Some(raw_action.clone()),
                    message: format!("Shortcut for \"{raw_action}\" was ignored: {error}"),
                });
            }
        }
    }
    overrides
}

/// Parse the `platforms` object into per-platform overrides, warning on any
/// unknown platform key. Mirror of Orca `parsePlatformOverrides` (`:176-210`).
fn parse_platform_overrides(
    document: &Map<String, Value>,
    diagnostics: &mut Vec<Diagnostic>,
) -> HashMap<KeybindingPlatform, KeybindingOverrides> {
    let Some(raw_platforms) = document.get("platforms") else {
        return HashMap::new();
    };
    let Some(object) = raw_platforms.as_object() else {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            section: Some("platforms".to_string()),
            action_id: None,
            message: "platforms must be an object with darwin, linux, or win32 sections."
                .to_string(),
        });
        return HashMap::new();
    };

    let mut result = HashMap::new();
    for (raw_platform, value) in object {
        let Some(platform) = platform_from_key(raw_platform) else {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                section: Some(format!("platforms.{raw_platform}")),
                action_id: None,
                message: format!("Unknown platform \"{raw_platform}\" was ignored."),
            });
            continue;
        };
        let section = format!("platforms.{raw_platform}");
        result.insert(
            platform,
            parse_binding_section(value, &section, diagnostics, false),
        );
    }
    result
}

/// The bounded drop-conflicts fixpoint. Repeatedly finds conflicts among the
/// merged overrides and removes every override participating in one, appending a
/// diagnostic each round, until a round finds no conflicting override. Mirror of
/// Orca `removeConflictingOverrides` (`:212-246`).
///
/// **Termination.** The loop is capped at 20 iterations (Orca `:218`). Each
/// productive round removes at least one entry from a finite map, so the fixpoint
/// converges in at most `overrides.len()` rounds; the cap is a belt-and-braces
/// bound that guarantees the function returns even on a pathological input rather
/// than spinning. When it converges, the round that finds nothing returns
/// immediately from inside the loop.
fn remove_conflicting_overrides(
    platform: KeybindingPlatform,
    overrides: KeybindingOverrides,
    diagnostics: &mut Vec<Diagnostic>,
) -> KeybindingOverrides {
    let mut next = overrides;
    for _ in 0..20 {
        let conflicts = find_keybinding_conflicts(platform, Some(&next));

        // First-seen order across all conflicts, deduped — mirrors Orca's `Set`.
        let mut conflicting: Vec<KeybindingActionId> = Vec::new();
        for conflict in &conflicts {
            for action in &conflict.action_ids {
                if next.contains_key(action) && !conflicting.contains(action) {
                    conflicting.push(*action);
                }
            }
        }

        if conflicting.is_empty() {
            return next;
        }

        for action in &conflicting {
            next.remove(action);
        }

        let titles = conflicting
            .iter()
            .map(|action| {
                action
                    .definition()
                    .map(|def| def.title)
                    .unwrap_or_else(|| action.as_str())
            })
            .collect::<Vec<_>>()
            .join(", ");
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            section: None,
            action_id: None,
            message: format!("Conflicting custom shortcuts were ignored: {titles}."),
        });
    }
    next
}

/// Read + parse the keybindings file at `path`, resolved for `platform`. Mirror
/// of Orca `readKeybindingFile` (`:248-293`).
///
/// A missing file yields an empty snapshot with no diagnostics (a first launch is
/// not an error). A present-but-malformed file yields an empty snapshot carrying
/// one error diagnostic — never a panic and never a silent "no overrides": a
/// transient/parse issue is always visible as a diagnostic. A well-formed file
/// has each section normalized (invalid entries dropped WITH a diagnostic), the
/// common + active-platform sections merged, and conflicting entries removed by
/// the [`remove_conflicting_overrides`] fixpoint.
pub fn read_keybinding_file(path: &Path, platform: KeybindingPlatform) -> KeybindingFileSnapshot {
    let empty = |exists: bool, diagnostics: Vec<Diagnostic>| KeybindingFileSnapshot {
        path: path.to_path_buf(),
        platform,
        exists,
        overrides: KeybindingOverrides::new(),
        common_overrides: KeybindingOverrides::new(),
        platform_overrides: HashMap::new(),
        diagnostics,
    };

    let document = match read_json_document(path) {
        RawDocument::Missing => return empty(false, Vec::new()),
        RawDocument::Malformed(error) => {
            return empty(
                true,
                vec![Diagnostic {
                    severity: Severity::Error,
                    section: None,
                    action_id: None,
                    message: format!("Could not read keybindings file: {error}"),
                }],
            );
        }
        RawDocument::Object(document) => document,
    };

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let root = Value::Object(document.clone());
    // Legacy flat-root tolerance (Orca `:273-276`): a file with no `keybindings`
    // key is an old flat-shape document — read its whole root as the common
    // section, skipping the reserved keys.
    let common_overrides = match document.get("keybindings") {
        None => parse_binding_section(&root, "root", &mut diagnostics, true),
        Some(section) => parse_binding_section(section, "keybindings", &mut diagnostics, false),
    };
    let platform_overrides = parse_platform_overrides(&document, &mut diagnostics);

    let mut merged = common_overrides.clone();
    if let Some(active) = platform_overrides.get(&platform) {
        for (action, bindings) in active {
            merged.insert(*action, bindings.clone());
        }
    }
    let overrides = remove_conflicting_overrides(platform, merged, &mut diagnostics);

    KeybindingFileSnapshot {
        path: path.to_path_buf(),
        platform,
        exists: true,
        overrides,
        common_overrides,
        platform_overrides,
        diagnostics,
    }
}

/// Merge `normalized` (or a reset) into the active platform section and write the
/// document atomically, preserving the common section and all other platforms.
/// Mirror of Orca `writeActivePlatformSection` (`:385-424`).
///
/// `fallback_common` is used only when the on-disk document has no `keybindings`
/// object (the legacy flat-root case): its parsed common overrides are folded
/// into the `keybindings` section so the next read is canonical.
fn write_active_platform_section(
    path: &Path,
    platform: KeybindingPlatform,
    fallback_common: &KeybindingOverrides,
    action: KeybindingActionId,
    normalized: Option<&[String]>,
) -> Result<(), WriteError> {
    let mut document = match read_json_document(path) {
        RawDocument::Missing => create_empty_document(),
        // A write must never replace a file it could not read (Orca `:394-396`).
        RawDocument::Malformed(error) => return Err(WriteError::Unreadable(error)),
        RawDocument::Object(document) => document,
    };

    // The common section: keep an existing `keybindings` object, else fold in the
    // parsed flat-root common overrides (legacy migration on write).
    let common = match document.get("keybindings") {
        Some(Value::Object(existing)) => existing.clone(),
        _ => overrides_to_json(fallback_common),
    };

    // Strip any legacy flat-root action keys now that they live under `keybindings`.
    let flat_root_actions: Vec<String> = document
        .keys()
        .filter(|key| KeybindingActionId::from_id(key).is_some())
        .cloned()
        .collect();
    for key in flat_root_actions {
        document.remove(&key);
    }

    // The platforms object, preserving any unknown platform keys already present.
    let mut platforms = match document.get("platforms") {
        Some(Value::Object(existing)) => existing.clone(),
        _ => Map::new(),
    };
    let active_key = platform_key(platform);
    let mut active_platform = match platforms.get(active_key) {
        Some(Value::Object(existing)) => existing.clone(),
        _ => Map::new(),
    };

    // The one mutation: set (or reset) this action in the active platform only.
    // A reset removes just the platform-specific mask; a hand-authored common
    // binding for other OSes is intentionally left untouched (Orca `:459-466`).
    match normalized {
        Some(bindings) => {
            let array = bindings
                .iter()
                .map(|chord| Value::String(chord.clone()))
                .collect();
            active_platform.insert(action.as_str().to_string(), Value::Array(array));
        }
        None => {
            active_platform.remove(action.as_str());
        }
    }

    // Reassemble, coercing the three known platform keys to objects and writing
    // the mutated active section back. Unknown platform keys survive via `platforms`.
    for known in PLATFORM_KEYS {
        let key = platform_key(known);
        if !platforms.get(key).is_some_and(Value::is_object) {
            platforms.insert(key.to_string(), Value::Object(Map::new()));
        }
    }
    platforms.insert(active_key.to_string(), Value::Object(active_platform));

    document.insert("version".to_string(), Value::from(FILE_VERSION));
    document.insert("keybindings".to_string(), Value::Object(common));
    document.insert("platforms".to_string(), Value::Object(platforms));

    write_json_document(path, &document)?;
    Ok(())
}

/// Write a single action's override to `path` for the active `platform`, or reset
/// it (`bindings == None`). Mirror of Orca `writeKeybindingOverride` (`:426-469`).
///
/// The bindings are normalized first (rejecting invalid chords), then checked for
/// conflicts against the file's current effective overrides — a write that would
/// collide with another shortcut is refused and nothing is written. On success
/// the value is written into the **active platform section only**, never the
/// common section or another platform, via an atomic temp+rename. The passed-in
/// `path` is the only file ever touched — a user's global config is never written.
pub fn write_keybinding_override(
    path: &Path,
    action: KeybindingActionId,
    bindings: Option<&[String]>,
    platform: KeybindingPlatform,
) -> Result<(), WriteError> {
    // Normalize the requested bindings (a reset stays `None`).
    let normalized: Option<Vec<String>> = match bindings {
        None => None,
        Some(list) => {
            let refs: Vec<&str> = list.iter().map(String::as_str).collect();
            match normalize_keybinding_array_for_action(action, &refs) {
                Ok(canonical) => Some(canonical),
                Err(reason) => return Err(WriteError::InvalidBinding { action, reason }),
            }
        }
    };

    // Build the candidate effective overrides and refuse a conflicting write.
    let current = read_keybinding_file(path, platform);
    let mut candidate = current.overrides.clone();
    match &normalized {
        Some(canonical) => {
            candidate.insert(action, canonical.clone());
        }
        None => {
            candidate.remove(&action);
        }
    }
    let blocking = find_keybinding_conflicts(platform, Some(&candidate))
        .into_iter()
        .find(|conflict| conflict.action_ids.contains(&action));
    if let Some(conflict) = blocking {
        return Err(WriteError::Conflict {
            binding: format_keybinding_list(&[conflict.binding.as_str()], platform),
        });
    }

    write_active_platform_section(
        path,
        platform,
        &current.common_overrides,
        action,
        normalized.as_deref(),
    )
}

#[cfg(test)]
mod tests;
