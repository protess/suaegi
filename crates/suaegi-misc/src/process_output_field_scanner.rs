//! Whitespace-bounded process output line/field scanning — verbatim port of
//! Orca's `src/shared/process-output-field-scanner.ts` (@ v1.4.150-rc.0).
//!
//! Why: process output can be multi-MB; callers should not materialize every
//! line up front, and command output can include noisy paste-sized rows —
//! port scanners only need early fields.

use crate::js_ws::is_js_whitespace;

/// Scan cap for [`get_process_output_fields`], counted in **UTF-16 code
/// units** (JS `.length` semantics) — see [`utf16_scan_limit_byte_offset`].
pub const PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS: usize = 4096;

/// Largest byte offset `<= s.len()` such that the UTF-16-code-unit count of
/// `s[..offset]` does not exceed `max_utf16_units`, snapped down to a char
/// boundary (D1). JS `.length`/`.slice` operate on UTF-16 code units, not
/// Unicode scalars, so a naive `min(s.len(), N)` byte cut can split a
/// multi-byte UTF-8 character and panic. Walking `char_indices()` while
/// accumulating `ch.len_utf16()` reproduces the JS cap exactly and can never
/// land mid-character. For ASCII input (all 4 oracles) this is identical to
/// `min(s.len(), max_utf16_units)`.
fn utf16_scan_limit_byte_offset(s: &str, max_utf16_units: usize) -> usize {
    let mut units = 0usize;
    for (byte_offset, ch) in s.char_indices() {
        let next_units = units + ch.len_utf16();
        if next_units > max_utf16_units {
            return byte_offset;
        }
        units = next_units;
    }
    s.len()
}

/// Lazily split `output` into lines on LF, CRLF, or lone CR, with **no
/// trailing synthetic empty line** (D2 — mirrors the `lineStart <
/// output.length` guard in `iterateProcessOutputLines`). `str::lines()` is
/// deliberately not used: it does not treat a lone `\r` as a line
/// terminator, which the oracle (`alpha\nbeta\r\ngamma\rdelta\n` → 4 lines)
/// requires.
pub fn iterate_process_output_lines(output: &str) -> ProcessOutputLines<'_> {
    ProcessOutputLines {
        output,
        pos: 0,
        line_start: 0,
        done: false,
    }
}

/// Iterator returned by [`iterate_process_output_lines`]. Lazy: each `next()`
/// call scans forward from where the previous call left off; no upfront
/// split-array is ever built.
pub struct ProcessOutputLines<'a> {
    output: &'a str,
    pos: usize,
    line_start: usize,
    done: bool,
}

impl<'a> Iterator for ProcessOutputLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.done {
            return None;
        }
        let bytes = self.output.as_bytes();
        let mut index = self.pos;
        while index < bytes.len() {
            let b = bytes[index];
            if b != b'\n' && b != b'\r' {
                index += 1;
                continue;
            }
            let line = &self.output[self.line_start..index];
            let mut advance_to = index + 1;
            if b == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                advance_to += 1;
            }
            self.line_start = advance_to;
            self.pos = advance_to;
            return Some(line);
        }
        // Final-chunk guard (D2): only emit a trailing line if there is
        // unconsumed content left, never a phantom empty final line.
        self.done = true;
        if self.line_start < self.output.len() {
            Some(&self.output[self.line_start..])
        } else {
            None
        }
    }
}

/// Extract up to `max_fields` whitespace-separated fields from the start of
/// `line`, scanning at most [`PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS`] UTF-16
/// units (D1). `max_fields <= 0` returns an empty list (D4).
pub fn get_process_output_fields(line: &str, max_fields: i64) -> Vec<&str> {
    if max_fields <= 0 {
        return Vec::new();
    }

    let scan_limit = utf16_scan_limit_byte_offset(line, PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS);
    let mut fields: Vec<&str> = Vec::new();
    let mut token_start: Option<usize> = None;

    // D4: mirrors the JS `for (index = 0; index <= scanLimit; index += 1)`
    // sentinel loop — one extra "isEnd" step at `scan_limit` flushes a token
    // that ends exactly at the scan boundary instead of losing it.
    let mut steps: Vec<(usize, Option<char>)> = line
        .char_indices()
        .take_while(|&(i, _)| i < scan_limit)
        .map(|(i, ch)| (i, Some(ch)))
        .collect();
    steps.push((scan_limit, None));

    for (idx, ch_opt) in steps {
        if let Some(ch) = ch_opt {
            if !is_js_whitespace(ch) {
                if token_start.is_none() {
                    token_start = Some(idx);
                }
                continue;
            }
        }

        let Some(start) = token_start else {
            continue;
        };
        fields.push(&line[start..idx]);
        token_start = None;
        if fields.len() as i64 >= max_fields {
            break;
        }
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: process-output-field-scanner.test.ts

    #[test]
    fn walks_lf_crlf_and_cr_lines_without_trailing_synthetic_line() {
        let lines: Vec<&str> =
            iterate_process_output_lines("alpha\nbeta\r\ngamma\rdelta\n").collect();
        assert_eq!(lines, vec!["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn returns_bounded_whitespace_separated_fields() {
        let fields =
            get_process_output_fields("  TCP\t127.0.0.1:3000   0.0.0.0:0 LISTENING 4242 extra", 5);
        assert_eq!(
            fields,
            vec!["TCP", "127.0.0.1:3000", "0.0.0.0:0", "LISTENING", "4242"]
        );
    }

    #[test]
    fn caps_scan_work_for_oversized_rows() {
        let row = "x".repeat(PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS + 100);
        let fields = get_process_output_fields(&row, 2);
        assert_eq!(
            fields,
            vec!["x".repeat(PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS)]
        );
    }

    #[test]
    fn keeps_a_field_that_ends_exactly_at_the_scan_boundary() {
        let boundary_field = "x".repeat(PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS);
        let fields = get_process_output_fields(&boundary_field, 1);
        assert_eq!(fields, vec![boundary_field.as_str()]);
    }

    // Extra pins (oracle-silent), per plan:

    /// D1 crux pin: a non-ASCII line whose UTF-16 cap boundary falls
    /// mid-character at the byte level must not panic, and the returned
    /// field must be snapped down to the char boundary (dropping the
    /// straddling character rather than splitting it).
    #[test]
    fn pin_non_ascii_cap_boundary_does_not_panic() {
        // 2047 * 2 = 4094 UTF-16 units; + 2 more BMP chars = 4096 exactly on
        // a char boundary, so append one more astral char (2 UTF-16 units)
        // that would straddle byte cap if counted naively.
        let mut line = "가".repeat(2048); // 2048 * 1 UTF-16 unit each = 2048 units, 3 bytes/char
        line.push_str(&"a".repeat(2048)); // + 2048 ascii units = 4096 units exactly
        line.push('🚀'); // astral char, 2 UTF-16 units, 4 bytes — pushes past cap
        let fields = get_process_output_fields(&line, 1);
        assert_eq!(fields.len(), 1);
        // The field must be valid UTF-8 (no panic) and capped at exactly the
        // 4096-UTF-16-unit prefix (the trailing astral char is excluded).
        let expected: String = "가".repeat(2048) + &"a".repeat(2048);
        assert_eq!(fields[0], expected.as_str());
    }

    /// D2: empty input yields no lines at all.
    #[test]
    fn pin_empty_input_yields_no_lines() {
        let lines: Vec<&str> = iterate_process_output_lines("").collect();
        assert_eq!(lines, Vec::<&str>::new());
    }

    /// D2: a trailing terminator with nothing after it must not emit a
    /// phantom empty final line.
    #[test]
    fn pin_trailing_terminator_no_phantom_empty_line() {
        let lines: Vec<&str> = iterate_process_output_lines("alpha\n").collect();
        assert_eq!(lines, vec!["alpha"]);
    }

    /// D2: a lone CR with no following LF and no trailing content.
    #[test]
    fn pin_lone_trailing_cr() {
        let lines: Vec<&str> = iterate_process_output_lines("alpha\rbeta").collect();
        assert_eq!(lines, vec!["alpha", "beta"]);
    }

    /// D4: `max_fields = 0` returns an empty list.
    #[test]
    fn pin_max_fields_zero_returns_empty() {
        assert_eq!(
            get_process_output_fields("alpha beta", 0),
            Vec::<&str>::new()
        );
    }

    /// D4: a negative `max_fields` also returns an empty list.
    #[test]
    fn pin_max_fields_negative_returns_empty() {
        assert_eq!(
            get_process_output_fields("alpha beta", -1),
            Vec::<&str>::new()
        );
    }

    /// D3 crux pin: U+FEFF (ZERO WIDTH NO-BREAK SPACE) IS ECMAScript
    /// whitespace, so it acts as leading whitespace/a field separator. Why:
    /// Rust `char::is_whitespace()` returns `false` for U+FEFF, so a
    /// regression to the Rust predicate would fold it into the first field
    /// instead of treating it as a separator — the existing NBSP (U+00A0)
    /// coverage does not discriminate here because Rust also treats U+00A0
    /// as whitespace.
    #[test]
    fn pin_js_whitespace_set_feff_is_whitespace() {
        assert_eq!(
            get_process_output_fields("\u{feff}alpha beta", 5),
            vec!["alpha", "beta"]
        );
    }

    /// D3 crux pin: U+0085 (NEXT LINE) is NOT ECMAScript whitespace, so it
    /// does not split a field in two. Why: Rust `char::is_whitespace()`
    /// returns `true` for U+0085, so a regression to the Rust predicate
    /// would incorrectly split `"a\u{85}b"` into two fields instead of one.
    #[test]
    fn pin_js_whitespace_set_u0085_is_not_whitespace() {
        assert_eq!(get_process_output_fields("a\u{85}b", 5), vec!["a\u{85}b"]);
    }
}
