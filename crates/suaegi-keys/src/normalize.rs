//! Normalize + validate a chord string, and the digit-index (`1`-`9` -> `1`)
//! canonicalization. This is the layer that turns "what the user typed" into a
//! stable canonical chord, applying the universal validity rules plus per-action
//! `allow_bare` / `allow_shift_only` / digit-index rules.
//!
//! Ported from Orca `src/shared/keybindings.ts`:
//!   - `KeybindingValidationResult` (:186) — modeled as a Rust **enum** with a
//!     typed rejection reason (not a `Result<String, String>` overload).
//!   - `isSafeBareKey` (:1351), `normalizeKeybindingWithOptions` (:1377)
//!   - `normalizeKeybinding` (:1414), `normalizeKeybindingList*` (:1443,:1422)
//!   - `normalizeOptionsForAction` (:1466)
//!   - `canonicalizeDigitIndexBinding` (:1475), `finalizeDigitIndexBindings` (:1486)
//!   - `normalizeKeybindingListForAction` (:1506), `normalizeKeybindingArrayForAction` (:1516)
//!
//! The digit-index key pattern is Orca `DIGIT_INDEX_KEY_PATTERN = /^[1-9]$/`
//! (`keybindings.ts:1095`).

use crate::chord::{
    canonicalize_parsed_keybinding, is_function_key_token, parse_keybinding, ParseError,
    ParsedKeybinding,
};
use crate::registry::{is_digit_index_action_id, KeybindingActionId};

/// Why a chord failed validation. Orca collapses every rejection into a single
/// `{ ok: false, error: string }` shape; splitting the causes into a typed enum
/// keeps the Orca-identical human message (via `Display`) while giving mutation
/// tests a specific variant to target. Mirror of the `error` strings emitted by
/// `normalizeKeybindingWithOptions` / `canonicalizeDigitIndexBinding`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidReason {
    /// The string did not parse as a chord at all (Orca: parse returned `null`).
    #[error("Use a shortcut like Ctrl+Shift+P or Cmd+K.")]
    Unparsable(#[source] ParseError),
    /// The virtual `Mod` was combined with a physical primary modifier
    /// (Cmd/Ctrl). Orca `keybindings.ts:1385-1386`.
    #[error("Use either Mod or a platform-specific modifier, not both.")]
    ModWithPlatformModifier,
    /// A bare / shift-only chord with no qualifying modifier, for an action that
    /// does not opt into it. Orca `keybindings.ts:1409`.
    #[error("Include at least one modifier key.")]
    MissingModifier,
    /// A digit-index action was given a chord whose key is not `1`-`9` (or was a
    /// double-tap / unparsable). Orca `keybindings.ts:1480`.
    #[error("Pick a number key 1\u{2013}9 with a modifier, like Cmd+1 or Ctrl+1.")]
    NotDigitIndexKey,
    /// A capture ([`keybinding_from_input`](crate::keybinding_from_input)) saw
    /// only modifier keys, no pressed key. Orca `keybindings.ts:1713`. This
    /// reason is produced by the M3 resolver's capture path, never by
    /// normalize/validate.
    #[error("Press a key, not only a modifier.")]
    PressAKey,
}

/// The result of validating a single chord. Rust enum mirror of Orca's
/// discriminated `KeybindingValidationResult` union (`keybindings.ts:186`):
/// `{ ok: true; value } | { ok: false; error }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindingValidationResult {
    /// The chord is valid; `canonical` is its canonical string form.
    Valid { canonical: String },
    /// The chord is rejected, with a typed reason.
    Invalid { reason: InvalidReason },
}

impl KeybindingValidationResult {
    /// Whether this is the `Valid` variant (Orca `result.ok === true`).
    pub fn is_valid(&self) -> bool {
        matches!(self, KeybindingValidationResult::Valid { .. })
    }

    /// The canonical chord if valid (Orca `result.value`).
    pub fn canonical(&self) -> Option<&str> {
        match self {
            KeybindingValidationResult::Valid { canonical } => Some(canonical),
            KeybindingValidationResult::Invalid { .. } => None,
        }
    }

    /// The rejection reason if invalid (Orca `result.error`).
    pub fn reason(&self) -> Option<&InvalidReason> {
        match self {
            KeybindingValidationResult::Valid { .. } => None,
            KeybindingValidationResult::Invalid { reason } => Some(reason),
        }
    }
}

/// A normalized, de-duplicated list of canonical chords, or the first rejection
/// encountered. Rust mirror of Orca's `KeybindingValidationResult | string[]`
/// union return: the `string[]` branch becomes `Ok(Vec<String>)`, the
/// `{ ok: false }` branch becomes `Err(InvalidReason)`.
pub type KeybindingListResult = Result<Vec<String>, InvalidReason>;

/// Per-action normalize options. Mirror of Orca `NormalizeKeybindingOptions`
/// (`keybindings.ts:181`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NormalizeOptions {
    pub(crate) allow_bare: bool,
    pub(crate) allow_shift_only: bool,
}

/// Whether a bare (modifier-less) key is safe to bind: function keys, and a set
/// of navigation/edit keys that produce no text. Mirror of Orca `isSafeBareKey`
/// (`keybindings.ts:1351-1375`). A `Shift`+letter stays unsafe; a `Shift`+
/// function-key is safe.
fn is_safe_bare_key(parsed: &ParsedKeybinding) -> bool {
    if parsed.is_mod || parsed.meta || parsed.control || parsed.alt {
        return false;
    }
    // Function keys produce no text, so they're safe bare or with Shift
    // (Shift+letter stays unsafe).
    if parsed.shift {
        return is_function_key_token(&parsed.key);
    }
    is_function_key_token(&parsed.key)
        || matches!(
            parsed.key.as_str(),
            "Backspace"
                | "Delete"
                | "Enter"
                | "Escape"
                | "Tab"
                | "ArrowLeft"
                | "ArrowRight"
                | "ArrowUp"
                | "ArrowDown"
                | "PageUp"
                | "PageDown"
        )
}

/// Validate + canonicalize a single chord under `options`. Mirror of Orca
/// `normalizeKeybindingWithOptions` (`keybindings.ts:1377-1412`).
pub(crate) fn normalize_keybinding_with_options(
    binding: &str,
    options: NormalizeOptions,
) -> KeybindingValidationResult {
    let parsed = match parse_keybinding(binding) {
        Ok(parsed) => parsed,
        Err(error) => {
            return KeybindingValidationResult::Invalid {
                reason: InvalidReason::Unparsable(error),
            };
        }
    };
    if parsed.is_mod && (parsed.meta || parsed.control) {
        return KeybindingValidationResult::Invalid {
            reason: InvalidReason::ModWithPlatformModifier,
        };
    }
    if parsed.double_tap_modifier.is_some() {
        return KeybindingValidationResult::Valid {
            canonical: canonicalize_parsed_keybinding(&parsed),
        };
    }
    let is_shift_insert = parsed.shift && parsed.key == "Insert";
    let is_bare_allowed = options.allow_bare && is_safe_bare_key(&parsed);
    let is_shift_only_allowed = options.allow_shift_only
        && parsed.shift
        && !parsed.is_mod
        && !parsed.meta
        && !parsed.control
        && !parsed.alt;
    if !parsed.is_mod
        && !parsed.meta
        && !parsed.control
        && !parsed.alt
        && !is_shift_insert
        && !is_bare_allowed
        && !is_shift_only_allowed
    {
        return KeybindingValidationResult::Invalid {
            reason: InvalidReason::MissingModifier,
        };
    }
    KeybindingValidationResult::Valid {
        canonical: canonicalize_parsed_keybinding(&parsed),
    }
}

/// Validate + canonicalize a single chord under the context-free (universal)
/// rules. Mirror of Orca `normalizeKeybinding` (`keybindings.ts:1414`).
pub fn normalize_keybinding(binding: &str) -> KeybindingValidationResult {
    normalize_keybinding_with_options(binding, NormalizeOptions::default())
}

/// Normalize a comma-separated list of chords, de-duplicating by canonical form
/// (first spelling wins) and returning the first rejection. Mirror of Orca
/// `normalizeKeybindingListWithOptions` (`keybindings.ts:1422-1441`).
fn normalize_keybinding_list_with_options(
    input: &str,
    options: NormalizeOptions,
) -> KeybindingListResult {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut normalized: Vec<String> = Vec::new();
    for piece in trimmed.split(',') {
        match normalize_keybinding_with_options(piece, options) {
            KeybindingValidationResult::Valid { canonical } => {
                if !normalized.contains(&canonical) {
                    normalized.push(canonical);
                }
            }
            KeybindingValidationResult::Invalid { reason } => return Err(reason),
        }
    }
    Ok(normalized)
}

/// Normalize a comma-separated chord list under the universal rules. Mirror of
/// Orca `normalizeKeybindingList` (`keybindings.ts:1443`).
pub fn normalize_keybinding_list(input: &str) -> KeybindingListResult {
    normalize_keybinding_list_with_options(input, NormalizeOptions::default())
}

/// Normalize an array of chords (each element may itself be comma-separated),
/// merged and de-duplicated. Mirror of Orca `normalizeKeybindingArrayWithOptions`
/// (`keybindings.ts:1447-1464`).
fn normalize_keybinding_array_with_options(
    input: &[&str],
    options: NormalizeOptions,
) -> KeybindingListResult {
    let mut normalized: Vec<String> = Vec::new();
    for binding in input {
        let pieces = normalize_keybinding_list_with_options(binding, options)?;
        for normalized_binding in pieces {
            if !normalized.contains(&normalized_binding) {
                normalized.push(normalized_binding);
            }
        }
    }
    Ok(normalized)
}

/// The `allow_bare` / `allow_shift_only` options for an action. Mirror of Orca
/// `normalizeOptionsForAction` (`keybindings.ts:1466-1472`).
pub(crate) fn normalize_options_for_action(action: KeybindingActionId) -> NormalizeOptions {
    match action.definition() {
        Some(def) => NormalizeOptions {
            allow_bare: def.allow_bare_keybindings,
            allow_shift_only: def.allow_shift_only_keybindings,
        },
        None => NormalizeOptions::default(),
    }
}

/// Whether `key` is a single `1`-`9` digit. Mirror of Orca
/// `DIGIT_INDEX_KEY_PATTERN = /^[1-9]$/` (`keybindings.ts:1095`).
pub(crate) fn is_digit_index_key(key: &str) -> bool {
    matches!(key.as_bytes(), [b'1'..=b'9'])
}

/// Rewrite a digit-index chord's key to `1` (its stable representative) so that
/// display and conflict detection stay identical across the `1`-`9` range;
/// reject any non-`1`-`9` key (or double-tap / unparsable). Mirror of Orca
/// `canonicalizeDigitIndexBinding` (`keybindings.ts:1475-1484`).
pub(crate) fn canonicalize_digit_index_binding(binding: &str) -> KeybindingValidationResult {
    let parsed = match parse_keybinding(binding) {
        Ok(parsed) => parsed,
        Err(_) => {
            return KeybindingValidationResult::Invalid {
                reason: InvalidReason::NotDigitIndexKey,
            };
        }
    };
    if parsed.double_tap_modifier.is_some() || !is_digit_index_key(&parsed.key) {
        return KeybindingValidationResult::Invalid {
            reason: InvalidReason::NotDigitIndexKey,
        };
    }
    let mut rewritten = parsed;
    rewritten.key = "1".to_string();
    KeybindingValidationResult::Valid {
        canonical: canonicalize_parsed_keybinding(&rewritten),
    }
}

/// Apply digit-index canonicalization to an already-normalized list, but only
/// for digit-index actions; all others pass through unchanged. Mirror of Orca
/// `finalizeDigitIndexBindings` (`keybindings.ts:1486-1504`).
fn finalize_digit_index_bindings(
    action: KeybindingActionId,
    result: KeybindingListResult,
) -> KeybindingListResult {
    if !is_digit_index_action_id(action) {
        return result;
    }
    let bindings = result?;
    let mut canonical: Vec<String> = Vec::new();
    for binding in bindings {
        match canonicalize_digit_index_binding(&binding) {
            KeybindingValidationResult::Valid { canonical: value } => {
                if !canonical.contains(&value) {
                    canonical.push(value);
                }
            }
            KeybindingValidationResult::Invalid { reason } => return Err(reason),
        }
    }
    Ok(canonical)
}

/// Normalize a comma-separated chord list for a specific action, applying its
/// per-action bare/shift-only rules and digit-index canonicalization. Mirror of
/// Orca `normalizeKeybindingListForAction` (`keybindings.ts:1506-1514`).
pub fn normalize_keybinding_list_for_action(
    action: KeybindingActionId,
    input: &str,
) -> KeybindingListResult {
    finalize_digit_index_bindings(
        action,
        normalize_keybinding_list_with_options(input, normalize_options_for_action(action)),
    )
}

/// Normalize an array of chords for a specific action (each element may be
/// comma-separated), applying per-action rules and digit-index
/// canonicalization. Mirror of Orca `normalizeKeybindingArrayForAction`
/// (`keybindings.ts:1516-1524`).
pub fn normalize_keybinding_array_for_action(
    action: KeybindingActionId,
    input: &[&str],
) -> KeybindingListResult {
    finalize_digit_index_bindings(
        action,
        normalize_keybinding_array_with_options(input, normalize_options_for_action(action)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::ParseError;
    use KeybindingActionId as A;

    /// Assert the binding normalizes (context-free) to `expected`.
    fn valid(binding: &str) -> String {
        match normalize_keybinding(binding) {
            KeybindingValidationResult::Valid { canonical } => canonical,
            KeybindingValidationResult::Invalid { reason } => {
                panic!("expected {binding:?} valid, got {reason:?}")
            }
        }
    }

    // --- Ported TS oracle vectors (keybindings.test.ts) ---------------------

    #[test]
    fn normalizes_editable_shortcut_input() {
        // keybindings.test.ts:29-39.
        assert_eq!(valid(" ctrl + shift + p "), "Ctrl+Shift+P");
        assert_eq!(valid("shift+insert"), "Shift+Insert");
        assert_eq!(valid("cmdorctrl+p"), "Mod+P");
        assert_eq!(
            normalize_keybinding_list("Ctrl+Shift+P, ctrl+shift+p, \u{2318}+k"),
            Ok(vec!["Ctrl+Shift+P".to_string(), "Cmd+K".to_string()])
        );
    }

    #[test]
    fn rejects_unsafe_bindings() {
        // keybindings.test.ts:41-43.
        // Shift+P: shift-only letter, no opt-in -> MissingModifier.
        assert_eq!(
            normalize_keybinding("Shift+P").reason(),
            Some(&InvalidReason::MissingModifier)
        );
        // Mod+Ctrl+P: Mod combined with a platform modifier.
        assert_eq!(
            normalize_keybinding("Mod+Ctrl+P").reason(),
            Some(&InvalidReason::ModWithPlatformModifier)
        );
        // Ctrl+Nope: unparsable key token.
        assert_eq!(
            normalize_keybinding("Ctrl+Nope").reason(),
            Some(&InvalidReason::Unparsable(ParseError::UnknownToken(
                "Nope".to_string()
            )))
        );
    }

    #[test]
    fn normalizes_and_rejects_double_tap() {
        // keybindings.test.ts:46-67.
        assert_eq!(valid("DoubleTap+Shift"), "DoubleTap+Shift");
        assert_eq!(valid(" doubletap + shift "), "DoubleTap+Shift");
        assert_eq!(valid("DoubleTap+Mod"), "DoubleTap+Mod");
        assert_eq!(valid("DoubleTap+Cmd"), "DoubleTap+Cmd");
        assert_eq!(valid("DoubleTap+Alt"), "DoubleTap+Alt");
        assert_eq!(valid("DoubleTap+Ctrl"), "DoubleTap+Ctrl");

        // A key after DoubleTap / two modifiers / bare DoubleTap: unparsable.
        assert!(!normalize_keybinding("DoubleTap+Shift+P").is_valid());
        assert!(!normalize_keybinding("DoubleTap+Shift+Alt").is_valid());
        assert!(!normalize_keybinding("DoubleTap").is_valid());

        // Mod + platform-specific reuses the shared error (test.ts:62-65). Note
        // this parses (M1) then normalize rejects it with the specific reason.
        assert_eq!(
            normalize_keybinding("DoubleTap+Mod+Cmd").reason(),
            Some(&InvalidReason::ModWithPlatformModifier)
        );
        assert_eq!(
            normalize_keybinding("DoubleTap+Mod+Cmd")
                .reason()
                .unwrap()
                .to_string(),
            "Use either Mod or a platform-specific modifier, not both."
        );
    }

    #[test]
    fn allows_safe_bare_keys_only_for_opt_in_actions() {
        // keybindings.test.ts:74-79.
        assert_eq!(
            normalize_keybinding("Delete").reason(),
            Some(&InvalidReason::MissingModifier)
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::FileExplorerDelete, "Delete"),
            Ok(vec!["Delete".to_string()])
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::FileExplorerDelete, "x"),
            Err(InvalidReason::MissingModifier)
        );
    }

    #[test]
    fn binds_f7_and_shift_f7_only_for_opt_in_actions() {
        // keybindings.test.ts:160-168.
        assert_eq!(
            normalize_keybinding_list_for_action(A::EditorNextChange, "F7"),
            Ok(vec!["F7".to_string()])
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::EditorPreviousChange, "Shift+F7"),
            Ok(vec!["Shift+F7".to_string()])
        );
        // ...but they stay unsafe for actions that do not opt in.
        assert!(!normalize_keybinding("F7").is_valid());
        assert!(!normalize_keybinding("Shift+F7").is_valid());
    }

    #[test]
    fn shift_only_chord_only_for_input_source_switching() {
        // keybindings.test.ts:82-96 (the normalize half; input-resolution is M3).
        // Shift+Space is shift-only: rejected context-free, accepted for the
        // opt-in action terminal.switchInputSource.
        assert!(!normalize_keybinding("Shift+Space").is_valid());
        assert_eq!(
            normalize_keybinding_list_for_action(A::TerminalSwitchInputSource, "Shift+Space"),
            Ok(vec!["Shift+Space".to_string()])
        );
    }

    #[test]
    fn digit_index_canonicalizes_to_one_and_rejects_non_numbers() {
        // keybindings.test.ts:1679-1709.
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorkspaceSelectByIndex, "Mod+5"),
            Ok(vec!["Mod+1".to_string()])
        );
        assert_eq!(
            normalize_keybinding_array_for_action(A::TabSelectByIndex, &["Ctrl+9"]),
            Ok(vec!["Ctrl+1".to_string()])
        );
        // Extra modifiers (e.g. Shift) are preserved; only the digit collapses.
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorkspaceSelectByIndex, "Mod+Shift+5"),
            Ok(vec!["Mod+Shift+1".to_string()])
        );
        // A non-number chord for a digit-index action is rejected.
        assert_eq!(
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Mod+P"),
            Err(InvalidReason::NotDigitIndexKey)
        );
    }

    // --- Crux tests ---------------------------------------------------------

    // Crux (bare-key rule): a safe bare key is Invalid for an action WITHOUT
    // allow_bare_keybindings, Valid for one WITH it. Mutating the
    // `options.allow_bare && ...` guard (forcing it true or false) flips one of
    // these two assertions.
    #[test]
    fn crux_bare_key_gated_on_allow_bare_flag() {
        // WorktreeQuickOpen does NOT allow bare keys: Delete is rejected.
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorktreeQuickOpen, "Delete"),
            Err(InvalidReason::MissingModifier)
        );
        // FileExplorerDelete DOES allow bare keys: Delete is accepted.
        assert_eq!(
            normalize_keybinding_list_for_action(A::FileExplorerDelete, "Delete"),
            Ok(vec!["Delete".to_string()])
        );
        // And a non-safe bare key ('A') is rejected even for the opt-in action:
        // is_safe_bare_key must gate on the key, not just the flag.
        assert_eq!(
            normalize_keybinding_list_for_action(A::FileExplorerDelete, "A"),
            Err(InvalidReason::MissingModifier)
        );
    }

    // Crux (shift-only rule): a shift-only chord is Invalid unless
    // allow_shift_only_keybindings. Mutating the
    // `options.allow_shift_only && ...` guard flips one of these.
    #[test]
    fn crux_shift_only_gated_on_allow_shift_only_flag() {
        // EditorSave does NOT allow shift-only: Shift+Space is rejected.
        assert_eq!(
            normalize_keybinding_list_for_action(A::EditorSave, "Shift+Space"),
            Err(InvalidReason::MissingModifier)
        );
        // TerminalSwitchInputSource DOES allow shift-only: Shift+Space accepted.
        assert_eq!(
            normalize_keybinding_list_for_action(A::TerminalSwitchInputSource, "Shift+Space"),
            Ok(vec!["Shift+Space".to_string()])
        );
    }

    // Crux (invalid combo): Mod combined with a platform primary modifier is
    // always rejected, regardless of per-action opt-ins. Mutating the
    // `parsed.is_mod && (parsed.meta || parsed.control)` check lets it through.
    #[test]
    fn crux_mod_plus_platform_modifier_rejected() {
        assert_eq!(
            normalize_keybinding("Mod+Ctrl+P").reason(),
            Some(&InvalidReason::ModWithPlatformModifier)
        );
        assert_eq!(
            normalize_keybinding("Mod+Cmd+P").reason(),
            Some(&InvalidReason::ModWithPlatformModifier)
        );
        // Even opt-in actions cannot override this universal rule.
        assert_eq!(
            normalize_keybinding_list_for_action(A::FileExplorerDelete, "Mod+Cmd+P"),
            Err(InvalidReason::ModWithPlatformModifier)
        );
    }

    // Crux (digit-index scoping): the 1->rewrite fires ONLY for digit-index
    // actions. Mutating canonicalize_digit_index_binding to skip the rewrite, or
    // finalize to apply it to all actions, fails one of these.
    #[test]
    fn crux_digit_index_rewrite_scoped_to_digit_index_actions() {
        // Digit-index action: Mod+2 rewrites to Mod+1 (same identity as Mod+1).
        assert_eq!(
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Ctrl+2"),
            Ok(vec!["Ctrl+1".to_string()])
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Ctrl+2"),
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Ctrl+1")
        );
        // NON digit-index action: Mod+2 is NOT rewritten (stays Mod+2) — proving
        // finalize only touches digit-index rows. (WorktreeQuickOpen is a normal
        // action; Mod+2 is a perfectly valid chord for it.)
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorktreeQuickOpen, "Mod+2"),
            Ok(vec!["Mod+2".to_string()])
        );
        // And a digit-index action rejects a non-1-9 chord (the rewrite path's
        // reject branch), whereas the normal action would accept Mod+P.
        assert_eq!(
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Mod+P"),
            Err(InvalidReason::NotDigitIndexKey)
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorktreeQuickOpen, "Mod+P"),
            Ok(vec!["Mod+P".to_string()])
        );
    }

    // Crux (list dedup): duplicate spellings collapse to one canonical entry,
    // first spelling wins, order preserved. Mutating the `!contains` dedup guard
    // (dropping it) leaves duplicates and fails this.
    #[test]
    fn crux_list_dedup_preserves_first_and_order() {
        assert_eq!(
            normalize_keybinding_list("Ctrl+Shift+P, shift+ctrl+p, Mod+K, mod+k"),
            Ok(vec!["Ctrl+Shift+P".to_string(), "Mod+K".to_string()])
        );
        // Array form dedups across elements too.
        assert_eq!(
            normalize_keybinding_array_for_action(
                A::WorktreeQuickOpen,
                &["Mod+P", "mod+p", "Mod+B"]
            ),
            Ok(vec!["Mod+P".to_string(), "Mod+B".to_string()])
        );
        // Digit-index dedup: Ctrl+2 and Ctrl+3 both collapse to Ctrl+1 -> one.
        assert_eq!(
            normalize_keybinding_array_for_action(A::TabSelectByIndex, &["Ctrl+2", "Ctrl+3"]),
            Ok(vec!["Ctrl+1".to_string()])
        );
    }

    // --- Edge/coverage tests ------------------------------------------------

    #[test]
    fn empty_and_whitespace_lists_are_empty() {
        // keybindings.ts:1426-1429.
        assert_eq!(normalize_keybinding_list(""), Ok(Vec::new()));
        assert_eq!(normalize_keybinding_list("   "), Ok(Vec::new()));
        assert_eq!(
            normalize_keybinding_array_for_action(A::WorktreeQuickOpen, &[]),
            Ok(Vec::new())
        );
    }

    #[test]
    fn list_returns_first_rejection() {
        assert_eq!(
            normalize_keybinding_list("Mod+P, Shift+Q"),
            Err(InvalidReason::MissingModifier)
        );
    }

    #[test]
    fn shift_insert_allowed_context_free() {
        // isShiftInsert exemption (keybindings.ts:1391): Shift+Insert needs no
        // other modifier even without any opt-in.
        assert_eq!(valid("Shift+Insert"), "Shift+Insert");
    }

    #[test]
    fn is_safe_bare_key_shift_letter_stays_unsafe() {
        // Shift+F7 safe (function key), Shift+A unsafe.
        assert_eq!(
            normalize_keybinding_list_for_action(A::EditorPreviousChange, "Shift+F7"),
            Ok(vec!["Shift+F7".to_string()])
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::EditorPreviousChange, "Shift+A"),
            Err(InvalidReason::MissingModifier)
        );
    }

    // --- Pinning tests (close hollow-test gaps; additive only) --------------

    /// The full non-function-key bare allow-list from `is_safe_bare_key`
    /// (Orca `keybindings.ts:1361-1373`). Every entry must be accepted bare for
    /// an opt-in action and rejected bare for a non-opt-in one; dropping any key
    /// from the allow-list must fail this. (Function keys are covered separately
    /// by the F7/Shift+F7 tests.)
    #[test]
    fn bare_allow_list_keys_gated_by_opt_in() {
        const BARE_KEYS: &[&str] = &[
            "Backspace",
            "Delete",
            "Enter",
            "Escape",
            "Tab",
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            "ArrowDown",
            "PageUp",
            "PageDown",
        ];
        for key in BARE_KEYS {
            // Accepted bare for an action that opts in (FileExplorerDelete has
            // allow_bare_keybindings = true).
            assert_eq!(
                normalize_keybinding_list_for_action(A::FileExplorerDelete, key),
                Ok(vec![(*key).to_string()]),
                "{key} should be accepted bare for an allow-bare action"
            );
            // Rejected bare for an action that does not opt in.
            assert_eq!(
                normalize_keybinding_list_for_action(A::WorktreeQuickOpen, key),
                Err(InvalidReason::MissingModifier),
                "{key} should be rejected bare for a non-opt-in action"
            );
        }
    }

    // Pins the `1`-`9` lower bound: `0` is not a valid digit-index key, so a
    // digit-index action must reject `Mod+0`. Widening `is_digit_index_key` to
    // `[b'0'..=b'9']` (which would rewrite Mod+0 -> Mod+1) fails this.
    #[test]
    fn digit_index_rejects_zero() {
        assert_eq!(
            normalize_keybinding_list_for_action(A::TabSelectByIndex, "Mod+0"),
            Err(InvalidReason::NotDigitIndexKey)
        );
        assert_eq!(
            normalize_keybinding_list_for_action(A::WorkspaceSelectByIndex, "Mod+0"),
            Err(InvalidReason::NotDigitIndexKey)
        );
    }

    // Pins every `InvalidReason` Display string to its exact Orca message,
    // including the en-dash (U+2013) in the digit-index message. Any one-char
    // edit to a message string fails this.
    #[test]
    fn invalid_reason_display_matches_orca() {
        assert_eq!(
            InvalidReason::MissingModifier.to_string(),
            "Include at least one modifier key."
        );
        assert_eq!(
            InvalidReason::NotDigitIndexKey.to_string(),
            "Pick a number key 1\u{2013}9 with a modifier, like Cmd+1 or Ctrl+1."
        );
        // Unparsable's Display is fixed regardless of the wrapped ParseError
        // (thiserror uses only the format string; the source is separate).
        assert_eq!(
            InvalidReason::Unparsable(ParseError::Empty).to_string(),
            "Use a shortcut like Ctrl+Shift+P or Cmd+K."
        );
        // ModWithPlatformModifier is also pinned in normalizes_and_rejects_double_tap;
        // re-asserted here so all four variants live in one place.
        assert_eq!(
            InvalidReason::ModWithPlatformModifier.to_string(),
            "Use either Mod or a platform-specific modifier, not both."
        );
    }
}
