//! Terminal OSC title scan-tail extraction — verbatim port of Orca's
//! `src/shared/osc-title-scan-tail.ts` (@ v1.4.150-rc.0).
//!
//! Carries the trailing, possibly-incomplete OSC title-set sequence
//! (`ESC ] 0;`/`1;`/`2;`) across chunk boundaries so a split title terminator
//! can be reconstructed. Non-title OSC payloads (`133;…`, `7;…`) are dropped.
//!
//! # Byte-safety (the one real trap)
//! The introducer/param/terminator scans all use ASCII needles (`ESC ]`, `;`,
//! `BEL`, `ESC \`, `ESC`), so byte offsets from `rfind`/`find`/`ends_with` never
//! split a multibyte UTF-8 sequence — those slices are safe. Only
//! [`trim_osc_title_scan_tail`] (`value.length > 4096`) risks it: JS slices by
//! UTF-16 unit and tolerates lone surrogates, but a raw Rust byte slice at a
//! non-boundary **panics**. We port with **byte** units (terminal-stream
//! fidelity) and snap every cut to a char boundary so it can never panic on a
//! non-ASCII title. This `> 4096` path has no upstream oracle — see the
//! non-ASCII pin below.

const OSC_TITLE_SCAN_TAIL_LIMIT: usize = 4096;
const OSC_TITLE_PREFIX_LENGTH: usize = 4;

/// `'0' | '1' | '2'` — the OSC codes that set a window/icon title.
fn is_osc_title_code(param: &str) -> bool {
    matches!(param, "0" | "1" | "2")
}

/// Largest char boundary `<= index` (mirrors the unstable `str::floor_char_boundary`).
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Smallest char boundary `>= index` (mirrors the unstable `str::ceil_char_boundary`).
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Extract the trailing OSC title scan-tail from `input`, or `""` when there is
/// no incomplete title candidate. A lone trailing `ESC` is preserved so a split
/// `ESC \` string terminator can be rejoined on the next chunk.
pub fn extract_osc_title_scan_tail(input: &str) -> String {
    match input.rfind("\u{1b}]") {
        Some(last_osc) => {
            let suffix = &input[last_osc..];
            // No BEL and no `ESC \` string terminator => still incomplete.
            if !suffix.contains('\u{07}') && !suffix.contains("\u{1b}\\") {
                extract_incomplete_title_osc_tail(suffix)
            } else if input.ends_with('\u{1b}') {
                "\u{1b}".to_string()
            } else {
                String::new()
            }
        }
        None => {
            if input.ends_with('\u{1b}') {
                "\u{1b}".to_string()
            } else {
                String::new()
            }
        }
    }
}

/// `suffix` starts with the `ESC ]` introducer (2 ASCII bytes). Decide whether
/// its (partial) parameter is a title code and, if so, return the bounded tail.
fn extract_incomplete_title_osc_tail(suffix: &str) -> String {
    // suffix.indexOf(';', 2): search from byte 2 (past `ESC ]`).
    match suffix[2..].find(';') {
        None => {
            let partial_parameter = &suffix[2..];
            // ['', '0', '1', '2'].includes(partialParameter)
            if partial_parameter.is_empty() || is_osc_title_code(partial_parameter) {
                trim_osc_title_scan_tail(suffix)
            } else {
                String::new()
            }
        }
        Some(rel) => {
            let parameter_end = rel + 2;
            let parameter = &suffix[2..parameter_end];
            if is_osc_title_code(parameter) {
                trim_osc_title_scan_tail(suffix)
            } else {
                String::new()
            }
        }
    }
}

/// Bound the retained tail to ~4096 bytes: keep the `ESC ]` introducer prefix
/// plus the newest payload bytes. Char-boundary-snapped so non-ASCII titles
/// never panic.
fn trim_osc_title_scan_tail(value: &str) -> String {
    if value.len() <= OSC_TITLE_SCAN_TAIL_LIMIT {
        return value.to_string();
    }
    // prefix = value.slice(0, min(4, len)); snapped down so we never split a char.
    let prefix_end = floor_char_boundary(value, OSC_TITLE_PREFIX_LENGTH.min(value.len()));
    let prefix = &value[..prefix_end];
    let suffix_budget = OSC_TITLE_SCAN_TAIL_LIMIT.saturating_sub(prefix.len());
    // value.slice(-suffixBudget): last `suffix_budget` bytes, snapped UP so a
    // straddling leading char is dropped rather than split (never panics).
    let start = ceil_char_boundary(value, value.len().saturating_sub(suffix_budget));
    format!("{}{}", prefix, &value[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: osc-title-scan-tail.test.ts

    #[test]
    fn keeps_incomplete_osc_title_candidates_only() {
        assert_eq!(
            extract_osc_title_scan_tail("\u{1b}]0;Codex work"),
            "\u{1b}]0;Codex work"
        );
        assert_eq!(
            extract_osc_title_scan_tail("\u{1b}]2;Codex working\u{1b}"),
            "\u{1b}]2;Codex working\u{1b}"
        );
        assert_eq!(extract_osc_title_scan_tail("\u{1b}]"), "\u{1b}]");
        assert_eq!(extract_osc_title_scan_tail("\u{1b}]1"), "\u{1b}]1");
    }

    #[test]
    fn does_not_carry_non_title_osc_payloads() {
        assert_eq!(extract_osc_title_scan_tail("\u{1b}]133;D;13"), "");
        assert_eq!(extract_osc_title_scan_tail("\u{1b}]7;file://host/tmp"), "");
        assert_eq!(
            extract_osc_title_scan_tail("\u{1b}]133;D;0\u{07}\u{1b}"),
            "\u{1b}"
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// `lastOsc === -1` branch: no OSC introducer at all.
    #[test]
    fn pin_no_introducer_branch() {
        assert_eq!(extract_osc_title_scan_tail("hello"), "");
        assert_eq!(extract_osc_title_scan_tail("hello\u{1b}"), "\u{1b}");
    }

    /// `ESC \` string-terminator path is "complete" — the trailing `\` is not a
    /// lone ESC, so it returns "".
    #[test]
    fn pin_st_terminator_branch() {
        assert_eq!(extract_osc_title_scan_tail("\u{1b}]0;t\u{1b}\\"), "");
    }

    /// The `> 4096` trim path with a NON-ASCII payload must not panic and must
    /// return a byte-safe, bounded tail with the introducer prefix intact.
    ///
    /// Construction forces the naive byte cut to land mid-character: prefix
    /// `ESC ]0;` (4 bytes) + 1364×`가` (3 bytes = 4092) + `a` (1 byte) = 4097
    /// bytes. suffixBudget = 4096 - 4 = 4092, so the cut point is byte
    /// `4097 - 4092 = 5`, which is the *second* byte of the first `가`. A raw
    /// `&value[5..]` would panic; the char-boundary snap advances it to 7.
    #[test]
    fn pin_large_non_ascii_trim_never_panics() {
        let payload = "가".repeat(1364);
        let input = format!("\u{1b}]0;{payload}a");
        assert_eq!(input.len(), 4097);
        // Precondition: the naive byte cut would split a char (=> would panic).
        assert!(!input.is_char_boundary(5));

        let out = extract_osc_title_scan_tail(&input);
        // Introducer prefix preserved.
        assert!(out.starts_with("\u{1b}]0;"));
        // Bounded to <= the limit and still valid UTF-8 (no panic, no split char).
        assert!(out.len() <= OSC_TITLE_SCAN_TAIL_LIMIT);
        // The first `가` (straddling the cut) is dropped; the tail is the rest.
        assert_eq!(out, format!("\u{1b}]0;{}a", "가".repeat(1363)));
    }

    /// Introducer/param scans stay byte-safe with a non-ASCII title payload
    /// under the 4096 limit (returned verbatim).
    #[test]
    fn pin_non_ascii_title_scan_is_byte_safe() {
        assert_eq!(
            extract_osc_title_scan_tail("\u{1b}]0;제목 🚀"),
            "\u{1b}]0;제목 🚀"
        );
    }
}
