//! `suaegi-gen-prompt` — a verbatim port of Orca's AI commit/PR generation
//! prompt-build + response-parse helpers (@ v1.4.150-rc.0).
//!
//! Four Orca modules, one Rust module each, plus a shared JS-whitespace
//! predicate:
//! - [`commit_message_agent_output`] — clean generated commit output, excerpt
//!   agent failure output, strip ANSI.
//! - [`commit_message_prompt`] — legacy commit prompt (C3 `$`-quirk), diff
//!   truncation (C1/C2), custom command tokenizer/planner.
//! - [`commit_message_generation`] — structured commit prompt, subject/body split.
//! - [`pull_request_generation`] — PR-fields prompt, JSON response parse (C4).
//!
//! # Fidelity contract (plan Codex decisions)
//! - **C1** — diff truncation measures Unicode scalars (chars), a documented
//!   divergence from Orca's UTF-16 code units; identical on the ASCII oracle,
//!   always char-boundary-safe (never panics). The `"...bytes omitted"` marker
//!   is a preserved historical misnomer.
//! - **C2** — the truncation marker is edge-exact, not "always appended".
//! - **C3** — `build_commit_prompt` reproduces JS `.replace('{{DIFF}}', diff)`'s
//!   `$`-pattern expansion verbatim (a deliberately-preserved Orca quirk).
//! - **C4** — `parse_generated_pull_request_fields` returns `Result`; malformed
//!   JSON and non-object top-levels are errors, but a JSON array is `Ok(fallback)`.
//! - **C5** — JS-faithful whitespace ([`js_ws`]); ASCII-only case fold.
//! - **C6** — every pattern is hand-rolled; NO `regex` crate.

pub mod commit_message_agent_output;
pub mod commit_message_generation;
pub mod commit_message_prompt;
pub mod js_ws;
pub mod pull_request_generation;

pub use commit_message_agent_output::{
    clean_generated_commit_message, excerpt_agent_failure_output, strip_ansi_control_sequences,
};
pub use commit_message_generation::{
    build_commit_message_prompt, split_generated_commit_message, CommitMessageDraftContext,
    GeneratedCommitMessage,
};
pub use commit_message_prompt::{
    build_commit_prompt, plan_custom_command, tokenize_custom_command_template,
    truncate_diff_for_prompt, truncate_diff_for_prompt_with_budget, CustomCommandPlan,
    CUSTOM_PROMPT_PLACEHOLDER, STAGED_DIFF_BYTE_BUDGET,
};
pub use pull_request_generation::{
    build_pull_request_fields_prompt, parse_generated_pull_request_fields,
    starts_with_ascii_ignore_case, ParseError, PullRequestDraftContext, PullRequestFallback,
    PullRequestFields,
};
