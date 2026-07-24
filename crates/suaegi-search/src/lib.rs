//! Content-search backend — the pure half of Orca's `src/shared/text-search.ts`
//! (`@ v1.4.150-rc.0`). No process spawning, no fs, no transport translation:
//! the driver (M4) owns execution. This is the peer of the already-ported Quick
//! Open file-name scorer (`suaegi-fuzzy`), for grepping file *contents* via
//! ripgrep (`--json`) with a git-grep fallback.
//!
//! # Milestone M1 — pure foundation
//! This crate currently contains only the pure, infallible foundation:
//! - **Types** ([`SearchMatch`], [`SearchFileResult`], [`SearchResult`],
//!   [`SearchOptions`]) — verbatim from `src/shared/types.ts:3543-3574`, plus
//!   the internal [`SearchAccumulator`] used by the M3 stream parser.
//! - **Constants** — the shared caps/timeouts (`text-search.ts:54-63`).
//! - **String helpers** — [`normalize_relative_path`], [`split_search_glob_patterns`],
//!   [`escape_regex`] (verbatim ports; `std::path` is deliberately avoided in
//!   path normalization because it is platform-dependent and would diverge from
//!   the JS oracle).
//! - **Match-count normalization** — [`normalize_search_result`] &friends
//!   (`src/shared/search-match-count.ts:3-30`).
//!
//! The argv builder (M2), stream parser (M3), and tokio drivers (M4) — and their
//! `serde_json`/`regex`/`tokio` dependencies — are intentionally NOT here yet.
//!
//! # JS→Rust boundary shape
//! The wire types serialize with `rename_all = "camelCase"` so they match Orca's
//! IPC shape (`filePath`, `matchCount`, …) once M4 crosses the boundary. Rust
//! field names stay `snake_case`.

mod constants;
mod glob;
mod js_trim;
mod match_count;
mod path;
mod regex_escape;
mod types;

pub use constants::{
    DEFAULT_SEARCH_MAX_RESULTS, MAX_LINE_CONTENT_LENGTH, MAX_MATCHES_PER_FILE,
    SEARCH_MAX_FILE_SIZE, SEARCH_TIMEOUT_MS, TRUNCATION_MARKER,
};
pub use glob::split_search_glob_patterns;
pub use match_count::{
    is_valid_match_count, normalize_search_file_match_count, normalize_search_result,
};
pub use path::normalize_relative_path;
pub use regex_escape::escape_regex;
pub use types::{
    create_accumulator, SearchAccumulator, SearchFileResult, SearchMatch, SearchOptions,
    SearchResult,
};
