//! Include/exclude glob splitting — verbatim port of `splitSearchGlobPatterns`
//! (`src/shared/text-search.ts:143-175`).

use crate::js_trim::js_trim;

/// Split a comma-separated glob string into individual patterns, honoring
/// backslash escapes so an escaped comma (`foo\,bar`) stays inside one pattern.
///
/// Verbatim state machine from `text-search.ts:143-175`, iterating by Unicode
/// scalar (`chars()`):
/// - While `escaping`: append `\` **and** the char (escape preserved verbatim),
///   then clear `escaping`.
/// - On an unescaped `\`: set `escaping`, consume (don't append yet).
/// - On an unescaped `,`: trim the current fragment, push it if non-empty, reset.
/// - Otherwise: append the char.
/// - After the loop: if still `escaping`, append a literal trailing `\`.
/// - Finally: trim the last fragment, push if non-empty.
///
/// Empty / whitespace-only fragments are dropped. **Fidelity note:** the two
/// fragment trims use [`js_trim`], not Rust `str::trim()` — JS `.trim()` strips
/// a different whitespace set (includes U+FEFF, excludes U+0085/NEL), so bare
/// `str::trim()` would diverge from the oracle on those two codepoints.
pub fn split_search_glob_patterns(patterns: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut escaping = false;

    for ch in patterns.chars() {
        if escaping {
            // Preserve the escape verbatim: the `\` AND the escaped char.
            current.push('\\');
            current.push(ch);
            escaping = false;
            continue;
        }
        if ch == '\\' {
            escaping = true;
            continue;
        }
        if ch == ',' {
            let trimmed = js_trim(&current);
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            current.clear();
            continue;
        }
        current.push(ch);
    }

    // A trailing lone `\` is preserved as literal glob input.
    if escaping {
        current.push('\\');
    }
    let trimmed = js_trim(&current);
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle case 6 (`text-search.test.ts:61`): splits comma-separated patterns
    /// while preserving an escaped comma inside one fragment and trimming
    /// surrounding whitespace.
    ///
    /// Mutation target (a): if the escaping branch appends only the char (drops
    /// the leading `\`), `foo\,bar/**` collapses to `foo,bar/**` and, crucially,
    /// the escaped comma is then re-processed as a split → this assertion fails.
    #[test]
    fn oracle_splits_preserving_escaped_comma() {
        assert_eq!(
            split_search_glob_patterns("foo\\,bar/**, *.ts, dist/**"),
            vec!["foo\\,bar/**", "*.ts", "dist/**"]
        );
    }

    /// Oracle case 7 (`text-search.test.ts:69`): a trailing lone backslash is
    /// preserved as literal glob input.
    ///
    /// Mutation target (b): dropping the post-loop `if escaping { push('\\') }`
    /// yields `["src"]` instead of `["src\\"]` → this assertion fails.
    #[test]
    fn oracle_preserves_trailing_backslash() {
        assert_eq!(split_search_glob_patterns("src\\"), vec!["src\\"]);
    }

    /// Codex-suggested edge cases exercising the escape state machine directly.
    #[test]
    fn escape_state_machine_edges() {
        // `\,` — escaped comma is literal, not a separator.
        assert_eq!(split_search_glob_patterns("\\,"), vec!["\\,"]);
        // `\\` — escaped backslash becomes a two-char `\\` fragment.
        assert_eq!(split_search_glob_patterns("\\\\"), vec!["\\\\"]);
        // `\x` — escaping any ordinary char keeps `\x` verbatim.
        assert_eq!(split_search_glob_patterns("\\x"), vec!["\\x"]);
    }

    /// Consecutive and whitespace-only fragments are dropped, non-empty kept.
    #[test]
    fn empty_and_whitespace_fragments_dropped() {
        // Consecutive commas -> the empty middle fragment is dropped.
        assert_eq!(split_search_glob_patterns("a,,b"), vec!["a", "b"]);
        // A whitespace-only fragment is dropped.
        assert_eq!(split_search_glob_patterns("a,   ,b"), vec!["a", "b"]);
        // Fully empty input -> no patterns.
        assert_eq!(split_search_glob_patterns(""), Vec::<String>::new());
    }

    /// A non-BMP scalar after `\` is preserved whole — pins that iteration is by
    /// Unicode scalar (`chars()`), not by UTF-16 code unit, so the surrogate
    /// pair of an astral char isn't split by the escape logic.
    #[test]
    fn escaped_non_bmp_char_preserved() {
        assert_eq!(split_search_glob_patterns("\\\u{1F600}"), vec!["\\\u{1F600}"]);
    }

    /// Fragment trimming must use the JS whitespace set, not Rust's. JS `.trim()`
    /// strips U+FEFF (so a BOM-only fragment becomes empty → dropped) but keeps
    /// U+0085/NEL (so a NEL-delimited fragment survives with the NEL attached).
    /// *Mutation:* reverting the two `js_trim` calls to `str::trim()` flips both
    /// assertions (BOM kept, NEL stripped) → this test fails.
    #[test]
    fn fragment_trim_matches_js_whitespace_set() {
        // U+FEFF-only fragment: JS trims to empty → dropped (Rust str::trim keeps it).
        assert_eq!(
            split_search_glob_patterns("a,\u{FEFF},b"),
            vec!["a", "b"]
        );
        // U+0085/NEL is NOT JS-whitespace → the fragment survives with NEL intact
        // (Rust str::trim would strip it, dropping the fragment).
        assert_eq!(
            split_search_glob_patterns("a,\u{0085}x,b"),
            vec!["a", "\u{0085}x", "b"]
        );
    }
}
