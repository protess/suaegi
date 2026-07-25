//! Task-list search-query DSL parser — a verbatim port of Orca's
//! `src/shared/task-query.ts` (@ v1.4.150-rc.0).
//!
//! The DSL is the GitHub-style `is:pr state:open author:alice label:bug ...`
//! filter language of Orca's task/work-item list. This crate is a pure,
//! **dependency-free** String↔struct layer: parse a raw query into a
//! [`ParsedTaskQuery`], serialize it back, apply a single filter change with
//! [`with_qualifier`], or strip `repo:` qualifiers with [`strip_repo_qualifiers`].
//!
//! # Faithfulness (plan Codex decisions C1–C4)
//! - **C1** — every `\s`/`.trim()` uses the hand-rolled ECMAScript whitespace
//!   set in [`js_ws`], not Rust's `char::is_whitespace`/`str::trim` (they diverge
//!   at U+FEFF and U+0085).
//! - **C2** — case-folding is ASCII-only (`to_ascii_lowercase`); recognition
//!   literals are all ASCII, so this exactly preserves Orca's `toLowerCase`.
//! - **C3** — two quote-handling defects are reproduced verbatim and pinned by
//!   regression tests (see [`serialize_task_query`]); do not "fix" them.
//! - **C4** — the tokenizer is a hand-rolled char state machine; no `regex` crate.

mod js_ws;
mod query;

pub use js_ws::{is_js_whitespace, js_trim};
pub use query::{
    parse_task_query, serialize_task_query, strip_repo_qualifiers, tokenize_search_query,
    with_qualifier, ParsedTaskQuery, QualifierValue, Scope, State, TaskQueryFilterKey,
};
