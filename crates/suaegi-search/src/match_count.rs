//! Match-count normalization — verbatim port of `search-match-count.ts:3-30`.
//!
//! Guards a `SearchResult` against under-counted or invalid per-file counts and
//! drops files that ended up with zero matches.

use crate::types::{SearchFileResult, SearchResult};

/// A per-file match count is valid iff it is a finite, non-negative integer
/// (`search-match-count.ts:3-7`).
///
/// In Rust the count is an `Option<i64>`, so "finite" and "integer" are
/// guaranteed by the type; only the non-negativity (and presence) checks remain.
/// `None` (Orca's `undefined`) is invalid → treated as `0` by the caller.
pub fn is_valid_match_count(value: Option<i64>) -> bool {
    matches!(value, Some(v) if v >= 0)
}

/// Normalize one file's count to at least the number of matches actually
/// collected: `max(valid_count_or_0, matches.len())` (`search-match-count.ts:9-14`).
///
/// An invalid or absent `match_count` contributes `0` to the `max`, so the
/// result is never less than `matches.len()`.
pub fn normalize_search_file_match_count(file: &SearchFileResult) -> usize {
    let count = if is_valid_match_count(file.match_count) {
        // Safe: `is_valid_match_count` proved `Some(v)` with `v >= 0`.
        file.match_count.unwrap_or(0).max(0) as usize
    } else {
        0
    };
    count.max(file.matches.len())
}

/// Filter out files with zero matches and normalize each surviving file's
/// `match_count` (`search-match-count.ts:23-30`).
///
/// The empty-matches filter wins even when a (malformed) payload claims a
/// non-zero `match_count`: a file with no matches is removed entirely.
pub fn normalize_search_result(result: SearchResult) -> SearchResult {
    let SearchResult {
        files,
        total_matches,
        truncated,
    } = result;

    let files = files
        .into_iter()
        .filter(|file| !file.matches.is_empty())
        .map(|mut file| {
            file.match_count = Some(normalize_search_file_match_count(&file) as i64);
            file
        })
        .collect();

    SearchResult {
        files,
        total_matches,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SearchMatch;

    fn a_match(line: usize) -> SearchMatch {
        SearchMatch {
            line,
            column: 1,
            match_length: 3,
            line_content: "foo".to_string(),
            display_column: None,
            display_match_length: None,
        }
    }

    fn file(rel: &str, match_count: Option<i64>, n_matches: usize) -> SearchFileResult {
        SearchFileResult {
            file_path: format!("/r/{rel}"),
            relative_path: rel.to_string(),
            matches: (1..=n_matches).map(a_match).collect(),
            match_count,
        }
    }

    /// Oracle case 37 (`text-search.test.ts:451`): missing and too-low per-file
    /// counts are raised to the real match count.
    ///
    /// Mutation target (d): replacing `max(count, len)` with just `count` makes
    /// `a.ts` report `0` (its `match_count` is absent) instead of `2` → fails.
    #[test]
    fn oracle_normalizes_missing_and_too_low_counts() {
        let result = SearchResult {
            files: vec![
                file("a.ts", None, 2),    // no match_count, 2 matches -> 2
                file("b.ts", Some(0), 1), // match_count 0 but 1 match -> 1
            ],
            total_matches: 3,
            truncated: false,
        };
        let out = normalize_search_result(result);
        let got: Vec<(&str, Option<i64>)> = out
            .files
            .iter()
            .map(|f| (f.relative_path.as_str(), f.match_count))
            .collect();
        assert_eq!(got, vec![("a.ts", Some(2)), ("b.ts", Some(1))]);
    }

    /// Oracle case 38 (`text-search.test.ts:475`): a file whose `match_count`
    /// claims matches but whose `matches` are empty is filtered out entirely —
    /// the empty-matches filter wins.
    #[test]
    fn oracle_filters_empty_file_despite_claimed_count() {
        let result = SearchResult {
            files: vec![file("a.ts", Some(2), 0)], // claims 2, has 0 matches
            total_matches: 0,
            truncated: false,
        };
        let out = normalize_search_result(result);
        assert!(out.files.is_empty());
    }

    /// A genuinely higher, valid `match_count` (e.g. from an rg `--max-count`
    /// cap where more hits existed than were surfaced) is preserved, not lowered
    /// to `matches.len()`.
    #[test]
    fn keeps_higher_valid_count() {
        let f = file("a.ts", Some(100), 3);
        assert_eq!(normalize_search_file_match_count(&f), 100);
    }

    /// A negative `match_count` is invalid → treated as 0, so the floor is
    /// `matches.len()`. Pins the `>= 0` validity check.
    #[test]
    fn negative_count_is_invalid_floored_to_len() {
        assert!(!is_valid_match_count(Some(-1)));
        assert!(!is_valid_match_count(None));
        assert!(is_valid_match_count(Some(0)));
        let f = file("a.ts", Some(-5), 2);
        assert_eq!(normalize_search_file_match_count(&f), 2);
    }

    /// `total_matches` and `truncated` pass through normalization unchanged.
    #[test]
    fn preserves_totals_and_truncated_flag() {
        let result = SearchResult {
            files: vec![file("a.ts", None, 1)],
            total_matches: 7,
            truncated: true,
        };
        let out = normalize_search_result(result);
        assert_eq!(out.total_matches, 7);
        assert!(out.truncated);
    }
}
