//! Port of Orca `commit-message-generation.ts` (@ v1.4.150-rc.0, 84L).
//!
//! The NEW structured commit prompt (built from staged context) plus
//! subject/body splitting of the agent's generated message.
//!
//! `TuiAgent`/`CommitMessageDraftAgent`/`CommitMessageDraftOptions` are dropped
//! (plan §0): the two ported pure functions never consume them, and pulling in
//! the giant `types.ts` string-literal union would add a needless dep surface.
//!
//! # Ported quirks / divergences
//! - **C6 no-`split('\n')` (spy-enforced):** `splitGeneratedCommitMessage` uses
//!   `indexOf('\n')` + slice, NOT `split('\n')` (Orca's oracle spies on
//!   `String.prototype.split` and forbids the `'\n'` separator). Hand-rolled
//!   here with [`str::find`].
//! - **C1 char-scalar (`.slice(0, 72)`):** the 72-char subject cut is measured
//!   in Unicode scalars; char-boundary-safe (never splits a scalar / panics).
//!   Identical to the ASCII oracle; documented divergence from UTF-16 otherwise.
//! - **C5 JS whitespace:** every `.trim()`/`.trimEnd()` is JS-faithful
//!   ([`crate::js_ws`]); `/[.]+$/g` strips a trailing run of `'.'` (ASCII period
//!   only, NOT `'…'`).

use crate::commit_message_prompt::{clean_generated_commit_message, truncate_diff_for_prompt};
use crate::js_ws::{js_trim, js_trim_end};

/// Staged context for [`build_commit_message_prompt`]. `branch == None` renders
/// as `(detached)`.
#[derive(Debug, Clone)]
pub struct CommitMessageDraftContext {
    pub branch: Option<String>,
    pub staged_summary: String,
    pub staged_patch: String,
}

/// The split result: normalized `subject`, preserved `body`, and re-composed
/// `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCommitMessage {
    pub subject: String,
    pub body: String,
    pub message: String,
}

/// `limitSection` (char-budget truncation). Line-boundary-agnostic, unlike
/// `truncate_diff_for_prompt`. Char (scalar) units per C1.
fn limit_section(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let omitted = value.chars().count() - max_chars;
    let head: String = value.chars().take(max_chars).collect();
    format!("{head}\n\n[truncated: {omitted} characters omitted]")
}

/// Builds the structured commit prompt from staged context. Uses
/// `truncate_diff_for_prompt` on the patch (asymmetry vs `build_commit_prompt`,
/// which embeds raw).
pub fn build_commit_message_prompt(
    context: &CommitMessageDraftContext,
    custom_prompt: &str,
) -> String {
    // Why: the staged patch is dropped when too large to read; fall back to the
    // file summary. NOTE (verbatim): emptiness is judged on the TRIMMED patch,
    // but truncation is applied to the ORIGINAL (untrimmed) `staged_patch`.
    let patch = if !js_trim(&context.staged_patch).is_empty() {
        truncate_diff_for_prompt(&context.staged_patch)
    } else {
        "(diff omitted — too large to read; infer the change from the staged file list above)"
            .to_string()
    };
    let branch = context.branch.as_deref().unwrap_or("(detached)");
    let branch_line = format!("Branch: {branch}");
    let staged_files = limit_section(&context.staged_summary, 6_000);

    let base = [
        "You are generating a single git commit message.",
        "Return only the commit message text. Do not include a preamble, quotes, or code fences.",
        "",
        "Rules:",
        "- First line: imperative mood, <= 72 chars, no trailing period.",
        "- Optional body: blank line, then short wrapped bullet points or prose explaining WHY.",
        "- Capture the primary user-visible or developer-visible change.",
        "- Use only the staged changes below as context.",
        "- Do not include \"Co-authored-by\" or other git trailers.",
        "",
        branch_line.as_str(),
        "",
        "Staged files:",
        staged_files.as_str(),
        "",
        "Staged patch:",
        "```diff",
        patch.as_str(),
        "```",
    ]
    .join("\n");

    let trimmed_prompt = js_trim(custom_prompt);
    if trimmed_prompt.is_empty() {
        return base;
    }
    let custom_section = limit_section(trimmed_prompt, 4_000);
    [
        base.as_str(),
        "",
        "Additional user prompt:",
        custom_section.as_str(),
    ]
    .join("\n")
}

/// Normalizes the subject and preserves the body. Cleans first, then splits on
/// the FIRST newline (`indexOf`/`find`, NOT `split('\n')` — C6 spy).
pub fn split_generated_commit_message(message: &str) -> GeneratedCommitMessage {
    let normalized = clean_generated_commit_message(message);
    let first_newline = normalized.find('\n');
    let subject_line = match first_newline {
        None => normalized.as_str(),
        Some(idx) => &normalized[..idx],
    };
    // `subjectLine.trim().replace(/[.]+$/g,'').slice(0,72).trimEnd()`.
    let trimmed = js_trim(subject_line);
    let no_trailing_dots = trimmed.trim_end_matches('.');
    let clipped: String = no_trailing_dots.chars().take(72).collect(); // C1 char-scalar cut
    let subject = js_trim_end(&clipped).to_string();

    let body = match first_newline {
        None => String::new(),
        Some(idx) => js_trim(&normalized[idx + 1..]).to_string(),
    };

    let safe_subject = if subject.is_empty() {
        "Update project files".to_string()
    } else {
        subject
    };
    let message = if body.is_empty() {
        safe_subject.clone()
    } else {
        format!("{safe_subject}\n\n{body}")
    };
    GeneratedCommitMessage {
        subject: safe_subject,
        body,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- buildCommitMessagePrompt: Orca oracle commit-message-generation.test.ts:8-54 --

    #[test]
    fn build_prompt_from_staged_context() {
        let prompt = build_commit_message_prompt(
            &CommitMessageDraftContext {
                branch: Some("feature/commit-drafts".into()),
                staged_summary: "M\tsrc/main/ipc/filesystem.ts".into(),
                staged_patch:
                    "diff --git a/src/main/ipc/filesystem.ts b/src/main/ipc/filesystem.ts\n+hello"
                        .into(),
            },
            "",
        );
        assert!(prompt.contains("Branch: feature/commit-drafts"));
        assert!(prompt.contains("Staged files:\nM\tsrc/main/ipc/filesystem.ts"));
        assert!(prompt.contains("Staged patch:\n```diff"));
        assert!(prompt.contains("+hello"));
        assert!(prompt.contains("Use only the staged changes below as context."));
        assert!(!prompt.contains("Additional user prompt:"));
    }

    #[test]
    fn build_prompt_keeps_custom_prompt_in_bounded_section() {
        let prompt = build_commit_message_prompt(
            &CommitMessageDraftContext {
                branch: None,
                staged_summary: "A\tREADME.md".into(),
                staged_patch: "+docs".into(),
            },
            "Use Conventional Commits.",
        );
        assert!(prompt.contains("Branch: (detached)"));
        assert!(prompt.contains("Additional user prompt:\nUse Conventional Commits."));
    }

    #[test]
    fn build_prompt_notes_omitted_diff() {
        let prompt = build_commit_message_prompt(
            &CommitMessageDraftContext {
                branch: Some("feature/big-diff".into()),
                staged_summary: "A\thuge.jsonl".into(),
                staged_patch: "".into(),
            },
            "",
        );
        assert!(prompt.contains("Staged files:\nA\thuge.jsonl"));
        assert!(prompt.contains("diff omitted — too large to read"));
    }

    // -- splitGeneratedCommitMessage: Orca oracle :56-83 --

    #[test]
    fn split_normalizes_subject_and_preserves_body() {
        let result = split_generated_commit_message(
            "Fix source control generation.\n\n- Move planning into main",
        );
        assert_eq!(
            result,
            GeneratedCommitMessage {
                subject: "Fix source control generation".into(),
                body: "- Move planning into main".into(),
                message: "Fix source control generation\n\n- Move planning into main".into(),
            }
        );
    }

    #[test]
    fn split_extracts_subject_and_body_without_line_array_splitting() {
        let body = "- Explain one generated change\n".repeat(10_000);
        let body = js_trim_end(&body); // .trimEnd()
        let result =
            split_generated_commit_message(&format!("Add generated paste protection\n\n{body}"));
        assert_eq!(result.subject, "Add generated paste protection");
        assert!(result.body.starts_with("- Explain one generated change\n"));
        assert!(result.body.ends_with("- Explain one generated change"));
    }

    // -- C1 pin (Codex extra): a multibyte subject > 72 chars is cut on a char
    //    boundary (never a panic / never a split scalar). --
    #[test]
    fn c1_multibyte_subject_72_cut_is_char_boundary_safe() {
        // 80 Korean scalars, no newline → subject only.
        let subject: String = "가".repeat(80);
        let result = split_generated_commit_message(&subject);
        assert_eq!(result.subject.chars().count(), 72);
        assert_eq!(result.subject, "가".repeat(72));
        // Astral (emoji) subject: also safe, 72 scalars, no panic.
        let emoji: String = "😀".repeat(80);
        let r2 = split_generated_commit_message(&emoji);
        assert_eq!(r2.subject.chars().count(), 72);
    }
}
