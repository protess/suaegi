//! git-grep argv construction — verbatim port of `toGitGlobPathspec`
//! (`src/shared/text-search.ts:291-295`) and `buildGitGrepArgs` (`:297-342`).
//!
//! git-grep is the fallback backend when `rg` is absent. As with rg, the argv
//! **order is load-bearing** and `-e query --` is the injection guard (`:322`).
//!
//! **Asymmetry (plan C4):** git-grep has NO per-file cap flag. rg carries
//! `--max-count 100`; git-grep does not — this is intentional and verbatim, so
//! the two backends' per-file result distributions differ. Do not "fix" it.

use crate::glob::split_search_glob_patterns;
use crate::types::SearchOptions;

/// Convert a user-facing glob into a git pathspec. Verbatim from `:291-295`.
///
/// A glob with no `/` (e.g. `*.ts`) is made recursive with a `**/` prefix, to
/// replicate rg's recursive-by-default globbing; a glob that already contains
/// `/` (e.g. `src/*.ts`) is used verbatim. The result is wrapped in
/// `:(exclude,glob)` when `exclude`, else `:(glob)`.
pub fn to_git_glob_pathspec(glob: &str, exclude: bool) -> String {
    let needs_recursive = !glob.contains('/');
    let pattern = if needs_recursive {
        format!("**/{glob}")
    } else {
        glob.to_string()
    };
    if exclude {
        format!(":(exclude,glob){pattern}")
    } else {
        format!(":(glob){pattern}")
    }
}

/// Build the argument vector for a `git grep` invocation. Verbatim from
/// `text-search.ts:297-342`:
///
/// 1. Fixed preamble, in order: `-c`, `submodule.recurse=false`, `grep`, `-n`,
///    `-I`, `--null`, `--no-color`, `--untracked`, `--no-recurse-submodules`
///    (`:299-309`). `-c submodule.recurse=false` + `--no-recurse-submodules`
///    avoids a conflict with `--untracked`; `--null` disambiguates
///    colon-containing filenames (`filename\0lineno\0content`).
/// 2. Conditional flags: `-i` when NOT case-sensitive, `-w` when whole-word,
///    `--fixed-strings` when NOT use-regex ELSE `--extended-regexp` (`:310-320`).
/// 3. Terminator `-e`, query, `--` (`:322`) — binds the pattern to `-e` and
///    ends the option/pathspec boundary; a `-`-leading query stays data.
/// 4. Include then exclude pathspecs via [`to_git_glob_pathspec`] (`:324-336`);
///    **if there are no pathspecs at all, push `.`** (`:338-340`) so git grep
///    scans the whole working tree.
///
/// Note there is **no** per-file `--max-count` — the rg/git-grep asymmetry
/// (plan C4) is intentional.
pub fn build_git_grep_args(query: &str, opts: &SearchOptions) -> Vec<String> {
    let mut git_args: Vec<String> = vec![
        "-c".to_string(),
        "submodule.recurse=false".to_string(),
        "grep".to_string(),
        "-n".to_string(),
        "-I".to_string(),
        "--null".to_string(),
        "--no-color".to_string(),
        "--untracked".to_string(),
        "--no-recurse-submodules".to_string(),
    ];

    if !opts.case_sensitive.unwrap_or(false) {
        git_args.push("-i".to_string());
    }
    if opts.whole_word.unwrap_or(false) {
        git_args.push("-w".to_string());
    }
    if !opts.use_regex.unwrap_or(false) {
        git_args.push("--fixed-strings".to_string());
    } else {
        git_args.push("--extended-regexp".to_string());
    }

    // Argv-injection guard: `-e query --`.
    git_args.push("-e".to_string());
    git_args.push(query.to_string());
    git_args.push("--".to_string());

    let mut has_pathspecs = false;
    if let Some(include) = opts.include_pattern.as_deref() {
        for pat in split_search_glob_patterns(include) {
            git_args.push(to_git_glob_pathspec(&pat, false));
            has_pathspecs = true;
        }
    }
    if let Some(exclude) = opts.exclude_pattern.as_deref() {
        for pat in split_search_glob_patterns(exclude) {
            git_args.push(to_git_glob_pathspec(&pat, true));
            has_pathspecs = true;
        }
    }
    // git grep needs a pathspec to search the working tree; `.` = everything.
    if !has_pathspecs {
        git_args.push(".".to_string());
    }
    git_args
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Oracle case 20 (`text-search.test.ts:234`): bare globs get the recursive
    /// `**/` prefix; a glob with `/` is verbatim; exclude uses `:(exclude,glob)`.
    ///
    /// *Mutation (c):* dropping the `**/` recursive prefix (using `glob`
    /// verbatim for the no-slash case) makes `*.ts` → `:(glob)*.ts` → this fails.
    #[test]
    fn oracle_to_git_glob_pathspec() {
        assert_eq!(to_git_glob_pathspec("*.ts", false), ":(glob)**/*.ts");
        assert_eq!(to_git_glob_pathspec("src/*.ts", false), ":(glob)src/*.ts");
        assert_eq!(to_git_glob_pathspec("*.ts", true), ":(exclude,glob)**/*.ts");
    }

    /// Oracle case 16 (`text-search.test.ts:206`): defaults surface `-i`,
    /// `--fixed-strings`, `--no-recurse-submodules`, and the **last arg is `.`**
    /// (the default pathspec).
    ///
    /// *Mutation (b):* dropping the `if !has_pathspecs { push('.') }` default
    /// makes the last arg `--` (the terminator) instead of `.` → this fails.
    #[test]
    fn oracle_git_defaults_and_default_pathspec() {
        let args = build_git_grep_args("q", &SearchOptions::default());
        assert!(args.iter().any(|a| a == "-i"));
        assert!(args.iter().any(|a| a == "--fixed-strings"));
        assert!(args.iter().any(|a| a == "--no-recurse-submodules"));
        assert_eq!(args.last().unwrap(), ".");
    }

    /// Pins the exact fixed preamble order (what git parses). *Mutation:*
    /// reordering any preamble arg fails.
    #[test]
    fn fixed_preamble_order() {
        let args = build_git_grep_args("q", &SearchOptions::default());
        assert_eq!(
            &args[..9],
            &[
                "-c",
                "submodule.recurse=false",
                "grep",
                "-n",
                "-I",
                "--null",
                "--no-color",
                "--untracked",
                "--no-recurse-submodules",
            ]
        );
    }

    /// Pins C4: git-grep carries NO per-file `--max-count` (the rg-only cap).
    /// *Mutation:* adding a `--max-count`/`-m` flag would fail this.
    #[test]
    fn no_per_file_max_count_flag() {
        let args = build_git_grep_args("q", &SearchOptions::default());
        assert!(!args.iter().any(|a| a == "--max-count" || a == "-m"));
    }

    /// Oracle case 17 (`text-search.test.ts:214`): useRegex → `--extended-regexp`
    /// and no `--fixed-strings`.
    #[test]
    fn oracle_git_use_regex_extended() {
        let o = SearchOptions {
            use_regex: Some(true),
            ..SearchOptions::default()
        };
        let args = build_git_grep_args("q", &o);
        assert!(args.iter().any(|a| a == "--extended-regexp"));
        assert!(!args.iter().any(|a| a == "--fixed-strings"));
    }

    /// Oracle case 18 (`text-search.test.ts:220`): include `*.ts` / exclude
    /// `dist/**` → `:(glob)**/*.ts` and `:(exclude,glob)dist/**`. When a
    /// pathspec is present, `.` must NOT be appended.
    #[test]
    fn oracle_git_include_exclude_pathspecs() {
        let o = SearchOptions {
            include_pattern: Some("*.ts".to_string()),
            exclude_pattern: Some("dist/**".to_string()),
            ..SearchOptions::default()
        };
        let args = build_git_grep_args("q", &o);
        assert!(args.iter().any(|a| a == ":(glob)**/*.ts"));
        assert!(args.iter().any(|a| a == ":(exclude,glob)dist/**"));
        assert_ne!(args.last().unwrap(), ".");
    }

    /// Oracle case 19 (`text-search.test.ts:226`): an escaped comma keeps the
    /// include as one pathspec (`:(glob)foo\,bar/**`), plus `:(glob)**/*.ts`.
    #[test]
    fn oracle_git_escaped_comma_pathspec() {
        let o = SearchOptions {
            include_pattern: Some("foo\\,bar/**, *.ts".to_string()),
            ..SearchOptions::default()
        };
        let args = build_git_grep_args("q", &o);
        assert!(args.iter().any(|a| a == ":(glob)foo\\,bar/**"));
        assert!(args.iter().any(|a| a == ":(glob)**/*.ts"));
    }

    /// Argv-injection extra pins (Codex Q1): a flag-like query lands right after
    /// `-e`, followed by the `--` terminator — never parsed as a git flag. The
    /// position (…, `-e`, query, `--`, …) is what git parses.
    #[test]
    fn injection_flaglike_query_after_dash_e() {
        for q in ["-e", "--help", "--"] {
            let args = build_git_grep_args(q, &SearchOptions::default());
            let e_pos = args.iter().position(|a| a == "-e").unwrap();
            assert_eq!(args[e_pos + 1], q, "query must follow -e: {args:?}");
            assert_eq!(args[e_pos + 2], "--", "terminator must follow query");
        }
    }
}
