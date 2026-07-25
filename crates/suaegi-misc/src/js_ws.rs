//! JS-faithful whitespace predicate + trim.
//!
//! Two of the six ported helpers touch whitespace: `image-data-uri`'s
//! `base64Content.replace(/\s/g, '')` and `stable-pane-id`'s `paneKey.trim()`.
//! Neither Rust `char::is_whitespace` nor `str::trim` reproduce the ECMAScript
//! whitespace set — they use the Unicode `White_Space` property, which
//! **diverges** from ECMAScript at two codepoints:
//!
//! - **U+FEFF** (ZWNBSP/BOM): ECMAScript whitespace, but NOT Unicode
//!   `White_Space` → Rust would keep it, JS strips it.
//! - **U+0085** (NEL): Unicode `White_Space`, but NOT ECMAScript whitespace →
//!   Rust would strip it, JS keeps it.
//!
//! So we re-derive the exact ECMAScript `WhiteSpace + LineTerminator` set here
//! (identical to `suaegi-search::js_trim` and `suaegi-taskquery::js_ws`) and use
//! it at both sites. Reverting either to a Rust built-in is observable — see the
//! FEFF/NEL pins in `image_data_uri` and `stable_pane_id`.

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
        | 0x2000..=0x200A // En Quad .. Hair Space
        | 0x2028 | 0x2029 // Line/Paragraph Separator
        | 0x202F // Narrow No-Break Space
        | 0x205F // Medium Mathematical Space
        | 0x3000 // Ideographic Space
        | 0xFEFF // Zero Width No-Break Space (BOM)
    )
}

/// Trim leading/trailing ECMAScript whitespace — the JS `String.prototype.trim`
/// set (see [`is_js_whitespace`]), NOT Rust's `str::trim`. Used for the
/// `.trim()` in `stable-pane-id`'s `parseLegacyNumericPaneKey`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|ch: char| is_js_whitespace(ch))
}

#[cfg(test)]
mod tests {
    use super::{is_js_whitespace, js_trim};

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
}
