//! Submatch locator regex — port of `buildSubmatchRegex`
//! (`src/shared/text-search.ts:351-364`), reflecting plan corrections C2/C3.
//!
//! git grep reports only the first hit per line and gives no column range, so
//! the M3 parser re-scans each matched line with this regex to find every
//! occurrence's column. It is a **best-effort locator, NOT a JS/ERE validator**
//! (plan C3): the Rust `regex` crate's accept/reject set differs from JS
//! `RegExp` (no backrefs/lookaround, different `\b` Unicode semantics), so we do
//! NOT try to replicate JS's exact behavior. Tests assert observable behaviors
//! (matches / doesn't-match / `None` on a compile failure), not JS source strings.
//!
//! **C2:** a `None` return means only "the Rust regex could not compile" — it is
//! NOT evidence that git-grep would reject the query (git-grep and JS/Rust regex
//! disagree on many inputs). The M3 caller falls back to a whole-line highlight.
//!
//! # JS→Rust flag mapping
//! - JS `g` (global) has no Rust flag equivalent: the M3 caller iterates with
//!   [`regex::Regex::find_iter`] to walk every occurrence.
//! - JS `i` (case-insensitive) is set via [`regex::RegexBuilder::case_insensitive`],
//!   not an inline `(?i)` prefix, so the compiled pattern source stays clean.

use crate::regex_escape::escape_regex;
use crate::types::SearchOptions;

/// Build the best-effort submatch locator for `query`.
///
/// Verbatim intent of `text-search.ts:351-364`:
/// - `pattern` = `query` when `use_regex`, else [`escape_regex`]`(query)` so a
///   fixed-string query matches literally (`.`, `*`, … are neutralized).
/// - When `whole_word`, wrap the pattern in `\b…\b`.
/// - Compile case-insensitively unless `case_sensitive`.
/// - On a compile error, return `None` (whole-line fallback in M3).
pub fn build_submatch_regex(query: &str, opts: &SearchOptions) -> Option<regex::Regex> {
    let mut pattern = if opts.use_regex.unwrap_or(false) {
        query.to_string()
    } else {
        escape_regex(query)
    };
    if opts.whole_word.unwrap_or(false) {
        pattern = format!("\\b{pattern}\\b");
    }
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive.unwrap_or(false))
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_with(
        use_regex: bool,
        whole_word: bool,
        case_sensitive: bool,
    ) -> SearchOptions {
        SearchOptions {
            use_regex: Some(use_regex),
            whole_word: Some(whole_word),
            case_sensitive: Some(case_sensitive),
            ..SearchOptions::default()
        }
    }

    /// Oracle case 21 (`text-search.test.ts:242`): `a.b` as a fixed string
    /// compiles and matches `a.b` **literally**, NOT `axb` — proving
    /// `escape_regex` neutralized the `.`. (Behaviors asserted, not JS source,
    /// since Rust regex ≠ JS regex — plan C3.)
    ///
    /// *Mutation (d):* using `query` verbatim instead of `escape_regex` for the
    /// fixed-string path makes `.` a wildcard, so `axb` would match → this fails.
    #[test]
    fn oracle_fixed_string_escapes_dot() {
        let re = build_submatch_regex("a.b", &SearchOptions::default()).unwrap();
        assert!(re.is_match("a.b"), "must match the literal a.b");
        assert!(!re.is_match("axb"), "must NOT match axb (dot escaped)");
    }

    /// Case-insensitivity is the default (query not case-sensitive): `a.b`
    /// matches `A.B` too. *Mutation:* setting `case_insensitive(false)` here
    /// fails this.
    #[test]
    fn default_is_case_insensitive() {
        let re = build_submatch_regex("a.b", &SearchOptions::default()).unwrap();
        assert!(re.is_match("A.B"));
    }

    /// Oracle case 22 (`text-search.test.ts:248`): `foo` with wholeWord matches
    /// `foo` but not `foobar` — the `\b…\b` wrap is in effect.
    ///
    /// *Mutation:* dropping the `whole_word` `\b` wrap makes `foobar` match too.
    #[test]
    fn oracle_whole_word_boundaries() {
        let re = build_submatch_regex("foo", &opts_with(false, true, false)).unwrap();
        assert!(re.is_match("a foo b"), "must match standalone foo");
        assert!(!re.is_match("foobar"), "must NOT match foo inside foobar");
    }

    /// Oracle case 23 (`text-search.test.ts:253`): `a|b` with useRegex +
    /// caseSensitive compiles as an alternation (matches `a` and `b`) and is
    /// case-sensitive (does NOT match `A`).
    #[test]
    fn oracle_use_regex_alternation_case_sensitive() {
        let re = build_submatch_regex("a|b", &opts_with(true, false, true)).unwrap();
        assert!(re.is_match("a"));
        assert!(re.is_match("b"));
        assert!(!re.is_match("A"), "case-sensitive: A must not match");
        assert!(!re.is_match("c"));
    }

    /// Oracle case 24 (`text-search.test.ts:259`): `(foo` and `[abc` with
    /// useRegex → **`None`** (Rust regex compile failure on the unbalanced
    /// group / unterminated class).
    ///
    /// Per plan C2 this does NOT prove git-grep divergence — git-grep may accept
    /// these; `None` only pins the "regex couldn't compile → whole-line
    /// fallback" path. Rust happens to reject the same two inputs JS does, but
    /// that coincidence is not the contract.
    ///
    /// *Mutation:* replacing `.build().ok()` with an `.unwrap()`/panic, or a
    /// fallback that returns `Some(<empty regex>)`, would fail these `is_none`s.
    #[test]
    fn oracle_unbalanced_regex_is_none() {
        assert!(build_submatch_regex("(foo", &opts_with(true, false, false)).is_none());
        assert!(build_submatch_regex("[abc", &opts_with(true, false, false)).is_none());
    }

    /// The same unbalanced inputs as a **fixed string** (default, not useRegex)
    /// DO compile — they are escaped to literals — and match themselves. Pins
    /// that `None` is tied to regex-mode compile failure, not the raw characters.
    #[test]
    fn unbalanced_as_fixed_string_compiles() {
        let re = build_submatch_regex("(foo", &SearchOptions::default()).unwrap();
        assert!(re.is_match("x(foox"));
        let re2 = build_submatch_regex("[abc", &SearchOptions::default()).unwrap();
        assert!(re2.is_match("y[abcy"));
    }
}
