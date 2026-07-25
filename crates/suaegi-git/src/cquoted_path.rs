//! Port of Orca `shared/git-cquoted-path.ts` (@ v1.4.150-rc.0).
//!
//! Decodes git's C-quoted path output (`"..."` with backslash + octal escapes).
//! Git octal-escapes non-ASCII bytes, so an adjacent `\NNN` run is a raw UTF-8
//! byte sequence that must be decoded as a whole, not per-byte.
//!
//! Byte-native: every escape-relevant byte is ASCII, so a byte-index scan over
//! `&str` bytes is panic-free and equivalent to Orca's UTF-16 scan. Bytes are
//! accumulated into one buffer and decoded once via lossy UTF-8 — equivalent to
//! Orca decoding each octal run separately (a literal char's first byte is never
//! a UTF-8 continuation byte, so runs never merge across a literal boundary).

fn is_octal_digit(b: u8) -> bool {
    (b'0'..=b'7').contains(&b)
}

/// Decode a git C-quoted path. Non-quoted input is returned unchanged. The
/// result may be lossy (invalid UTF-8 byte sequences become U+FFFD), matching
/// Orca's `TextDecoder('utf-8')`.
pub fn decode_git_cquoted_path(value: &str) -> String {
    let b = value.as_bytes();
    let n = b.len();
    if n < 2 || b[0] != b'"' || b[n - 1] != b'"' {
        return value.to_string();
    }

    let mut decoded: Vec<u8> = Vec::new();
    let mut index = 1;
    while index < n - 1 {
        let ch = b[index];
        if ch != b'\\' {
            decoded.push(ch);
            index += 1;
            continue;
        }

        index += 1; // consume the backslash; `index <= n-1` guaranteed
        let escaped = b[index];
        match escaped {
            b'a' => decoded.push(0x07),
            b'b' => decoded.push(0x08),
            b'f' => decoded.push(0x0c),
            b'n' => decoded.push(0x0a),
            b'r' => decoded.push(0x0d),
            b't' => decoded.push(0x09),
            b'v' => decoded.push(0x0b),
            b'\\' | b'"' => decoded.push(escaped),
            _ if is_octal_digit(escaped) => {
                // Accumulate the contiguous run of `\NNN` escapes into `decoded`.
                let mut octal_start = index;
                while octal_start < n - 1 {
                    // Read up to 3 octal digits starting at octal_start.
                    let mut octal_end = octal_start;
                    let mut count = 1;
                    while octal_end + 1 < n - 1 && count < 3 && is_octal_digit(b[octal_end + 1]) {
                        octal_end += 1;
                        count += 1;
                    }
                    // b[octal_start..=octal_end] are ASCII octal digits.
                    let octal_str =
                        std::str::from_utf8(&b[octal_start..=octal_end]).unwrap_or("0");
                    // E1: Orca accepts \000-\777 (up to 511) and narrows mod 256
                    // via Uint8Array. `u8::from_str_radix` would fail on \400-\777.
                    let parsed = u16::from_str_radix(octal_str, 8).unwrap_or(0);
                    decoded.push((parsed & 0xff) as u8);
                    index = octal_end;
                    // Continue only across an immediately adjacent `\NNN`.
                    if b.get(index + 1) != Some(&b'\\')
                        || !b.get(index + 2).is_some_and(|&c| is_octal_digit(c))
                    {
                        break;
                    }
                    octal_start = index + 2;
                }
            }
            _ => decoded.push(escaped), // unknown escape: drop backslash, keep char
        }
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- git-cquoted-path.test.ts oracle ---

    #[test]
    fn decodes_a_leading_utf8_bom_octal_run() {
        // \357\273\277 = EF BB BF = UTF-8 BOM (preserved, not stripped).
        assert_eq!(decode_git_cquoted_path(r#""\357\273\277name""#), "\u{feff}name");
    }

    #[test]
    fn decodes_adjacent_multibyte_octal_run_as_one_utf8_sequence() {
        // EF BB BF + E3 81 82 (U+3042 あ).
        assert_eq!(
            decode_git_cquoted_path(r#""\357\273\277\343\201\202""#),
            "\u{feff}\u{3042}"
        );
    }

    // --- non-quoted passthrough + single-char escapes ---

    #[test]
    fn returns_non_quoted_input_unchanged() {
        assert_eq!(decode_git_cquoted_path("plain/path"), "plain/path");
        assert_eq!(decode_git_cquoted_path(""), "");
        assert_eq!(decode_git_cquoted_path("\""), "\""); // single quote, len 1 < 2
    }

    #[test]
    fn decodes_single_char_c_escapes() {
        assert_eq!(decode_git_cquoted_path(r#""a\tb\nc""#), "a\tb\nc");
        assert_eq!(decode_git_cquoted_path(r#""quote\"back\\slash""#), "quote\"back\\slash");
        assert_eq!(decode_git_cquoted_path(r#""\a\b\f\r\v""#), "\u{7}\u{8}\u{c}\r\u{b}");
    }

    #[test]
    fn unknown_escape_drops_backslash_keeps_char() {
        assert_eq!(decode_git_cquoted_path(r#""a\zb""#), "azb");
    }

    // --- E1: octal domain 0-511 narrows mod 256 ---

    #[test]
    fn e1_octal_777_narrows_to_0xff_then_lossy() {
        // \777 = 511 -> &0xff = 0xFF (a lone invalid UTF-8 byte) -> U+FFFD.
        assert_eq!(decode_git_cquoted_path(r#""\777""#), "\u{fffd}");
    }

    #[test]
    fn e1_octal_ascii_byte() {
        // \101 = 65 = 'A'.
        assert_eq!(decode_git_cquoted_path(r#""\101""#), "A");
    }

    #[test]
    fn one_and_two_digit_octal() {
        // \0 = NUL, \11 = tab (0x09).
        assert_eq!(decode_git_cquoted_path(r#""\11""#), "\t");
        assert_eq!(decode_git_cquoted_path(r#""a\0b""#), "a\u{0}b");
    }

    #[test]
    fn non_ascii_literal_content_never_panics() {
        // A literal (already-decoded) multibyte char between quotes passes through.
        assert_eq!(decode_git_cquoted_path("\"한\""), "한");
    }
}
