//! The event->action resolver — the crown jewel. Given a live key event
//! ([`KeybindingInput`]) it answers "which canonical chord did the user press?"
//! ([`keybinding_from_input`]), "does this chord match the event?"
//! ([`keybinding_matches_input`]), "does this action fire?"
//! ([`keybinding_matches_action`]), and "which 1-9 index was pressed?"
//! ([`match_keybinding_digit_index`]).
//!
//! Ported **verbatim** from Orca `src/shared/keybindings.ts`. The subtlety lives
//! in the platform-semantic fallbacks: macOS Option+key *composes* (Option+A
//! reports `'å'`), and non-Latin / AltGr layouts report non-Latin logical keys
//! for physical letters. Both fall back to the physical `code` — but AltGr text
//! entry must NOT be hijacked into a shortcut. Cited helpers:
//!   - `logicalKeyTokenFromInput` (:1569), `canUsePhysicalCodeFallback` (:1584)
//!   - `isLatinShortcutKey` (:1589), `shouldUseNonLatinShortcutPhysicalFallback`
//!     (:1598), `canFallBackToPhysicalCode` (:1621)
//!   - `physicalCodeKeyTokenFromInput` (:1627), `numpadCodeKeyTokenFromInput` (:1639)
//!   - `shouldUseMacOptionComposedCaptureFallback` (:1644), `keyTokenFromInput` (:1666)
//!   - `canonicalDoubleTapToken` (:1686), `keybindingFromInput*` (:1700-1759)
//!   - `platformModifiers` (:1837), `modifierStateMatches` (:1850)
//!   - `shouldUseMacOptionLetterPhysicalFallback` (:1864),
//!     `shouldUseMacOptionPunctuationPhysicalFallback` (:1878)
//!   - `letterKeyMatches` (:1892), `digitKeyMatches` (:1909), `keyMatches` (:1955)
//!   - `semanticPunctuationKey` (:1925), `shouldUseSemanticPunctuation` (:1935)
//!   - `keybindingMatchesInput` (:2018), `getEffectiveKeybindingsForAction` (:1772)
//!   - `keybindingIsActiveInContext` (:1823) + terminal policy (:1809)
//!   - `keybindingMatchesAction` (:2083), `matchKeybindingDigitIndex` (:2113)

use std::collections::HashMap;

use crate::chord::{
    canonicalize_parsed_keybinding, normalize_key_token, parse_keybinding, resolve_modifier_token,
    ModifierToken, ParsedKeybinding,
};
use crate::normalize::{
    canonicalize_digit_index_binding, is_digit_index_key, normalize_keybinding_with_options,
    normalize_options_for_action, KeybindingValidationResult,
};
use crate::registry::{is_digit_index_action_id, KeybindingActionId, KeybindingPlatform, Scope};

/// A live keyboard event, in the DOM `KeyboardEvent` vocabulary Orca switches on.
///
/// **F1 (the `to_latin` ban) — read before writing the M6 iced adapter.** The
/// two fields `key` and `code` are load-bearing and must be fed as the *raw*
/// values from the platform:
///   - `key` is the **logical** value: the character the layout+modifiers
///     produced (`KeyboardEvent.key`). On macOS, Option+A composes to `"å"`; on
///     a Cyrillic layout physical C produces `"с"`. It is a *character*, e.g.
///     `"j"`, `"J"`, `"å"`, `"с"`, `"["`, `"ArrowLeft"`, `"Delete"`, `" "`
///     (Space), or `""` / `"Dead"` / `"Unidentified"` when the platform can't
///     report one.
///   - `code` is the **physical** value: the key's position, independent of
///     layout (`KeyboardEvent.code`), e.g. `"KeyA"`, `"Digit1"`, `"BracketLeft"`,
///     `"NumpadAdd"`, `"Comma"`.
///
/// The resolver's fallbacks branch on `key` vs `code` exactly as Orca does. Do
/// **NOT** collapse them through anything like iced's `Key::to_latin(physical)`:
/// that helper returns any scalar below U+0370 as-if-Latin, so it would hand back
/// `'å'` (U+00E5) for macOS Option+A instead of leaving `key` composed so the
/// physical-`code` fallback can fire. Feed raw `key` + `code` + the four modifier
/// booleans; never a pre-latinized key. (See plan F1 / F4.)
///
/// Modifiers are the **four canonical booleans** (`alt`/`meta`/`control`/`shift`)
/// — not Orca's 8-field DOM compat shim (plan F4). `double_tap_modifier` is set
/// only by the double-tap detector and is always a *physical* token (never
/// `Mod`); when present, `key`/`code`/the four bools are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeybindingInput {
    /// Logical `KeyboardEvent.key` (see the F1 note). Empty string = none.
    pub key: String,
    /// Physical `KeyboardEvent.code` (see the F1 note). Empty string = none.
    pub code: String,
    pub alt: bool,
    pub meta: bool,
    pub control: bool,
    pub shift: bool,
    /// Set only for a synthetic double-tap gesture; a physical token, never `Mod`.
    pub double_tap_modifier: Option<ModifierToken>,
}

/// Which UI surface currently has focus. Mirror of Orca `KeybindingContext`
/// (`keybindings.ts:15`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindingContext {
    App,
    Terminal,
    Browser,
}

/// How app shortcuts behave inside a terminal. Mirror of Orca
/// `TerminalShortcutPolicy` (`keybindings.ts:19`). `OrcaFirst` (the default) keeps
/// app shortcuts winning inside terminals; `TerminalFirst` passes non-terminal
/// chords through to the shell/TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerminalShortcutPolicy {
    #[default]
    OrcaFirst,
    TerminalFirst,
}

/// Context knobs for action-level matching. Mirror of Orca
/// `KeybindingMatchOptions` (`keybindings.ts:21`). Both fields optional, matching
/// the TS `?` (an absent `context` is treated as "not a terminal").
#[derive(Debug, Clone, Copy, Default)]
pub struct KeybindingMatchOptions {
    pub context: Option<KeybindingContext>,
    pub terminal_shortcut_policy: Option<TerminalShortcutPolicy>,
}

/// Per-action user overrides: each mapped action's *complete* effective binding
/// list replaces its defaults. Mirror of Orca `KeybindingOverrides`
/// (`keybindings.ts:115` = `Partial<Record<KeybindingActionId, string[]>>`).
pub type KeybindingOverrides = HashMap<KeybindingActionId, Vec<String>>;

// --- Vocabulary sets (verbatim Orca constants) ------------------------------

/// Modifier logical-key names that never count as the pressed key. Mirror of Orca
/// `MODIFIER_KEYS` (`keybindings.ts:1526-1539`).
fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "Alt"
            | "AltGraph"
            | "Control"
            | "Meta"
            | "Shift"
            | "OS"
            | "Fn"
            | "FnLock"
            | "Hyper"
            | "Super"
            | "Symbol"
            | "SymbolLock"
    )
}

/// Canonical punctuation key tokens. Mirror of Orca `PUNCTUATION_KEY_TOKENS`
/// (`keybindings.ts:1541-1555`).
fn is_punctuation_key_token(token: Option<&str>) -> bool {
    matches!(
        token,
        Some(
            "BracketLeft"
                | "BracketRight"
                | "Minus"
                | "Underscore"
                | "Equal"
                | "Plus"
                | "Comma"
                | "Period"
                | "Slash"
                | "Backslash"
                | "Semicolon"
                | "Quote"
                | "Backquote"
        )
    )
}

/// The logical `key` values that mean "the platform could not report a key" and
/// so license the physical-code fallback. Mirror of Orca
/// `PHYSICAL_CODE_FALLBACK_KEYS = new Set(['', 'Dead', 'Unidentified'])` (`:1557`).
fn is_physical_code_fallback_key(key: &str) -> bool {
    matches!(key, "" | "Dead" | "Unidentified")
}

/// Shifted-punctuation aliases: the char a shifted punctuation key produces maps
/// back to its base token. Mirror of Orca `SHIFTED_PUNCTUATION_KEY_TOKENS`
/// (`keybindings.ts:1559-1567`).
fn shifted_punctuation_key_token(key: &str) -> Option<&'static str> {
    Some(match key {
        "<" => "Comma",
        ">" => "Period",
        "?" => "Slash",
        "|" => "Backslash",
        ":" => "Semicolon",
        "\"" => "Quote",
        "~" => "Backquote",
        _ => return None,
    })
}

// --- Logical / physical key extraction --------------------------------------

/// The logical key token an input produced, or `None`. Mirror of Orca
/// `logicalKeyTokenFromInput` (`keybindings.ts:1569-1582`).
fn logical_key_token_from_input(input: &KeybindingInput) -> Option<String> {
    let key = input.key.as_str();
    if is_modifier_key(key) {
        return None;
    }
    if let Some(normalized) = normalize_key_token(key) {
        return Some(normalized);
    }
    if input.shift {
        return shifted_punctuation_key_token(key).map(str::to_string);
    }
    None
}

/// Whether the input's logical key is `''`/`Dead`/`Unidentified` — the only case
/// where the physical code is trusted unconditionally. Mirror of Orca
/// `canUsePhysicalCodeFallback` (`keybindings.ts:1584-1587`).
fn can_use_physical_code_fallback(input: &KeybindingInput) -> bool {
    is_physical_code_fallback_key(input.key.as_str())
}

/// Whether `key` names a Latin shortcut char (A-Z / 0-9), in which case the
/// non-Latin physical-code fallback must NOT fire. Mirror of Orca
/// `isLatinShortcutKey` (`keybindings.ts:1589-1596`).
///
/// Ported **verbatim**, including the `toUpperCase()` string-range comparison
/// (not `is_ascii_alphanumeric`). This is load-bearing: JS `'ß'.toUpperCase()`
/// is `"SS"`, and `"SS" >= "A" && "SS" <= "Z"` is `true`, so Orca counts `ß` as
/// Latin and blocks the physical fallback. Using `is_ascii_alphanumeric` would
/// call `ß` non-Latin and wrongly fire the fallback — on a German/Austrian/Swiss
/// QWERTZ layout `ß`'s physical code is `Minus`, so `Ctrl+ß` would misfire
/// `zoom.out` (Mod+Minus). Ligatures expand the same way (`'ﬀ'.toUpperCase()`
/// is `"FF"`). The `key.length !== 1` guard (Orca `:1591`) is present, so
/// multi-char logical keys (e.g. `"Dead"`) are correctly not Latin.
fn is_latin_shortcut_key(key: &str) -> bool {
    if key.chars().count() != 1 {
        return false;
    }
    // The verbatim Orca range compares (`upper >= 'A' && upper <= 'Z'`, etc.),
    // expressed as inclusive-range `contains` on the string slices.
    let upper = key.to_uppercase();
    ("A"..="Z").contains(&upper.as_str()) || ("0"..="9").contains(&key)
}

/// Whether a non-Latin layout (e.g. Cyrillic/Greek) reported a non-Latin logical
/// key for a physical letter, so the physical code should drive the match. Mirror
/// of Orca `shouldUseNonLatinShortcutPhysicalFallback` (`keybindings.ts:1598-1619`,
/// issue #6274).
///
/// The AltGr gate (`control && alt` -> `false`, `:1611`) is the crux: on
/// Windows/Linux AltGr surfaces as Ctrl+Alt, so a composed char typed via AltGr
/// must stay text input, never a Mod+Alt shortcut.
fn should_use_non_latin_shortcut_physical_fallback(
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    if platform == KeybindingPlatform::Darwin {
        return false;
    }
    let has_primary_modifier = input.control || input.meta;
    if !has_primary_modifier {
        return false;
    }
    // AltGr surfaces as Ctrl+Alt on Windows/Linux; treat it as text, not a chord.
    if input.control && input.alt {
        return false;
    }
    if logical_key_token_from_input(input).is_some() {
        return false;
    }
    let key = input.key.as_str();
    !key.is_empty() && !is_modifier_key(key) && !is_latin_shortcut_key(key)
}

/// Whether the physical `code` may substitute for the logical key. Mirror of Orca
/// `canFallBackToPhysicalCode` (`keybindings.ts:1621-1625`).
fn can_fall_back_to_physical_code(input: &KeybindingInput, platform: KeybindingPlatform) -> bool {
    can_use_physical_code_fallback(input)
        || should_use_non_latin_shortcut_physical_fallback(input, platform)
}

/// The key token derived from the physical `code` (`KeyA` -> `A`, `Digit1` ->
/// `1`, else `normalizeKeyToken(code)`). Mirror of Orca
/// `physicalCodeKeyTokenFromInput` (`keybindings.ts:1627-1637`).
fn physical_code_key_token_from_input(input: &KeybindingInput) -> Option<String> {
    let code = input.code.as_str();
    if let Some(rest) = code.strip_prefix("Key") {
        if rest.len() == 1 {
            return Some(rest.to_uppercase());
        }
    }
    if let Some(rest) = code.strip_prefix("Digit") {
        if rest.len() == 1 {
            return Some(rest.to_string());
        }
    }
    normalize_key_token(code)
}

/// The key token for an explicit numpad `+`/`-` code, else `None`. Mirror of Orca
/// `numpadCodeKeyTokenFromInput` (`keybindings.ts:1639-1642`).
fn numpad_code_key_token_from_input(input: &KeybindingInput) -> Option<String> {
    match input.code.as_str() {
        "NumpadAdd" | "NumpadSubtract" => normalize_key_token(input.code.as_str()),
        _ => None,
    }
}

/// Whether capturing a macOS Option-composed key should use the physical code.
/// Mirror of Orca `shouldUseMacOptionComposedCaptureFallback`
/// (`keybindings.ts:1644-1664`).
fn should_use_mac_option_composed_capture_fallback(
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    if platform != KeybindingPlatform::Darwin || !input.alt || is_modifier_key(input.key.as_str()) {
        return false;
    }
    let Some(physical_token) = physical_code_key_token_from_input(input) else {
        return false;
    };
    is_single_ascii_upper(&physical_token) || is_punctuation_key_token(Some(&physical_token))
}

/// The key token a capture should record for `input`, or `None` if it is only a
/// modifier. Mirror of Orca `keyTokenFromInput` (`keybindings.ts:1666-1683`).
fn key_token_from_input(input: &KeybindingInput, platform: KeybindingPlatform) -> Option<String> {
    if let Some(numpad_key) = numpad_code_key_token_from_input(input) {
        return Some(numpad_key);
    }
    if let Some(logical_key) = logical_key_token_from_input(input) {
        return Some(logical_key);
    }
    if !can_use_physical_code_fallback(input)
        && !should_use_mac_option_composed_capture_fallback(input, platform)
        && !should_use_non_latin_shortcut_physical_fallback(input, platform)
    {
        return None;
    }
    physical_code_key_token_from_input(input)
}

/// A single ASCII uppercase letter (A-Z)?
fn is_single_ascii_upper(token: &str) -> bool {
    token.len() == 1 && token.as_bytes()[0].is_ascii_uppercase()
}

/// A single ASCII digit (0-9)?
fn is_single_ascii_digit(token: &str) -> bool {
    token.len() == 1 && token.as_bytes()[0].is_ascii_digit()
}

// --- Capture: event -> canonical chord --------------------------------------

/// The platform-primary modifier canonicalizes to `Mod` (Cmd on mac / Ctrl
/// elsewhere) for a double-tap capture. Mirror of Orca `canonicalDoubleTapToken`
/// (`keybindings.ts:1686-1698`).
fn canonical_double_tap_token(
    modifier: ModifierToken,
    platform: KeybindingPlatform,
) -> ModifierToken {
    let is_mac = platform == KeybindingPlatform::Darwin;
    match modifier {
        ModifierToken::Cmd if is_mac => ModifierToken::Mod,
        ModifierToken::Ctrl if !is_mac => ModifierToken::Mod,
        other => other,
    }
}

/// The canonical spelling of a modifier token in a chord string (`Mod`/`Cmd`/...).
fn modifier_token_str(modifier: ModifierToken) -> &'static str {
    match modifier {
        ModifierToken::Mod => "Mod",
        ModifierToken::Cmd => "Cmd",
        ModifierToken::Ctrl => "Ctrl",
        ModifierToken::Alt => "Alt",
        ModifierToken::Shift => "Shift",
    }
}

/// Capture a live event into a canonical, validated chord under `options`. Mirror
/// of Orca `keybindingFromInputWithOptions` (`keybindings.ts:1700-1737`).
fn keybinding_from_input_with_options(
    input: &KeybindingInput,
    platform: KeybindingPlatform,
    options: crate::normalize::NormalizeOptions,
) -> KeybindingValidationResult {
    if let Some(double_tap) = input.double_tap_modifier {
        let token = canonical_double_tap_token(double_tap, platform);
        return normalize_keybinding_with_options(
            &format!("DoubleTap+{}", modifier_token_str(token)),
            options,
        );
    }
    let Some(key) = key_token_from_input(input, platform) else {
        return KeybindingValidationResult::Invalid {
            reason: crate::normalize::InvalidReason::PressAKey,
        };
    };

    let is_mac = platform == KeybindingPlatform::Darwin;
    let mut parts: Vec<&str> = Vec::new();
    let primary_modifier_pressed = if is_mac { input.meta } else { input.control };
    if primary_modifier_pressed {
        parts.push("Mod");
    }
    if is_mac && input.control {
        parts.push("Ctrl");
    }
    if !is_mac && input.meta {
        parts.push("Cmd");
    }
    if input.alt {
        parts.push("Alt");
    }
    if input.shift {
        parts.push("Shift");
    }
    parts.push(&key);

    normalize_keybinding_with_options(&parts.join("+"), options)
}

/// Capture a live event into its canonical editable chord (context-free rules).
/// Mirror of Orca `keybindingFromInput` (`keybindings.ts:1739-1744`).
pub fn keybinding_from_input(
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> KeybindingValidationResult {
    keybinding_from_input_with_options(
        input,
        platform,
        crate::normalize::NormalizeOptions::default(),
    )
}

/// Capture a live event into the canonical chord for a specific action, applying
/// that action's bare/shift-only rules and digit-index (`1`-`9` -> `1`)
/// canonicalization. Mirror of Orca `keybindingFromInputForAction`
/// (`keybindings.ts:1746-1760`).
pub fn keybinding_from_input_for_action(
    action: KeybindingActionId,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> KeybindingValidationResult {
    let result =
        keybinding_from_input_with_options(input, platform, normalize_options_for_action(action));
    match &result {
        KeybindingValidationResult::Valid { canonical } if is_digit_index_action_id(action) => {
            canonicalize_digit_index_binding(canonical)
        }
        _ => result,
    }
}

// --- Match: chord vs event --------------------------------------------------

/// Expected physical modifier state for a parsed chord, resolving the virtual
/// `Mod` per platform. Mirror of Orca `platformModifiers` (`keybindings.ts:1837-1848`).
struct ExpectedModifiers {
    meta: bool,
    control: bool,
    alt: bool,
    shift: bool,
}

fn platform_modifiers(
    parsed: &ParsedKeybinding,
    platform: KeybindingPlatform,
) -> ExpectedModifiers {
    let is_mac = platform == KeybindingPlatform::Darwin;
    ExpectedModifiers {
        meta: parsed.meta || (parsed.is_mod && is_mac),
        control: parsed.control || (parsed.is_mod && !is_mac),
        alt: parsed.alt,
        shift: parsed.shift,
    }
}

/// Whether the pressed modifier state exactly matches the chord's. Mirror of Orca
/// `modifierStateMatches` (`keybindings.ts:1850-1862`).
fn modifier_state_matches(
    parsed: &ParsedKeybinding,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    let expected = platform_modifiers(parsed, platform);
    input.meta == expected.meta
        && input.control == expected.control
        && input.alt == expected.alt
        && input.shift == expected.shift
}

/// Whether a macOS Option+letter composed to a non-Latin char, forcing the
/// physical-code fallback. Mirror of Orca `shouldUseMacOptionLetterPhysicalFallback`
/// (`keybindings.ts:1864-1876`).
fn should_use_mac_option_letter_physical_fallback(
    parsed: &ParsedKeybinding,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    platform == KeybindingPlatform::Darwin
        && parsed.alt
        && input.alt
        && logical_key_token_from_input(input).is_none()
}

/// Whether a macOS Option+punctuation composed to a dead-key value, forcing the
/// physical-code fallback. Mirror of Orca
/// `shouldUseMacOptionPunctuationPhysicalFallback` (`keybindings.ts:1878-1890`).
/// (Orca's body is identical to the letter variant; kept separate to mirror the
/// source 1:1.)
fn should_use_mac_option_punctuation_physical_fallback(
    parsed: &ParsedKeybinding,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    platform == KeybindingPlatform::Darwin
        && parsed.alt
        && input.alt
        && logical_key_token_from_input(input).is_none()
}

/// Whether a chord's A-Z letter matches the event. Mirror of Orca
/// `letterKeyMatches` (`keybindings.ts:1892-1907`).
fn letter_key_matches(
    input: &KeybindingInput,
    letter: &str,
    parsed: &ParsedKeybinding,
    platform: KeybindingPlatform,
) -> bool {
    if let Some(logical_key) = logical_key_token_from_input(input) {
        if is_single_ascii_upper(&logical_key) {
            return logical_key == letter.to_uppercase();
        }
    }
    (can_fall_back_to_physical_code(input, platform)
        || should_use_mac_option_letter_physical_fallback(parsed, input, platform))
        && input.code == format!("Key{}", letter.to_uppercase())
}

/// Whether a chord's 0-9 digit matches the event. Mirror of Orca `digitKeyMatches`
/// (`keybindings.ts:1909-1919`).
fn digit_key_matches(input: &KeybindingInput, digit: &str, platform: KeybindingPlatform) -> bool {
    if let Some(logical_key) = logical_key_token_from_input(input) {
        if is_single_ascii_digit(&logical_key) {
            return logical_key == digit;
        }
    }
    can_fall_back_to_physical_code(input, platform) && input.code == format!("Digit{digit}")
}

/// The logical punctuation token the event produced, if any. Mirror of Orca
/// `semanticPunctuationKey` (`keybindings.ts:1925-1928`).
fn semantic_punctuation_key(input: &KeybindingInput) -> Option<String> {
    let logical_key = logical_key_token_from_input(input);
    match logical_key {
        Some(token) if is_punctuation_key_token(Some(&token)) => Some(token),
        _ => None,
    }
}

/// The physical punctuation token the event's code produced, if any. Mirror of
/// Orca `physicalPunctuationKey` (`keybindings.ts:1930-1933`).
fn physical_punctuation_key(input: &KeybindingInput) -> Option<String> {
    let physical_key = physical_code_key_token_from_input(input);
    match physical_key {
        Some(token) if is_punctuation_key_token(Some(&token)) => Some(token),
        _ => None,
    }
}

/// Whether to trust a produced punctuation char as a shortcut. Mirror of Orca
/// `shouldUseSemanticPunctuation` (`keybindings.ts:1935-1953`). The `false` branch
/// is the AltGr text-entry guard: on Windows/Linux, Ctrl+Alt (AltGr) producing a
/// non-punctuation-physical char is international text, not a Mod+Alt shortcut.
fn should_use_semantic_punctuation(
    parsed: &ParsedKeybinding,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    if platform != KeybindingPlatform::Darwin
        && parsed.is_mod
        && parsed.alt
        && input.control
        && input.alt
        && !input.meta
        && physical_punctuation_key(input).is_none()
    {
        return false;
    }
    true
}

/// Whether a parsed chord's key matches the event. Mirror of Orca `keyMatches`
/// (`keybindings.ts:1955-1998`).
fn key_matches(
    parsed_key: &str,
    input: &KeybindingInput,
    parsed: &ParsedKeybinding,
    platform: KeybindingPlatform,
) -> bool {
    if is_single_ascii_upper(parsed_key) {
        return letter_key_matches(input, parsed_key, parsed, platform);
    }
    if is_single_ascii_digit(parsed_key) {
        return digit_key_matches(input, parsed_key, platform);
    }

    if parsed_key == "NumpadAdd" || parsed_key == "NumpadSubtract" {
        return numpad_code_key_token_from_input(input).as_deref() == Some(parsed_key)
            || logical_key_token_from_input(input).as_deref() == Some(parsed_key);
    }

    if is_punctuation_key_token(Some(parsed_key)) {
        if let Some(semantic_key) = semantic_punctuation_key(input) {
            if !should_use_semantic_punctuation(parsed, input, platform) {
                return false;
            }
            return semantic_key == parsed_key;
        }
        return (can_fall_back_to_physical_code(input, platform)
            || should_use_mac_option_punctuation_physical_fallback(parsed, input, platform))
            && physical_punctuation_key(input).as_deref() == Some(parsed_key);
    }

    if let Some(logical_key) = logical_key_token_from_input(input) {
        return logical_key == parsed_key;
    }
    can_fall_back_to_physical_code(input, platform)
        && physical_code_key_token_from_input(input).as_deref() == Some(parsed_key)
}

/// Whether a canonical chord string matches a live event. Mirror of Orca
/// `keybindingMatchesInput` (`keybindings.ts:2018-2041`). Double-tap chords match
/// only synthetic double-tap input (and vice-versa), resolved per platform.
pub fn keybinding_matches_input(
    binding: &str,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
) -> bool {
    let Ok(parsed) = parse_keybinding(binding) else {
        return false;
    };
    if let Some(parsed_double_tap) = parsed.double_tap_modifier {
        return match input.double_tap_modifier {
            Some(input_double_tap) => {
                resolve_modifier_token(parsed_double_tap, platform)
                    == resolve_modifier_token(input_double_tap, platform)
            }
            None => false,
        };
    }
    if input.double_tap_modifier.is_some() {
        return false;
    }
    modifier_state_matches(&parsed, input, platform)
        && key_matches(&parsed.key, input, &parsed, platform)
}

// --- Effective bindings + action matching -----------------------------------

/// The normalized default chords for an action on `platform`. Mirror of Orca
/// `getDefaultBindings` (`keybindings.ts:1762-1770`). A binding that fails to
/// normalize is passed through unchanged (matching Orca's `ok ? value : binding`).
fn get_default_bindings(
    definition: &crate::registry::KeybindingDefinition,
    platform: KeybindingPlatform,
) -> Vec<String> {
    let options = crate::normalize::NormalizeOptions {
        allow_bare: definition.allow_bare_keybindings,
        allow_shift_only: definition.allow_shift_only_keybindings,
    };
    definition
        .default_bindings
        .for_platform(platform)
        .iter()
        .map(
            |binding| match normalize_keybinding_with_options(binding, options) {
                KeybindingValidationResult::Valid { canonical } => canonical,
                KeybindingValidationResult::Invalid { .. } => (*binding).to_string(),
            },
        )
        .collect()
}

/// The effective binding list for an action: user overrides replace defaults
/// wholesale, else the platform defaults. Mirror of Orca
/// `getEffectiveKeybindingsForAction` (`keybindings.ts:1772-1803`). Digit-index
/// overrides are canonicalized to `<mods>+1` (deduped); other overrides are
/// normalized and dropped if invalid (not deduped — matching Orca's `flatMap`).
pub fn get_effective_keybindings_for_action(
    action: KeybindingActionId,
    platform: KeybindingPlatform,
    overrides: Option<&KeybindingOverrides>,
) -> Vec<String> {
    let Some(definition) = action.definition() else {
        return Vec::new();
    };
    if let Some(override_list) = overrides.and_then(|map| map.get(&action)) {
        if is_digit_index_action_id(action) {
            let mut canonical: Vec<String> = Vec::new();
            for binding in override_list {
                if let KeybindingValidationResult::Valid { canonical: value } =
                    canonicalize_digit_index_binding(binding)
                {
                    if !canonical.contains(&value) {
                        canonical.push(value);
                    }
                }
            }
            return canonical;
        }
        let options = normalize_options_for_action(action);
        return override_list
            .iter()
            .filter_map(
                |binding| match normalize_keybinding_with_options(binding, options) {
                    KeybindingValidationResult::Valid { canonical } => Some(canonical),
                    KeybindingValidationResult::Invalid { .. } => None,
                },
            )
            .collect();
    }
    get_default_bindings(definition, platform)
}

/// Normalize a terminal-shortcut policy, defaulting absent/`OrcaFirst`. Mirror of
/// Orca `normalizeTerminalShortcutPolicy` (`keybindings.ts:1809-1813`).
fn normalize_terminal_shortcut_policy(
    policy: Option<TerminalShortcutPolicy>,
) -> TerminalShortcutPolicy {
    match policy {
        Some(TerminalShortcutPolicy::TerminalFirst) => TerminalShortcutPolicy::TerminalFirst,
        _ => TerminalShortcutPolicy::OrcaFirst,
    }
}

/// Whether an action is allowed to fire inside a terminal. Mirror of Orca
/// `isKeybindingAllowedInTerminal` (`keybindings.ts:1815-1817`).
fn is_keybinding_allowed_in_terminal(definition: &crate::registry::KeybindingDefinition) -> bool {
    definition.scope == Scope::Terminal || definition.allow_in_terminal
}

/// Whether an action is active in the current context. Mirror of Orca
/// `keybindingIsActiveInContext` (`keybindings.ts:1823-1835`). Only terminal
/// context under `terminal-first` policy gates non-terminal actions off.
fn keybinding_is_active_in_context(
    definition: &crate::registry::KeybindingDefinition,
    options: &KeybindingMatchOptions,
) -> bool {
    if options.context != Some(KeybindingContext::Terminal) {
        return true;
    }
    if normalize_terminal_shortcut_policy(options.terminal_shortcut_policy)
        == TerminalShortcutPolicy::OrcaFirst
    {
        return true;
    }
    is_keybinding_allowed_in_terminal(definition)
}

/// Whether an action fires for a live event, honoring overrides and terminal
/// policy. Mirror of Orca `keybindingMatchesAction` (`keybindings.ts:2083-2100`).
pub fn keybinding_matches_action(
    action: KeybindingActionId,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
    overrides: Option<&KeybindingOverrides>,
    options: &KeybindingMatchOptions,
) -> bool {
    let Some(definition) = action.definition() else {
        return false;
    };
    if !keybinding_is_active_in_context(definition, options) {
        return false;
    }
    get_effective_keybindings_for_action(action, platform, overrides)
        .iter()
        .any(|binding| keybinding_matches_input(binding, input, platform))
}

/// The pressed digit (`1`-`9`), or `None`. Mirror of Orca `digitFromInput`
/// (`keybindings.ts:2102-2110`).
fn digit_from_input(input: &KeybindingInput, platform: KeybindingPlatform) -> Option<String> {
    for value in 1..=9u32 {
        let digit = value.to_string();
        if digit_key_matches(input, &digit, platform) {
            return Some(digit);
        }
    }
    None
}

/// Resolve a digit-index action to the 0-based index the event selected, or
/// `None`. Mirror of Orca `matchKeybindingDigitIndex` (`keybindings.ts:2113-2139`):
/// a digit-index row's representative chord fires for any `1`-`9`, so the pressed
/// digit is substituted into the chord and re-run through the normal matcher.
pub fn match_keybinding_digit_index(
    action: KeybindingActionId,
    input: &KeybindingInput,
    platform: KeybindingPlatform,
    overrides: Option<&KeybindingOverrides>,
    options: &KeybindingMatchOptions,
) -> Option<usize> {
    let definition = action.definition()?;
    if !keybinding_is_active_in_context(definition, options) {
        return None;
    }
    let digit = digit_from_input(input, platform)?;
    for binding in get_effective_keybindings_for_action(action, platform, overrides) {
        let Ok(parsed) = parse_keybinding(&binding) else {
            continue;
        };
        if parsed.double_tap_modifier.is_some() || !is_digit_index_key(&parsed.key) {
            continue;
        }
        let mut candidate = parsed;
        candidate.key = digit.clone();
        let candidate_binding = canonicalize_parsed_keybinding(&candidate);
        if keybinding_matches_input(&candidate_binding, input, platform) {
            // digit is 1-9 by construction, so this parse never fails.
            return digit.parse::<usize>().ok().map(|value| value - 1);
        }
    }
    None
}

#[cfg(test)]
mod tests;
