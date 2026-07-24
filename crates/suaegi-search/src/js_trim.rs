//! JS-faithful string trim.
//!
//! Rust's [`str::trim`] strips the Unicode `White_Space` property, which
//! **diverges** from ECMAScript `String.prototype.trim` at two codepoints:
//! U+FEFF (JS strips, Rust keeps) and U+0085/NEL (Rust strips, JS keeps). Every
//! `.trim()` in Orca's `text-search.ts` is a JS trim (e.g. the glob-fragment
//! trimming in `splitSearchGlobPatterns`), so we replicate its exact whitespace
//! set — ECMAScript WhiteSpace + LineTerminator, which **includes U+FEFF** and
//! **excludes U+0085**. Use this wherever the JS source calls `.trim()`, never
//! bare `str::trim()`. (Same correction class as `suaegi-automation::cron::js_trim`.)

/// True for the exact ECMAScript trim whitespace set: WhiteSpace (Tab, VT, FF,
/// SP, NBSP, U+FEFF, and the Unicode `Zs` space-separators) + LineTerminator
/// (LF, CR, LS U+2028, PS U+2029). Notably **includes U+FEFF, excludes U+0085**.
fn is_ecmascript_whitespace(code: u32) -> bool {
    matches!(
        code,
        0x0009 | 0x000A | 0x000B | 0x000C | 0x000D // Tab, LF, VT, FF, CR
        | 0x0020 // Space
        | 0x00A0 // No-Break Space
        | 0x1680 // Ogham Space Mark
        | 0x2000..=0x200A // En Quad .. Hair Space
        | 0x2028 | 0x2029 // Line/Paragraph Separator
        | 0x202F // Narrow No-Break Space
        | 0x205F // Medium Mathematical Space
        | 0x3000 // Ideographic Space
        | 0xFEFF // Zero Width No-Break Space (BOM)
    )
}

/// Trim leading/trailing ECMAScript whitespace — the JS `String.prototype.trim`
/// set, not Rust's `White_Space` property. See module docs for why this matters.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|ch: char| is_ecmascript_whitespace(ch as u32))
}

#[cfg(test)]
mod tests {
    use super::js_trim;

    /// U+FEFF (BOM/ZWNBSP): JS `trim` strips it; Rust `str::trim()` does NOT.
    /// A glob fragment delimited only by a BOM must trim to empty like JS.
    #[test]
    fn strips_bom_like_js() {
        assert_eq!(js_trim("\u{FEFF}abc\u{FEFF}"), "abc");
        assert_eq!(js_trim("\u{FEFF}"), "");
    }

    /// U+0085 (NEL): ECMAScript `trim` does NOT strip it; Rust `str::trim()`
    /// WOULD. It must be preserved to match JS.
    #[test]
    fn preserves_nel_like_js() {
        assert_eq!(js_trim("\u{0085}abc"), "\u{0085}abc");
        assert_eq!(js_trim("\u{0085}"), "\u{0085}");
    }

    /// Ordinary ASCII/Unicode spaces trim identically to `str::trim()`.
    #[test]
    fn agrees_on_ordinary_whitespace() {
        assert_eq!(js_trim("  a b \t\n"), "a b");
        assert_eq!(js_trim("\u{00A0}\u{2028}\u{3000}x\u{2000}"), "x");
    }
}
