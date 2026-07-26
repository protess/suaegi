//! Codex CLI auth-error detection/extraction — verbatim port of Orca's
//! `src/shared/codex-auth-errors.ts` (@ v1.4.150-rc.0).
//!
//! Why: app-server rejects account/rateLimits/read with an auth error when
//! `auth.json` holds only an API key; without classification the fetcher
//! falls through to a hidden PTY probe that can only time out (15s) on every
//! refresh.
//!
//! # ASCII-only case folding (F0a/F1)
//! JS's non-`u` `/i` flag does NOT fold non-ASCII characters into ASCII:
//! `/k/i` does not match U+212A KELVIN SIGN, and `/s/i` does not match U+017F
//! LATIN SMALL LETTER LONG S. All 10 source patterns are pure ASCII, so
//! lowercasing the input with `to_ascii_lowercase` (NOT `str::to_lowercase`,
//! which folds non-ASCII characters and can even expand length, e.g. `İ` →
//! `i̇`) and testing `contains` against each of the 15 expanded literals is
//! exactly equivalent to the 10 source `/i` regexes — no closer and no
//! further.
//!
//! # ANSI stripping is CSI-only (F2)
//! `ANSI_ESCAPE_RE` is `/\x1b\[[0-9;]*[a-zA-Z]/g`: ESC, then `[`, then a run
//! of digits/semicolons, then exactly one ASCII letter. OSC (`ESC ]`), a bare
//! ESC, and a CSI whose final byte is not an ASCII letter are NOT stripped.
//! Because the digit/semicolon class and the letter class are disjoint, a
//! failed match at a given ESC never has a shorter-match fallback to
//! backtrack into (see [`strip_ansi_csi`] for the byte-safe scan).
//!
//! # UTF-16 cap, not bytes (F3)
//! `.slice(0, 4_000)` (both call sites) and the `cleanPrefix.length < 4_000`
//! accumulation guard all count **UTF-16 code units** (JS `.length`/`.slice`
//! semantics), snapped down to a char boundary so a raw byte cut can never
//! split — and panic on — a multi-byte UTF-8 character.
//!
//! # Line iteration uses a `<=` final-chunk guard (F4)
//! [`iterate_codex_output_lines`] mirrors `iterateCodexOutputLines`'s
//! `lineStart <= output.length` guard, which is an *invariant* here (never
//! `false`) — so it always yields exactly one final (possibly empty) trailing
//! chunk. This is deliberately different from
//! [`crate::process_output_field_scanner::iterate_process_output_lines`],
//! whose `<` guard suppresses that trailing empty chunk. They are separate,
//! unrelated iterators — not a shared implementation.

use crate::js_ws::js_trim;

/// The 10 source `/…/i` patterns expanded into their 15 literal alternatives,
/// already lowercased. Every `(?:a|b)` / `(?:a|b|c)` alternation in the
/// source becomes 2 or 3 literals here (F1).
const CODEX_AUTH_ERROR_LITERALS: [&str; 15] = [
    "access token could not be refreshed",
    "authentication session could not be refreshed",
    "refresh token has expired",
    "refresh token was already used",
    "refresh token was revoked",
    "you have since logged out or signed in to another account",
    "please log out and sign in again",
    "please sign in again",
    "please reauthenticate",
    "not logged in",
    "token data is not available",
    "auth is missing",
    "auth tokens are missing",
    "auth does not expose",
    // Why: app-server rejects account/rateLimits/read with this when
    // auth.json holds only an API key; without classification the fetcher
    // falls through to a hidden PTY probe that can only time out (15s) on
    // every refresh.
    "chatgpt authentication required",
];

/// UTF-16 cap for [`extract_codex_auth_error`]'s slices and accumulation
/// guard (F3), counted in **UTF-16 code units**, never raw bytes.
const CODEX_AUTH_ERROR_MAX_UTF16_UNITS: usize = 4_000;

/// Largest byte offset `<= s.len()` such that the UTF-16-code-unit count of
/// `s[..offset]` does not exceed `max_utf16_units`, snapped down to a char
/// boundary. Duplicated from the identical helper in
/// `process_output_field_scanner.rs` / `command_token_scanner.rs` rather than
/// shared — each module in this crate is self-contained (only `js_ws` is the
/// explicit shared exception).
fn utf16_slice_prefix(s: &str, max_utf16_units: usize) -> &str {
    let mut units = 0usize;
    for (byte_offset, ch) in s.char_indices() {
        let next_units = units + ch.len_utf16();
        if next_units > max_utf16_units {
            return &s[..byte_offset];
        }
        units = next_units;
    }
    s
}

/// UTF-16 code-unit length of `s` (JS `.length` semantics).
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Strip only ANSI CSI sequences (`ESC` `[` + `[0-9;]*` + one ASCII letter)
/// from `line`, mirroring `ANSI_ESCAPE_RE` (F2). OSC (`ESC ]`), a bare ESC,
/// and a CSI whose final byte is not an ASCII letter are left untouched.
fn strip_ansi_csi(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j] == b';' || bytes[j].is_ascii_digit()) {
                j += 1;
            }
            if j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                // Full CSI match: ESC, `[`, digit/`;` run, one ASCII letter —
                // drop it entirely and resume scanning right after it.
                i = j + 1;
                continue;
            }
        }
        // No CSI match starting at `i` (a bare ESC, an ESC not followed by
        // `[`, or a CSI with no valid letter final): copy exactly one char
        // and advance past it. `i` is always a char boundary here — every
        // step above only ever advances over single-byte ASCII (ESC, `[`,
        // digits, `;`, a letter) or over one full char via `chars().next()`.
        let ch = line[i..]
            .chars()
            .next()
            .expect("i < bytes.len() implies a char at i");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// True when `error` (after JS-trimming) contains any of the 15 auth-error
/// literals, case-insensitively via ASCII-only folding (F1). `None`, empty,
/// or whitespace-only input is not an error.
pub fn is_codex_auth_error(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    let message = js_trim(error);
    if message.is_empty() {
        return false;
    }
    let lowered = message.to_ascii_lowercase();
    CODEX_AUTH_ERROR_LITERALS
        .iter()
        .any(|literal| lowered.contains(literal))
}

/// Scan `output` line-by-line (F4 `<=` iterator) for the first line that,
/// after CSI stripping (F2) and JS-trimming, matches
/// [`is_codex_auth_error`], returning that **whole cleaned line** (not the
/// match span — F8), capped to 4,000 UTF-16 units (F3). `None`/empty input
/// returns `None`.
pub fn extract_codex_auth_error(output: Option<&str>) -> Option<String> {
    let output = match output {
        Some(o) if !o.is_empty() => o,
        _ => return None,
    };

    let mut clean_prefix = String::new();
    for raw_line in iterate_codex_output_lines(output) {
        let stripped = strip_ansi_csi(raw_line);
        let line = js_trim(&stripped);
        if line.is_empty() {
            continue;
        }
        if is_codex_auth_error(Some(line)) {
            return Some(utf16_slice_prefix(line, CODEX_AUTH_ERROR_MAX_UTF16_UNITS).to_string());
        }
        if utf16_len(&clean_prefix) < CODEX_AUTH_ERROR_MAX_UTF16_UNITS {
            clean_prefix = if clean_prefix.is_empty() {
                line.to_string()
            } else {
                format!("{clean_prefix}\n{line}")
            };
            clean_prefix =
                utf16_slice_prefix(&clean_prefix, CODEX_AUTH_ERROR_MAX_UTF16_UNITS).to_string();
        }
    }

    // F0b/F8: unreachable in practice. `clean_prefix` is built only from
    // lines that individually already failed `is_codex_auth_error`, joined by
    // literal `\n` bytes; since none of the 15 literals contain `\n`, no join
    // across a line boundary can complete a literal that no single
    // constituent line already contained on its own. Ported anyway for
    // fidelity with the TS source's trailing `return
    // isCodexAuthError(cleanPrefix) ? cleanPrefix : null` (line 46).
    if is_codex_auth_error(Some(&clean_prefix)) {
        Some(clean_prefix)
    } else {
        None
    }
}

/// Lazily split `output` into lines on LF, CRLF, or lone CR (F4). Unlike
/// [`crate::process_output_field_scanner::iterate_process_output_lines`]
/// (`<` guard, no trailing synthetic empty line), this iterator's `<=` guard
/// is an invariant that always holds, so it **always** yields exactly one
/// final (possibly empty) trailing chunk: `""` → `[""]`, `"a\n"` → `["a",
/// ""]`. These are separate, independently-ported iterators — do not unify
/// them.
pub fn iterate_codex_output_lines(output: &str) -> CodexOutputLines<'_> {
    CodexOutputLines {
        output,
        pos: 0,
        line_start: 0,
        done: false,
    }
}

/// Iterator returned by [`iterate_codex_output_lines`].
pub struct CodexOutputLines<'a> {
    output: &'a str,
    pos: usize,
    line_start: usize,
    done: bool,
}

impl<'a> Iterator for CodexOutputLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.done {
            return None;
        }
        let bytes = self.output.as_bytes();
        let mut index = self.pos;
        while index < bytes.len() {
            let b = bytes[index];
            if b != b'\n' && b != b'\r' {
                index += 1;
                continue;
            }
            let line = &self.output[self.line_start..index];
            let mut advance_to = index + 1;
            if b == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                advance_to += 1;
            }
            self.line_start = advance_to;
            self.pos = advance_to;
            return Some(line);
        }
        // F4: unconditional final chunk (the source's `lineStart <=
        // output.length` guard always holds here).
        self.done = true;
        Some(&self.output[self.line_start..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: codex-auth-errors.test.ts

    #[test]
    fn matches_codex_authentication_refresh_failures() {
        assert!(is_codex_auth_error(Some(
            "Access token could not be refreshed"
        )));
        assert!(!is_codex_auth_error(Some("plain provider error")));
        assert!(!is_codex_auth_error(None));
    }

    #[test]
    fn returns_the_first_matching_auth_line() {
        assert_eq!(
            extract_codex_auth_error(Some(
                "startup log\nERROR: not logged in. Please sign in again.\nmore log"
            )),
            Some("ERROR: not logged in. Please sign in again.".to_string())
        );
    }

    #[test]
    fn strips_ansi_color_from_matching_lines() {
        assert_eq!(
            extract_codex_auth_error(Some("\u{1b}[31mnot logged in\u{1b}[0m\n")),
            Some("not logged in".to_string())
        );
    }

    /// Functional half of the oracle's "scans newline-heavy output without
    /// line-array splitting" test: 10,000 CRLF-terminated non-matching lines
    /// followed by one matching line still resolves correctly. (The oracle's
    /// `String.prototype.split` spy assertion has no Rust analog — this
    /// iterator is lazy by construction: `CodexOutputLines::next` only ever
    /// scans forward from the previous position, never materializing a
    /// line array.)
    #[test]
    fn scans_newline_heavy_output_without_materializing_a_line_array() {
        let output = format!(
            "{}please reauthenticate\r\n",
            "startup log\r\n".repeat(10_000)
        );
        assert_eq!(
            extract_codex_auth_error(Some(&output)),
            Some("please reauthenticate".to_string())
        );
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(
            extract_codex_auth_error(Some("startup log\nmore log\n")),
            None
        );
    }

    #[test]
    fn none_and_empty_output_return_none() {
        assert_eq!(extract_codex_auth_error(None), None);
        assert_eq!(extract_codex_auth_error(Some("")), None);
    }

    // F1: each of the 15 literals individually.

    #[test]
    fn pin_each_of_the_15_literals_matches_individually() {
        for literal in CODEX_AUTH_ERROR_LITERALS {
            assert!(
                is_codex_auth_error(Some(literal)),
                "literal did not match itself: {literal:?}"
            );
            // Case-insensitive (ASCII fold) against the literal's uppercase form.
            assert!(
                is_codex_auth_error(Some(&literal.to_ascii_uppercase())),
                "uppercased literal did not match: {literal:?}"
            );
        }
    }

    /// F0a/F1 crux pin: U+212A KELVIN SIGN must NOT fold to ASCII `k`. Built
    /// from the "access token could not be refreshed" literal (which
    /// contains a real `k` in "token") with that `k` replaced by U+212A —
    /// `to_ascii_lowercase` leaves U+212A untouched (unlike full
    /// `to_lowercase`, which maps it to `k`), so this must NOT match.
    #[test]
    fn pin_kelvin_sign_does_not_fold_to_ascii_k() {
        let line = "access to\u{212A}en could not be refreshed";
        // Precondition: a real Rust `to_lowercase` WOULD fold U+212A to 'k',
        // proving the divergence this pin guards against.
        assert_eq!('\u{212A}'.to_lowercase().collect::<String>(), "k");
        assert!(!is_codex_auth_error(Some(line)));
    }

    /// F0a/F1 crux pin: U+017F LATIN SMALL LETTER LONG S must NOT fold to
    /// ASCII `s`. Built from "please sign in again" with the `s` in "sign"
    /// replaced by U+017F.
    #[test]
    fn pin_long_s_does_not_fold_to_ascii_s() {
        let line = "please \u{17F}ign in again";
        assert!(!is_codex_auth_error(Some(line)));
    }

    // F2: OSC / bare ESC survive; non-letter-final CSI is not stripped.

    #[test]
    fn pin_osc_sequence_is_not_stripped() {
        let line = "\u{1b}]0;title\u{7}not logged in";
        assert_eq!(strip_ansi_csi(line), line);
    }

    #[test]
    fn pin_bare_esc_is_not_stripped() {
        let line = "\u{1b}not logged in";
        assert_eq!(strip_ansi_csi(line), line);
    }

    #[test]
    fn pin_csi_with_non_letter_final_is_not_stripped() {
        let line = "\u{1b}[123!not logged in";
        assert_eq!(strip_ansi_csi(line), line);
    }

    // F3: exact 4,000-UTF-16-unit cap boundary and an astral straddle.

    #[test]
    fn pin_cap_boundary_at_exactly_4000_units_keeps_everything() {
        let line = format!("not logged in{}", "a".repeat(4_000 - 13));
        assert_eq!(utf16_len(&line), 4_000);
        assert_eq!(extract_codex_auth_error(Some(&line)), Some(line.clone()));
    }

    #[test]
    fn pin_cap_boundary_at_4001_units_truncates_to_4000() {
        let line = format!("not logged in{}", "a".repeat(4_001 - 13));
        assert_eq!(utf16_len(&line), 4_001);
        let expected: String = line.chars().take(4_000).collect();
        assert_eq!(extract_codex_auth_error(Some(&line)), Some(expected));
    }

    /// An astral character (2 UTF-16 units) straddling the 4,000-unit cap
    /// must not panic and must be excluded wholesale rather than split.
    #[test]
    fn pin_astral_character_straddling_cap_does_not_panic() {
        let mut line = format!("not logged in{}", "a".repeat(3_999 - 13));
        assert_eq!(utf16_len(&line), 3_999);
        line.push('\u{1F680}'); // rocket emoji: astral, 2 UTF-16 units
        assert_eq!(utf16_len(&line), 4_001);

        let result = extract_codex_auth_error(Some(&line)).expect("still matches");
        let expected: String = format!("not logged in{}", "a".repeat(3_999 - 13));
        assert_eq!(result, expected);
        assert_eq!(utf16_len(&result), 3_999);
    }

    // F4: exact yielded sequences, including the documented divergence from
    // the `<` sibling in `process_output_field_scanner`.

    #[test]
    fn pin_empty_input_yields_one_empty_line() {
        let lines: Vec<&str> = iterate_codex_output_lines("").collect();
        assert_eq!(lines, vec![""]);
        // Divergence: the `<` sibling yields no lines at all for "".
        assert_eq!(
            crate::process_output_field_scanner::iterate_process_output_lines("")
                .collect::<Vec<_>>(),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn pin_trailing_lf_yields_a_trailing_empty_line() {
        let lines: Vec<&str> = iterate_codex_output_lines("a\n").collect();
        assert_eq!(lines, vec!["a", ""]);
        // Divergence: the `<` sibling drops the trailing empty line.
        assert_eq!(
            crate::process_output_field_scanner::iterate_process_output_lines("a\n")
                .collect::<Vec<_>>(),
            vec!["a"]
        );
    }

    #[test]
    fn pin_lone_lf_yields_two_empty_lines() {
        let lines: Vec<&str> = iterate_codex_output_lines("\n").collect();
        assert_eq!(lines, vec!["", ""]);
    }

    #[test]
    fn pin_trailing_cr_yields_a_trailing_empty_line() {
        let lines: Vec<&str> = iterate_codex_output_lines("a\r").collect();
        assert_eq!(lines, vec!["a", ""]);
    }

    #[test]
    fn pin_trailing_crlf_yields_a_trailing_empty_line() {
        let lines: Vec<&str> = iterate_codex_output_lines("a\r\n").collect();
        assert_eq!(lines, vec!["a", ""]);
    }

    #[test]
    fn pin_crlf_and_lone_cr_mixed() {
        let lines: Vec<&str> = iterate_codex_output_lines("alpha\r\nbeta\rgamma\n").collect();
        assert_eq!(lines, vec!["alpha", "beta", "gamma", ""]);
    }
}
