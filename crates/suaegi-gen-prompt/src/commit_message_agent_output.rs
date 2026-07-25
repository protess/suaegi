//! Port of Orca `commit-message-agent-output.ts` (@ v1.4.150-rc.0, 197L).
//!
//! Strips noise around an agent's generated commit message, and excerpts an
//! agent's *failure* output (stdout/stderr) positionally so the operative error
//! reaches the user without per-CLI parsing.
//!
//! # Ported quirks / divergences
//! - **No-regex spy sites (plan C6):** Orca's tests (`commit-message-prompt.test.ts`
//!   :106-125) spy on `String.prototype.replace`/`.match` to FORBID CRLF
//!   normalization via `/\r\n/` and fence extraction via `[\s\S]`. Both are
//!   hand-rolled here by scanning (as they are in Orca).
//! - **JS whitespace/case-fold (plan C5):** `.trim()` and `/\S/` use the
//!   ECMAScript whitespace set — [`crate::js_ws`], NOT Rust `str::trim`.
//! - **Char-scalar vs UTF-16 (plan C1):** Orca measures `.length`/`.slice` in
//!   UTF-16 code units; we measure in Unicode scalars (chars). Identical on the
//!   all-ASCII oracle; a documented divergence on astral input (the excerpt
//!   window/budget cuts). Char-boundary-safe — never panics.

use crate::js_ws::{is_js_whitespace, js_trim, js_trim_end};

/// Strips noise around the agent's output: surrounding whitespace, a single
/// enclosing fenced code block, lone "Generating…"/"Thinking…" preamble lines,
/// and one leading list marker.
pub fn clean_generated_commit_message(raw: &str) -> String {
    // Why: agent output can include very large generated bodies; normalize and
    // unwrap by scanning boundaries instead of building newline-sized arrays.
    let normalized = normalize_generated_commit_message_line_feeds(raw);
    let mut text = js_trim(&normalized).to_string();

    // Why: real commit messages never start with an ellipsis or the word
    // "Generating"/"Thinking" — those leak from CLIs that print a status line.
    if let Some(first_newline) = text.find('\n') {
        let is_preamble = is_preamble_line(&text[..first_newline]);
        if is_preamble {
            text = js_trim(&text[first_newline + 1..]).to_string();
        }
    }

    if let Some(fenced) = find_enclosing_commit_message_fence_body(&text) {
        text = js_trim(fenced).to_string();
    }

    // Why: some CLIs format a one-shot answer as a list item even when the
    // prompt asks for raw text; a Git subject should not carry that marker.
    // Orca: `.replace(/^(\s*)(?:[-*•●]\s+|\d+[.)]\s+)/, '$1').trim()`.
    strip_leading_list_marker(&text)
}

/// Reproduces `/^(generating|thinking)\b/i.test(line) || /^[.…]+$/.test(line.trim())`.
fn is_preamble_line(first_line: &str) -> bool {
    if starts_with_word_ignore_case(first_line, "generating")
        || starts_with_word_ignore_case(first_line, "thinking")
    {
        return true;
    }
    // `/^[.…]+$/` — a line that is one-or-more of '.' (U+002E) or '…' (U+2026).
    let trimmed = js_trim(first_line);
    !trimmed.is_empty() && trimmed.chars().all(|c| c == '.' || c == '\u{2026}')
}

/// True if `s` starts with `word` (ASCII case-insensitive) followed by a JS
/// `\b` word boundary — i.e. the next char is a non-word char or end-of-string.
/// `word` must be lowercase ASCII. Mirrors JS `/^word\b/i` (ASCII `\w`).
fn starts_with_word_ignore_case(s: &str, word: &str) -> bool {
    let sb = s.as_bytes();
    let wb = word.as_bytes();
    if sb.len() < wb.len() {
        return false;
    }
    for i in 0..wb.len() {
        if !sb[i].eq_ignore_ascii_case(&wb[i]) {
            return false;
        }
    }
    match s[wb.len()..].chars().next() {
        None => true,
        Some(c) => !is_ascii_word_char(c),
    }
}

/// JS non-unicode-mode `\w` == `[A-Za-z0-9_]`.
fn is_ascii_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// CRLF -> LF, hand-rolled (`.replace(/\r\n/g)` FORBIDDEN by the oracle spy).
/// Only "\r\n" is converted; a lone "\r" is preserved (Orca :32-51).
fn normalize_generated_commit_message_line_feeds(value: &str) -> String {
    let first = match value.find("\r\n") {
        None => return value.to_string(),
        Some(i) => i,
    };
    let mut normalized = String::with_capacity(value.len());
    normalized.push_str(&value[..first]);
    normalized.push('\n');
    let mut chunk_start = first + 2;
    while let Some(rel) = value[chunk_start..].find("\r\n") {
        let crlf = chunk_start + rel;
        normalized.push_str(&value[chunk_start..crlf]);
        normalized.push('\n');
        chunk_start = crlf + 2;
    }
    normalized.push_str(&value[chunk_start..]);
    normalized
}

/// Detects a single enclosing ```-fenced block and returns its body (fence
/// newlines excluded), hand-rolled (`[\s\S]` match FORBIDDEN by the oracle
/// spy). Mirrors Orca :53-79. charCodeAt is ported as ASCII byte inspection:
/// fence markers/info-chars/`\n` are ASCII, so byte indices agree with UTF-16
/// on any real fence (documented C1 divergence otherwise).
fn find_enclosing_commit_message_fence_body(text: &str) -> Option<&str> {
    if !text.starts_with("```") {
        return None;
    }
    let bytes = text.as_bytes();
    let mut header_end = 3usize;
    while header_end < bytes.len() && bytes[header_end] != 10 {
        if !is_commit_fence_info_character(bytes[header_end]) {
            return None;
        }
        header_end += 1;
    }
    if header_end >= bytes.len() {
        return None;
    }
    let closing_fence_start = bytes.len() - 3;
    if closing_fence_start <= header_end || !text.ends_with("```") {
        return None;
    }
    if bytes[closing_fence_start - 1] != 10 {
        return None;
    }
    let body_start = header_end + 1;
    let body_end = closing_fence_start - 1;
    if body_end < body_start {
        return Some("");
    }
    Some(&text[body_start..body_end])
}

/// Info-tag charset for a fence header: `[0-9A-Za-z]` + `-` + `_` (Orca :81-89).
fn is_commit_fence_info_character(code: u8) -> bool {
    code.is_ascii_alphanumeric() || code == b'-' || code == b'_'
}

/// Removes at most one leading list marker, hand-rolled from
/// `/^(\s*)(?:[-*•●]\s+|\d+[.)]\s+)/` (replace with `$1`), then JS-trims.
fn strip_leading_list_marker(text: &str) -> String {
    let replaced = match match_leading_list_marker(text) {
        Some((ws_end, match_end)) => {
            let mut s = String::with_capacity(text.len());
            s.push_str(&text[..ws_end]); // capture group $1 (leading whitespace)
            s.push_str(&text[match_end..]);
            s
        }
        None => text.to_string(),
    };
    js_trim(&replaced).to_string()
}

/// Returns `(ws_end, match_end)` byte offsets when the list-marker pattern
/// matches at position 0. `ws_end` is the end of the leading `\s*` capture;
/// `match_end` is the end of the whole match (marker + trailing `\s+`).
fn match_leading_list_marker(text: &str) -> Option<(usize, usize)> {
    let ws_end = count_js_whitespace_run(text);
    let rest = &text[ws_end..];

    // Alternative A: `[-*•●]\s+`. Tried first (regex alternation order).
    if let Some(c0) = rest.chars().next() {
        if matches!(c0, '-' | '*' | '\u{2022}' | '\u{25CF}') {
            let after = ws_end + c0.len_utf8();
            let ws2 = count_js_whitespace_run(&text[after..]);
            if ws2 > 0 {
                return Some((ws_end, after + ws2));
            }
        }
    }

    // Alternative B: `\d+[.)]\s+`.
    let digit_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len > 0 {
        let after_digits = ws_end + digit_len;
        if let Some(&punct) = text.as_bytes().get(after_digits) {
            if punct == b'.' || punct == b')' {
                let after_punct = after_digits + 1;
                let ws2 = count_js_whitespace_run(&text[after_punct..]);
                if ws2 > 0 {
                    return Some((ws_end, after_punct + ws2));
                }
            }
        }
    }
    None
}

/// Byte length of the leading run of JS whitespace.
fn count_js_whitespace_run(s: &str) -> usize {
    s.char_indices()
        .find(|(_, c)| !is_js_whitespace(*c))
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

// ---------------------------------------------------------------------------
// ANSI stripping + failure-output excerpting
// ---------------------------------------------------------------------------

const ESC: char = '\u{1B}';
const BEL: u8 = 0x07;

/// Removes ANSI CSI (colors/cursor) and OSC (titles/hyperlinks) sequences.
/// Hand-rolled scanner (policy: no `regex` crate); mirrors Orca's regex
/// `ESC(?:\[[0-?]*[ -/]*[@-~] | \][^BEL ESC \r\n]*(?:BEL|ESC\\))`.
pub fn strip_ansi_control_sequences(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == 0x1B && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'[' => {
                    // CSI: `[` [0x30-0x3F]* [0x20-0x2F]* [0x40-0x7E]
                    let mut j = i + 2;
                    while j < bytes.len() && (0x30..=0x3F).contains(&bytes[j]) {
                        j += 1;
                    }
                    while j < bytes.len() && (0x20..=0x2F).contains(&bytes[j]) {
                        j += 1;
                    }
                    if j < bytes.len() && (0x40..=0x7E).contains(&bytes[j]) {
                        i = j + 1;
                        continue;
                    }
                }
                b']' => {
                    // OSC: `]` [^BEL ESC CR LF]* (BEL | ESC '\')
                    let mut j = i + 2;
                    let mut new_i = None;
                    while j < bytes.len() {
                        let b = bytes[j];
                        if b == BEL {
                            new_i = Some(j + 1);
                            break;
                        }
                        if b == 0x1B {
                            if j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                                new_i = Some(j + 2);
                            }
                            break;
                        }
                        if b == 0x0D || b == 0x0A {
                            break;
                        }
                        j += 1;
                    }
                    if let Some(ni) = new_i {
                        i = ni;
                        continue;
                    }
                }
                _ => {}
            }
        }
        // Not a control sequence: copy the whole UTF-8 char at `i`.
        let ch = value[i..].chars().next().expect("i on char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn strip_ansi_if_present(value: &str) -> String {
    if value.contains(ESC) {
        strip_ansi_control_sequences(value)
    } else {
        value.to_string()
    }
}

// Only the two ends of the output are read, like glancing at the first and
// last lines of a long log.
const FAILURE_EXCERPT_SCAN_WINDOW: usize = 8192;
const FAILURE_EXCERPT_HEAD_LINE_COUNT: usize = 2;
const FAILURE_EXCERPT_HEAD_BUDGET: usize = 100;
const FAILURE_EXCERPT_TAIL_BUDGET: usize = 130;
const FAILURE_EXCERPT_SINGLE_BUDGET: usize = 240;

/// Excerpts an agent's failure output positionally (first lines + last line) so
/// every CLI's real failure text reaches the user without per-CLI parsing.
/// Returns `None` when both streams are blank. Mirrors Orca :125-159.
pub fn excerpt_agent_failure_output(stdout: &str, stderr: &str) -> Option<String> {
    // stderr is where CLIs put diagnostics; stdout is the fallback (and often
    // echoes the prompt, so it never overrides a non-blank stderr).
    let source = if has_non_whitespace(stderr) {
        stderr
    } else {
        stdout
    };
    if !has_non_whitespace(source) {
        return None;
    }

    if source.chars().count() <= FAILURE_EXCERPT_SCAN_WINDOW {
        let lines = collect_excerpt_lines(source, usize::MAX);
        if lines.is_empty() {
            return None;
        }
        if lines.len() <= FAILURE_EXCERPT_HEAD_LINE_COUNT + 1 {
            return Some(truncate_excerpt_part(
                &lines.join(" "),
                FAILURE_EXCERPT_SINGLE_BUDGET,
            ));
        }
        let head = &lines[..FAILURE_EXCERPT_HEAD_LINE_COUNT];
        let tail = lines.last().map(String::as_str);
        return Some(compose_two_end_excerpt(head, tail));
    }

    let head_lines = collect_excerpt_lines(
        &char_slice_head(source, FAILURE_EXCERPT_SCAN_WINDOW),
        FAILURE_EXCERPT_HEAD_LINE_COUNT,
    );
    let tail_line =
        collect_excerpt_lines_from_end(&char_slice_tail(source, FAILURE_EXCERPT_SCAN_WINDOW), 1)
            .into_iter()
            .next();
    if head_lines.is_empty() {
        return tail_line.map(|t| truncate_excerpt_part(&t, FAILURE_EXCERPT_SINGLE_BUDGET));
    }
    Some(compose_two_end_excerpt(&head_lines, tail_line.as_deref()))
}

fn compose_two_end_excerpt(head_lines: &[String], tail_line: Option<&str>) -> String {
    let head_part = truncate_excerpt_part(&head_lines.join(" "), FAILURE_EXCERPT_HEAD_BUDGET);
    // Repeated lines (spinner/retry frames) would otherwise show twice.
    match tail_line {
        None => head_part,
        Some(t) if head_lines.iter().any(|h| h == t) => head_part,
        Some(t) => format!(
            "{head_part} … {}",
            truncate_excerpt_part(t, FAILURE_EXCERPT_TAIL_BUDGET)
        ),
    }
}

/// `value.length > budget ? value.slice(0, budget).trimEnd() + '…' : value`,
/// measured in Unicode scalars (C1 divergence). Char-boundary-safe.
fn truncate_excerpt_part(value: &str, budget: usize) -> String {
    if value.chars().count() > budget {
        let sliced: String = value.chars().take(budget).collect();
        format!("{}\u{2026}", js_trim_end(&sliced))
    } else {
        value.to_string()
    }
}

fn collect_excerpt_lines(text: &str, max: usize) -> Vec<String> {
    let mut collected = Vec::new();
    for line in split_lines(text) {
        if collected.len() >= max {
            break;
        }
        let trimmed = js_trim(&strip_ansi_if_present(line)).to_string();
        if !trimmed.is_empty() {
            collected.push(trimmed);
        }
    }
    collected
}

fn collect_excerpt_lines_from_end(text: &str, max: usize) -> Vec<String> {
    let lines = split_lines(text);
    let mut collected = Vec::new();
    for line in lines.iter().rev() {
        if collected.len() >= max {
            break;
        }
        let trimmed = js_trim(&strip_ansi_if_present(line)).to_string();
        if !trimmed.is_empty() {
            collected.push(trimmed);
        }
    }
    collected
}

/// Splits on `\r\n | \r | \n` (bare `\r` is a boundary too — progress bars
/// redraw with carriage returns). Mirrors JS `.split(/\r\n|\r|\n/)`, including
/// the trailing empty segment after a terminal newline.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&text[start..i]);
                i += if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                lines.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push(&text[start..]);
    lines
}

/// JS `/\S/.test(s)` — has at least one non-(JS-)whitespace char.
fn has_non_whitespace(s: &str) -> bool {
    s.chars().any(|c| !is_js_whitespace(c))
}

/// First `n` chars (scalars) of `s` — mirrors `s.slice(0, n)` on ASCII.
fn char_slice_head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Last `n` chars (scalars) of `s` — mirrors `s.slice(s.length - n)`.
fn char_slice_tail(s: &str, n: usize) -> String {
    let total = s.chars().count();
    let skip = total.saturating_sub(n);
    s.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- clean_generated_commit_message: Orca oracle commit-message-prompt.test.ts:82-137 --

    #[test]
    fn clean_trims_whitespace() {
        assert_eq!(
            clean_generated_commit_message("  feat: hello  \n"),
            "feat: hello"
        );
    }

    #[test]
    fn clean_strips_single_enclosing_fence() {
        assert_eq!(
            clean_generated_commit_message("```\nfeat: hello\n```"),
            "feat: hello"
        );
    }

    #[test]
    fn clean_strips_fence_with_language_tag() {
        assert_eq!(
            clean_generated_commit_message("```text\nfix: bug\n```"),
            "fix: bug"
        );
    }

    #[test]
    fn clean_drops_generating_preamble() {
        assert_eq!(
            clean_generated_commit_message("Generating\u{2026}\nfeat: hello world"),
            "feat: hello world"
        );
    }

    #[test]
    fn clean_normalizes_crlf() {
        assert_eq!(
            clean_generated_commit_message("feat: a\r\nbody line\r\n"),
            "feat: a\nbody line"
        );
    }

    /// Oracle :106-125 — the no-regex spy case: large CRLF+fence input cleaned
    /// by hand-rolled scanning (Orca forbids `/\r\n/` replace and `[\s\S]` match).
    #[test]
    fn clean_large_fenced_crlf_without_regex_wide_normalization() {
        let fence = "```";
        let raw = format!(
            "\r\n{fence}text\r\nfeat: large output\r\n{}{fence}\r\n",
            "body line\r\n".repeat(10_000)
        );
        let result = clean_generated_commit_message(&raw);
        assert!(result.starts_with("feat: large output\nbody line"));
        assert!(result.ends_with("body line"));
        assert!(!result.contains("\r\n"));
    }

    #[test]
    fn clean_strips_leading_list_marker() {
        assert_eq!(
            clean_generated_commit_message("\u{25CF} Add Copilot entry to agent results"),
            "Add Copilot entry to agent results"
        );
        assert_eq!(
            clean_generated_commit_message("1. Add numbered entry"),
            "Add numbered entry"
        );
    }

    #[test]
    fn clean_returns_empty_for_whitespace() {
        assert_eq!(clean_generated_commit_message("   \n\t"), "");
    }

    // -- excerpt_agent_failure_output: Orca oracle commit-message-prompt.test.ts:139-248 --

    fn codex_error_line() -> String {
        "ERROR: {\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.3-codex-spark' model is not supported when using Codex with a ChatGPT account.\"}}".to_string()
    }

    fn codex_stderr() -> String {
        [
            "--------",
            "workdir: C:\\Storage\\Projects\\bagplanner",
            "model: gpt-5.3-codex-spark",
            "reasoning effort: medium",
            "--------",
            "user",
            "You are generating a single git commit message...",
            "hook: SessionStart",
            "hook: SessionStart Completed",
            &codex_error_line(),
        ]
        .join("\n")
    }

    #[test]
    fn excerpt_both_ends_tail_anchored_codex_error() {
        let err = codex_error_line();
        let tail: String = err.chars().take(130).collect();
        let expected = format!(
            "-------- workdir: C:\\Storage\\Projects\\bagplanner … {}\u{2026}",
            js_trim_end(&tail)
        );
        assert_eq!(
            excerpt_agent_failure_output("", &codex_stderr()),
            Some(expected)
        );
    }

    #[test]
    fn excerpt_head_anchored_pi_auth_failure() {
        let pi = [
            "No API key found for github-copilot.",
            "",
            "Use /login to log into a provider via OAuth or API key. See:",
            "  /private/tmp/pi-exit1-repro/node_modules/@earendil-works/pi-coding-agent/docs/providers.md",
            "  /private/tmp/pi-exit1-repro/node_modules/@earendil-works/pi-coding-agent/docs/models.md",
        ]
        .join("\n");
        assert_eq!(
            excerpt_agent_failure_output("", &pi),
            Some("No API key found for github-copilot. Use /login to log into a provider via OAuth or API key. See: … /private/tmp/pi-exit1-repro/node_modules/@earendil-works/pi-coding-agent/docs/models.md".to_string())
        );
    }

    #[test]
    fn excerpt_prefers_stderr_over_echoed_stdout() {
        assert_eq!(
            excerpt_agent_failure_output(
                "You are generating a single git commit message for /secret/repo",
                "No API key found for openai."
            ),
            Some("No API key found for openai.".to_string())
        );
    }

    #[test]
    fn excerpt_falls_back_to_stdout_when_stderr_blank() {
        assert_eq!(
            excerpt_agent_failure_output("Not logged in · Please run /login", " \n"),
            Some("Not logged in · Please run /login".to_string())
        );
    }

    #[test]
    fn excerpt_returns_none_when_both_blank() {
        assert_eq!(excerpt_agent_failure_output("   \n\t", ""), None);
    }

    #[test]
    fn excerpt_joins_up_to_three_lines_without_ellipsis() {
        assert_eq!(
            excerpt_agent_failure_output("", "one\ntwo\nthree\n"),
            Some("one two three".to_string())
        );
    }

    #[test]
    fn excerpt_does_not_parse_json_payloads() {
        assert_eq!(
            excerpt_agent_failure_output("", "401: {\"message\":\"Invalid API key provided\"}"),
            Some("401: {\"message\":\"Invalid API key provided\"}".to_string())
        );
    }

    #[test]
    fn excerpt_strips_ansi_colors_and_osc_titles() {
        let esc = '\u{1B}';
        let bel = '\u{07}';
        let input = format!("{esc}]0;pi{bel}{esc}[91mError: no payment method{esc}[0m\n");
        assert_eq!(
            excerpt_agent_failure_output("", &input),
            Some("Error: no payment method".to_string())
        );
    }

    #[test]
    fn excerpt_treats_bare_cr_as_line_boundary() {
        assert_eq!(
            excerpt_agent_failure_output("", "Fetching 50%\rFetching 100%\rConnection error."),
            Some("Fetching 50% Fetching 100% Connection error.".to_string())
        );
    }

    #[test]
    fn excerpt_handles_crlf() {
        assert_eq!(
            excerpt_agent_failure_output("", "one\r\ntwo\r\n"),
            Some("one two".to_string())
        );
    }

    #[test]
    fn excerpt_collapses_repeated_retry_lines() {
        assert_eq!(
            excerpt_agent_failure_output("", &"Retrying request\u{2026}\n".repeat(10)),
            Some("Retrying request\u{2026} Retrying request\u{2026}".to_string())
        );
    }

    #[test]
    fn excerpt_truncates_overlong_single_line() {
        let line = format!("Error: {}", "m".repeat(300));
        assert_eq!(
            excerpt_agent_failure_output("", &line),
            Some(format!("Error: {}\u{2026}", "m".repeat(233)))
        );
    }

    #[test]
    fn excerpt_reads_head_and_tail_windows_of_oversized_output() {
        let stderr = format!(
            "first line\n{}last: operative error",
            "filler line\n".repeat(3000)
        );
        assert_eq!(
            excerpt_agent_failure_output("", &stderr),
            Some("first line filler line … last: operative error".to_string())
        );
    }

    #[test]
    fn excerpt_bounds_giant_single_line_stream() {
        assert_eq!(
            excerpt_agent_failure_output("", &"x".repeat(20_000)),
            Some(format!("{}\u{2026}", "x".repeat(100)))
        );
    }

    // -- Codex extra pin (C5): ANSI strip is a lone helper; verify directly. --
    #[test]
    fn strip_ansi_removes_csi_and_osc() {
        let esc = '\u{1B}';
        let bel = '\u{07}';
        let input = format!("{esc}]8;;http://x{bel}link{esc}[1mbold{esc}[0m");
        assert_eq!(strip_ansi_control_sequences(&input), "linkbold");
    }
}
