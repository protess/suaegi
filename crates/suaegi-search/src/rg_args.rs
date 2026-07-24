//! ripgrep argv construction — verbatim port of `buildRgArgs`
//! (`src/shared/text-search.ts:183-215`).
//!
//! The argv **order is load-bearing**: it is what a real `rg` process parses.
//! The fixed leading array comes first, then the conditional flags, then the
//! include/exclude globs, and finally the `--`, query, target terminator — the
//! cardinal argv-injection guard (`:213`) that makes a `-`-leading query or
//! target land as data, never as a flag.

use crate::constants::{MAX_MATCHES_PER_FILE, SEARCH_MAX_FILE_SIZE};
use crate::glob::split_search_glob_patterns;
use crate::types::SearchOptions;

/// Build the argument vector for a ripgrep (`--json`) invocation.
///
/// `target` is the search root (`root_path`) passed through **verbatim** — no
/// WSL/path translation here; the M4 driver routes the spawn and translates
/// output paths (Orca `:180-182`). Verbatim from `text-search.ts:183-215`:
///
/// 1. Fixed leading array, in order: `--json`, `--hidden`, `--glob`, `!.git`,
///    `--max-count`, `"100"`, `--max-filesize`, `"5M"` (`:184-193`). The `5M`
///    is `SEARCH_MAX_FILE_SIZE / 1024 / 1024` floored (`:192`).
/// 2. Conditional flags, in order: `--ignore-case` when NOT case-sensitive
///    (default is case-insensitive), `--word-regexp` when whole-word,
///    `--fixed-strings` when NOT use-regex (default is literal) (`:194-202`).
/// 3. Include globs (`--glob`, pat) then exclude globs (`--glob`, `!pat`), each
///    split via [`split_search_glob_patterns`] (`:203-212`).
/// 4. Terminator `--`, query, target as the **last three** args (`:213`).
pub fn build_rg_args(query: &str, target: &str, opts: &SearchOptions) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--json".to_string(),
        "--hidden".to_string(),
        "--glob".to_string(),
        "!.git".to_string(),
        "--max-count".to_string(),
        MAX_MATCHES_PER_FILE.to_string(),
        "--max-filesize".to_string(),
        // `Math.floor(SEARCH_MAX_FILE_SIZE / 1024 / 1024)` == 5 → "5M". Integer
        // division floors, matching Orca's `Math.floor`.
        format!("{}M", SEARCH_MAX_FILE_SIZE / 1024 / 1024),
    ];

    // Default is case-insensitive (no smart-case `-S`).
    if !opts.case_sensitive.unwrap_or(false) {
        args.push("--ignore-case".to_string());
    }
    if opts.whole_word.unwrap_or(false) {
        args.push("--word-regexp".to_string());
    }
    // Default is fixed-strings (literal search).
    if !opts.use_regex.unwrap_or(false) {
        args.push("--fixed-strings".to_string());
    }

    if let Some(include) = opts.include_pattern.as_deref() {
        for pat in split_search_glob_patterns(include) {
            args.push("--glob".to_string());
            args.push(pat);
        }
    }
    if let Some(exclude) = opts.exclude_pattern.as_deref() {
        for pat in split_search_glob_patterns(exclude) {
            args.push("--glob".to_string());
            args.push(format!("!{pat}"));
        }
    }

    // Argv-injection guard: `--` first, then the query, then the target. A query
    // or target that begins with `-` is read as data, never as a flag.
    args.push("--".to_string());
    args.push(query.to_string());
    args.push(target.to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SearchOptions {
        SearchOptions::default()
    }

    /// Oracle case 2 (`text-search.test.ts:29`) + the argv-injection pin. The
    /// defaults surface `--json`/`--hidden`/`--ignore-case`/`--fixed-strings`,
    /// `!.git` sits after a `--glob`, and the **last three args are exactly
    /// `["--", "needle", "/root"]`** — the cardinal injection guard.
    ///
    /// *Mutation (a):* moving or removing the `--` terminator (e.g. pushing
    /// query/target without the leading `--`, or before the globs) makes
    /// `args[len-3..]` differ from `["--","needle","/root"]` → this fails.
    #[test]
    fn oracle_rg_defaults_and_injection_guard() {
        let args = build_rg_args("needle", "/root", &opts());
        assert!(args.iter().any(|a| a == "--json"));
        assert!(args.iter().any(|a| a == "--hidden"));
        assert!(args.iter().any(|a| a == "--ignore-case"));
        assert!(args.iter().any(|a| a == "--fixed-strings"));

        let git_idx = args.iter().position(|a| a == "!.git").unwrap();
        let first_glob = args.iter().position(|a| a == "--glob").unwrap();
        assert!(
            git_idx > first_glob,
            "!.git must follow a --glob: {args:?}"
        );

        assert_eq!(&args[args.len() - 3..], &["--", "needle", "/root"]);
    }

    /// Pins the exact fixed leading array (order is what rg parses). *Mutation:*
    /// reordering any of the first eight args, or changing `100`/`5M`, fails.
    #[test]
    fn fixed_leading_array_order() {
        let args = build_rg_args("q", "/r", &opts());
        assert_eq!(
            &args[..8],
            &[
                "--json",
                "--hidden",
                "--glob",
                "!.git",
                "--max-count",
                "100",
                "--max-filesize",
                "5M",
            ]
        );
    }

    /// Oracle case 3 (`text-search.test.ts:39`): caseSensitive + wholeWord +
    /// useRegex → no `--ignore-case`, has `--word-regexp`, no `--fixed-strings`.
    #[test]
    fn oracle_rg_case_sensitive_whole_word_regex() {
        let o = SearchOptions {
            case_sensitive: Some(true),
            whole_word: Some(true),
            use_regex: Some(true),
            ..SearchOptions::default()
        };
        let args = build_rg_args("q", "/r", &o);
        assert!(!args.iter().any(|a| a == "--ignore-case"));
        assert!(args.iter().any(|a| a == "--word-regexp"));
        assert!(!args.iter().any(|a| a == "--fixed-strings"));
    }

    /// Oracle case 4 (`text-search.test.ts:46`): include `*.ts, *.tsx` + exclude
    /// `*.md` → the split globs appear as `--glob *.ts`, `--glob *.tsx`, and
    /// `--glob !*.md` (exclude prefixed with `!`).
    #[test]
    fn oracle_rg_include_exclude_globs() {
        let o = SearchOptions {
            include_pattern: Some("*.ts, *.tsx".to_string()),
            exclude_pattern: Some("*.md".to_string()),
            ..SearchOptions::default()
        };
        let args = build_rg_args("q", "/r", &o);
        assert!(args.iter().any(|a| a == "*.ts"));
        assert!(args.iter().any(|a| a == "*.tsx"));
        assert!(args.iter().any(|a| a == "!*.md"));

        // The exclude glob is a `--glob` value, and its `!` prefix is on the
        // pattern (not a separate arg).
        let excl = args.iter().position(|a| a == "!*.md").unwrap();
        assert_eq!(args[excl - 1], "--glob");
    }

    /// Oracle case 5 (`text-search.test.ts:53`): an escaped comma keeps the
    /// include pattern as one glob (`foo\,bar/**`), plus the `*.ts` fragment.
    #[test]
    fn oracle_rg_escaped_comma_include_one_glob() {
        let o = SearchOptions {
            include_pattern: Some("foo\\,bar/**, *.ts".to_string()),
            ..SearchOptions::default()
        };
        let args = build_rg_args("q", "/r", &o);
        assert!(args.iter().any(|a| a == "foo\\,bar/**"));
        assert!(args.iter().any(|a| a == "*.ts"));
    }

    /// Argv-injection extra pins (Codex Q1): a query that looks like a flag
    /// (`-e`, `--help`, `--`) and a target beginning with `-` must still land as
    /// the final args after `--`, never parsed as flags. The arg vector is what
    /// a real rg parses, so position — not just membership — is asserted.
    #[test]
    fn injection_flaglike_query_and_target_land_after_terminator() {
        for q in ["-e", "--help", "--"] {
            let args = build_rg_args(q, "-target", &opts());
            assert_eq!(
                &args[args.len() - 3..],
                &["--", q, "-target"],
                "flag-like query {q:?} must land after the terminator"
            );
            // Exactly one `--` terminator precedes the query/target; the query's
            // own literal `--` is data at the tail, not an earlier separator.
            let terminator_pos = args.len() - 3;
            assert_eq!(args[terminator_pos], "--");
        }
    }
}
