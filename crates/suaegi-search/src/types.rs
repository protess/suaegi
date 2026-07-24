//! Search result types — verbatim from `src/shared/types.ts:3543-3574`, plus
//! the internal accumulator (`text-search.ts:18-26`) used by the M3 parser.
//!
//! Rust fields are `snake_case`; serde `rename_all = "camelCase"` reproduces
//! Orca's IPC wire shape (`filePath`, `matchCount`, `lineContent`, …) for when
//! M4 crosses the boundary. Optional match fields serialize only when present,
//! matching Orca's conditional inclusion.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One match within a file. `types.ts:3544-3551`.
///
/// `column` is 1-based; `match_length` is the match's length. Per plan C1 these
/// are preserved as the canonical (byte-derived) source coordinates the rg
/// `--json` submatch reports — the M3 parser keeps them numeric, never
/// converting to char indices. The `display_*` fields carry the separately
/// computed, render-safe snippet coordinates (only set when a line was clamped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    /// 1-based line number.
    pub line: usize,
    /// 1-based column of the match start (canonical source coordinate).
    pub column: usize,
    /// Length of the match (canonical source coordinate).
    pub match_length: usize,
    /// The (possibly clamped) line context text.
    pub line_content: String,
    /// Render-safe column into `line_content` when the line was clamped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_column: Option<usize>,
    /// Render-safe match length into `line_content` when the line was clamped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_match_length: Option<usize>,
}

/// All matches for one file. `types.ts:3553-3558`.
///
/// `match_count` is the per-file hit count. It is modeled as `Option<i64>` (not
/// `Option<usize>`) so [`crate::is_valid_match_count`] can meaningfully reject
/// the one invalid state still representable in Rust — a negative count — that a
/// malformed cross-boundary payload could carry. JS's other invalid `number`
/// states (NaN, Infinity, non-integer) are unrepresentable here: serde rejects
/// them for an integer field upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileResult {
    /// Absolute path to the file.
    pub file_path: String,
    /// Path relative to the search root (normalized).
    pub relative_path: String,
    /// The matches found in this file.
    pub matches: Vec<SearchMatch>,
    /// Reported per-file match count; normalized up to `matches.len()` by
    /// [`crate::normalize_search_result`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_count: Option<i64>,
}

/// A complete search result. `types.ts:3560-3564`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Files with at least one match.
    pub files: Vec<SearchFileResult>,
    /// Total matches across all files.
    pub total_matches: usize,
    /// True when the result was cut short (per-total cap or timeout).
    pub truncated: bool,
}

/// Options for a search request. `types.ts:3566-3574`.
///
/// The boolean flags are `Option<bool>` (Orca's `caseSensitive?` etc.), with an
/// absent value meaning "off"; the M2 argv builder reads them as `unwrap_or(false)`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    /// The search query (literal or regex depending on `use_regex`).
    pub query: String,
    /// The root directory to search under.
    pub root_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_sensitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whole_word: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_regex: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,
}

/// Mutable accumulator threaded through the M3 stream parser. Orca's
/// `SearchAccumulator` (`text-search.ts:18-26`) keys results by absolute path in
/// a JS `Map`, which iterates in insertion order.
///
/// Rust's [`HashMap`] is unordered, so insertion order is tracked separately in
/// `file_order`; the M3 `finalize` must iterate `file_order` (not `file_map`) to
/// reproduce Orca's deterministic file ordering. This is the one structural
/// deviation from Orca's single `Map`, forced by Rust's unordered hash map.
#[derive(Debug, Clone, Default)]
pub struct SearchAccumulator {
    /// Absolute-path → accumulated result for that file.
    pub file_map: HashMap<String, SearchFileResult>,
    /// Absolute paths in first-insertion order (for deterministic finalize).
    pub file_order: Vec<String>,
    /// Running total of accepted matches.
    pub total_matches: usize,
    /// Set once the total cap or timeout cut the search short.
    pub truncated: bool,
}

/// Create an empty accumulator. `text-search.ts:24-26`.
pub fn create_accumulator() -> SearchAccumulator {
    SearchAccumulator {
        file_map: HashMap::new(),
        file_order: Vec::new(),
        total_matches: 0,
        truncated: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh accumulator is empty and not truncated.
    #[test]
    fn create_accumulator_is_empty() {
        let acc = create_accumulator();
        assert!(acc.file_map.is_empty());
        assert!(acc.file_order.is_empty());
        assert_eq!(acc.total_matches, 0);
        assert!(!acc.truncated);
    }

    /// Pin the camelCase IPC wire shape (the crate's central fidelity claim: it
    /// matches Orca's `src/shared/types.ts` JSON). Serializes to the EXACT keys
    /// Orca emits, and omits `None` optionals like `JSON.stringify` drops
    /// `undefined`. *Mutation:* dropping `rename_all = "camelCase"` (→ snake_case
    /// keys) or a wrong `rename` flips these key assertions.
    #[test]
    fn serializes_to_orca_camelcase_wire_keys() {
        // A clamped match carries the display_* fields; an unclamped one omits them.
        let clamped = SearchMatch {
            line: 3,
            column: 10,
            match_length: 6,
            line_content: "…snippet…".to_string(),
            display_column: Some(2),
            display_match_length: Some(6),
        };
        let v = serde_json::to_value(&clamped).unwrap();
        let obj = v.as_object().unwrap();
        // Exact wire keys, no snake_case leakage.
        for k in [
            "line",
            "column",
            "matchLength",
            "lineContent",
            "displayColumn",
            "displayMatchLength",
        ] {
            assert!(obj.contains_key(k), "missing wire key {k}: {v}");
        }
        assert!(!obj.contains_key("match_length"), "snake_case leaked: {v}");

        // Unclamped: display_* omitted (like JSON.stringify dropping undefined).
        let plain = SearchMatch {
            line: 1,
            column: 1,
            match_length: 3,
            line_content: "abc".to_string(),
            display_column: None,
            display_match_length: None,
        };
        let pv = serde_json::to_value(&plain).unwrap();
        assert!(!pv.as_object().unwrap().contains_key("displayColumn"));

        let file = SearchFileResult {
            file_path: "/root/a.ts".to_string(),
            relative_path: "a.ts".to_string(),
            matches: vec![plain],
            match_count: Some(1),
        };
        let fv = serde_json::to_value(&file).unwrap();
        let fobj = fv.as_object().unwrap();
        for k in ["filePath", "relativePath", "matches", "matchCount"] {
            assert!(fobj.contains_key(k), "missing file wire key {k}: {fv}");
        }

        let result = SearchResult {
            files: vec![file],
            total_matches: 1,
            truncated: true,
        };
        let rv = serde_json::to_value(&result).unwrap();
        let robj = rv.as_object().unwrap();
        for k in ["files", "totalMatches", "truncated"] {
            assert!(robj.contains_key(k), "missing result wire key {k}: {rv}");
        }

        // SearchOptions wire keys (deserialize from Orca's shape).
        let opts: SearchOptions = serde_json::from_str(
            r#"{"query":"q","rootPath":"/r","caseSensitive":true,"maxResults":50}"#,
        )
        .unwrap();
        assert_eq!(opts.query, "q");
        assert_eq!(opts.root_path, "/r");
        assert_eq!(opts.case_sensitive, Some(true));
        assert_eq!(opts.max_results, Some(50));
    }
}
