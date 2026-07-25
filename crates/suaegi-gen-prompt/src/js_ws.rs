//! JS-faithful whitespace predicate + trim (plan Codex decision C5).
//!
//! Orca's gen-prompt helpers lean on the ECMAScript whitespace set in many
//! places: `.trim()`, `/\s/`, `/\s+$/`, and `String.prototype.trim`. Neither
//! Rust `char::is_whitespace` nor `str::trim` reproduce that set — they use the
//! Unicode `White_Space` property, which **diverges** from ECMAScript at two
//! codepoints:
//!
//! - **U+FEFF** (ZWNBSP/BOM): ECMAScript whitespace, but NOT Unicode
//!   `White_Space` → Rust would keep it, JS strips/splits on it.
//! - **U+0085** (NEL): Unicode `White_Space`, but NOT ECMAScript whitespace →
//!   Rust would strip/split on it, JS keeps it.
//!
//! This is the exact same ECMAScript `WhiteSpace + LineTerminator` set used by
//! `suaegi-taskquery::js_ws` (and `suaegi-search`). Copied verbatim (leaf crate,
//! ZERO suaegi-crate deps) so every `\s`/`.trim()` site stays JS-faithful.

/// True for the exact ECMAScript whitespace set used by `\s` and
/// `String.prototype.trim`: WhiteSpace (Tab, VT, FF, SP, NBSP, U+FEFF, and the
/// Unicode `Zs` space-separators) + LineTerminator (LF, CR, LS U+2028, PS
/// U+2029). Notably **includes U+FEFF, excludes U+0085 and U+180E**.
pub fn is_js_whitespace(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0009 | 0x000A | 0x000B | 0x000C | 0x000D // Tab, LF, VT, FF, CR
        | 0x0020 // Space
        | 0x00A0 // No-Break Space
        | 0x1680 // Ogham Space Mark
        | 0x2000
            ..=0x200A // En Quad .. Hair Space
        | 0x2028 | 0x2029 // Line/Paragraph Separator
        | 0x202F // Narrow No-Break Space
        | 0x205F // Medium Mathematical Space
        | 0x3000 // Ideographic Space
        | 0xFEFF // Zero Width No-Break Space (BOM)
    )
}

/// Trim leading/trailing ECMAScript whitespace — the JS `String.prototype.trim`
/// set (see [`is_js_whitespace`]), NOT Rust's `str::trim`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// Trim only TRAILING ECMAScript whitespace — mirrors JS `String.prototype.trimEnd`
/// and the `/\s+$/g` replacement used by `parse_generated_pull_request_fields`
/// (body) and the excerpt truncator. Leading whitespace is preserved.
pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

#[cfg(test)]
mod tests {
    use super::{is_js_whitespace, js_trim, js_trim_end};

    /// U+FEFF (BOM/ZWNBSP): JS treats it as whitespace; Rust `char::is_whitespace`
    /// does NOT. It must count as whitespace to match JS.
    #[test]
    fn feff_is_js_whitespace() {
        assert!(is_js_whitespace('\u{FEFF}'));
        assert!(!char::is_whitespace('\u{FEFF}')); // documents the divergence
        assert_eq!(js_trim("\u{FEFF}abc\u{FEFF}"), "abc");
    }

    /// U+0085 (NEL): JS does NOT treat it as whitespace; Rust `char::is_whitespace`
    /// DOES. It must be preserved to match JS.
    #[test]
    fn nel_is_not_js_whitespace() {
        assert!(!is_js_whitespace('\u{0085}'));
        assert!(char::is_whitespace('\u{0085}')); // documents the divergence
        assert_eq!(js_trim("\u{0085}abc"), "\u{0085}abc");
    }

    #[test]
    fn agrees_on_ordinary_whitespace() {
        assert_eq!(js_trim("  a b \t\n"), "a b");
        assert_eq!(js_trim("\u{00A0}\u{2028}\u{3000}x\u{2000}"), "x");
    }

    #[test]
    fn trim_end_keeps_leading() {
        assert_eq!(js_trim_end("  a  "), "  a");
        assert_eq!(js_trim_end("a\r\n"), "a");
    }
}
