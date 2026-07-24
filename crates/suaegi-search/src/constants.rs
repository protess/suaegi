//! Constants shared by both search callers — verbatim from
//! `src/shared/text-search.ts:54-63`.

/// Per-file match cap applied by the rg backend (`--max-count`). rg-only in
/// Orca; git-grep has no per-file cap (see plan C4). `text-search.ts:54`.
pub const MAX_MATCHES_PER_FILE: usize = 100;

/// Default total-result cap when `SearchOptions::max_results` is unset.
/// `text-search.ts:55`.
pub const DEFAULT_SEARCH_MAX_RESULTS: usize = 2000;

/// Search wall-clock timeout in milliseconds; the M4 driver kills the child at
/// this point and marks the result `truncated`. `text-search.ts:56`.
pub const SEARCH_TIMEOUT_MS: u64 = 15_000;

/// Max rendered length of a single match's line context, so mega-byte
/// (minified/generated) lines can't blow past the SSH relay message cap. This
/// is a code-unit budget in Orca (JS `String.length` = UTF-16 units); the M3
/// clamp will state the Rust measurement unit explicitly. `text-search.ts:62`.
pub const MAX_LINE_CONTENT_LENGTH: usize = 500;

/// Files larger than this are skipped (rg `--max-filesize`, expressed as whole
/// MiB). `text-search.ts:59` (module-private in Orca; exported here for M2).
pub const SEARCH_MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Marker inserted where a clamped line context was truncated (U+2026 HORIZONTAL
/// ELLIPSIS). `text-search.ts:63` (module-private in Orca; exported here for M3).
pub const TRUNCATION_MARKER: &str = "\u{2026}";

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the exact constant values against Orca `text-search.ts:54-63`.
    /// These ARE the search budget/caps; a silent drift changes result counts
    /// and truncation behavior.
    #[test]
    fn constants_match_orca() {
        assert_eq!(MAX_MATCHES_PER_FILE, 100);
        assert_eq!(DEFAULT_SEARCH_MAX_RESULTS, 2000);
        assert_eq!(SEARCH_TIMEOUT_MS, 15_000);
        assert_eq!(MAX_LINE_CONTENT_LENGTH, 500);
        assert_eq!(SEARCH_MAX_FILE_SIZE, 5 * 1024 * 1024);
        assert_eq!(SEARCH_MAX_FILE_SIZE, 5_242_880);
        // U+2026, a single scalar; the byte form must be the 3-byte UTF-8 ellipsis.
        assert_eq!(TRUNCATION_MARKER, "…");
        assert_eq!(TRUNCATION_MARKER.chars().count(), 1);
        assert_eq!(TRUNCATION_MARKER.chars().next(), Some('\u{2026}'));
    }
}
