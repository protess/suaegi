//! Content-search backend — the pure half of Orca's `src/shared/text-search.ts`
//! (`@ v1.4.150-rc.0`). No process spawning, no fs, no transport translation:
//! the driver (M4) owns execution. This is the peer of the already-ported Quick
//! Open file-name scorer (`suaegi-fuzzy`), for grepping file *contents* via
//! ripgrep (`--json`) with a git-grep fallback.
//!
//! # Milestones M1–M4 — pure foundation + argv builders + stream parsers + drivers
//! The pure, IO-free half of Orca's module (M1–M3):
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
//! - **M2 — argv builders + submatch locator** — [`build_rg_args`],
//!   [`build_git_grep_args`], [`to_git_glob_pathspec`] (verbatim argv order; the
//!   `--`/`-e … --` terminators are the argv-injection guard), and
//!   [`build_submatch_regex`] (the `regex`-crate best-effort locator, plan
//!   C2/C3). This is where the `regex` dependency lands.
//! - **M3 — stream parsers + accumulator** — [`clamp_line_context`] (plan **C1**
//!   byte-safety: canonical byte-derived source coords + char-boundary-safe render
//!   window), [`ingest_rg_json_line`] (tolerant `serde_json` parse; **C6** byte-safe
//!   empty-submatch fallback), [`ingest_git_grep_line`] (modern NUL + legacy colon
//!   formats; git-confirmed-hit fallback), and [`finalize`]. The `push_match`
//!   truncated invariant (**C5**) and `Ingest` verdict live here. This is where
//!   `serde_json` becomes a real dependency.
//!
//! - **M4 — the IMPURE process drivers** — [`run_search`] spawns `rg` (or falls
//!   back to `git grep`) and streams the child's stdout through the M1–M3 parsers,
//!   with a wall-clock timeout, explicit kill/reap, and the **transient≠empty**
//!   contract ([`SearchError`]). This is where `tokio`/`thiserror`/`libc` land.
//!
//! # JS→Rust boundary shape
//! The wire types serialize with `rename_all = "camelCase"` so they match Orca's
//! IPC shape (`filePath`, `matchCount`, …) once M4 crosses the boundary. Rust
//! field names stay `snake_case`.

mod clamp;
mod constants;
mod driver;
mod git_grep_args;
mod glob;
mod ingest;
mod js_trim;
mod match_count;
mod path;
mod regex_escape;
mod rg_args;
mod submatch;
mod types;

pub use clamp::{clamp_line_context, Clamped};
pub use driver::{run_search, SearchError};
pub use constants::{
    DEFAULT_SEARCH_MAX_RESULTS, MAX_LINE_CONTENT_LENGTH, MAX_MATCHES_PER_FILE,
    SEARCH_MAX_FILE_SIZE, SEARCH_TIMEOUT_MS, TRUNCATION_MARKER,
};
pub use ingest::{finalize, ingest_git_grep_line, ingest_rg_json_line, Ingest};
pub use git_grep_args::{build_git_grep_args, to_git_glob_pathspec};
pub use glob::split_search_glob_patterns;
pub use rg_args::build_rg_args;
pub use submatch::build_submatch_regex;
pub use match_count::{
    is_valid_match_count, normalize_search_file_match_count, normalize_search_result,
};
pub use path::normalize_relative_path;
pub use regex_escape::escape_regex;
pub use types::{
    create_accumulator, SearchAccumulator, SearchFileResult, SearchMatch, SearchOptions,
    SearchResult,
};
