//! Chord grammar: parse a `"Mod+Shift+P"` string into a [`ParsedKeybinding`],
//! canonicalize it back, resolve the virtual `Mod` modifier per platform, and
//! parse double-tap chords. Pure `String` <-> struct; no platform is needed to
//! *parse* (mirroring Orca — `Mod` stays virtual until format/resolve time).
//!
//! Ported from Orca `src/shared/keybindings.ts`:
//!   - `parseModifierToken` (:1219), `normalizeKeyToken` (:1137)
//!   - `parseKeybinding` (:1294), `parseDoubleTapKeybinding` (:1258)
//!   - `canonicalizeParsedKeybinding` (:1327)
//!   - `resolveModifierToken` (:2000) — the `Mod` -> platform resolution.

use crate::registry::KeybindingPlatform;

/// A modifier token as written in a chord string. Mirror of Orca `ModifierToken`
/// (`keybindings.ts:153`). `Mod` is the *virtual* primary modifier (Cmd on
/// darwin, Ctrl elsewhere); it stays virtual through parse/canonicalize and is
/// only resolved to a physical modifier by [`resolve_modifier_token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierToken {
    Mod,
    Cmd,
    Ctrl,
    Alt,
    Shift,
}

impl ModifierToken {
    /// The canonical spelling used in canonicalized chord strings.
    const fn as_canonical_str(self) -> &'static str {
        match self {
            ModifierToken::Mod => "Mod",
            ModifierToken::Cmd => "Cmd",
            ModifierToken::Ctrl => "Ctrl",
            ModifierToken::Alt => "Alt",
            ModifierToken::Shift => "Shift",
        }
    }
}

/// A physical modifier — the resolved form of a [`ModifierToken`] on a given
/// platform. Mirror of Orca `resolveModifierToken`'s return union
/// (`keybindings.ts:2003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalModifier {
    Meta,
    Control,
    Alt,
    Shift,
}

/// A parsed chord. Mirror of Orca `ParsedKeybinding` (`keybindings.ts:171-179`).
///
/// Note the five modifier-ish booleans: `is_mod` (the field Orca calls `mod`, a
/// Rust keyword here) is kept **separate** from `meta`/`control` so the virtual
/// `Mod` survives canonicalization intact — it is not collapsed into a physical
/// modifier until format/resolve time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKeybinding {
    /// The virtual `Mod` primary modifier (Orca's `mod`).
    pub is_mod: bool,
    pub meta: bool,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// The non-modifier key in canonical token form (e.g. `"P"`, `"BracketLeft"`).
    /// Empty for double-tap chords.
    pub key: String,
    /// Set only for double-tap chords; then `key` is empty.
    pub double_tap_modifier: Option<ModifierToken>,
}

impl ParsedKeybinding {
    fn empty() -> Self {
        ParsedKeybinding {
            is_mod: false,
            meta: false,
            control: false,
            alt: false,
            shift: false,
            key: String::new(),
            double_tap_modifier: None,
        }
    }

    fn apply_modifier(&mut self, modifier: ModifierToken) {
        match modifier {
            ModifierToken::Mod => self.is_mod = true,
            ModifierToken::Cmd => self.meta = true,
            ModifierToken::Ctrl => self.control = true,
            ModifierToken::Alt => self.alt = true,
            ModifierToken::Shift => self.shift = true,
        }
    }
}

/// Why a chord string failed to parse. Orca's `parseKeybinding` collapses all of
/// these into `null`; splitting them out gives sharper diagnostics and mutation
/// targets without changing the accept/reject behavior.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("empty keybinding")]
    Empty,
    #[error("more than one non-modifier key")]
    MultipleKeys,
    #[error("unrecognized token: {0}")]
    UnknownToken(String),
    #[error("modifiers with no key")]
    NoKey,
    #[error("invalid double-tap chord")]
    InvalidDoubleTap,
}

/// Resolve a modifier token to its physical form on `platform`. This is the
/// `Mod` virtual -> Cmd(darwin)/Ctrl(else) resolution. Mirror of Orca
/// `resolveModifierToken` (`keybindings.ts:2000-2016`).
pub fn resolve_modifier_token(
    modifier: ModifierToken,
    platform: KeybindingPlatform,
) -> PhysicalModifier {
    match modifier {
        ModifierToken::Mod => {
            if platform == KeybindingPlatform::Darwin {
                PhysicalModifier::Meta
            } else {
                PhysicalModifier::Control
            }
        }
        ModifierToken::Cmd => PhysicalModifier::Meta,
        ModifierToken::Ctrl => PhysicalModifier::Control,
        ModifierToken::Alt => PhysicalModifier::Alt,
        ModifierToken::Shift => PhysicalModifier::Shift,
    }
}

/// Parse a single `+`-delimited part into a modifier token, if it is one.
/// Mirror of Orca `parseModifierToken` (`keybindings.ts:1219-1237`). Word tokens
/// are matched case-insensitively; the glyph forms (⌘⌃⌥⇧) are matched verbatim.
pub fn parse_modifier_token(raw_part: &str) -> Option<ModifierToken> {
    let part = raw_part.to_lowercase();
    if part == "mod" || part == "cmdorctrl" || part == "commandorcontrol" {
        return Some(ModifierToken::Mod);
    }
    if part == "cmd" || part == "command" || part == "meta" || raw_part == "\u{2318}" {
        return Some(ModifierToken::Cmd);
    }
    if part == "ctrl" || part == "control" || raw_part == "\u{2303}" {
        return Some(ModifierToken::Ctrl);
    }
    if part == "alt" || part == "option" || part == "opt" || raw_part == "\u{2325}" {
        return Some(ModifierToken::Alt);
    }
    if part == "shift" || raw_part == "\u{21e7}" {
        return Some(ModifierToken::Shift);
    }
    None
}

/// F1-F24 function-key token (no leading zero, matching Orca's regex
/// `^F([1-9]|1[0-9]|2[0-4])$` at `keybindings.ts:1134`). `upper` is expected
/// already uppercased.
pub(crate) fn is_function_key_token(upper: &str) -> bool {
    let Some(rest) = upper.strip_prefix('F') else {
        return false;
    };
    if rest.is_empty() || rest.starts_with('0') || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    matches!(rest.parse::<u32>(), Ok(1..=24))
}

/// Normalize a non-modifier key token to its canonical form, or `None` if it is
/// not a recognized key. Mirror of Orca `normalizeKeyToken` (`keybindings.ts:1137-1217`).
pub fn normalize_key_token(token: &str) -> Option<String> {
    // A literal single space is the Space key (checked before trimming).
    if token == " " {
        return Some("Space".to_string());
    }
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    let upper = trimmed.to_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(upper);
        }
    }
    if is_function_key_token(&upper) {
        return Some(upper);
    }
    let canonical = match upper.as_str() {
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "{" => "BracketLeft",
        "}" => "BracketRight",
        "-" => "Minus",
        "_" => "Underscore",
        "=" => "Equal",
        "+" => "Plus",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "`" => "Backquote",
        "RETURN" => "Enter",
        "ESC" => "Escape",
        "SPACEBAR" => "Space",
        "PGUP" => "PageUp",
        "PGDN" => "PageDown",
        "PLUS" => "Plus",
        "MINUS" => "Minus",
        "EQUAL" => "Equal",
        "UNDERSCORE" => "Underscore",
        "ARROWLEFT" => "ArrowLeft",
        "LEFT" => "ArrowLeft",
        "ARROWRIGHT" => "ArrowRight",
        "RIGHT" => "ArrowRight",
        "ARROWUP" => "ArrowUp",
        "UP" => "ArrowUp",
        "ARROWDOWN" => "ArrowDown",
        "DOWN" => "ArrowDown",
        "PAGEUP" => "PageUp",
        "PAGEDOWN" => "PageDown",
        "BACKSPACE" => "Backspace",
        "DELETE" => "Delete",
        "DEL" => "Delete",
        "INSERT" => "Insert",
        "INS" => "Insert",
        "ENTER" => "Enter",
        "TAB" => "Tab",
        "ESCAPE" => "Escape",
        "SPACE" => "Space",
        "BRACKETLEFT" => "BracketLeft",
        "BRACKETRIGHT" => "BracketRight",
        "NUMPADADD" => "NumpadAdd",
        "NUMPADSUBTRACT" => "NumpadSubtract",
        "ADD" => "NumpadAdd",
        "SUBTRACT" => "NumpadSubtract",
        "COMMA" => "Comma",
        "PERIOD" => "Period",
        "SLASH" => "Slash",
        "BACKSLASH" => "Backslash",
        "SEMICOLON" => "Semicolon",
        "QUOTE" => "Quote",
        "BACKQUOTE" => "Backquote",
        _ => return None,
    };
    Some(canonical.to_string())
}

/// Parse a double-tap chord (a bare modifier, no key). Mirror of Orca
/// `parseDoubleTapKeybinding` (`keybindings.ts:1258-1292`). Modifier validity
/// (e.g. `Mod` combined with a physical modifier) is *not* rejected here — that
/// is deferred to normalize (M2), matching Orca.
fn parse_double_tap_keybinding(raw_parts: &[&str]) -> Result<ParsedKeybinding, ParseError> {
    let mut modifiers: Vec<ModifierToken> = Vec::new();
    let mut saw_double_tap = false;
    for raw_part in raw_parts {
        if raw_part.to_lowercase() == "doubletap" {
            if saw_double_tap {
                return Err(ParseError::InvalidDoubleTap);
            }
            saw_double_tap = true;
            continue;
        }
        match parse_modifier_token(raw_part) {
            Some(modifier) => modifiers.push(modifier),
            None => return Err(ParseError::InvalidDoubleTap),
        }
    }
    if modifiers.is_empty() {
        return Err(ParseError::InvalidDoubleTap);
    }
    let mut parsed = ParsedKeybinding::empty();
    for modifier in &modifiers {
        parsed.apply_modifier(*modifier);
    }
    // Keep both flags when Mod is combined with a platform modifier, so normalize
    // (M2) can emit the shared "Mod or platform-specific, not both" error.
    if parsed.is_mod && (parsed.meta || parsed.control) {
        parsed.double_tap_modifier = Some(ModifierToken::Mod);
        return Ok(parsed);
    }
    if modifiers.len() > 1 {
        return Err(ParseError::InvalidDoubleTap);
    }
    parsed.double_tap_modifier = Some(modifiers[0]);
    Ok(parsed)
}

/// Parse a chord string into a [`ParsedKeybinding`]. Mirror of Orca
/// `parseKeybinding` (`keybindings.ts:1294-1325`). No platform is needed — the
/// virtual `Mod` is preserved as-is.
pub fn parse_keybinding(binding: &str) -> Result<ParsedKeybinding, ParseError> {
    let raw_parts: Vec<&str> = binding
        .split('+')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect();
    if raw_parts.is_empty() {
        return Err(ParseError::Empty);
    }

    if raw_parts
        .iter()
        .any(|part| part.to_lowercase() == "doubletap")
    {
        return parse_double_tap_keybinding(&raw_parts);
    }

    let mut parsed = ParsedKeybinding::empty();
    for raw_part in raw_parts {
        if let Some(modifier) = parse_modifier_token(raw_part) {
            parsed.apply_modifier(modifier);
            continue;
        }
        if !parsed.key.is_empty() {
            return Err(ParseError::MultipleKeys);
        }
        match normalize_key_token(raw_part) {
            Some(key) => parsed.key = key,
            None => return Err(ParseError::UnknownToken(raw_part.to_string())),
        }
    }

    if parsed.key.is_empty() {
        Err(ParseError::NoKey)
    } else {
        Ok(parsed)
    }
}

/// Canonicalize a parsed chord back to its stable string form. Mirror of Orca
/// `canonicalizeParsedKeybinding` (`keybindings.ts:1327-1349`). The modifier
/// order is fixed: `Mod`, `Cmd`, `Ctrl`, `Alt`, `Shift`, then the key.
pub fn canonicalize_parsed_keybinding(parsed: &ParsedKeybinding) -> String {
    if let Some(modifier) = parsed.double_tap_modifier {
        return format!("DoubleTap+{}", modifier.as_canonical_str());
    }
    let mut parts: Vec<&str> = Vec::new();
    if parsed.is_mod {
        parts.push("Mod");
    }
    if parsed.meta {
        parts.push("Cmd");
    }
    if parsed.control {
        parts.push("Ctrl");
    }
    if parsed.alt {
        parts.push("Alt");
    }
    if parsed.shift {
        parts.push("Shift");
    }
    parts.push(&parsed.key);
    parts.join("+")
}

/// Whether a chord string is a double-tap binding. Mirror of Orca
/// `isDoubleTapBinding` (`keybindings.ts:1418`).
pub fn is_double_tap_binding(binding: &str) -> bool {
    parse_keybinding(binding)
        .ok()
        .and_then(|parsed| parsed.double_tap_modifier)
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use KeybindingPlatform::*;

    /// Parse then canonicalize, panicking on parse error — the common test path.
    fn canon(binding: &str) -> String {
        canonicalize_parsed_keybinding(&parse_keybinding(binding).expect("parses"))
    }

    // --- Ported TS oracle vectors (keybindings.test.ts) ---------------------

    #[test]
    fn parses_and_canonicalizes_editable_input() {
        // keybindings.test.ts:30-35 (canonical values; validation is M2).
        assert_eq!(canon(" ctrl + shift + p "), "Ctrl+Shift+P");
        assert_eq!(canon("shift+insert"), "Shift+Insert");
        assert_eq!(canon("cmdorctrl+p"), "Mod+P");
        assert_eq!(canon("\u{2318}+k"), "Cmd+K");
    }

    #[test]
    fn rejects_unknown_key_token() {
        // keybindings.test.ts:43 — 'Ctrl+Nope' has no recognized key.
        assert_eq!(
            parse_keybinding("Ctrl+Nope"),
            Err(ParseError::UnknownToken("Nope".to_string()))
        );
    }

    #[test]
    fn parses_and_canonicalizes_double_tap() {
        // keybindings.test.ts:47-55.
        assert_eq!(canon("DoubleTap+Shift"), "DoubleTap+Shift");
        assert_eq!(canon(" doubletap + shift "), "DoubleTap+Shift");
        assert_eq!(canon("DoubleTap+Mod"), "DoubleTap+Mod");
        assert_eq!(canon("DoubleTap+Cmd"), "DoubleTap+Cmd");
        assert_eq!(canon("DoubleTap+Alt"), "DoubleTap+Alt");
        assert_eq!(canon("DoubleTap+Ctrl"), "DoubleTap+Ctrl");
    }

    #[test]
    fn rejects_invalid_double_tap() {
        // keybindings.test.ts:58-67 (parse-level rejections; the Mod+Cmd case is
        // parse-accepted but normalize-rejected in M2, so it is not asserted here).
        assert_eq!(
            parse_keybinding("DoubleTap+Shift+P"),
            Err(ParseError::InvalidDoubleTap)
        );
        assert_eq!(
            parse_keybinding("DoubleTap+Shift+Alt"),
            Err(ParseError::InvalidDoubleTap)
        );
        assert_eq!(
            parse_keybinding("DoubleTap"),
            Err(ParseError::InvalidDoubleTap)
        );
    }

    #[test]
    fn double_tap_mod_plus_platform_is_parse_accepted() {
        // keybindings.test.ts:62 — DoubleTap+Mod+Cmd parses (canonical DoubleTap+Mod);
        // it is normalize (M2) that rejects it.
        let parsed = parse_keybinding("DoubleTap+Mod+Cmd").expect("parses");
        assert_eq!(parsed.double_tap_modifier, Some(ModifierToken::Mod));
        assert!(parsed.is_mod && parsed.meta);
        assert_eq!(canonicalize_parsed_keybinding(&parsed), "DoubleTap+Mod");
    }

    #[test]
    fn is_double_tap_binding_matches_ts() {
        // keybindings.test.ts:69-71.
        assert!(is_double_tap_binding("DoubleTap+Shift"));
        assert!(!is_double_tap_binding("Mod+P"));
        assert!(!is_double_tap_binding("not-a-binding"));
    }

    // --- Crux tests ---------------------------------------------------------

    // Crux: all three modifiers of Cmd+Shift+P are parsed. Mutating
    // parse_modifier_token to drop Shift (or any modifier) fails here.
    #[test]
    fn crux_parses_all_three_modifiers() {
        let parsed = parse_keybinding("Cmd+Shift+P").expect("parses");
        assert!(parsed.meta, "Cmd -> meta");
        assert!(parsed.shift, "Shift -> shift");
        assert_eq!(parsed.key, "P");
        assert!(!parsed.control && !parsed.alt && !parsed.is_mod);
        assert_eq!(canonicalize_parsed_keybinding(&parsed), "Cmd+Shift+P");
    }

    // Crux: canonicalize normalizes modifier ORDER. Shift+Cmd+P and Cmd+Shift+P
    // produce the same canonical string. Mutating the canonical push order fails.
    #[test]
    fn crux_canonicalize_normalizes_modifier_order() {
        assert_eq!(canon("Shift+Cmd+P"), "Cmd+Shift+P");
        assert_eq!(canon("Cmd+Shift+P"), "Cmd+Shift+P");
        assert_eq!(canon("Shift+Cmd+P"), canon("Cmd+Shift+P"));
        // Full ordering: Mod, Cmd, Ctrl, Alt, Shift, key.
        assert_eq!(
            canon("Shift+Alt+Ctrl+Cmd+Mod+P"),
            "Mod+Cmd+Ctrl+Alt+Shift+P"
        );
    }

    // Crux: Mod resolves to Meta on darwin, Control on linux/win32. Swapping the
    // darwin/else branch fails here.
    #[test]
    fn crux_mod_resolves_per_platform() {
        assert_eq!(
            resolve_modifier_token(ModifierToken::Mod, Darwin),
            PhysicalModifier::Meta
        );
        assert_eq!(
            resolve_modifier_token(ModifierToken::Mod, Linux),
            PhysicalModifier::Control
        );
        assert_eq!(
            resolve_modifier_token(ModifierToken::Mod, Win32),
            PhysicalModifier::Control
        );
    }

    #[test]
    fn resolve_physical_modifiers_are_platform_independent() {
        for platform in [Darwin, Linux, Win32] {
            assert_eq!(
                resolve_modifier_token(ModifierToken::Cmd, platform),
                PhysicalModifier::Meta
            );
            assert_eq!(
                resolve_modifier_token(ModifierToken::Ctrl, platform),
                PhysicalModifier::Control
            );
            assert_eq!(
                resolve_modifier_token(ModifierToken::Alt, platform),
                PhysicalModifier::Alt
            );
            assert_eq!(
                resolve_modifier_token(ModifierToken::Shift, platform),
                PhysicalModifier::Shift
            );
        }
    }

    // --- Token-level tests --------------------------------------------------

    #[test]
    fn modifier_token_aliases() {
        assert_eq!(parse_modifier_token("Mod"), Some(ModifierToken::Mod));
        assert_eq!(parse_modifier_token("cmdorctrl"), Some(ModifierToken::Mod));
        assert_eq!(
            parse_modifier_token("CommandOrControl"),
            Some(ModifierToken::Mod)
        );
        assert_eq!(parse_modifier_token("command"), Some(ModifierToken::Cmd));
        assert_eq!(parse_modifier_token("meta"), Some(ModifierToken::Cmd));
        assert_eq!(parse_modifier_token("\u{2318}"), Some(ModifierToken::Cmd));
        assert_eq!(parse_modifier_token("control"), Some(ModifierToken::Ctrl));
        assert_eq!(parse_modifier_token("\u{2303}"), Some(ModifierToken::Ctrl));
        assert_eq!(parse_modifier_token("option"), Some(ModifierToken::Alt));
        assert_eq!(parse_modifier_token("opt"), Some(ModifierToken::Alt));
        assert_eq!(parse_modifier_token("\u{2325}"), Some(ModifierToken::Alt));
        assert_eq!(parse_modifier_token("\u{21e7}"), Some(ModifierToken::Shift));
        assert_eq!(parse_modifier_token("P"), None);
    }

    #[test]
    fn key_token_normalization() {
        assert_eq!(normalize_key_token("a"), Some("A".to_string()));
        assert_eq!(normalize_key_token("5"), Some("5".to_string()));
        assert_eq!(normalize_key_token(" "), Some("Space".to_string()));
        assert_eq!(normalize_key_token("f7"), Some("F7".to_string()));
        assert_eq!(normalize_key_token("F24"), Some("F24".to_string()));
        assert_eq!(normalize_key_token("f25"), None);
        assert_eq!(normalize_key_token("["), Some("BracketLeft".to_string()));
        assert_eq!(normalize_key_token("return"), Some("Enter".to_string()));
        assert_eq!(normalize_key_token("pgup"), Some("PageUp".to_string()));
        assert_eq!(
            normalize_key_token("arrowdown"),
            Some("ArrowDown".to_string())
        );
        assert_eq!(normalize_key_token("Nope"), None);
        assert_eq!(normalize_key_token("  "), None);
    }

    #[test]
    fn empty_binding_is_error() {
        assert_eq!(parse_keybinding(""), Err(ParseError::Empty));
        assert_eq!(parse_keybinding("+++"), Err(ParseError::Empty));
    }

    #[test]
    fn modifiers_without_key_is_error() {
        assert_eq!(parse_keybinding("Ctrl+Shift"), Err(ParseError::NoKey));
    }

    #[test]
    fn two_keys_is_error() {
        assert_eq!(parse_keybinding("Ctrl+A+B"), Err(ParseError::MultipleKeys));
    }
}
