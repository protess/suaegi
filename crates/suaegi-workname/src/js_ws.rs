//! JS-faithful whitespace predicate + trim + `\s` class (plan Codex decision C3).
//!
//! This is a **local copy** of the same 3 items in `suaegi-workref::js_ws`
//! (itself identical to `suaegi-taskquery::js_ws`). workref keeps `js_ws` private
//! (`mod js_ws;`), so rather than widen its public API we copy the ~30-line
//! predicate here — the established pattern across the ≥4 crates that need it.
//!
//! Neither Rust `char::is_whitespace` nor `str::trim` reproduce the ECMAScript
//! whitespace set — they use Unicode `White_Space`, which **diverges** at two
//! codepoints:
//!
//! - **U+FEFF** (ZWNBSP/BOM): ECMAScript whitespace, but NOT Unicode
//!   `White_Space` → Rust would keep it, JS strips/splits on it.
//! - **U+0085** (NEL): Unicode `White_Space`, but NOT ECMAScript whitespace →
//!   Rust would strip/split on it, JS keeps it.
//!
//! The whole name cluster relies on this exact set: the scanner's hardcoded
//! `isWorkspaceNameWhitespace` (`workspace-name-text-scanner.ts:115-129`) lists
//! the *same* codepoints, and every `.trim()` / `\s` site in `workspace-name.ts`
//! uses JS whitespace. Reverting to the Rust built-ins is observable (see the C3
//! spy-equivalent pins in `text_scanner`/`workspace_name`).

/// Regex character-class *body* (no brackets) for the exact ECMAScript
/// whitespace set — the JS `\s` equivalent. Interpolated into ported patterns as
/// `[{WS_CLASS}]`. Kept in lockstep with [`is_js_whitespace`]: includes U+FEFF,
/// excludes U+0085/U+180E.
pub const WS_CLASS: &str = concat!(
    r"\x09\x0A\x0B\x0C\x0D", // Tab, LF, VT, FF, CR
    r"\x20",                 // Space
    r"\x{A0}",               // No-Break Space
    r"\x{1680}",             // Ogham Space Mark
    r"\x{2000}-\x{200A}",    // En Quad .. Hair Space
    r"\x{2028}\x{2029}",     // Line/Paragraph Separator
    r"\x{202F}",             // Narrow No-Break Space
    r"\x{205F}",             // Medium Mathematical Space
    r"\x{3000}",             // Ideographic Space
    r"\x{FEFF}",             // Zero Width No-Break Space (BOM)
);

/// True for the exact ECMAScript whitespace set used by `\s` and
/// `String.prototype.trim`: WhiteSpace (Tab, VT, FF, SP, NBSP, U+FEFF, and the
/// Unicode `Zs` space-separators) + LineTerminator (LF, CR, LS U+2028, PS
/// U+2029). Notably **includes U+FEFF, excludes U+0085 and U+180E**. Identical to
/// the scanner's `isWorkspaceNameWhitespace` code-point list.
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
}
