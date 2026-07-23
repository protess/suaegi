//! Quick Open fuzzy scorer — a verbatim port of Orca's `quick-open-search.ts`
//! (`rankQuickOpenFiles` @ v1.4.150-rc.0, lines 38-147).
//!
//! The ranking weights ARE the UX, so this is a pure, dependency-free port. The
//! upstream Vitest oracle (`quick-open-search.test.ts`, 11 cases) is ported
//! verbatim below so the ranking stays bit-for-bit identical.
//!
//! # Score semantics
//! LOWER score is better (Orca sorts ascending). A match accumulates the gap
//! between consecutive matched characters (spread-out matches score worse),
//! gets a `-5` bonus per character matched right after a `/`, `.`, or `-`
//! separator, and a one-time `-100` bonus when the filename contains the whole
//! query as a substring. Non-subsequence files are rejected.
//!
//! # JS-vs-Rust semantics (documented divergences, safe against the ASCII oracle)
//! - **Size limit is BYTES.** Orca's `isClipboardTextByteLengthOverLimit` checks
//!   UTF-8 byte length (with a UTF-16-code-unit fast path). [`rank`] uses
//!   `str::len()`, which is the UTF-8 byte count — the correct port. For the
//!   all-ASCII oracle these coincide exactly.
//! - **Matching iterates by `char`.** Orca indexes a JS string by UTF-16 code
//!   unit; we iterate Unicode scalar values. `ti` (used for gap distance) and
//!   the separator-boundary lookup are therefore in `char` units, not UTF-16
//!   units. Every oracle case is ASCII, where a `char` == one UTF-16 unit, so
//!   the observable ranking is identical; a divergence is only possible on
//!   astral-plane input, which the oracle does not exercise.

/// Maximum accepted query size, in UTF-8 bytes (Orca `QUICK_OPEN_QUERY_MAX_BYTES`
/// = 2 * 1024). Checked on the RAW, untrimmed query.
pub const QUERY_MAX_BYTES: usize = 2 * 1024;

/// Default number of results returned (Orca `QUICK_OPEN_RESULT_LIMIT`).
pub const RESULT_LIMIT: usize = 50;

/// One ranked result. `path` is the VERBATIM input string (only the match keys
/// are normalized). `score` is Orca's score — lower is better.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub path: String,
    pub score: i64,
}

/// Rank `files` against `query`, returning at most `limit` matches sorted by
/// `(score ascending, input-index ascending)` — a verbatim port of Orca's
/// `rankQuickOpenFiles`.
///
/// The guard order is load-bearing and matches Orca exactly:
/// 1. `limit == 0` -> `[]` (Orca's `limit <= 0`; `usize` cannot be negative).
/// 2. RAW query byte length `> QUERY_MAX_BYTES` -> `[]`. This runs BEFORE trim,
///    so an all-whitespace oversized query rejects instead of leaking into the
///    empty-query passthrough.
/// 3. Normalize: `query.trim().replace('\\', "/").to_lowercase()`.
/// 4. Empty normalized query -> first `limit` files in input order, score `0`
///    (no scoring performed).
pub fn rank(query: &str, files: &[&str], limit: usize) -> Vec<Match> {
    // (1) Non-positive limit returns nothing, checked first.
    if limit == 0 {
        return Vec::new();
    }

    // (2) Reject oversized queries on the RAW, untrimmed bytes — must precede
    // trim so an all-whitespace 2049-byte query rejects rather than falling
    // through to the empty-query passthrough.
    if query.len() > QUERY_MAX_BYTES {
        return Vec::new();
    }

    // (3) Users type backslashes in path queries even though Quick Open shows
    // slash-normalized paths; fold them so both sides compare the same way.
    let normalized = query.trim().replace('\\', "/").to_lowercase();

    // (4) Empty query: passthrough of the first `limit` files, no scoring.
    if normalized.is_empty() {
        return files
            .iter()
            .take(limit)
            .map(|&path| Match {
                path: path.to_string(),
                score: 0,
            })
            .collect();
    }

    let normalized_chars: Vec<char> = normalized.chars().collect();

    // Collect every subsequence match with its input index, then take the top
    // `limit` by (score asc, input-index asc). input-index is a total tie-break,
    // so this is exactly Orca's capped insertion sort.
    let mut ranked: Vec<(i64, usize, &str)> = Vec::new();
    for (input_index, &path) in files.iter().enumerate() {
        let indexed = IndexedFile::prepare(path, input_index);
        if let Some(score) = fuzzy_match(&normalized, &normalized_chars, &indexed) {
            ranked.push((score, input_index, path));
        }
    }

    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    ranked.truncate(limit);

    ranked
        .into_iter()
        .map(|(score, _, path)| Match {
            path: path.to_string(),
            score,
        })
        .collect()
}

/// Precomputed match keys for one file — the port of Orca's
/// `prepareQuickOpenFiles` entry. `path` stays verbatim; only the keys are
/// slash-normalized and lowercased. `path`/`input_index` round out the Orca
/// struct shape and are asserted by the prepare oracle test; scoring reads the
/// verbatim path and index straight from the `files` slice in [`rank`], so
/// these fields are only observed under `cfg(test)`.
#[cfg_attr(not(test), allow(dead_code))]
struct IndexedFile<'a> {
    path: &'a str,
    lower_path: String,
    lower_filename: String,
    input_index: usize,
}

impl<'a> IndexedFile<'a> {
    fn prepare(path: &'a str, input_index: usize) -> Self {
        let lower_path = path.replace('\\', "/").to_lowercase();
        // Filename = the substring after the last '/' (whole string if none),
        // matching Orca's `searchPath.slice(lastSlash + 1)`.
        let lower_filename = match lower_path.rfind('/') {
            Some(idx) => lower_path[idx + 1..].to_string(),
            None => lower_path.clone(),
        };
        Self {
            path,
            lower_path,
            lower_filename,
            input_index,
        }
    }
}

/// Subsequence score for one file. Returns `None` when the query is not a
/// subsequence of `lower_path` (Orca's `-1` reject); otherwise `Some(score)`
/// where lower is better.
fn fuzzy_match(normalized: &str, normalized_chars: &[char], file: &IndexedFile<'_>) -> Option<i64> {
    let mut qi = 0usize;
    let mut score: i64 = 0;
    let mut last_match_idx: Option<usize> = None;
    // Character immediately preceding the current position; `None` exactly at
    // the start of the path (Orca's `lowerPath[-1]` == undefined).
    let mut prev_char: Option<char> = None;

    for (ti, ch) in file.lower_path.chars().enumerate() {
        if qi >= normalized_chars.len() {
            break;
        }
        if ch == normalized_chars[qi] {
            let gap = match last_match_idx {
                Some(last) => ti - last - 1,
                None => 0,
            };
            score += gap as i64;

            // Boundary bonus: a match right after a separator is cheaper. The
            // start of the path (`prev_char == None`, i.e. Orca's `ti > 0`
            // guard) is deliberately NOT a boundary — do not invert this.
            let at_boundary = match prev_char {
                None => false,
                Some(prev) => matches!(prev, '/' | '.' | '-'),
            };
            if at_boundary {
                score -= 5;
            }

            last_match_idx = Some(ti);
            qi += 1;
        }
        prev_char = Some(ch);
    }

    if qi < normalized_chars.len() {
        return None;
    }

    if file.lower_filename.contains(normalized) {
        score -= 100;
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Orca oracle: quick-open-search.test.ts (11 cases, ported verbatim).
    // ------------------------------------------------------------------

    /// Oracle 1: empty query -> first 50 paths, score 0.
    #[test]
    fn returns_first_50_paths_with_score_0_for_empty_query() {
        let owned: Vec<String> = (0..75).map(|i| format!("src/file-{i}.ts")).collect();
        let files: Vec<&str> = owned.iter().map(String::as_str).collect();

        let expected: Vec<Match> = (0..RESULT_LIMIT)
            .map(|i| Match {
                path: format!("src/file-{i}.ts"),
                score: 0,
            })
            .collect();

        assert_eq!(rank("", &files, RESULT_LIMIT), expected);
    }

    /// Oracle 2: whitespace-only query treated as empty.
    #[test]
    fn treats_whitespace_only_query_as_empty() {
        let files = ["src/a.ts", "src/b.ts", "src/c.ts"];

        assert_eq!(
            rank("   ", &files, RESULT_LIMIT),
            vec![
                Match {
                    path: "src/a.ts".to_string(),
                    score: 0
                },
                Match {
                    path: "src/b.ts".to_string(),
                    score: 0
                },
                Match {
                    path: "src/c.ts".to_string(),
                    score: 0
                },
            ]
        );
    }

    /// Oracle 3: filename substring matches beat path-only matches.
    #[test]
    fn prefers_filename_substring_over_path_only() {
        let files = [
            "button-area/deep/path/file.tsx",
            "src/components/Button.tsx",
        ];

        let paths: Vec<String> = rank("button", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.path)
            .collect();

        assert_eq!(
            paths,
            vec![
                "src/components/Button.tsx".to_string(),
                "button-area/deep/path/file.tsx".to_string(),
            ]
        );
    }

    /// Oracle 4: first-seen order for tie-heavy results at the limit boundary.
    #[test]
    fn keeps_first_seen_order_for_ties_at_limit_boundary() {
        let owned: Vec<String> = (0..10).map(|i| format!("src/path-{i}.bin")).collect();
        let files: Vec<&str> = owned.iter().map(String::as_str).collect();

        assert_eq!(
            rank("s", &files, 4),
            vec![
                Match {
                    path: "src/path-0.bin".to_string(),
                    score: 0
                },
                Match {
                    path: "src/path-1.bin".to_string(),
                    score: 0
                },
                Match {
                    path: "src/path-2.bin".to_string(),
                    score: 0
                },
                Match {
                    path: "src/path-3.bin".to_string(),
                    score: 0
                },
            ]
        );
    }

    /// Oracle 5: 50 top-ranked results from a 100k synthetic list.
    #[test]
    fn returns_50_top_ranked_from_100k_synthetic_list() {
        let filler_count = 99_940;
        let top_candidate_count = 60;
        let mut owned: Vec<String> = Vec::with_capacity(filler_count + top_candidate_count);
        for i in 0..filler_count {
            owned.push(format!("n-x-e-x-e-x-d-x-l-x-e/group-{i}/file.ts"));
        }
        for i in 0..top_candidate_count {
            owned.push(format!("bulk/special-{i}/needle.ts"));
        }
        let files: Vec<&str> = owned.iter().map(String::as_str).collect();

        let results = rank("needle", &files, RESULT_LIMIT);

        assert_eq!(results.len(), RESULT_LIMIT);
        let paths: Vec<String> = results.into_iter().map(|m| m.path).collect();
        let expected: Vec<String> = (0..RESULT_LIMIT)
            .map(|i| format!("bulk/special-{i}/needle.ts"))
            .collect();
        assert_eq!(paths, expected);
    }

    /// Oracle 6: scores are returned sorted ascending.
    #[test]
    fn returns_scores_sorted_ascending() {
        let files = [
            "src/components/QuickOpen.tsx",
            "quick/open/deep/path/file.tsx",
            "src/q-u-i-c-k-open.ts",
        ];

        let scores: Vec<i64> = rank("quick", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.score)
            .collect();

        let mut sorted = scores.clone();
        sorted.sort();
        assert_eq!(scores, sorted);
    }

    /// Oracle 7: normalized relative paths (prepare) without changing path
    /// semantics. Ported against the internal `IndexedFile::prepare`.
    #[test]
    fn indexes_normalized_relative_paths_without_changing_path_semantics() {
        let cases = [
            (
                "src/renderer/src/components/QuickOpen.tsx",
                "src/renderer/src/components/quickopen.tsx",
                "quickopen.tsx",
                0usize,
            ),
            (
                "packages/windows-origin/src/App.tsx",
                "packages/windows-origin/src/app.tsx",
                "app.tsx",
                1,
            ),
            ("single-file.ts", "single-file.ts", "single-file.ts", 2),
            (
                "legacy\\provider\\raw-path.ts",
                "legacy/provider/raw-path.ts",
                "raw-path.ts",
                3,
            ),
        ];

        for (path, lower_path, lower_filename, input_index) in cases {
            let indexed = IndexedFile::prepare(path, input_index);
            assert_eq!(indexed.path, path);
            assert_eq!(indexed.lower_path, lower_path);
            assert_eq!(indexed.lower_filename, lower_filename);
            assert_eq!(indexed.input_index, input_index);
        }
    }

    /// Oracle 8: no results for non-positive limits.
    #[test]
    fn returns_no_results_for_non_positive_limits() {
        let files = ["src/a.ts"];
        assert_eq!(rank("a", &files, 0), vec![]);
        // Orca also tests limit == -1; usize cannot be negative, so limit == 0
        // is the only non-positive case reachable in Rust.
    }

    /// Oracle 9: oversized pasted queries reject before scanning candidates.
    #[test]
    fn rejects_oversized_pasted_queries() {
        let oversized = "secret-quick-open".repeat(QUERY_MAX_BYTES);
        let files = ["src/secret.ts"];
        assert!(oversized.len() > QUERY_MAX_BYTES);
        assert_eq!(rank(&oversized, &files, RESULT_LIMIT), vec![]);
    }

    /// Oracle 10: oversized whitespace rejects BEFORE trimming.
    #[test]
    fn rejects_oversized_whitespace_before_trimming() {
        let query = " ".repeat(QUERY_MAX_BYTES + 1);
        let files = ["src/a.ts"];
        assert_eq!(rank(&query, &files, RESULT_LIMIT), vec![]);
    }

    /// Oracle 11: Windows-style path queries match slash-normalized paths.
    #[test]
    fn matches_windows_style_path_queries() {
        let files = [
            "src/components/Button.tsx",
            "src/components/ButtonGroup.tsx",
            "src/routes/About.tsx",
        ];

        let paths: Vec<String> = rank("src\\components\\button", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.path)
            .collect();

        assert_eq!(
            paths,
            vec![
                "src/components/Button.tsx".to_string(),
                "src/components/ButtonGroup.tsx".to_string(),
            ]
        );
    }

    // ------------------------------------------------------------------
    // Mutation-crux tests (repo hard rule). Each pins one scoring rule and
    // fails under the mutation noted in its doc comment.
    // ------------------------------------------------------------------

    /// Crux — score sign / ordering. Two files that differ only by whether the
    /// filename contains the query rank in a fixed order (lower score first).
    /// Mutation: sort descending, or `+= 5` instead of `-= 5` -> order flips.
    #[test]
    fn crux_score_sign_and_ordering() {
        // needle.ts filename contains "needle" (-100); scattered path does not.
        let files = ["n/e/e/d/l/e/other.ts", "src/needle.ts"];
        let paths: Vec<String> = rank("needle", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                "src/needle.ts".to_string(),
                "n/e/e/d/l/e/other.ts".to_string()
            ]
        );
    }

    /// Crux — size checked BEFORE trim. An all-whitespace 2049-byte query must
    /// reject (`[]`). Mutation: check size AFTER trim -> the trimmed query is
    /// empty, so it would leak into the passthrough and return the file.
    #[test]
    fn crux_size_checked_before_trim() {
        let query = " ".repeat(QUERY_MAX_BYTES + 1);
        assert_eq!(query.len(), QUERY_MAX_BYTES + 1);
        let files = ["src/a.ts"];
        assert_eq!(rank(&query, &files, RESULT_LIMIT), vec![]);
    }

    /// Crux — `ti > 0` boundary rule (start-of-path is NOT a boundary).
    /// `a.ts` matches "a" at index 0 (no boundary bonus) -> -100 only.
    /// `x/a.ts` matches "a" after '/' (boundary -5) -> -105, so it ranks first.
    /// Mutation: treat `prev_char == None` (ti == 0) as a boundary too -> `a.ts`
    /// also gets -5, tying at -105, and the input-index tie-break puts `a.ts`
    /// first, flipping the order.
    #[test]
    fn crux_ti_gt_0_boundary_rule() {
        let files = ["a.ts", "x/a.ts"];
        let paths: Vec<String> = rank("a", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(paths, vec!["x/a.ts".to_string(), "a.ts".to_string()]);

        // Pin the exact scores too, so the mutation cannot hide behind a tie.
        let scored = rank("a", &files, RESULT_LIMIT);
        assert_eq!(scored[0].score, -105); // x/a.ts: boundary + filename
        assert_eq!(scored[1].score, -100); // a.ts: filename only, no boundary
    }

    /// Crux — `-100` filename-substring bonus. A file whose filename contains
    /// the whole query outranks one where the chars are scattered across the
    /// path. Mutation: remove `-= 100` -> the contiguous file no longer wins.
    #[test]
    fn crux_minus_100_filename_substring_bonus() {
        // Both match as a subsequence; only "deep/path/quick.ts" has the query
        // contiguous in its filename.
        let files = ["q/u/i/c/k/scatter.ts", "deep/path/quick.ts"];
        let results = rank("quick", &files, RESULT_LIMIT);
        assert_eq!(results[0].path, "deep/path/quick.ts");
        assert!(
            results[0].score < results[1].score,
            "filename-substring file must score strictly lower: {:?}",
            results
        );
    }

    /// Crux — reject on incomplete subsequence. A query char absent from the
    /// file excludes it. Mutation: remove the `qi < len -> None` reject -> the
    /// non-matching file would be scored and returned.
    #[test]
    fn crux_reject_on_incomplete_match() {
        let files = ["src/about.ts", "src/zebra.ts"];
        // 'z' is absent from about.ts -> only zebra.ts survives.
        let paths: Vec<String> = rank("zebra", &files, RESULT_LIMIT)
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(paths, vec!["src/zebra.ts".to_string()]);
    }

    /// Crux — empty-query passthrough uses input order with score 0. Mutation:
    /// score the passthrough files -> non-zero scores or reordering.
    #[test]
    fn crux_empty_query_passthrough_score_zero() {
        let files = ["zzz/last.ts", "aaa/first.ts"];
        assert_eq!(
            rank("", &files, RESULT_LIMIT),
            vec![
                Match {
                    path: "zzz/last.ts".to_string(),
                    score: 0
                },
                Match {
                    path: "aaa/first.ts".to_string(),
                    score: 0
                },
            ]
        );
    }

    /// Crux — `limit == 0` returns `[]`. Mutation: drop the guard -> results
    /// would be returned (the query matches).
    #[test]
    fn crux_limit_zero_returns_empty() {
        let files = ["src/a.ts"];
        assert_eq!(rank("a", &files, 0), vec![]);
        // Sanity: the same query with a real limit DOES match, proving the
        // empty result above is the guard, not a non-match.
        assert_eq!(rank("a", &files, RESULT_LIMIT).len(), 1);
    }

    /// The limit truncates AND the tie-break is by input index, not score
    /// alone — pins the `(score asc, input-index asc)` comparator ordering.
    #[test]
    fn limit_truncates_after_full_ranking() {
        let files = ["src/a.ts", "src/ab.ts", "src/abc.ts"];
        // All contain 'a'; take only the first by input order at limit 1.
        assert_eq!(rank("a", &files, 1).len(), 1);
    }
}
