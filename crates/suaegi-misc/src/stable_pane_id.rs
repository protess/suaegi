//! Stable pane-id / pane-key validation — verbatim port of Orca's
//! `src/shared/stable-pane-id.ts` (@ v1.4.150-rc.0).
//!
//! **No hashing/crypto**: the "stable id" is the caller-provided terminal-leaf
//! UUID itself; this module only validates and composes `${tab}:${leaf}`.
//!
//! Three oracle-silent traps are pinned below:
//! - UUID matching is **lowercase-exact** (`[0-9a-f]`, version `[1-5]`, variant
//!   `[89ab]`) — uppercase is rejected; NEVER `to_ascii_lowercase`.
//! - The legacy numeric check is **ASCII digits only** (`[0-9]`), NEVER Unicode
//!   `\d`/`char::is_numeric` (which would accept Arabic-Indic/fullwidth digits).
//! - `.trim()` uses the ECMAScript whitespace set ([`crate::js_ws::js_trim`]),
//!   NOT `str::trim` (they diverge at U+FEFF / U+0085).

use crate::js_ws::js_trim;

/// Lowercase hex digit `[0-9a-f]` (case-sensitive — NO uppercase).
fn is_lower_hex(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f')
}

/// Hand-rolled equivalent of
/// `/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/`
/// (no `i` flag): exactly 36 ASCII chars, lowercase hex, version `[1-5]`,
/// variant `[89ab]`.
fn matches_uuid(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 36 {
        return false;
    }
    // Hyphen anchors.
    if b[8] != b'-' || b[13] != b'-' || b[18] != b'-' || b[23] != b'-' {
        return false;
    }
    // Group 1: 8 hex.
    if !b[0..8].iter().all(|&c| is_lower_hex(c)) {
        return false;
    }
    // Group 2: 4 hex.
    if !b[9..13].iter().all(|&c| is_lower_hex(c)) {
        return false;
    }
    // Group 3: version [1-5] + 3 hex.
    if !matches!(b[14], b'1'..=b'5') {
        return false;
    }
    if !b[15..18].iter().all(|&c| is_lower_hex(c)) {
        return false;
    }
    // Group 4: variant [89ab] + 3 hex.
    if !matches!(b[19], b'8' | b'9' | b'a' | b'b') {
        return false;
    }
    if !b[20..23].iter().all(|&c| is_lower_hex(c)) {
        return false;
    }
    // Group 5: 12 hex.
    b[24..36].iter().all(|&c| is_lower_hex(c))
}

/// `true` when `value` is a valid stable pane id (terminal-leaf UUID).
pub fn is_stable_pane_id(value: &str) -> bool {
    matches_uuid(value)
}

/// Alias for [`is_stable_pane_id`] (terminal leaf ids ARE stable pane ids).
pub fn is_terminal_leaf_id(value: &str) -> bool {
    is_stable_pane_id(value)
}

/// Error from [`make_pane_key`] — mirrors Orca's two `throw new Error(...)`
/// sites. Message strings are preserved verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakePaneKeyError {
    /// `tabId` was empty or contained `":"`.
    TabId,
    /// `stableLeafId` was not a UUID.
    LeafId,
}

impl core::fmt::Display for MakePaneKeyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MakePaneKeyError::TabId => {
                f.write_str("tabId must be non-empty and must not contain \":\"")
            }
            MakePaneKeyError::LeafId => f.write_str("stableLeafId must be a UUID"),
        }
    }
}

impl std::error::Error for MakePaneKeyError {}

/// Compose `${tabId}:${stableLeafId}`. Errors if `tabId` is empty or contains
/// `":"`, or if `stableLeafId` is not a UUID.
pub fn make_pane_key(tab_id: &str, stable_leaf_id: &str) -> Result<String, MakePaneKeyError> {
    if tab_id.is_empty() || tab_id.contains(':') {
        return Err(MakePaneKeyError::TabId);
    }
    if !is_terminal_leaf_id(stable_leaf_id) {
        return Err(MakePaneKeyError::LeafId);
    }
    Ok(format!("{tab_id}:{stable_leaf_id}"))
}

/// Parsed result of [`parse_pane_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPaneKey {
    pub tab_id: String,
    pub leaf_id: String,
    pub stable_pane_id: String,
}

/// Parse `tabId:leafId`, requiring exactly one `:` with a non-empty tab and a
/// UUID leaf. Returns `None` for anything ambiguous or non-UUID.
pub fn parse_pane_key(pane_key: &str) -> Option<ParsedPaneKey> {
    let first = pane_key.find(':');
    // first <= 0 : no colon, or colon at index 0 (empty tab).
    let first = match first {
        None | Some(0) => return None,
        Some(i) => i,
    };
    // first !== lastIndexOf(':') : more than one colon.
    if Some(first) != pane_key.rfind(':') {
        return None;
    }
    // first === length - 1 : empty leaf.
    if first == pane_key.len() - 1 {
        return None;
    }
    let leaf_id = &pane_key[first + 1..];
    if !is_terminal_leaf_id(leaf_id) {
        return None;
    }
    Some(ParsedPaneKey {
        tab_id: pane_key[..first].to_string(),
        leaf_id: leaf_id.to_string(),
        stable_pane_id: leaf_id.to_string(),
    })
}

/// Parsed result of [`parse_legacy_numeric_pane_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyNumericPaneKey {
    pub tab_id: String,
    pub numeric_pane_id: String,
    pub pane_key: String,
}

/// Parse a legacy `tabId:12` migration alias: one `:`, non-empty tab, and an
/// **ASCII-digits-only** numeric id. The returned `pane_key` is the trimmed
/// input. Over-long input (`> 256`) → `None`.
///
/// (Orca's `paneKey: unknown` typeof-string guard is modeled by the `&str`
/// parameter — callers pass only strings.)
pub fn parse_legacy_numeric_pane_key(pane_key: &str) -> Option<LegacyNumericPaneKey> {
    // JS `paneKey.length > 256` counts UTF-16 code units, not scalars — an astral
    // char is 2 units. `encode_utf16().count()` reproduces that exactly (a scalar
    // `chars().count()` would diverge on astral input).
    if pane_key.encode_utf16().count() > 256 {
        return None;
    }
    let trimmed = js_trim(pane_key);
    let delimiter = match trimmed.find(':') {
        None | Some(0) => return None,
        Some(i) => i,
    };
    if Some(delimiter) != trimmed.rfind(':') || delimiter == trimmed.len() - 1 {
        return None;
    }
    let numeric_pane_id = &trimmed[delimiter + 1..];
    // /^\d+$/ — ASCII digits only, at least one.
    if numeric_pane_id.is_empty() || !numeric_pane_id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(LegacyNumericPaneKey {
        tab_id: trimmed[..delimiter].to_string(),
        numeric_pane_id: numeric_pane_id.to_string(),
        pane_key: trimmed.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAF_ID: &str = "11111111-1111-4111-8111-111111111111";

    // Oracle: stable-pane-id.test.ts

    #[test]
    fn recognizes_uuid_leaf_ids_as_stable_pane_ids() {
        assert!(is_stable_pane_id(LEAF_ID));
        assert!(is_terminal_leaf_id(LEAF_ID));
    }

    #[test]
    fn rejects_legacy_numeric_ids_and_malformed_uuids() {
        for value in ["1", "pane:1", "11111111-1111-6111-8111-111111111111", ""] {
            assert!(
                !is_stable_pane_id(value),
                "expected {value:?} to be rejected"
            );
            assert!(!is_terminal_leaf_id(value));
        }
    }

    #[test]
    fn builds_and_parses_pane_keys() {
        let pane_key = make_pane_key("tab-1", LEAF_ID).unwrap();
        assert_eq!(pane_key, format!("tab-1:{LEAF_ID}"));
        assert_eq!(
            parse_pane_key(&pane_key),
            Some(ParsedPaneKey {
                tab_id: "tab-1".to_string(),
                leaf_id: LEAF_ID.to_string(),
                stable_pane_id: LEAF_ID.to_string(),
            })
        );
    }

    #[test]
    fn rejects_ambiguous_tab_ids_and_non_uuid_leaf_ids_when_building() {
        assert_eq!(make_pane_key("", LEAF_ID), Err(MakePaneKeyError::TabId));
        assert_eq!(
            make_pane_key("tab:1", LEAF_ID),
            Err(MakePaneKeyError::TabId)
        );
        assert_eq!(make_pane_key("tab-1", "1"), Err(MakePaneKeyError::LeafId));
        // Message strings preserved verbatim.
        assert!(MakePaneKeyError::TabId.to_string().contains("tabId"));
        assert!(MakePaneKeyError::LeafId.to_string().contains("UUID"));
    }

    #[test]
    fn rejects_ambiguous_or_legacy_pane_key_inputs_when_parsing() {
        assert_eq!(parse_pane_key("tab-1:1"), None);
        assert_eq!(parse_pane_key(&format!("tab:1:{LEAF_ID}")), None);
        assert_eq!(parse_pane_key(&format!(":{LEAF_ID}")), None);
        assert_eq!(parse_pane_key("tab-1:"), None);
    }

    #[test]
    fn parses_legacy_numeric_pane_keys_only_for_migration_aliases() {
        assert_eq!(
            parse_legacy_numeric_pane_key(" tab-1:12 "),
            Some(LegacyNumericPaneKey {
                tab_id: "tab-1".to_string(),
                numeric_pane_id: "12".to_string(),
                pane_key: "tab-1:12".to_string(),
            })
        );
        assert_eq!(
            parse_legacy_numeric_pane_key(&format!("tab-1:{LEAF_ID}")),
            None
        );
        assert_eq!(parse_legacy_numeric_pane_key("tab:1:12"), None);
    }

    // Mandatory extra pins (oracle-silent):

    /// UUID matching is lowercase-exact — uppercase hex is rejected (NO
    /// case-folding).
    #[test]
    fn pin_uppercase_uuid_rejected() {
        assert!(!is_stable_pane_id("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA"));
    }

    /// Variant nibble must be `[89ab]` — `c` is rejected (version alone is
    /// covered by the oracle).
    #[test]
    fn pin_variant_violation_rejected() {
        assert!(!is_stable_pane_id("11111111-1111-4111-c111-111111111111"));
    }

    /// Legacy numeric check is ASCII-digit only — Arabic-Indic digits `١٢` are
    /// NOT digits (Rust Unicode `\d`/`is_numeric` would wrongly accept them).
    #[test]
    fn pin_arabic_indic_digits_rejected() {
        assert_eq!(
            parse_legacy_numeric_pane_key("tab-1:\u{0661}\u{0662}"),
            None
        );
    }

    /// `.trim()` strips U+FEFF (JS whitespace) from a legacy key.
    #[test]
    fn pin_feff_wrapped_legacy_key_trims() {
        assert_eq!(
            parse_legacy_numeric_pane_key("\u{FEFF}tab-1:12\u{FEFF}"),
            Some(LegacyNumericPaneKey {
                tab_id: "tab-1".to_string(),
                numeric_pane_id: "12".to_string(),
                pane_key: "tab-1:12".to_string(),
            })
        );
    }

    /// Review nit: the `> 256` cap counts JS UTF-16 code units, not Rust scalars.
    /// 200 astral chars = 400 UTF-16 units (> 256) → rejected, matching JS
    /// `paneKey.length`. *Mutation:* `encode_utf16().count()` → `chars().count()`
    /// counts 203 scalars (≤ 256) and wrongly returns `Some` → this fails.
    #[test]
    fn pin_length_cap_counts_utf16_units() {
        let key = format!("{}:12", "\u{1F600}".repeat(200)); // 200 emoji + ":12"
        assert_eq!(parse_legacy_numeric_pane_key(&key), None);
    }
}
