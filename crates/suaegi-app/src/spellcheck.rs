//! Native rich-Markdown spellcheck support.

const SPELLCHECK_SCAN_CHARS: usize = 32 * 1024;
const SPELLCHECK_RESULT_LIMIT: usize = 8;

fn markdown_prose(source: &str) -> String {
    let mut prose = String::new();
    let mut fenced = false;
    for line in source.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut inline_code = false;
        for token in line.split_whitespace() {
            if token.starts_with('`') {
                inline_code = !inline_code;
            }
            if !inline_code && !token.starts_with("http://") && !token.starts_with("https://") {
                prose.push_str(token);
                prose.push(' ');
            }
            if token.ends_with('`') && !token.starts_with('`') {
                inline_code = false;
            }
        }
        prose.push('\n');
    }
    prose.chars().take(SPELLCHECK_SCAN_CHARS).collect()
}

#[cfg(target_os = "macos")]
pub fn misspellings(source: &str) -> Vec<String> {
    use objc2_app_kit::NSSpellChecker;
    use objc2_foundation::{NSNotFound, NSString};

    let prose = markdown_prose(source);
    if prose.trim().is_empty() {
        return Vec::new();
    }
    let utf16: Vec<u16> = prose.encode_utf16().collect();
    let native = NSString::from_str(&prose);
    let checker = NSSpellChecker::sharedSpellChecker();
    let mut start = 0usize;
    let mut found = Vec::new();
    while start < utf16.len() && found.len() < SPELLCHECK_RESULT_LIMIT {
        let range = checker.checkSpellingOfString_startingAt(&native, start as isize);
        if range.location == NSNotFound as usize || range.length == 0 {
            break;
        }
        let end = range.location.saturating_add(range.length).min(utf16.len());
        if range.location >= end {
            break;
        }
        let word = String::from_utf16_lossy(&utf16[range.location..end]);
        if !found.iter().any(|existing| existing == &word) {
            found.push(word);
        }
        start = end;
    }
    found
}

#[cfg(not(target_os = "macos"))]
pub fn misspellings(_source: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_filter_skips_fenced_code_inline_code_and_urls() {
        let prose = markdown_prose(
            "Check this prose\n```rust\nmispelled_code()\n```\n`teh_code` https://example.com",
        );
        assert!(prose.contains("Check this prose"));
        assert!(!prose.contains("mispelled_code"));
        assert!(!prose.contains("teh_code"));
        assert!(!prose.contains("example.com"));
    }
}
