//! Regex metacharacter escaping — verbatim port of `escapeRegex`
//! (`src/shared/string-utils.ts:10-12`).

/// The regex metacharacters escaped by Orca's
/// `str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')`. Each occurrence gets a single
/// preceding `\`.
const REGEX_META: &[char] = &[
    '.', '*', '+', '?', '^', '$', '{', '}', '(', ')', '|', '[', ']', '\\',
];

/// Escape every regex metacharacter in `s` so it matches literally when compiled
/// into a regex. Verbatim behavior of `string-utils.ts:10-12`.
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if REGEX_META.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lone metachar is escaped; ordinary chars pass through.
    #[test]
    fn escapes_single_metachar() {
        assert_eq!(escape_regex("a.b"), "a\\.b");
        assert_eq!(escape_regex("abc"), "abc");
    }

    /// Every one of the 14 metacharacters in the class is escaped, and only
    /// those — pins the exact metachar set against `string-utils.ts:10-12`.
    #[test]
    fn escapes_full_metachar_set() {
        assert_eq!(
            escape_regex(".*+?^${}()|[]\\"),
            "\\.\\*\\+\\?\\^\\$\\{\\}\\(\\)\\|\\[\\]\\\\"
        );
        // A mixed real-world query.
        assert_eq!(escape_regex("a+b*c"), "a\\+b\\*c");
        // A non-metachar that regexes DO treat specially elsewhere but Orca does
        // NOT escape (e.g. `-`, `/`, `<`) must pass through untouched.
        assert_eq!(escape_regex("a-b/c<d"), "a-b/c<d");
    }
}
