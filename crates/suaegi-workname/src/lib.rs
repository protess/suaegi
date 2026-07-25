//! Work / branch / workspace **name generation** — a verbatim port of Orca's
//! `src/shared/` naming cluster (@ v1.4.150-rc.0): `marine-creatures.ts`,
//! `branch-name-from-work.ts`, `display-name-from-work.ts`, `workspace-name.ts`,
//! and the hidden 5th dependency `workspace-name-text-scanner.ts`.
//!
//! Every module is pure and deterministic. The one suaegi dependency is the
//! already-merged [`suaegi_workref`] (`extract_work_identifier`,
//! `format_identifier_first`). No serde.
//!
//! # Modules
//! - [`marine_creatures`] — the 552-entry recognition corpus (data, C5).
//! - [`js_ws`] — local copy of the ECMAScript-whitespace predicate (C3).
//! - [`branch_name`] — branch-slug sanitize / creature recognition / prompt.
//! - [`text_scanner`] — the hand-rolled whitespace/word scanner (C3).
//! - [`workspace_name`] — slugify / title-case / intent pipeline (C1/C2/C4/C6).
//! - [`display_name`] — sidebar display-name composition.
//!
//! # Codex divergence summary (see each module for detail)
//! - **C1** full `char::to_lowercase()` before the ASCII whitelist (not
//!   `to_ascii_lowercase`) — `İ`→`i`, `K`(U+212A)→`k`.
//! - **C2** apostrophe helpers hand-scan scalars, classifying neighbors with an
//!   EXACT `[\p{L}\p{N}]` General Category regex (not `char::is_alphabetic`).
//! - **C3** whitespace folding/tokenizing is hand-rolled (no `/\s+/`, an oracle
//!   spy contract).
//! - **C4** every `\d`→`[0-9]`, `\b`→`(?-u:\b)`; dynamic regexes use
//!   `regex::escape`; Unicode stays ON globally for the C2 GC classification.
//! - **C5** `MARINE_CREATURES` is exactly 552 verbatim entries.
//! - **C6** UTF-16 code-unit indexing is ported to Unicode scalars
//!   (char-boundary-safe; narrow, documented astral divergence).

pub mod branch_name;
mod js_ws;
pub mod marine_creatures;
pub mod text_scanner;

pub mod display_name;
pub mod workspace_name;

pub use branch_name::{
    build_branch_name_prompt, humanize_branch_slug, is_auto_generated_creature_branch_name,
    sanitize_branch_slug, strip_configured_branch_prefix, BranchNameWorkContext,
    MAX_BRANCH_NAME_WORDS,
};
pub use display_name::derive_workspace_display_name;
pub use marine_creatures::MARINE_CREATURES;
pub use text_scanner::{collect_compact_workspace_words, fold_workspace_name_whitespace_to_hyphen};
pub use workspace_name::{
    get_linear_issue_workspace_name, get_linked_work_item_suggested_name,
    get_linked_work_item_workspace_name, get_workspace_intent_name, resolve_workspace_create_name,
    slugify_for_workspace_name, WorkItemType, WorkspaceIntentName, WorkspaceIntentWorkItem,
};
