//! Port of Orca `commit-message-prompt.ts` (@ v1.4.150-rc.0, 250L).
//!
//! Assembles the (legacy) commit prompt, truncates an oversized diff on a fair
//! per-file budget, and tokenizes/plans a user-supplied custom command template.
//! Re-exports [`clean_generated_commit_message`] / [`excerpt_agent_failure_output`]
//! (Orca :20-23).
//!
//! # Ported quirks / divergences
//! - **C3 `$`-quirk (deliberately preserved Orca behavior):** `buildCommitPrompt`
//!   uses `String.prototype.replace('{{DIFF}}', diff)`, which interprets `$`
//!   patterns IN THE REPLACEMENT (`diff`). Reproduced verbatim — see
//!   [`build_commit_prompt`]. A literal replace was probably intended, but a
//!   unilateral divergence is forbidden (task-query C3 precedent).
//! - **C1 truncation unit = Unicode scalars (chars), NOT UTF-16.** Orca measures
//!   `.length`/`.slice`/`.lastIndexOf` in UTF-16 code units; exact UTF-16
//!   reproduction would need a u16 index space + surrogate-splitting + lone-
//!   surrogate serialization — overkill for an LLM prompt-size heuristic (not a
//!   security boundary). We measure in chars: identical to the ASCII oracle, a
//!   DOCUMENTED divergence on non-ASCII, always char-boundary-safe (never panics).
//! - **C2 marker contract (edge-exact, NOT "always append"):** see
//!   [`clip_section_on_line_boundary`]. The marker text `"...bytes omitted"` is a
//!   preserved historical misnomer (the unit is code units, not bytes) — kept
//!   verbatim.
//! - **C6 no-regex:** section split (`indexOf` loop) and `{prompt}` handling are
//!   hand-rolled; the tokenizer is a state machine, not a regex.

use crate::js_ws::{is_js_whitespace, js_trim};

pub use crate::commit_message_agent_output::{
    clean_generated_commit_message, excerpt_agent_failure_output,
};

const COMMIT_MESSAGE_BASE_PROMPT: &str = r#"You are generating a single git commit message.
Read the staged diff below and produce the message.

Rules:
- First line: imperative mood, <= 72 chars, no trailing period.
- Optional body: blank line, then wrapped at 72 chars explaining WHY.
- Output ONLY the commit message - no preamble, no code fences, no quotes.
- Do not include "Co-authored-by" trailers - Orca appends them after generation when configured.

Staged diff:
```diff
{{DIFF}}
```
"#;

/// Builds the final (legacy) prompt sent to the agent. The custom suffix is
/// appended verbatim when non-empty.
///
/// **C3 `$`-quirk (deliberately-preserved Orca behavior):** the placeholder
/// substitution reproduces JS `String.prototype.replace(searchString,
/// replaceString)` — the *replacement* (`diff`) has its `$$`/`$&`/`` $` ``/`$'`
/// patterns expanded (`$n` is literal, there are no capture groups). This does
/// NOT truncate the diff (raw embed), unlike `build_commit_message_prompt`.
pub fn build_commit_prompt(diff: &str, custom_suffix: &str) -> String {
    let base = js_replace_first_with_patterns(COMMIT_MESSAGE_BASE_PROMPT, "{{DIFF}}", diff);
    let trimmed_suffix = js_trim(custom_suffix);
    if trimmed_suffix.is_empty() {
        return base;
    }
    format!("{base}\n\nAdditional user prompt:\n{trimmed_suffix}")
}

/// Replaces the FIRST occurrence of `needle` in `haystack`, expanding JS
/// replacement-string patterns in `replacement`:
/// `$$`→`$`, `$&`→matched text, `` $` ``→prefix-before-match, `$'`→suffix-after,
/// and `$`+anything-else→literal `$`. Reproduces the C3 quirk.
fn js_replace_first_with_patterns(haystack: &str, needle: &str, replacement: &str) -> String {
    let pos = match haystack.find(needle) {
        None => return haystack.to_string(),
        Some(p) => p,
    };
    let prefix = &haystack[..pos];
    let matched = needle;
    let suffix = &haystack[pos + needle.len()..];

    let mut out = String::with_capacity(haystack.len() + replacement.len());
    out.push_str(prefix);
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                Some('$') => {
                    out.push('$');
                    chars.next();
                }
                Some('&') => {
                    out.push_str(matched);
                    chars.next();
                }
                Some('`') => {
                    out.push_str(prefix);
                    chars.next();
                }
                Some('\'') => {
                    out.push_str(suffix);
                    chars.next();
                }
                // `$n`/`$<..>`/lone `$` → literal `$`; the next char is handled
                // normally on the following iteration.
                _ => out.push('$'),
            }
        } else {
            out.push(c);
        }
    }
    out.push_str(suffix);
    out
}

/// The default diff budget. **Historical misnomer:** the unit is UTF-16 code
/// units in Orca (chars here, per C1), NOT bytes.
pub const STAGED_DIFF_BYTE_BUDGET: usize = 200_000;

/// Splits a unified diff into one section per file, keyed on the `diff --git`
/// header. Each section keeps the leading newline that preceded its header so
/// concatenating the sections reproduces the original byte-for-byte.
fn split_diff_into_file_sections(diff: &str) -> Vec<&str> {
    let boundary = "\ndiff --git ";
    let mut sections = Vec::new();
    let mut start = 0usize;
    while let Some(rel) = diff[start..].find(boundary) {
        let next = start + rel;
        // Include the boundary newline in the current section; the next section
        // starts at the `diff --git` header itself.
        sections.push(&diff[start..next + 1]);
        start = next + 1;
    }
    sections.push(&diff[start..]);
    sections
}

/// Clips one section to `limit` chars on a line boundary and records how many
/// chars were dropped.
///
/// **C2 marker contract (edge-exact):**
/// - not truncated (`len <= limit`) → section unchanged, NO marker;
/// - truncated + `limit <= 0` → empty string, NO marker;
/// - truncated + `marker.len >= limit` → marker prefix only (`marker[..limit]`);
/// - else → body clipped on a line boundary + full marker, with a marker-length
///   re-clip (`min(cut, max(0, limit - marker.len))`) so the total stays `<= limit`.
fn clip_section_on_line_boundary(section: &str, limit: usize) -> String {
    let section_len = section.chars().count();
    if section_len <= limit {
        return section.to_string();
    }
    if limit == 0 {
        // JS `limit <= 0`; allocations are non-negative so `== 0` is exact.
        return String::new();
    }

    let marker_for = |omitted: usize| format!("\n...(diff truncated, {omitted} bytes omitted)\n");
    let marker = marker_for(section_len);
    if marker.chars().count() >= limit {
        // Even the marker doesn't fit — return it clipped to `limit` chars.
        return marker.chars().take(limit).collect();
    }

    // Reserve headroom for the marker, then back up to the previous newline
    // unless that would discard most of the budget (one very long line).
    let target = limit - marker.chars().count();
    let line_break = last_index_of_newline(section, target);
    let cut = match line_break {
        Some(lb) if (lb as f64) > (target as f64) / 2.0 => lb,
        _ => target,
    };
    let omitted = section_len - cut;
    let marker = marker_for(omitted);
    let upper = limit.saturating_sub(marker.chars().count());
    let body_end = cut.min(upper);
    let body: String = section.chars().take(body_end).collect();
    format!("{body}{marker}")
}

/// `section.lastIndexOf('\n', target)` in char (scalar) units: the greatest
/// char index `i <= target` where the char is `'\n'`, or `None`.
fn last_index_of_newline(section: &str, target: usize) -> Option<usize> {
    let mut result = None;
    for (idx, c) in section.chars().enumerate() {
        if idx > target {
            break;
        }
        if c == '\n' {
            result = Some(idx);
        }
    }
    result
}

/// Water-filling fair allocation: everyone starts with an equal share, and the
/// slack from files that fit is handed back to the files that don't. Keeps one
/// huge generated file from starving human-authored changes elsewhere.
fn allocate_budget_fairly(sizes: &[usize], budget: usize) -> Vec<usize> {
    let mut alloc = vec![0usize; sizes.len()];
    let mut active: Vec<usize> = (0..sizes.len()).collect();
    let mut remaining = budget;
    while !active.is_empty() && remaining > 0 {
        let share = remaining / active.len(); // Math.floor
        if share == 0 {
            break;
        }
        let mut still_active = Vec::new();
        for &i in &active {
            let need = sizes[i] - alloc[i];
            let grant = need.min(share);
            alloc[i] += grant;
            remaining -= grant;
            if grant < need {
                still_active.push(i);
            }
        }
        active = still_active;
    }
    alloc
}

/// Truncates a diff that exceeds `budget` ([`STAGED_DIFF_BYTE_BUDGET`]). Splits
/// the budget fairly across files and clips on line boundaries.
pub fn truncate_diff_for_prompt(diff: &str) -> String {
    truncate_diff_for_prompt_with_budget(diff, STAGED_DIFF_BYTE_BUDGET)
}

/// [`truncate_diff_for_prompt`] with an explicit budget (Orca's default param).
pub fn truncate_diff_for_prompt_with_budget(diff: &str, budget: usize) -> String {
    if diff.chars().count() <= budget {
        return diff.to_string();
    }
    let sections = split_diff_into_file_sections(diff);
    if sections.len() <= 1 {
        return clip_section_on_line_boundary(diff, budget);
    }
    let sizes: Vec<usize> = sections.iter().map(|s| s.chars().count()).collect();
    let allocations = allocate_budget_fairly(&sizes, budget);
    let mut out = String::new();
    for (i, section) in sections.iter().enumerate() {
        out.push_str(&clip_section_on_line_boundary(section, allocations[i]));
    }
    out
}

pub const CUSTOM_PROMPT_PLACEHOLDER: &str = "{prompt}";

/// A spawn-ready plan for a custom command template, or an error message.
/// Mirrors Orca's `CustomCommandPlan` discriminated union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomCommandPlan {
    Ok {
        binary: String,
        args: Vec<String>,
        stdin_payload: Option<String>,
    },
    Err(String),
}

/// Tokenizes a custom command template. POSIX-shell **grouping only** (single +
/// double quotes, backslash escapes inside double quotes). `$VAR`/command-subst/
/// globs/`~` are NOT expanded. Returns `Ok(tokens)` or `Err(message)` (never
/// panics; mirrors the `{ok:false}` union, not a JS throw).
pub fn tokenize_custom_command_template(template: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = quote {
            if ch == '\\' && q == '"' && i + 1 < chars.len() {
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == q {
                quote = None;
                i += 1;
                // Leaving a quoted region keeps the token open — `a"b"c` → `abc`.
                in_token = true;
                continue;
            }
            current.push(ch);
            i += 1;
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            in_token = true;
            i += 1;
            continue;
        }

        if ch == '\\' && i + 1 < chars.len() {
            current.push(chars[i + 1]);
            in_token = true;
            i += 2;
            continue;
        }

        if is_js_whitespace(ch) {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
            i += 1;
            continue;
        }

        current.push(ch);
        in_token = true;
        i += 1;
    }

    if quote.is_some() {
        return Err("Unclosed quote in command template.".to_string());
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Parses a template into a spawn-ready binary + argv, substituting `{prompt}`.
/// When the template contains no `{prompt}`, the prompt is delivered via stdin
/// (mirrors `claude -p`). Quoting is a tokenizer concern only (argv, no shell).
pub fn plan_custom_command(template: &str, prompt: &str) -> CustomCommandPlan {
    let tokens = match tokenize_custom_command_template(template) {
        Ok(t) => t,
        Err(e) => return CustomCommandPlan::Err(e),
    };
    if tokens.is_empty() {
        return CustomCommandPlan::Err("Custom command is empty.".to_string());
    }
    let binary = &tokens[0];
    let rest = &tokens[1..];
    if binary.is_empty() {
        return CustomCommandPlan::Err("Custom command must start with a binary name.".to_string());
    }

    // `token.split('{prompt}').join(prompt)` = replaceAll, hand-rolled.
    let substitute = |token: &str| -> String {
        if token.contains(CUSTOM_PROMPT_PLACEHOLDER) {
            token
                .split(CUSTOM_PROMPT_PLACEHOLDER)
                .collect::<Vec<_>>()
                .join(prompt)
        } else {
            token.to_string()
        }
    };
    let uses_placeholder = tokens.iter().any(|t| t.contains(CUSTOM_PROMPT_PLACEHOLDER));
    if uses_placeholder {
        CustomCommandPlan::Ok {
            binary: substitute(binary),
            args: rest.iter().map(|t| substitute(t)).collect(),
            stdin_payload: None,
        }
    } else {
        CustomCommandPlan::Ok {
            binary: binary.clone(),
            args: rest.to_vec(),
            stdin_payload: Some(prompt.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- buildCommitPrompt: Orca oracle :16-33 --

    #[test]
    fn build_embeds_the_diff() {
        let prompt = build_commit_prompt("diff --git a/foo b/foo\n+hello", "");
        assert!(prompt.contains("diff --git a/foo b/foo"));
        assert!(prompt.contains("+hello"));
        assert!(prompt.contains("First line: imperative mood"));
    }

    #[test]
    fn build_appends_custom_suffix_when_non_empty() {
        let prompt = build_commit_prompt("diff", "Use Conventional Commits.");
        assert!(prompt.contains("Additional user prompt:"));
        assert!(prompt.ends_with("Use Conventional Commits."));
    }

    #[test]
    fn build_omits_suffix_block_for_whitespace_only_suffix() {
        let prompt = build_commit_prompt("diff", "   \n  ");
        assert!(!prompt.contains("Additional user prompt:"));
    }

    // -- C3 `$`-quirk pin (Codex extra; oracle-uncovered). Deliberately-preserved
    //    Orca behavior: JS `.replace('{{DIFF}}', diff)` expands `$` patterns in
    //    the replacement `diff`. A literal replace was probably intended, but we
    //    reproduce verbatim rather than diverge unilaterally. --

    #[test]
    fn build_reproduces_dollar_ampersand_quirk() {
        // `$&` in the diff expands to the MATCHED text, i.e. the literal
        // `{{DIFF}}` placeholder — so it reappears where the diff should be.
        let prompt = build_commit_prompt("$&", "");
        assert!(prompt.contains("```diff\n{{DIFF}}\n```"));
    }

    #[test]
    fn build_reproduces_dollar_dollar_quirk() {
        // `$$` in the diff collapses to a single literal `$`.
        let prompt = build_commit_prompt("$$", "");
        assert!(prompt.contains("```diff\n$\n```"));
    }

    #[test]
    fn build_plain_diff_is_verbatim() {
        // No `$` → verbatim embed (the common, oracle-covered path).
        let prompt = build_commit_prompt("just a plain diff", "");
        assert!(prompt.contains("```diff\njust a plain diff\n```"));
    }

    #[test]
    fn build_literal_dollar_n_is_kept() {
        // `$5` has no capture group → kept literally (Codex C3 correction).
        let prompt = build_commit_prompt("cost is $5 now", "");
        assert!(prompt.contains("```diff\ncost is $5 now\n```"));
    }

    // -- truncateDiffForPrompt: Orca oracle :36-79 --

    #[test]
    fn truncate_returns_unchanged_within_budget() {
        let diff = "line\n".repeat(10);
        assert_eq!(truncate_diff_for_prompt(&diff), diff);
    }

    #[test]
    fn truncate_appends_marker_when_over_budget() {
        let oversized = "line\n".repeat(STAGED_DIFF_BYTE_BUDGET / 5 + 100);
        let result = truncate_diff_for_prompt(&oversized);
        assert!(result.chars().count() < oversized.chars().count());
        assert!(marker_matches(&result));
    }

    #[test]
    fn truncate_clips_on_line_boundary() {
        let diff = "keep this line\n".repeat(40);
        let result = truncate_diff_for_prompt_with_budget(&diff, 95);
        let body = result.split("\n...(diff truncated").next().unwrap();
        for line in body.split('\n').filter(|l| !l.is_empty()) {
            assert_eq!(line, "keep this line");
        }
    }

    #[test]
    fn truncate_keeps_output_within_tight_budget() {
        let files: String = (0..20)
            .map(|i| {
                format!(
                    "diff --git a/file-{i}.txt b/file-{i}.txt\n{}",
                    "+x\n".repeat(200)
                )
            })
            .collect();
        let result = truncate_diff_for_prompt_with_budget(&files, 120);
        assert!(result.chars().count() <= 120);
    }

    #[test]
    fn truncate_shares_budget_fairly() {
        let huge = format!(
            "diff --git a/data.jsonl b/data.jsonl\n{}",
            "+x\n".repeat(5000)
        );
        let small = "diff --git a/src/app.ts b/src/app.ts\n+const meaningful = true\n";
        let result = truncate_diff_for_prompt_with_budget(&format!("{huge}{small}"), 1_000);
        assert!(result.contains("a/src/app.ts"));
        assert!(result.contains("const meaningful = true"));
        assert!(marker_matches(&result));
    }

    /// Emulates the oracle's `/diff truncated, \d+ bytes omitted/` (no regex).
    fn marker_matches(s: &str) -> bool {
        let Some(after) = s.split("diff truncated, ").nth(1) else {
            return false;
        };
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        !digits.is_empty() && after[digits.len()..].starts_with(" bytes omitted")
    }

    // -- C2 marker-edge pins (Codex extra; oracle covers only mid-budget). We
    //    test the private clip directly. --

    #[test]
    fn c2_budget_zero_yields_empty_no_marker() {
        assert_eq!(clip_section_on_line_boundary("some long section\n", 0), "");
    }

    #[test]
    fn c2_tiny_positive_budget_yields_marker_prefix() {
        // marker.len (>= 39) >= limit(5) → first 5 chars of the marker only.
        let out = clip_section_on_line_boundary("some long section\n", 5);
        assert_eq!(out, "\n...(");
        assert!(!out.contains("bytes omitted"));
    }

    #[test]
    fn c2_zero_budget_sections_are_omitted_in_multifile() {
        // 30 tiny files but budget 20 < section count → the very first
        // water-fill `share = floor(20/30) == 0` breaks the loop immediately,
        // so every section is allocated 0 budget and clips to "" (omitted, no
        // marker). Concatenation is the empty string. (This is the only way a
        // section reaches 0 budget: the egalitarian water-fill never leaves a
        // *reached* section at 0.) Faithful to Orca's `share === 0` break.
        let files: String = (0..30)
            .map(|i| format!("diff --git a/f{i} b/f{i}\n+{i}\n"))
            .collect();
        assert!(files.contains("\ndiff --git ")); // sanity: it really is multi-section
        let result = truncate_diff_for_prompt_with_budget(&files, 20);
        assert_eq!(result, "");
        assert!(!result.contains("bytes omitted"));
    }

    #[test]
    fn c2_normal_case_full_marker_with_reclip() {
        let section = "keep this line\n".repeat(40);
        let out = clip_section_on_line_boundary(&section, 95);
        assert!(out.chars().count() <= 95);
        assert!(marker_matches(&out));
        assert!(out.ends_with(" bytes omitted)\n"));
    }

    // -- C1 non-ASCII truncation pins (Codex extra; oracle is 100% ASCII).
    //    Char-scalar cut = documented divergence from Orca's UTF-16 units.
    //    The point of these pins is: NEVER PANIC on multibyte / astral cuts. --

    #[test]
    fn c1_korean_diff_truncates_without_panic() {
        let diff = "가나다라마바사아\n".repeat(400); // BMP, multibyte UTF-8
        let result = truncate_diff_for_prompt_with_budget(&diff, 95);
        assert!(result.chars().count() <= 95);
        assert!(marker_matches(&result));
    }

    #[test]
    fn c1_astral_would_split_surrogate_does_not_panic() {
        // Emoji are astral (a surrogate PAIR in UTF-16). A cut landing "inside"
        // a pair would produce a lone surrogate in JS; our char-scalar slice can
        // never split a scalar, so it just never panics. Force a cut boundary
        // that lands right at an emoji.
        let diff = "😀😀😀😀😀😀😀😀\n".repeat(200);
        let result = truncate_diff_for_prompt_with_budget(&diff, 61);
        assert!(result.chars().count() <= 61);
        // A range of tiny budgets — each must be char-boundary-safe (no panic).
        for budget in 1..80 {
            let out = truncate_diff_for_prompt_with_budget(&diff, budget);
            assert!(out.chars().count() <= budget);
        }
    }

    // -- tokenizeCustomCommandTemplate: Orca oracle :250-288 --

    #[test]
    fn tokenize_splits_on_whitespace() {
        assert_eq!(
            tokenize_custom_command_template("claude -p"),
            Ok(vec!["claude".into(), "-p".into()])
        );
    }

    #[test]
    fn tokenize_groups_double_quoted() {
        assert_eq!(
            tokenize_custom_command_template("claude --msg \"hello world\""),
            Ok(vec!["claude".into(), "--msg".into(), "hello world".into()])
        );
    }

    #[test]
    fn tokenize_groups_single_quoted_verbatim() {
        assert_eq!(
            tokenize_custom_command_template("agent --json '{\"k\":\"v\"}'"),
            Ok(vec![
                "agent".into(),
                "--json".into(),
                "{\"k\":\"v\"}".into()
            ])
        );
    }

    #[test]
    fn tokenize_honors_backslash_escapes_in_double_quotes() {
        assert_eq!(
            tokenize_custom_command_template("claude --msg \"she said \\\"hi\\\"\""),
            Ok(vec![
                "claude".into(),
                "--msg".into(),
                "she said \"hi\"".into()
            ])
        );
    }

    #[test]
    fn tokenize_keeps_adjacent_quoted_unquoted_in_one_token() {
        assert_eq!(
            tokenize_custom_command_template("foo a\"b\"c"),
            Ok(vec!["foo".into(), "abc".into()])
        );
    }

    #[test]
    fn tokenize_errors_on_unclosed_quote() {
        let r = tokenize_custom_command_template("claude --msg \"no end");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_lowercase().contains("unclosed"));
    }

    #[test]
    fn tokenize_empty_token_list_for_whitespace_only() {
        assert_eq!(tokenize_custom_command_template("   \t  "), Ok(vec![]));
    }

    // -- planCustomCommand: Orca oracle :290-329 --

    #[test]
    fn plan_routes_prompt_via_stdin_when_no_placeholder() {
        assert_eq!(
            plan_custom_command("claude -p", "COMMIT MSG"),
            CustomCommandPlan::Ok {
                binary: "claude".into(),
                args: vec!["-p".into()],
                stdin_payload: Some("COMMIT MSG".into()),
            }
        );
    }

    #[test]
    fn plan_substitutes_placeholder_as_whole_token() {
        assert_eq!(
            plan_custom_command("codex exec {prompt}", "PROMPT"),
            CustomCommandPlan::Ok {
                binary: "codex".into(),
                args: vec!["exec".into(), "PROMPT".into()],
                stdin_payload: None,
            }
        );
    }

    #[test]
    fn plan_treats_quoted_placeholder_identically() {
        let a = plan_custom_command("codex exec {prompt}", "PROMPT");
        let b = plan_custom_command("codex exec \"{prompt}\"", "PROMPT");
        assert_eq!(a, b);
    }

    #[test]
    fn plan_substitutes_placeholder_embedded_in_token() {
        assert_eq!(
            plan_custom_command("agent --msg={prompt}", "PROMPT"),
            CustomCommandPlan::Ok {
                binary: "agent".into(),
                args: vec!["--msg=PROMPT".into()],
                stdin_payload: None,
            }
        );
    }

    #[test]
    fn plan_errors_on_empty_template() {
        assert!(matches!(
            plan_custom_command("   ", "PROMPT"),
            CustomCommandPlan::Err(_)
        ));
    }

    #[test]
    fn plan_propagates_tokenizer_errors() {
        match plan_custom_command("agent \"unclosed", "PROMPT") {
            CustomCommandPlan::Err(e) => assert!(e.to_lowercase().contains("unclosed")),
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
