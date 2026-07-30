//! Orca-compatible stable tab titles derived from the first known agent prompt.

use std::sync::OnceLock;

use regex::Regex;
use suaegi_workref::{
    extract_work_identifier, format_identifier_first, strip_work_identifier_echo,
};

pub const GENERATED_TAB_TITLE_MAX_LENGTH: usize = 40;
pub const GENERATED_TAB_TITLE_SOURCE_SCAN_LIMIT: usize = 512;

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        url: Regex::new(r"(?i)https?://\S+").unwrap(),
        markup: Regex::new(r"[`*_~#>\[\]\{\}\(\)]").unwrap(),
        leading_issue: Regex::new(r"(?i)^(?:issue|task|bug|feature|pr)\s*(?:#?[0-9]+)?\s*[:-]\s*")
            .unwrap(),
        unsafe_title: Regex::new(r"[^\p{L}\p{N}\s]").unwrap(),
        filler: [
            r"(?i)^(?:can|could|would)\s+you(?:\s+please)?\s+",
            r"(?i)^please(?:\s+|$)",
            r"(?i)^i\s+(?:want|need)\s+(?:you\s+)?to\s+",
            r"(?i)^help\s+me(?:\s+to)?\s+",
            r"(?i)^help\s+",
            r"(?i)^let'?s\s+",
            r"(?i)^we\s+need\s+to\s+",
            r"(?i)^need\s+to\s+",
        ]
        .map(|pattern| Regex::new(pattern).unwrap()),
    })
}

struct Patterns {
    url: Regex,
    markup: Regex,
    leading_issue: Regex,
    unsafe_title: Regex,
    filler: [Regex; 8],
}

fn capitalize_first(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

fn truncate_at_word_boundary(value: &str, max_length: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_length {
        return value.to_string();
    }
    let sliced: String = chars[..max_length].iter().collect();
    let trimmed = sliced.trim();
    if trimmed.len() < sliced.len() {
        return trimmed.to_string();
    }
    if let Some(space) = trimmed.rfind(' ') {
        if space >= max_length * 55 / 100 {
            return trimmed[..space].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn fold_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn derive_generated_tab_title(prompt: &str) -> Option<String> {
    let prompt_preview: String = prompt
        .chars()
        .take(GENERATED_TAB_TITLE_SOURCE_SCAN_LIMIT)
        .collect();
    let patterns = patterns();
    let without_urls = patterns.url.replace_all(prompt_preview.trim(), " ");
    let without_markup = patterns.markup.replace_all(&without_urls, " ");
    let without_prefix = patterns.leading_issue.replace(&without_markup, "");
    let first_clause = without_prefix
        .split(['.', '!', '?', ';', '\n', '\r', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or_default()
        .trim();
    if first_clause.is_empty() {
        return None;
    }

    let mut candidate = first_clause.to_string();
    for _ in 0..3 {
        let before = candidate.trim().to_string();
        for filler in &patterns.filler {
            candidate = filler.replace(&candidate, "").into_owned();
        }
        candidate = candidate.trim().to_string();
        if candidate == before {
            break;
        }
    }
    candidate = fold_whitespace(&patterns.unsafe_title.replace_all(&candidate, " "));

    if let Some(identifier) = extract_work_identifier(&prompt_preview) {
        let tokens: Vec<&str> = identifier.tokens.iter().map(String::as_str).collect();
        let detail = capitalize_first(&strip_work_identifier_echo(&candidate, &tokens));
        return Some(truncate_at_word_boundary(
            &format_identifier_first(&identifier.label, &detail),
            GENERATED_TAB_TITLE_MAX_LENGTH,
        ));
    }
    if candidate.is_empty() {
        return None;
    }
    Some(truncate_at_word_boundary(
        &capitalize_first(&candidate),
        GENERATED_TAB_TITLE_MAX_LENGTH,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_orca_prompt_title_examples() {
        let cases = [
            (
                "Can you please refactor the auth middleware to use JWT tokens?",
                "Refactor the auth middleware to use JWT",
            ),
            (
                "Please fix `src/auth.ts`!!! https://example.com 🔥 then add tests",
                "Fix src auth",
            ),
            (
                "Please 修正\u{a0}résumé\t検索\u{3000}１２３!!!",
                "修正 résumé 検索 １２３",
            ),
            (
                "Issue #2056: Opt-in generated tab titles for agents",
                "Issue 2056 - Opt in generated tab",
            ),
            (
                "Review this community PR https://github.com/EveryInc/plugin/pull/1094",
                "PR 1094 - Review this community",
            ),
            (
                "fix https://gitlab.com/group/app/-/merge_requests/42 quickly",
                "MR 42 - Fix quickly",
            ),
            (
                "implement ENG-456 login flow",
                "ENG-456 - Implement login flow",
            ),
            (
                "implement SHA-256 hashing in the signer",
                "Implement SHA 256 hashing in the signer",
            ),
        ];
        for (prompt, expected) in cases {
            assert_eq!(
                derive_generated_tab_title(prompt).as_deref(),
                Some(expected)
            );
        }
        assert_eq!(derive_generated_tab_title("please!!!"), None);
    }

    #[test]
    fn scan_and_output_are_bounded() {
        let prompt = format!(
            "Please fix src/auth.ts {}",
            "large pasted text ".repeat(5_000)
        );
        let title = derive_generated_tab_title(&prompt).expect("useful prompt");
        assert!(title.chars().count() <= GENERATED_TAB_TITLE_MAX_LENGTH);
    }
}
