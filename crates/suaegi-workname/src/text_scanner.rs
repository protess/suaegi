//! `workspace-name-text-scanner.ts` — the hidden 5th module (129 lines, no test
//! file in Orca). A hand-rolled scalar scanner that `workspace-name.ts` imports.
//!
//! # Why hand-rolled (plan C3 — a CONTRACT, not an optimization)
//! `slugifyForWorkspaceName` must fold whitespace and `compactWords` must tokenize
//! **without** a `/\s+/` regex — the workspace-name oracle *spies* on
//! `String.prototype.replace`/`split` and asserts zero `/\s+/` usages
//! (`workspace-name.test.ts:31-41`, `:235-251`). So both routines scan the
//! ECMAScript whitespace set by hand via [`is_js_whitespace`].
//!
//! # C6 (UTF-16 → scalar) — documented narrow divergence
//! Orca iterates UTF-16 code units (`charCodeAt`, `input[index]`,
//! `input.slice(a,b)`). We iterate Unicode scalars. For astral characters the
//! per-index bookkeeping differs, but the *emitted strings* are identical (an
//! astral char emits as its two surrogates in JS, which reconcatenate to the same
//! text) and non-ASCII is stripped by the later `[^a-z0-9._-]` pass anyway. The
//! scalar port is char-boundary-safe and never panics.

use crate::js_ws::is_js_whitespace;

/// `foldWorkspaceNameWhitespaceToHyphen` (`:1-16`). Collapse each run of
/// whitespace (leading/trailing included) into a single `-`; the caller's later
/// trim cleans up edge hyphens.
pub fn fold_workspace_name_whitespace_to_hyphen(input: &str) -> String {
    let mut result = String::new();
    let mut pending_hyphen = false;
    for ch in input.chars() {
        if is_js_whitespace(ch) {
            pending_hyphen = true;
            continue;
        }
        if pending_hyphen {
            result.push('-');
            pending_hyphen = false;
        }
        result.push(ch);
    }
    result
}

/// `collectCompactWorkspaceWords` (`:18-53`). Tokenize on
/// [`is_compact_workspace_word_separator`], skip whole `http(s)://…` URLs, and
/// collect up to `max_words` non-stopword tokens (stopword test is a full
/// lowercase compare).
pub fn collect_compact_workspace_words(
    input: &str,
    max_words: usize,
    stop_words: &[&str],
) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut words: Vec<String> = Vec::new();
    let mut token_start: Option<usize> = None;
    // Manual index because the URL branch jumps `index` forward (JS `for` loop).
    let mut index = 0usize;
    while index <= len {
        let is_end = index == len;
        if !is_end && starts_with_http_url(&chars, index) {
            finish_compact_workspace_token(
                &chars,
                token_start,
                index,
                &mut words,
                max_words,
                stop_words,
            );
            token_start = None;
            while index < len && !is_js_whitespace(chars[index]) {
                index += 1;
            }
            if words.len() >= max_words {
                break;
            }
            index += 1; // JS `continue` triggers the for-loop increment.
            continue;
        }
        if !is_end && !is_compact_workspace_word_separator(chars[index]) {
            if token_start.is_none() {
                token_start = Some(index);
            }
            index += 1;
            continue;
        }
        if token_start.is_some() {
            finish_compact_workspace_token(
                &chars,
                token_start,
                index,
                &mut words,
                max_words,
                stop_words,
            );
            token_start = None;
            if words.len() >= max_words {
                break;
            }
        }
        index += 1;
    }
    words
}

/// `finishCompactWorkspaceToken` (`:55-71`). Push the `[token_start, token_end)`
/// slice unless it is empty, a stopword, or the cap is already reached.
fn finish_compact_workspace_token(
    chars: &[char],
    token_start: Option<usize>,
    token_end: usize,
    words: &mut Vec<String>,
    max_words: usize,
    stop_words: &[&str],
) {
    let token_start = match token_start {
        Some(ts) if words.len() < max_words => ts,
        _ => return,
    };
    let word: String = chars[token_start..token_end].iter().collect();
    if word.is_empty() {
        return;
    }
    let lower: String = word.chars().flat_map(char::to_lowercase).collect();
    if !stop_words.contains(&lower.as_str()) {
        words.push(word);
    }
}

/// `startsWithHttpUrl` (`:73-78`). ASCII-insensitive `http://` / `https://`.
fn starts_with_http_url(chars: &[char], index: usize) -> bool {
    starts_with_ascii_insensitive(chars, index, "http://")
        || starts_with_ascii_insensitive(chars, index, "https://")
}

/// `startsWithAsciiInsensitive` (`:80-90`). `prefix` is ASCII, so its byte length
/// equals its scalar length.
fn starts_with_ascii_insensitive(chars: &[char], index: usize, prefix: &str) -> bool {
    if index + prefix.len() > chars.len() {
        return false;
    }
    for (offset, pc) in prefix.chars().enumerate() {
        // `toLowerAsciiCode`: only A-Z is folded — exactly `char::to_ascii_lowercase`.
        if chars[index + offset].to_ascii_lowercase() != pc {
            return false;
        }
    }
    true
}

/// `isCompactWorkspaceWordSeparator` (`:96-113`): whitespace plus
/// `" # ( ) - / : [ \ ] _ { }`.
fn is_compact_workspace_word_separator(ch: char) -> bool {
    is_js_whitespace(ch)
        || matches!(
            ch,
            '"' | '#' | '(' | ')' | '/' | ':' | '[' | '\\' | ']' | '_' | '{' | '}' | '-'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOP: &[&str] = &["the", "a"];

    #[test]
    fn fold_collapses_runs_and_edges() {
        assert_eq!(fold_workspace_name_whitespace_to_hyphen("a  b"), "a-b");
        // Leading run emits a hyphen (before `a`); the trailing run sets
        // `pending_hyphen` but emits nothing since no char follows.
        assert_eq!(fold_workspace_name_whitespace_to_hyphen("  a b "), "-a-b");
    }

    // ---- C3 spy-equivalent: prove the hand-roll uses the ECMAScript ws set ----

    /// U+FEFF IS ECMAScript whitespace → folded to a hyphen and treated as a
    /// token separator. U+0085 (NEL) is NOT → kept literally, never a separator.
    /// Reverting to Rust `char::is_whitespace` would flip both.
    #[test]
    fn c3_feff_is_ws_but_nel_is_not() {
        // fold: FEFF becomes a hyphen; NEL is passed through unchanged.
        assert_eq!(
            fold_workspace_name_whitespace_to_hyphen("a\u{FEFF}b"),
            "a-b"
        );
        assert_eq!(
            fold_workspace_name_whitespace_to_hyphen("a\u{0085}b"),
            "a\u{0085}b"
        );
        // tokenizer: FEFF splits words; NEL keeps them as one token.
        assert_eq!(
            collect_compact_workspace_words("one\u{FEFF}two", 5, STOP),
            vec!["one".to_string(), "two".to_string()]
        );
        assert_eq!(
            collect_compact_workspace_words("one\u{0085}two", 5, STOP),
            vec!["one\u{0085}two".to_string()]
        );
    }

    #[test]
    fn collect_skips_urls_and_stopwords() {
        assert_eq!(
            collect_compact_workspace_words("visit https://example.com/x then the fix", 5, STOP),
            vec!["visit".to_string(), "then".to_string(), "fix".to_string()]
        );
    }

    #[test]
    fn collect_caps_at_max_words() {
        assert_eq!(
            collect_compact_workspace_words("one two three four five", 3, STOP),
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
    }
}
