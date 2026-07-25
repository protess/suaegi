//! Port of Orca `shared/terminal-reply-query-extraction.ts` (@ v1.4.150-rc.0).
//!
//! Salvages reply-eliciting query sequences (DSR/CPR, DA1/DA2, DECRQM, CSI
//! queries, OSC 10/11 color probes) from terminal output about to be dropped,
//! so the program that sent them isn't left waiting forever for a reply.
//!
//! This module has NO test file of its own in Orca — only `findCsiFinalByteIndex`
//! is exercised indirectly via `terminal-reply-query-scan.test.ts`. Per the plan
//! (C4) every export is pinned here. Byte-native (`&[u8]`); the 64-byte pending
//! cap is BYTES (deliberate divergence from Orca's 64 UTF-16 code units, C1).

use super::osc_color_reply::{
    find_subslice, parse_terminal_osc_color_query, TerminalOscColorQueryParseResult,
};

/// Max bytes of an incomplete query retained as `pending`. In Orca this is 64
/// UTF-16 code units; here it is 64 BYTES (C1 — byte-native, panic-proof).
pub const HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS: usize = 64;

const ESC: u8 = 0x1b;

/// Queries salvaged from a drop-bound output chunk, bucketed by kind, plus any
/// trailing incomplete sequence to prepend to the next chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedRendererQueryData {
    pub stateless_query_data: Vec<u8>,
    pub stateful_query_data: Vec<u8>,
    pub osc_color_query_data: Vec<u8>,
    pub pending: Vec<u8>,
}

/// Scan `pending + data` for reply-eliciting queries, bucketing each and
/// returning any trailing incomplete sequence as the new `pending`.
pub fn extract_hidden_startup_renderer_query_data(
    data: &[u8],
    pending: &[u8],
) -> ExtractedRendererQueryData {
    let input: Vec<u8> = [pending, data].concat();
    let mut stateless_query_data: Vec<u8> = Vec::new();
    let mut stateful_query_data: Vec<u8> = Vec::new();
    let mut osc_color_query_data: Vec<u8> = Vec::new();
    let mut offset = 0;

    while offset < input.len() {
        let candidate_index = match find_byte(&input, ESC, offset) {
            Some(i) => i,
            None => break,
        };
        // A lone trailing ESC — retain it (necessarily one byte here).
        if candidate_index + 1 >= input.len() {
            return ExtractedRendererQueryData {
                stateless_query_data,
                stateful_query_data,
                osc_color_query_data,
                pending: input[candidate_index..].to_vec(),
            };
        }

        if input[candidate_index..].starts_with(b"\x1b[") {
            match find_csi_final_byte_index(&input, candidate_index + 2) {
                None => {
                    let end = (candidate_index + HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS)
                        .min(input.len());
                    return ExtractedRendererQueryData {
                        stateless_query_data,
                        stateful_query_data,
                        osc_color_query_data,
                        pending: input[candidate_index..end].to_vec(),
                    };
                }
                Some(final_byte_index) => {
                    let sequence = &input[candidate_index..final_byte_index + 1];
                    if is_stateless_renderer_reply_csi_query(sequence) {
                        stateless_query_data.extend_from_slice(sequence);
                    } else if is_stateful_renderer_reply_csi_query(sequence) {
                        stateful_query_data.extend_from_slice(sequence);
                    }
                    offset = final_byte_index + 1;
                    continue;
                }
            }
        }

        if input[candidate_index..].starts_with(b"\x1b]") {
            match parse_terminal_osc_color_query(&input, candidate_index) {
                TerminalOscColorQueryParseResult::Partial => {
                    let end = (candidate_index + HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS)
                        .min(input.len());
                    return ExtractedRendererQueryData {
                        stateless_query_data,
                        stateful_query_data,
                        osc_color_query_data,
                        pending: input[candidate_index..end].to_vec(),
                    };
                }
                TerminalOscColorQueryParseResult::None => {
                    offset = candidate_index + 2;
                    continue;
                }
                TerminalOscColorQueryParseResult::Match { end_index, .. } => {
                    osc_color_query_data.extend_from_slice(&input[candidate_index..end_index]);
                    offset = end_index;
                    continue;
                }
            }
        }

        // unreachable: Orca had an OSC-partial fallback here (source :86), but it
        // can never fire — a lone ESC is handled above, `\x1b]` is handled just
        // above, and `parseTerminalOscColorQuery` only returns Partial for an
        // `\x1b]`-prefixed fragment. So an ESC that is neither of those never
        // yields Partial. Removed (structural cleanup, no behavior change, C5).

        // ESC followed by any other byte: advance past just the ESC.
        offset = candidate_index + 1;
    }

    ExtractedRendererQueryData {
        stateless_query_data,
        stateful_query_data,
        osc_color_query_data,
        pending: Vec::new(),
    }
}

/// True if `data` contains any complete reply-eliciting CSI query (stateless or
/// stateful). An incomplete FIRST CSI (no final byte) short-circuits to `false`.
pub fn contains_csi_renderer_query(data: &[u8]) -> bool {
    let mut offset = find_subslice(data, b"\x1b[", 0);
    while let Some(off) = offset {
        let final_byte_index = match find_csi_final_byte_index(data, off + 2) {
            Some(i) => i,
            None => return false,
        };
        let sequence = &data[off..final_byte_index + 1];
        if is_stateless_renderer_reply_csi_query(sequence)
            || is_stateful_renderer_reply_csi_query(sequence)
        {
            return true;
        }
        offset = find_subslice(data, b"\x1b[", final_byte_index + 1);
    }
    false
}

/// True if `data` contains any complete STATEFUL reply-eliciting CSI query.
pub fn contains_stateful_renderer_query(data: &[u8]) -> bool {
    let mut offset = find_subslice(data, b"\x1b[", 0);
    while let Some(off) = offset {
        let final_byte_index = match find_csi_final_byte_index(data, off + 2) {
            Some(i) => i,
            None => return false,
        };
        let sequence = &data[off..final_byte_index + 1];
        if is_stateful_renderer_reply_csi_query(sequence) {
            return true;
        }
        offset = find_subslice(data, b"\x1b[", final_byte_index + 1);
    }
    false
}

/// Index of the first CSI final byte (0x40..=0x7e) at or after `offset`, or
/// `None`. Non-ASCII bytes (>= 0x80) are never final, matching Orca's
/// per-code-unit scan.
pub fn find_csi_final_byte_index(data: &[u8], offset: usize) -> Option<usize> {
    (offset..data.len()).find(|&index| (0x40..=0x7e).contains(&data[index]))
}

/// A CSI query that elicits a *stateless* reply.
pub fn is_stateless_renderer_reply_csi_query(sequence: &[u8]) -> bool {
    if sequence.ends_with(b"c") {
        return true;
    }
    const LITERALS: [&[u8]; 4] = [b"\x1b[5n", b"\x1b[>q", b"\x1b[14t", b"\x1b[16t"];
    LITERALS.contains(&sequence)
}

/// A CSI query that elicits a *stateful* reply (cursor position / DECRQM).
pub fn is_stateful_renderer_reply_csi_query(sequence: &[u8]) -> bool {
    sequence == b"\x1b[6n".as_slice()
        || (sequence.starts_with(b"\x1b[?") && sequence.ends_with(b"$p"))
}

/// Find `byte` in `hay` at or after index `from`.
fn find_byte(hay: &[u8], byte: u8, from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..].iter().position(|&b| b == byte).map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- C4: findCsiFinalByteIndex boundary bytes ---

    #[test]
    fn c4_find_csi_final_byte_index_boundaries() {
        // 0x3f `?` is below 0x40 → skipped; 0x40 `@` is the first final.
        assert_eq!(find_csi_final_byte_index(b"\x3f\x40", 0), Some(1));
        // 0x40 and 0x7e are inclusive finals.
        assert_eq!(find_csi_final_byte_index(b"\x40", 0), Some(0));
        assert_eq!(find_csi_final_byte_index(b"\x7e", 0), Some(0));
        // 0x3f and 0x7f are OUT of range → no final.
        assert_eq!(find_csi_final_byte_index(b"\x3f\x7f", 0), None);
        // Non-ASCII (multibyte) bytes before the final are skipped.
        assert_eq!(find_csi_final_byte_index("한n".as_bytes(), 0), Some(3));
        // Nothing final.
        assert_eq!(find_csi_final_byte_index(b"123;", 0), None);
    }

    // --- C4: stateless / stateful classifiers ---

    #[test]
    fn c4_is_stateless_literals_and_endswith_c() {
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[5n"));
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[>q"));
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[14t"));
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[16t"));
        // Broad endsWith('c') accepts any CSI ending in 'c' (DA1/2/3).
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[?1;2c"));
        assert!(is_stateless_renderer_reply_csi_query(b"\x1b[c"));
        // Near-miss finals.
        assert!(!is_stateless_renderer_reply_csi_query(b"\x1b[5m"));
        assert!(!is_stateless_renderer_reply_csi_query(b"\x1b[15t"));
    }

    #[test]
    fn c4_is_stateful_cpr_and_private_mode() {
        assert!(is_stateful_renderer_reply_csi_query(b"\x1b[6n"));
        // Private-prefix DECRQM `$p`.
        assert!(is_stateful_renderer_reply_csi_query(b"\x1b[?2026$p"));
        // Non-private `$p` is rejected (must start `\x1b[?`).
        assert!(!is_stateful_renderer_reply_csi_query(b"\x1b[2026$p"));
        // Prefix/suffix near-misses.
        assert!(!is_stateful_renderer_reply_csi_query(b"\x1b[?2026$q"));
        assert!(!is_stateful_renderer_reply_csi_query(b"\x1b[5n"));
    }

    // --- C4: containsCsi / containsStateful ---

    #[test]
    fn c4_contains_csi_finds_after_noise() {
        assert!(contains_csi_renderer_query(b"noise\x1b[5nmore"));
        assert!(contains_csi_renderer_query(b"noise\x1b[6nmore"));
        // Several non-matching complete CSIs before a match.
        assert!(contains_csi_renderer_query(b"\x1b[1m\x1b[0K\x1b[6n"));
        // No query at all.
        assert!(!contains_csi_renderer_query(b"\x1b[1m\x1b[0Kplain"));
    }

    #[test]
    fn c4_contains_csi_incomplete_first_csi_is_immediate_false() {
        // Incomplete first CSI (no final byte) → false even though a later
        // complete query exists after it (Orca returns on the first -1).
        assert!(!contains_csi_renderer_query(b"\x1b[123456789012345678901234567890"));
    }

    #[test]
    fn c4_contains_stateful_distinguishes_from_stateless() {
        assert!(contains_stateful_renderer_query(b"x\x1b[6ny"));
        assert!(contains_stateful_renderer_query(b"x\x1b[?2026$py"));
        // A stateless-only query is not a stateful one.
        assert!(!contains_stateful_renderer_query(b"x\x1b[5ny"));
        assert!(!contains_stateful_renderer_query(b"x\x1b[?1;2cy"));
    }

    // --- C4: extract buckets, order, DCS skip, pending caps ---

    #[test]
    fn c4_extract_all_three_buckets_in_encounter_order() {
        // stateless (DA `c`), stateful (`6n`), OSC 10 color — one input.
        let data = b"a\x1b[?1;2cb\x1b[6nc\x1b]10;?\x1b\\d";
        let got = extract_hidden_startup_renderer_query_data(data, b"");
        assert_eq!(got.stateless_query_data, b"\x1b[?1;2c".to_vec());
        assert_eq!(got.stateful_query_data, b"\x1b[6n".to_vec());
        assert_eq!(got.osc_color_query_data, b"\x1b]10;?\x1b\\".to_vec());
        assert_eq!(got.pending, Vec::<u8>::new());
    }

    #[test]
    fn c4_extract_bucket_local_order_accumulates() {
        // Two stateless queries accumulate in encounter order.
        let data = b"\x1b[5n\x1b[14t";
        let got = extract_hidden_startup_renderer_query_data(data, b"");
        assert_eq!(got.stateless_query_data, b"\x1b[5n\x1b[14t".to_vec());
    }

    #[test]
    fn c4_extract_nonmatching_csi_discarded() {
        let got = extract_hidden_startup_renderer_query_data(b"\x1b[1m\x1b[0K", b"");
        assert_eq!(got, ExtractedRendererQueryData::default());
    }

    #[test]
    fn c4_extract_combined_osc_color() {
        let data = b"\x1b]10;?;?\x1b\\";
        let got = extract_hidden_startup_renderer_query_data(data, b"");
        assert_eq!(got.osc_color_query_data, data.to_vec());
    }

    #[test]
    fn c4_extract_incomplete_csi_retained_and_capped_to_64_bytes() {
        // 100-byte CSI with no final byte → retained, capped at 64 bytes.
        let mut data = b"\x1b[".to_vec();
        data.extend(std::iter::repeat_n(b'1', 98)); // digits, never final
        let got = extract_hidden_startup_renderer_query_data(&data, b"");
        assert_eq!(got.pending.len(), 64);
        assert_eq!(&got.pending[..2], b"\x1b[");
    }

    #[test]
    fn c4_extract_incomplete_osc_retained_and_capped() {
        // OSC prefix split before the terminator → Partial → retained + capped.
        let got = extract_hidden_startup_renderer_query_data(b"\x1b]10;?\x1b", b"");
        assert_eq!(got.pending, b"\x1b]10;?\x1b".to_vec());
    }

    #[test]
    fn c4_extract_lone_trailing_esc_retained() {
        let got = extract_hidden_startup_renderer_query_data(b"abc\x1b", b"");
        assert_eq!(got.pending, b"\x1b".to_vec());
    }

    #[test]
    fn c4_extract_prepends_existing_pending() {
        // Query split across chunks: pending holds the head, data the tail.
        let got = extract_hidden_startup_renderer_query_data(b"6n", b"\x1b[");
        assert_eq!(got.stateful_query_data, b"\x1b[6n".to_vec());
        assert_eq!(got.pending, Vec::<u8>::new());
    }

    #[test]
    fn c4_extract_dcs_is_skipped_asymmetry_vs_scan() {
        // extraction has NO ESC-P branch: `\x1bP…` advances past ESC only, so a
        // DCS `$q` query is NOT salvaged (scan DOES accept `$q`/`+q` — the
        // documented asymmetry).
        let got = extract_hidden_startup_renderer_query_data(b"\x1bP1$q\x1b\\", b"");
        assert_eq!(got, ExtractedRendererQueryData::default());
    }

    #[test]
    fn c4_extract_other_introducer_skipped() {
        // `ESC O` (SS3) is not a query introducer → skipped past the ESC.
        let got = extract_hidden_startup_renderer_query_data(b"\x1bOP", b"");
        assert_eq!(got, ExtractedRendererQueryData::default());
    }

    #[test]
    fn c4_extract_multiple_candidates_after_skipped_sequences() {
        // OSC 'none' (not a color query) is skipped, then a CSI 6n is found.
        let data = b"\x1b]0;title\x07\x1b[6n";
        let got = extract_hidden_startup_renderer_query_data(data, b"");
        // `\x1b]0;` is not a 10/11 prefix → None → advance +2, eventually 6n.
        assert_eq!(got.stateful_query_data, b"\x1b[6n".to_vec());
    }

    // --- C5: the removed :86 branch is unreachable ---

    #[test]
    fn c5_no_esc_prefixed_pair_reaches_the_removed_osc_partial_branch() {
        // Exhaustively: ESC followed by any second byte that is neither `[`
        // nor `]`. None may retain pending via an OSC-partial path — they must
        // either be handled (skipped) or, if second byte forms nothing, leave
        // empty pending. The only way pending is non-empty from a two-byte-plus
        // input starting with such an ESC is the CSI/OSC caps (second byte `[`/
        // `]`), which are excluded here. So pending stays empty.
        for second in 0u8..=255 {
            if second == b'[' || second == b']' {
                continue;
            }
            let data = [ESC, second, b'x'];
            let got = extract_hidden_startup_renderer_query_data(&data, b"");
            assert_eq!(
                got.pending,
                Vec::<u8>::new(),
                "ESC + {second:#x} unexpectedly retained pending"
            );
        }
    }

    // --- C1: non-ASCII payload never panics ---

    #[test]
    fn c1_non_ascii_payload_never_panics() {
        let data = "한\x1b[6n국\x1b]10;?\x1b\\어".as_bytes();
        let got = extract_hidden_startup_renderer_query_data(data, b"");
        assert_eq!(got.stateful_query_data, b"\x1b[6n".to_vec());
        assert_eq!(got.osc_color_query_data, b"\x1b]10;?\x1b\\".to_vec());
    }
}
