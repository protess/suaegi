//! VERBATIM port of the sensitive-pattern / env-masking layer of Orca's
//! `src/shared/mcp-config.ts` (@ v1.4.150-rc.0), milestone M2a.
//!
//! Ported: `SENSITIVE_ENV_KEY_PATTERN`/`SENSITIVE_ENV_VALUE_PATTERN`
//! (`O:103-106`) as hand-rolled predicates, [`MASKED_ENV_VALUE`] (`O:152`),
//! and [`mask_mcp_env`] (`O:142-156`).
//!
//! # W6 — hand-rolled, no `regex` crate
//! The key pattern (`O:103-104`) is `/(...)/i` — case-insensitive, no `/u`.
//! JS's `/i` WITHOUT `/u` folds ASCII case only; it does NOT fold non-ASCII
//! letters that happen to share a Unicode "same letter, different case" story
//! (e.g. U+017F LATIN SMALL LETTER LONG S folding to `s` requires full
//! Unicode case folding, which `/i`-without-`/u` does not perform; U+212A
//! KELVIN SIGN folding to `k` is the same story in reverse). Rust's `regex`
//! crate's `(?i)` IS Unicode-aware and would match both — silently widening
//! what counts as "sensitive" relative to Orca. Hand-rolling with
//! `str::to_ascii_lowercase` (which only touches `A-Z`) reproduces the
//! ASCII-only fold exactly ([[js-lowercase-two-mechanisms]]).
//!
//! The value pattern (`O:105-106`) has NO flags (case-SENSITIVE) — do not
//! share the lowercasing helper between the two predicates.

use crate::json::{js_string_of, JsonValue};

/// `O:152` — `'••••••••'`: 8 x U+2022 BULLET. No length or prefix
/// preservation relative to the original value.
pub const MASKED_ENV_VALUE: &str =
    "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// `O:103-104` substrings, `api[_-]?key`/`private[_-]?key` expanded to their
/// 3 literal forms each. Order does not matter — this is `.any()`, not a
/// leftmost-match search.
const SENSITIVE_KEY_SUBSTRINGS: &[&str] = &[
    "apikey",
    "api_key",
    "api-key",
    "auth",
    "bearer",
    "cookie",
    "credential",
    "password",
    "privatekey",
    "private_key",
    "private-key",
    "secret",
    "session",
    "token",
];

/// `O:103-104` — `SENSITIVE_ENV_KEY_PATTERN.test(key)`. ASCII-only
/// lowercasing, unanchored substring search.
fn sensitive_env_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEY_SUBSTRINGS
        .iter()
        .any(|needle| lowered.contains(needle))
}

/// `sk-` + >=12 chars of `[A-Za-z0-9_-]`.
fn is_sk_run_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// `gh[pousr]_` + >=12 chars of `[A-Za-z0-9_]` (note: NO `-` in this charset,
/// unlike the other two — the three charsets deliberately differ).
fn is_gh_run_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `xox[baprs]-` + >=12 chars of `[A-Za-z0-9-]` (note: NO `_` in this
/// charset).
fn is_xox_run_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// Scans every byte position in `haystack` for any of `prefixes` immediately
/// followed by a run of `min_run` or more bytes satisfying `is_run_byte`.
/// Unanchored (matches anywhere), case-sensitive (prefixes must already be
/// the exact case to match — this function does no lowercasing).
fn has_prefixed_run(
    haystack: &[u8],
    prefixes: &[&[u8]],
    min_run: usize,
    is_run_byte: fn(u8) -> bool,
) -> bool {
    for start in 0..haystack.len() {
        for prefix in prefixes {
            if haystack[start..].starts_with(prefix) {
                let run_start = start + prefix.len();
                let run_len = haystack[run_start..]
                    .iter()
                    .take_while(|&&byte| is_run_byte(byte))
                    .count();
                if run_len >= min_run {
                    return true;
                }
            }
        }
    }
    false
}

/// `O:105-106` — `SENSITIVE_ENV_VALUE_PATTERN.test(value)`. Case-sensitive,
/// no lowercasing. All three prefix families are ASCII, and `is_*_run_byte`
/// only ever accepts ASCII bytes, so scanning `value.as_bytes()` directly
/// never risks slicing a multi-byte UTF-8 char in half: any non-ASCII lead or
/// continuation byte (>= 0x80) simply fails every `is_run_byte` check and
/// terminates the run, exactly as a codepoint outside `[A-Za-z0-9_-]` would
/// terminate the equivalent regex character class.
fn sensitive_env_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    has_prefixed_run(bytes, &[b"sk-"], 12, is_sk_run_byte)
        || has_prefixed_run(
            bytes,
            &[b"ghp_", b"gho_", b"ghu_", b"ghs_", b"ghr_"],
            12,
            is_gh_run_byte,
        )
        || has_prefixed_run(
            bytes,
            &[b"xoxb-", b"xoxa-", b"xoxp-", b"xoxr-", b"xoxs-"],
            12,
            is_xox_run_byte,
        )
}

// ---------------------------------------------------------------------------
// mask_mcp_env
// ---------------------------------------------------------------------------

/// `O:142-156`.
///
/// # W8 — guard
/// JS: `!env || typeof env !== 'object' || Array.isArray(env)`. In this
/// value model that collapses to exactly "is it a [`JsonValue::Object`]?" —
/// [`JsonValue::Object`] (including the EMPTY object) is always JS-truthy and
/// always `typeof === 'object'` and never an array, so it alone survives the
/// guard; every other variant (including `Null`, which is `typeof ===
/// 'object'` in JS but fails the leading `!env` falsy check) is rejected.
/// `{}` therefore yields `Some(vec![])`, never `None` — do not fold an empty
/// result into `None`.
///
/// # W7 — mask condition applies to the COERCED value
/// `O:149-151` computes `value` via `String(rawValue)` (see
/// [`js_string_of`]) BEFORE testing the value pattern, so a non-string env
/// value (e.g. an array containing a token) can still trigger masking.
///
/// # W9 — order preservation
/// Iterates `env`'s entries in the order already established by
/// [`crate::json::parse_json`] (W2/W3 applied at parse time), so the
/// returned `Vec` reflects JS's `Object.entries` order.
pub fn mask_mcp_env(env: &JsonValue) -> Option<Vec<(String, String)>> {
    let entries = match env {
        JsonValue::Object(entries) => entries,
        _ => return None,
    };

    let mut masked = Vec::with_capacity(entries.len());
    for (key, raw_value) in entries {
        let value = match raw_value {
            JsonValue::String(s) => s.clone(),
            other => js_string_of(other),
        };
        let masked_value = if sensitive_env_key(key) || sensitive_env_value(&value) {
            MASKED_ENV_VALUE.to_string()
        } else {
            value
        };
        masked.push((key.clone(), masked_value));
    }
    Some(masked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse_json;

    // -- Oracle test 6 (mcp-config.test.ts:122-134) --------------------------

    #[test]
    fn oracle_masks_env_values_that_look_sensitive_by_key_or_value() {
        let env =
            parse_json(r#"{"NORMAL":"visible","PASSWORD":"hunter2","MAYBE":"sk-abc123456789xyz"}"#)
                .expect("valid JSON");
        assert_eq!(
            mask_mcp_env(&env),
            Some(vec![
                ("NORMAL".to_string(), "visible".to_string()),
                ("PASSWORD".to_string(), MASKED_ENV_VALUE.to_string()),
                ("MAYBE".to_string(), MASKED_ENV_VALUE.to_string()),
            ])
        );
    }

    // -- W6: key pattern -------------------------------------------------------

    #[test]
    fn w6_ascii_only_fold_does_not_match_latin_small_letter_long_s() {
        // U+017F ('ſ') is a non-ASCII "long s"; folding it to ASCII 's' would
        // require full Unicode case folding, which `/i`-without-`/u` (and our
        // `to_ascii_lowercase` port) does not perform.
        assert!(!sensitive_env_key("\u{017F}ecret"));
    }

    #[test]
    fn w6_ascii_only_fold_does_not_match_kelvin_sign() {
        // "TOKEN" with the 'K' replaced by U+212A KELVIN SIGN.
        let key = format!("TO{}EN", '\u{212A}');
        assert!(!sensitive_env_key(&key));
    }

    #[test]
    fn w6_ascii_case_insensitive_matches() {
        assert!(sensitive_env_key("SECRET"));
        assert!(sensitive_env_key("TOKEN"));
        assert!(sensitive_env_key("ApIkEy"));
        assert!(sensitive_env_key("API-KEY"));
        assert!(sensitive_env_key("API_KEY"));
        assert!(sensitive_env_key("MY_AUTHORITY_X"));
    }

    #[test]
    fn w6_key_pattern_non_matches_are_not_swallowed_by_a_looser_boundary() {
        assert!(!sensitive_env_key("API KEY"));
        assert!(!sensitive_env_key("APIIKEY"));
    }

    // -- W6: value pattern ------------------------------------------------------

    #[test]
    fn w6_value_pattern_is_case_sensitive() {
        assert!(!sensitive_env_value("SK-abcdefghijkl"));
        assert!(sensitive_env_value("sk-abcdefghijkl"));
    }

    #[test]
    fn w6_value_pattern_boundary_is_exactly_twelve_run_chars() {
        assert!(!sensitive_env_value("sk-12345678901")); // 11 chars after `sk-`
        assert!(sensitive_env_value("sk-123456789012")); // 12 chars after `sk-`
    }

    #[test]
    fn w6_gh_prefix_family_is_a_fixed_five_letter_set() {
        assert!(!sensitive_env_value("ghx_abcdefghijkl"));
        assert!(sensitive_env_value("ghp_abcdefghijkl"));
    }

    #[test]
    fn w6_xox_prefix_boundary_is_exactly_twelve_run_chars() {
        assert!(sensitive_env_value("xoxb-abcdefghijkl")); // 12 chars after `xoxb-`
        assert!(!sensitive_env_value("xoxb-abcdefghijk")); // 11 chars after `xoxb-`
    }

    #[test]
    fn w6_value_pattern_is_unanchored() {
        assert!(sensitive_env_value("prefix sk-abcdefghijkl suffix"));
    }

    // -- W7: array value is String()-coerced before the value pattern runs ---

    #[test]
    fn w7_array_value_is_masked_via_its_coerced_string_form() {
        let env = parse_json(r#"{"K":["sk-abcdefghijkl"]}"#).expect("valid JSON");
        assert_eq!(
            mask_mcp_env(&env),
            Some(vec![("K".to_string(), MASKED_ENV_VALUE.to_string())])
        );
    }

    // -- W8: guard --------------------------------------------------------------

    #[test]
    fn w8_empty_object_is_some_empty_not_none() {
        let env = parse_json("{}").expect("valid JSON");
        assert_eq!(mask_mcp_env(&env), Some(Vec::new()));
    }

    #[test]
    fn w8_non_object_variants_are_all_none() {
        assert_eq!(mask_mcp_env(&parse_json("null").unwrap()), None);
        assert_eq!(mask_mcp_env(&parse_json("[]").unwrap()), None);
        assert_eq!(mask_mcp_env(&parse_json("\"str\"").unwrap()), None);
        assert_eq!(mask_mcp_env(&parse_json("0").unwrap()), None);
    }

    // -- W9: env key order (W2 hoisting + insertion order) survives masking --

    #[test]
    fn w9_env_key_order_is_preserved_and_hoisted() {
        let env = parse_json(r#"{"b":"vb","2":"v2","a":"va"}"#).expect("valid JSON");
        assert_eq!(
            mask_mcp_env(&env),
            Some(vec![
                ("2".to_string(), "v2".to_string()),
                ("b".to_string(), "vb".to_string()),
                ("a".to_string(), "va".to_string()),
            ])
        );
    }
}
