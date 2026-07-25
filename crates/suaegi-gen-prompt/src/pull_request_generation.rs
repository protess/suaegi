//! Port of Orca `pull-request-generation.ts` (@ v1.4.150-rc.0, 175L).
//!
//! Builds the PR-fields prompt and parses the LLM's JSON response.
//!
//! # C4 — JSON parse decision: **serde_json** (not hand-rolled)
//! `parse_generated_pull_request_fields` mirrors JS `JSON.parse`. A hand-rolled
//! JSON parser faithful to `JSON.parse` (SyntaxError on malformed *and* trailing
//! tokens, duplicate-key last-wins, string escapes + `\uXXXX`, number edge
//! cases, nested skip of unused fields) is error-prone — exactly the
//! "hollow-test magnet" this repo has shipped >=5 times. The plan C4 note
//! authorizes `serde_json` (already a workspace dep) here for correctness.
//! Mapping to Orca's throw semantics (`transient != garbage`):
//! - malformed / trailing-token JSON → `Err(ParseError::InvalidJson)` (JS SyntaxError);
//! - `null` / number / string / bool top-level → `Err(ParseError::NotObject)`
//!   (JS `throw 'Expected a JSON object.'`);
//! - **array `[]` → `Ok(fallback)`** — `typeof [] === 'object'` in JS, so it is
//!   treated as an object with no fields, NOT an error (C4 correction);
//! - object → per-field parse with fallbacks.
//!
//! # Other ported quirks (C5/C6)
//! - `strip_json_fence`/`get_json_fence_body` are hand-rolled (no `[\s\S]` match,
//!   no `/\r\n/` replace — the oracle spies forbid them).
//! - [`starts_with_ascii_ignore_case`] folds ASCII case ONLY (`A`-`Z` +32), never
//!   Unicode — matches Orca's intent (`to_ascii_lowercase`, not `to_lowercase`).
//! - `/[.]+$/g` (title) strips a trailing run of `'.'`; `/\s+$/g` (body) strips
//!   trailing JS whitespace only (leading preserved).

use crate::commit_message_prompt::truncate_diff_for_prompt;
use crate::js_ws::{js_trim, js_trim_end};

/// PR draft context for [`build_pull_request_fields_prompt`].
#[derive(Debug, Clone)]
pub struct PullRequestDraftContext {
    pub branch: Option<String>,
    pub base: String,
    pub branch_changed_by_preparation: bool,
    pub current_title: String,
    pub current_body: String,
    pub current_draft: bool,
    pub commit_summary: String,
    pub change_summary: String,
    pub patch: String,
}

/// The parsed/derived PR fields (`GeneratedPullRequestFields`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestFields {
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

/// The `Pick<PullRequestDraftContext, 'base'|'currentTitle'|'currentBody'|'currentDraft'>`
/// fallback passed to [`parse_generated_pull_request_fields`].
#[derive(Debug, Clone)]
pub struct PullRequestFallback {
    pub base: String,
    pub current_title: String,
    pub current_body: String,
    pub current_draft: bool,
}

/// Reasons [`parse_generated_pull_request_fields`] throws (Orca's two throw
/// paths — the `transient` signal for a retry, distinct from `garbage` that is
/// silently filled from fallbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// `JSON.parse` SyntaxError (malformed or trailing-token JSON).
    InvalidJson,
    /// Parsed a non-object top-level value (`null`/number/string/bool).
    NotObject,
}

/// `limitSection` (char-budget truncation), char (scalar) units per C1.
fn limit_section(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let omitted = value.chars().count() - max_chars;
    let head: String = value.chars().take(max_chars).collect();
    format!("{head}\n\n[truncated: {omitted} characters omitted]")
}

/// Builds the PR-fields prompt. `branch == None` → `(detached)`; empty
/// title/body/summary render as `(empty)`/`(none)`; the patch is truncated with
/// the default budget.
pub fn build_pull_request_fields_prompt(
    context: &PullRequestDraftContext,
    custom_prompt: &str,
) -> String {
    let branch = context.branch.as_deref().unwrap_or("(detached)");
    let current_title = if context.current_title.is_empty() {
        "(empty)"
    } else {
        context.current_title.as_str()
    };
    let current_desc = if context.current_body.is_empty() {
        "(empty)"
    } else {
        context.current_body.as_str()
    };
    let current_draft = if context.current_draft {
        "true"
    } else {
        "false"
    };
    let commit_line = limit_section(
        if context.commit_summary.is_empty() {
            "(none)"
        } else {
            context.commit_summary.as_str()
        },
        8_000,
    );
    let change_line = limit_section(
        if context.change_summary.is_empty() {
            "(none)"
        } else {
            context.change_summary.as_str()
        },
        8_000,
    );
    let patch_trunc = truncate_diff_for_prompt(&context.patch);
    let head_line = format!("Head branch: {branch}");
    let base_line = format!("Current base: {}", context.base);
    let title_line = format!("Current title: {current_title}");
    let desc_line = format!("Current description: {current_desc}");
    let draft_line = format!("Current draft: {current_draft}");

    let base = [
        "You are generating pull request details.",
        "Return ONLY compact JSON with this exact shape:",
        "{\"base\":\"branch-name\",\"title\":\"short title\",\"body\":\"markdown description\",\"draft\":false}",
        "",
        "Rules:",
        "- Use the branch diff and commits below as source of truth.",
        "- Keep the base branch as the current base unless the diff clearly targets a different branch.",
        "- Title: concise, specific, no trailing period.",
        "- Body: useful Markdown summary for reviewers. Include testing notes only when evidence exists.",
        "- If Current description contains a pull request or merge request template, preserve its headings, required sections, and checklists while filling relevant sections from the branch changes.",
        "- Leave genuinely unknown template items as TODO or unchecked instead of deleting them.",
        "- draft: true only when the changes clearly look unfinished, WIP, or unsafe to review.",
        "- Do not include labels, reviewers, code fences, prose, or any keys beyond base/title/body/draft.",
        "",
        head_line.as_str(),
        base_line.as_str(),
        title_line.as_str(),
        desc_line.as_str(),
        draft_line.as_str(),
        "",
        "Commits:",
        commit_line.as_str(),
        "",
        "Changed files:",
        change_line.as_str(),
        "",
        "Patch:",
        "```diff",
        patch_trunc.as_str(),
        "```",
    ]
    .join("\n");

    let trimmed_prompt = js_trim(custom_prompt);
    if trimmed_prompt.is_empty() {
        return [
            base.as_str(),
            "",
            "Final output requirement:",
            "Return compact JSON only with keys base, title, body, and draft. No prose or code fences.",
        ]
        .join("\n");
    }
    let custom_section = limit_section(trimmed_prompt, 4_000);
    [
        base.as_str(),
        "",
        "Additional user prompt:",
        custom_section.as_str(),
        "",
        "Final output requirement:",
        "Return compact JSON only with keys base, title, body, and draft. No prose or code fences.",
    ]
    .join("\n")
}

/// Extracts JSON text: JS-trim, strip an enclosing ```-fence (hand-rolled), then
/// take the span from the first `{` to the last `}`.
fn strip_json_fence(raw: &str) -> String {
    let trimmed = js_trim(raw);
    let text: String = match get_json_fence_body(trimmed) {
        Some(body) => js_trim(body).to_string(),
        None => trimmed.to_string(),
    };
    let start = text.find('{');
    let end = text.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if e > s => text[s..=e].to_string(),
        _ => text,
    }
}

/// Hand-rolled fence detection (no `[\s\S]` match). `charCodeAt` is ported as
/// ASCII byte inspection; fence markers/`\n`/`\r` are ASCII so byte indices
/// agree with UTF-16 on any real fence (documented C1 divergence otherwise).
fn get_json_fence_body(text: &str) -> Option<&str> {
    let mut body_start = get_line_break_end(text, 3);
    if body_start.is_none() && starts_with_ascii_ignore_case(text, "```json", 0) {
        body_start = get_line_break_end(text, 7);
    }
    let body_start = body_start?;
    if !text.ends_with("```") {
        return None;
    }
    let close_start = text.len() - 3;
    let body_end = get_body_end_before_closing_fence(text, close_start)?;
    if body_end < body_start {
        return Some(""); // JS `slice(a, b)` with `b < a` yields ''.
    }
    Some(&text[body_start..body_end])
}

/// `getLineBreakEnd`: byte index just past a LF/CRLF/CR at `index`, or `None`
/// (mirrors `charCodeAt(index)` returning NaN out of range → `null`).
fn get_line_break_end(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    match bytes.get(index).copied() {
        Some(10) => Some(index + 1),
        Some(13) => Some(if bytes.get(index + 1) == Some(&10) {
            index + 2
        } else {
            index + 1
        }),
        _ => None,
    }
}

/// `getBodyEndBeforeClosingFence`: strips a CRLF/LF/CR immediately before the
/// closing fence, or `None`.
fn get_body_end_before_closing_fence(text: &str, close_start: usize) -> Option<usize> {
    if close_start == 0 {
        return None; // charCodeAt(-1) → NaN.
    }
    let bytes = text.as_bytes();
    match bytes.get(close_start - 1).copied() {
        Some(10) => Some(
            if close_start >= 2 && bytes.get(close_start - 2) == Some(&13) {
                close_start - 2
            } else {
                close_start - 1
            },
        ),
        Some(13) => Some(close_start - 1),
        _ => None,
    }
}

/// ASCII-only case-insensitive prefix match (Orca :137-149, C5). `search` must
/// be lowercase ASCII. Folds ONLY `A`-`Z` (`+32`) — never Unicode case. Compares
/// bytes: a multibyte lead byte never equals an ASCII search byte, so a non-ASCII
/// "uppercase" is NOT matched (the whole point vs `to_lowercase`).
pub fn starts_with_ascii_ignore_case(value: &str, search: &str, start_index: usize) -> bool {
    let v = value.as_bytes();
    let s = search.as_bytes();
    if start_index + s.len() > v.len() {
        return false;
    }
    for i in 0..s.len() {
        let code = v[start_index + i];
        let normalized = if code.is_ascii_uppercase() {
            code + 32
        } else {
            code
        };
        if normalized != s[i] {
            return false;
        }
    }
    true
}

/// Parses the LLM's JSON PR-fields response. See the module C4 note for the
/// throw semantics.
pub fn parse_generated_pull_request_fields(
    raw: &str,
    fallback: &PullRequestFallback,
) -> Result<PullRequestFields, ParseError> {
    let stripped = strip_json_fence(raw);
    let parsed: serde_json::Value =
        serde_json::from_str(&stripped).map_err(|_| ParseError::InvalidJson)?;

    // JS: `if (!parsed || typeof parsed !== 'object') throw`. `null` → !parsed;
    // number/string/bool → typeof !== 'object'. Array → typeof === 'object' →
    // treated as a record with no fields (NOT an error).
    let record = match &parsed {
        serde_json::Value::Object(map) => Some(map),
        serde_json::Value::Array(_) => None,
        _ => return Err(ParseError::NotObject),
    };
    let field_str =
        |key: &str| -> Option<&str> { record.and_then(|m| m.get(key)).and_then(|v| v.as_str()) };
    let field_bool =
        |key: &str| -> Option<bool> { record.and_then(|m| m.get(key)).and_then(|v| v.as_bool()) };

    // base = typeof string ? trim : fallback.base
    let base = match field_str("base") {
        Some(s) => js_trim(s).to_string(),
        None => fallback.base.clone(),
    };
    // title = (string && trim truthy) ? trim.replace(/[.]+$/g,'') : fallback.currentTitle.trim()
    let title = match field_str("title") {
        Some(s) if !js_trim(s).is_empty() => js_trim(s).trim_end_matches('.').to_string(),
        _ => js_trim(&fallback.current_title).to_string(),
    };
    // body = typeof string ? replace(/\s+$/g,'') : fallback.currentBody (raw)
    let body = match field_str("body") {
        Some(s) => js_trim_end(s).to_string(),
        None => fallback.current_body.clone(),
    };
    // draft = typeof boolean ? draft : fallback.currentDraft
    let draft = field_bool("draft").unwrap_or(fallback.current_draft);

    Ok(PullRequestFields {
        base: if base.is_empty() {
            fallback.base.clone()
        } else {
            base
        },
        title: if title.is_empty() {
            "Update project files".to_string()
        } else {
            title
        },
        body,
        draft,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> PullRequestDraftContext {
        PullRequestDraftContext {
            branch: Some("feature/pr-details".into()),
            base: "main".into(),
            branch_changed_by_preparation: false,
            current_title: "Feature pr details".into(),
            current_body: "- Add form".into(),
            current_draft: false,
            commit_summary: "- feat: add generated PR details".into(),
            change_summary: "M\tsrc/file.ts".into(),
            patch: "diff --git a/src/file.ts b/src/file.ts\n+export const value = true".into(),
        }
    }

    fn fallback_from(ctx: &PullRequestDraftContext) -> PullRequestFallback {
        PullRequestFallback {
            base: ctx.base.clone(),
            current_title: ctx.current_title.clone(),
            current_body: ctx.current_body.clone(),
            current_draft: ctx.current_draft,
        }
    }

    // -- buildPullRequestFieldsPrompt: Orca oracle pull-request-generation.test.ts:24-47 --

    #[test]
    fn build_asks_for_compact_json_and_includes_context() {
        let prompt = build_pull_request_fields_prompt(&context(), "Use conventional PR titles.");
        assert!(prompt.contains("Return ONLY compact JSON"));
        assert!(prompt.contains("Head branch: feature/pr-details"));
        assert!(prompt.contains("Current base: main"));
        assert!(prompt.contains("Additional user prompt:"));
        assert!(prompt.contains("Use conventional PR titles."));
    }

    #[test]
    fn build_tells_agent_to_preserve_templates() {
        let mut ctx = context();
        ctx.current_body = "## Summary\n\n## Testing\n\n- [ ] Required checks".into();
        let prompt = build_pull_request_fields_prompt(&ctx, "");
        assert!(prompt.contains("preserve its headings, required sections, and checklists"));
        assert!(prompt.contains("Leave genuinely unknown template items as TODO or unchecked"));
    }

    // -- parseGeneratedPullRequestFields: Orca oracle :49-96 --

    #[test]
    fn parse_fenced_json_output() {
        let ctx = context();
        let fields = parse_generated_pull_request_fields(
            "```json\n{\"base\":\"main\",\"title\":\"fix: add details.\",\"body\":\"Summary\",\"draft\":true}\n```",
            &fallback_from(&ctx),
        )
        .unwrap();
        assert_eq!(
            fields,
            PullRequestFields {
                base: "main".into(),
                title: "fix: add details".into(),
                body: "Summary".into(),
                draft: true,
            }
        );
    }

    #[test]
    fn parse_crlf_fenced_json_output() {
        // Uppercase ```JSON tag + CRLF, handled by the hand-rolled fence scanner.
        let ctx = context();
        let fields = parse_generated_pull_request_fields(
            "```JSON\r\n{\"base\":\"main\",\"title\":\"fix: add details.\",\"body\":\"Summary\",\"draft\":true}\r\n```",
            &fallback_from(&ctx),
        )
        .unwrap();
        assert_eq!(fields.title, "fix: add details");
    }

    #[test]
    fn parse_falls_back_for_missing_optional_values() {
        let ctx = context();
        let fields =
            parse_generated_pull_request_fields("{\"title\":\"\"}", &fallback_from(&ctx)).unwrap();
        assert_eq!(
            fields,
            PullRequestFields {
                base: "main".into(),
                title: "Feature pr details".into(),
                body: "- Add form".into(),
                draft: false,
            }
        );
    }

    // -- C4 throw pins (Codex extra; oracle covers neither throw path). --

    #[test]
    fn c4_malformed_json_is_invalid_json_error() {
        let ctx = context();
        // `{bad` — first `{` found, no `}` → strip returns "{bad" → SyntaxError.
        assert_eq!(
            parse_generated_pull_request_fields("{bad", &fallback_from(&ctx)),
            Err(ParseError::InvalidJson)
        );
    }

    #[test]
    fn c4_non_object_top_levels_are_not_object_error() {
        let ctx = context();
        let fb = fallback_from(&ctx);
        for raw in ["null", "42", "\"str\"", "true"] {
            assert_eq!(
                parse_generated_pull_request_fields(raw, &fb),
                Err(ParseError::NotObject),
                "input {raw:?} should be NotObject"
            );
        }
    }

    #[test]
    fn c4_array_is_ok_fallback_not_error() {
        // `typeof [] === 'object'` in JS → treated as a record with no fields →
        // Ok(fallback), NOT an error. This is the cardinal C4 correction.
        let ctx = context();
        let fields = parse_generated_pull_request_fields("[]", &fallback_from(&ctx)).unwrap();
        assert_eq!(
            fields,
            PullRequestFields {
                base: "main".into(),
                title: "Feature pr details".into(), // fallback.currentTitle.trim()
                body: "- Add form".into(),
                draft: false,
            }
        );
    }

    #[test]
    fn c4_empty_title_falls_back_to_update_project_files_when_fallback_empty() {
        // With an EMPTY fallback title, `{"title":""}` → `title || 'Update
        // project files'` hardcoded default.
        let fb = PullRequestFallback {
            base: "main".into(),
            current_title: "".into(),
            current_body: "".into(),
            current_draft: false,
        };
        let fields = parse_generated_pull_request_fields("{\"title\":\"\"}", &fb).unwrap();
        assert_eq!(fields.title, "Update project files");
        assert_eq!(fields.base, "main");
    }

    #[test]
    fn c4_full_valid_object_parses_all_fields() {
        let fb = PullRequestFallback {
            base: "old-base".into(),
            current_title: "old title".into(),
            current_body: "old body".into(),
            current_draft: false,
        };
        let fields = parse_generated_pull_request_fields(
            "{\"base\":\"release\",\"title\":\"Ship it...\",\"body\":\"Body text  \\n\",\"draft\":true}",
            &fb,
        )
        .unwrap();
        assert_eq!(
            fields,
            PullRequestFields {
                base: "release".into(),
                title: "Ship it".into(),  // trailing dots stripped
                body: "Body text".into(), // trailing whitespace stripped, not leading
                draft: true,
            }
        );
    }

    // -- C5 pins (Codex extra): ASCII-only case fold. --

    #[test]
    fn c5_starts_with_ascii_ignore_case_folds_ascii_only() {
        assert!(starts_with_ascii_ignore_case("```JSON rest", "```json", 0));
        assert!(starts_with_ascii_ignore_case("HELLO", "hello", 0));
        // A non-ASCII "uppercase" whose UNICODE lowercase would be ASCII 'k'
        // (U+212A KELVIN SIGN) must NOT match — ASCII fold only.
        assert!(!starts_with_ascii_ignore_case("\u{212A}elvin", "kelvin", 0));
        // Out-of-range start / too-short value.
        assert!(!starts_with_ascii_ignore_case("ab", "abc", 0));
    }
}
