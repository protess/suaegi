//! First-command-token classification — verbatim port of Orca's
//! `src/shared/command-token-scanner.ts` (@ v1.4.150-rc.0).
//!
//! Why: command strings may include pasted scripts; first-token
//! classification must stay bounded.

use crate::js_ws::is_js_whitespace;

/// Scan cap for [`get_first_command_token`] and [`command_contains_token`],
/// counted in **UTF-16 code units** (JS `.length` semantics) — see
/// [`utf16_scan_limit_byte_offset`].
pub const COMMAND_TOKEN_SCAN_MAX_CHARS: usize = 4096;

/// Largest byte offset `<= s.len()` such that the UTF-16-code-unit count of
/// `s[..offset]` does not exceed `max_utf16_units`, snapped down to a char
/// boundary (D1). See the identical helper's doc comment in
/// `process_output_field_scanner.rs` for the full rationale; duplicated here
/// rather than shared because each module in this crate is self-contained
/// (only `js_ws` is an explicitly shared exception).
fn utf16_scan_limit_byte_offset(s: &str, max_utf16_units: usize) -> usize {
    let mut units = 0usize;
    for (byte_offset, ch) in s.char_indices() {
        let next_units = units + ch.len_utf16();
        if next_units > max_utf16_units {
            return byte_offset;
        }
        units = next_units;
    }
    s.len()
}

/// Extract the first whitespace-delimited token from `command`, scanning at
/// most [`COMMAND_TOKEN_SCAN_MAX_CHARS`] UTF-16 units (D1).
///
/// D5 quote handling: leading whitespace is skipped first. If the next char
/// is `"` or `'` *and* at least one more char remains within the scan
/// window, we look for a matching closing quote within the window:
/// - a closing quote found with a **non-empty** interior → return the
///   interior slice (quotes stripped);
/// - an **empty** quote pair (`""`/`''`) → break out and fall back to the
///   unquoted path below (the quote chars end up included in the result);
/// - an **unterminated** quote (no closing quote within the window) → also
///   fall back to the unquoted path (quote chars included).
///
/// Returns `""` if `command` is empty or all whitespace within the scan
/// window.
pub fn get_first_command_token(command: &str) -> &str {
    let scan_limit = utf16_scan_limit_byte_offset(command, COMMAND_TOKEN_SCAN_MAX_CHARS);
    let mut index = 0usize;

    while index < scan_limit {
        let ch = command[index..]
            .chars()
            .next()
            .expect("index < scan_limit implies a char");
        if !is_js_whitespace(ch) {
            break;
        }
        index += ch.len_utf8();
    }
    if index >= scan_limit {
        return "";
    }

    let quote = command[index..]
        .chars()
        .next()
        .expect("index < scan_limit implies a char");
    if quote == '"' || quote == '\'' {
        let token_start = index + quote.len_utf8();
        if token_start < scan_limit {
            let mut end = token_start;
            while end < scan_limit {
                let ch = command[end..]
                    .chars()
                    .next()
                    .expect("end < scan_limit implies a char");
                if ch == quote {
                    if end > token_start {
                        return &command[token_start..end];
                    }
                    break;
                }
                end += ch.len_utf8();
            }
        }
    }

    let token_start = index;
    while index < scan_limit {
        let ch = command[index..]
            .chars()
            .next()
            .expect("index < scan_limit implies a char");
        if is_js_whitespace(ch) {
            break;
        }
        index += ch.len_utf8();
    }
    &command[token_start..index]
}

/// Return the path basename of `token`: the suffix after the last `/` or
/// `\`, or the whole token if it contains neither.
pub fn get_command_token_path_basename(token: &str) -> &str {
    for (index, ch) in token.char_indices().rev() {
        if ch == '/' || ch == '\\' {
            return &token[index + ch.len_utf8()..];
        }
    }
    token
}

/// Whether any whitespace-delimited token in `command` equals `expected_token`
/// exactly (whole-token equality, not substring containment).
///
/// D5 — deliberately asymmetric with [`get_first_command_token`]: **no quote
/// handling** here (a quoted token is matched literally, quotes included). An
/// empty `expected_token` always returns `false`. Progress through `command`
/// is guaranteed: each outer-loop iteration either advances `index` past at
/// least one whitespace or one token character before looping again, so this
/// can never hang.
pub fn command_contains_token(command: &str, expected_token: &str) -> bool {
    if expected_token.is_empty() {
        return false;
    }

    let scan_limit = utf16_scan_limit_byte_offset(command, COMMAND_TOKEN_SCAN_MAX_CHARS);
    let mut index = 0usize;

    while index < scan_limit {
        while index < scan_limit {
            let ch = command[index..]
                .chars()
                .next()
                .expect("index < scan_limit implies a char");
            if !is_js_whitespace(ch) {
                break;
            }
            index += ch.len_utf8();
        }
        let token_start = index;
        while index < scan_limit {
            let ch = command[index..]
                .chars()
                .next()
                .expect("index < scan_limit implies a char");
            if is_js_whitespace(ch) {
                break;
            }
            index += ch.len_utf8();
        }
        if token_start < index && &command[token_start..index] == expected_token {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: command-token-scanner.test.ts

    #[test]
    fn extracts_first_command_token_across_pasted_whitespace() {
        let command = format!(" {}codex\t--resume", '\u{00A0}');
        assert_eq!(get_first_command_token(&command), "codex");
    }

    #[test]
    fn preserves_quoted_command_paths_with_spaces() {
        assert_eq!(
            get_first_command_token(r#""C:\Program Files\Orca\codex.cmd" --resume"#),
            r"C:\Program Files\Orca\codex.cmd"
        );
    }

    #[test]
    fn extracts_path_basenames_without_allocating_path_segment_arrays() {
        assert_eq!(
            get_command_token_path_basename(r"C:\Program Files\Orca\codex.cmd"),
            "codex.cmd"
        );
        assert_eq!(get_command_token_path_basename("/usr/local/bin/omp"), "omp");
    }

    #[test]
    fn bounds_pathological_single_token_commands() {
        let command = "a".repeat(COMMAND_TOKEN_SCAN_MAX_CHARS + 100);
        let token = get_first_command_token(&command);
        assert_eq!(token.len(), COMMAND_TOKEN_SCAN_MAX_CHARS);
        assert_eq!(token, "a".repeat(COMMAND_TOKEN_SCAN_MAX_CHARS));
    }

    #[test]
    fn finds_exact_command_tokens_without_regex_splitting() {
        let command = "/Applications/serve-sim/bin/serve-sim-bin\tUDID-1 --port 3100";
        assert!(command_contains_token(command, "UDID-1"));
        assert!(!command_contains_token(command, "UDID"));
    }

    // Extra pins (oracle-silent), per plan D1/D5:

    /// D5: an unterminated quote (no closing quote within the scan window)
    /// falls back to the unquoted path — the quote character itself IS
    /// included in the returned token.
    #[test]
    fn pin_unterminated_quote_falls_back_to_unquoted_path() {
        assert_eq!(
            get_first_command_token(r#""unterminated"#),
            r#""unterminated"#
        );
    }

    /// D5: an empty quote pair (`""`) breaks out of the quote scan and
    /// falls back to the unquoted path, so the token is the two adjacent
    /// quote characters themselves (whitespace then terminates it).
    #[test]
    fn pin_empty_quote_pair_falls_back_to_unquoted_path() {
        assert_eq!(get_first_command_token("\"\" foo"), "\"\"");
    }

    /// D5: `command_contains_token` with an empty expected token always
    /// returns `false`, regardless of command content.
    #[test]
    fn pin_empty_expected_token_returns_false() {
        assert!(!command_contains_token("alpha beta", ""));
        assert!(!command_contains_token("", ""));
    }

    /// D5: a token ending in `/` is matched whole (not confused with a path
    /// separator) and whole-token equality (not substring) is enforced.
    #[test]
    fn pin_token_ending_in_slash_and_whole_token_equality() {
        assert!(command_contains_token("cd path/ ; ls", "path/"));
        assert!(!command_contains_token("cd path/ ; ls", "path"));
    }

    /// D1: a non-ASCII command whose UTF-16 cap boundary falls
    /// mid-character at the byte level must not panic.
    #[test]
    fn pin_non_ascii_cap_boundary_does_not_panic() {
        let mut command = "가".repeat(2048); // 2048 UTF-16 units
        command.push_str(&"b".repeat(2048)); // + 2048 units = 4096 exactly
        command.push('🚀'); // astral, 2 units — would straddle a naive byte cap
        let token = get_first_command_token(&command);
        let expected: String = "가".repeat(2048) + &"b".repeat(2048);
        assert_eq!(token, expected.as_str());
    }

    /// D2-adjacent/D5: empty command input returns an empty token and no
    /// match for `command_contains_token`.
    #[test]
    fn pin_empty_command_input() {
        assert_eq!(get_first_command_token(""), "");
        assert!(!command_contains_token("", "anything"));
    }

    /// `get_command_token_path_basename` distinguishes both separators and
    /// returns the whole token when neither is present.
    #[test]
    fn pin_basename_no_separator_returns_whole_token() {
        assert_eq!(get_command_token_path_basename("codex"), "codex");
    }

    /// D3 crux pin: U+FEFF (ZERO WIDTH NO-BREAK SPACE) IS ECMAScript
    /// whitespace, so it is skipped as leading whitespace before the first
    /// token. Why: Rust `char::is_whitespace()` returns `false` for U+FEFF,
    /// so a regression to the Rust predicate would fold it into the token
    /// instead of skipping it — the existing NBSP (U+00A0) test does not
    /// discriminate here because Rust also treats U+00A0 as whitespace.
    #[test]
    fn pin_js_whitespace_set_feff_is_whitespace() {
        assert_eq!(get_first_command_token("\u{feff}codex --resume"), "codex");
    }

    /// D3 crux pin: U+0085 (NEXT LINE) is NOT ECMAScript whitespace, so it
    /// stays part of the first token rather than being treated as a
    /// separator. Why: Rust `char::is_whitespace()` returns `true` for
    /// U+0085, so a regression to the Rust predicate would incorrectly split
    /// it off as leading whitespace.
    #[test]
    fn pin_js_whitespace_set_u0085_is_not_whitespace() {
        assert_eq!(get_first_command_token("\u{85}codex"), "\u{85}codex");
    }

    /// D3: a U+FEFF separator between tokens is recognized by
    /// `command_contains_token`, matching ECMAScript whitespace semantics.
    #[test]
    fn pin_js_whitespace_set_feff_separates_tokens() {
        assert!(command_contains_token("codex\u{feff}UDID-1", "UDID-1"));
    }
}
