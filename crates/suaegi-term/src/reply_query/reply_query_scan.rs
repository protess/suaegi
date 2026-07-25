//! Port of Orca `shared/terminal-reply-query-scan.ts` (@ v1.4.150-rc.0).
//!
//! Scans a PTY output stream for reply-eliciting query sequences and records
//! each with its absolute output-byte coordinates (`start_seq`/`end_seq`), so a
//! transport that buffers/hides output can still answer queries. Assembles
//! queries split across contiguous chunks via a pending buffer keyed on output
//! sequence continuity.
//!
//! Byte-native (C1): `chunk_start_seq` and the returned coordinates are absolute
//! BYTE positions (Orca uses UTF-16 code-unit positions — the daemon that owns
//! the counter is deferred, so this port defines the byte-based contract). The
//! 4096-byte pending cap is BYTES. `end_seq` is EXCLUSIVE. DCS is ST-only and,
//! unlike the extraction module, IS accepted here (documented asymmetry).

use super::osc_color_reply::{
    find_subslice, parse_terminal_osc_color_query, TerminalOscColorQueryParseResult,
};
use super::reply_query_extraction::find_csi_final_byte_index;
use regex::bytes::{Regex, RegexBuilder};
use std::sync::LazyLock;

const ESC: u8 = 0x1b;
/// Max bytes of an incomplete query carried to the next chunk (Orca: 4096
/// UTF-16 code units; here 4096 BYTES, C1).
const MAX_PENDING_QUERY_CHARS: usize = 4096;

fn anchored(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .build()
        .expect("static terminal-reply-query-scan regex")
}

static DEVICE_ATTRIBUTES_QUERY_RE: LazyLock<Regex> =
    LazyLock::new(|| anchored(r"\A\x1b\[[?>=]?[0-9;]*c\z"));
static MODE_QUERY_RE: LazyLock<Regex> =
    LazyLock::new(|| anchored(r"\A\x1b\[\??[0-9;]+\$p\z"));

/// A reply-eliciting query located in the stream, with absolute byte
/// coordinates. `end_seq` is EXCLUSIVE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalReplyQuerySequence {
    pub data: Vec<u8>,
    pub start_seq: u64,
    pub end_seq: u64,
}

/// Carry-over state between chunks: a trailing incomplete sequence and the
/// absolute byte position where it began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalReplyQueryScanState {
    pub pending: Vec<u8>,
    pub pending_start_seq: Option<u64>,
}

/// The empty scan state (no pending bytes).
pub const EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE: TerminalReplyQueryScanState =
    TerminalReplyQueryScanState {
        pending: Vec::new(),
        pending_start_seq: None,
    };

/// Result of scanning one chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    pub queries: Vec<TerminalReplyQuerySequence>,
    pub state: TerminalReplyQueryScanState,
}

fn is_reply_eliciting_csi(sequence: &[u8]) -> bool {
    if DEVICE_ATTRIBUTES_QUERY_RE.is_match(sequence) {
        return true;
    }
    if MODE_QUERY_RE.is_match(sequence) {
        return true;
    }
    const LITERALS: [&[u8]; 10] = [
        b"\x1b[5n",
        b"\x1b[6n",
        b"\x1b[?6n",
        b"\x1b[?996n",
        b"\x1b[>q",
        b"\x1b[14t",
        b"\x1b[16t",
        b"\x1b[18t",
        b"\x1b[?u",
        b"\x1b[?2031h",
    ];
    LITERALS.contains(&sequence)
}

fn bounded_pending(input: &[u8], start_index: usize) -> Vec<u8> {
    let end = (start_index + MAX_PENDING_QUERY_CHARS).min(input.len());
    input[start_index..end].to_vec()
}

/// Scan `data` (following `previous`) for reply-eliciting queries. `chunk_start_seq`
/// is the absolute output-byte count immediately before `data`.
pub fn scan_terminal_reply_query_sequences(
    data: &[u8],
    chunk_start_seq: u64,
    previous: &TerminalReplyQueryScanState,
) -> ScanResult {
    let continues_pending = previous
        .pending_start_seq
        .is_some_and(|s| s + previous.pending.len() as u64 == chunk_start_seq);
    let pending: &[u8] = if continues_pending {
        &previous.pending
    } else {
        &[]
    };
    let input: Vec<u8> = [pending, data].concat();
    // Safe: when continues_pending, pending_start_seq + pending.len == chunk_start_seq
    // so chunk_start_seq >= pending.len; otherwise pending is empty.
    let input_start_seq = chunk_start_seq - pending.len() as u64;
    let mut queries: Vec<TerminalReplyQuerySequence> = Vec::new();
    let mut offset = 0;

    while offset < input.len() {
        let candidate_index = match find_byte(&input, ESC, offset) {
            Some(i) => i,
            None => break,
        };
        if candidate_index + 1 >= input.len() {
            let next_pending = bounded_pending(&input, candidate_index);
            return ScanResult {
                queries,
                state: TerminalReplyQueryScanState {
                    pending: next_pending,
                    pending_start_seq: Some(input_start_seq + candidate_index as u64),
                },
            };
        }

        // `end_index == None` mirrors Orca's `-1` sentinel (incomplete → carry).
        let mut end_index: Option<usize> = None;
        let mut matches = false;
        if input[candidate_index..].starts_with(b"\x1b[") {
            end_index = find_csi_final_byte_index(&input, candidate_index + 2);
            if let Some(ei) = end_index {
                matches = is_reply_eliciting_csi(&input[candidate_index..ei + 1]);
            }
        } else if input[candidate_index..].starts_with(b"\x1b]") {
            match parse_terminal_osc_color_query(&input, candidate_index) {
                TerminalOscColorQueryParseResult::Partial => end_index = None,
                TerminalOscColorQueryParseResult::Match { end_index: e, .. } => {
                    end_index = Some(e - 1); // exclusive → inclusive
                    matches = true;
                }
                TerminalOscColorQueryParseResult::None => {
                    end_index = Some(candidate_index + 1);
                }
            }
        } else if input[candidate_index..].starts_with(b"\x1bP") {
            if let Some(terminator_index) = find_subslice(&input, b"\x1b\\", candidate_index + 2) {
                end_index = Some(terminator_index + 1);
                let body = &input[candidate_index + 2..terminator_index];
                matches = body.starts_with(b"$q") || body.starts_with(b"+q");
            }
        } else {
            end_index = Some(candidate_index);
        }

        let ei = match end_index {
            Some(ei) => ei,
            None => {
                let next_pending = bounded_pending(&input, candidate_index);
                return ScanResult {
                    queries,
                    state: TerminalReplyQueryScanState {
                        pending: next_pending,
                        pending_start_seq: Some(input_start_seq + candidate_index as u64),
                    },
                };
            }
        };
        if matches {
            queries.push(TerminalReplyQuerySequence {
                data: input[candidate_index..ei + 1].to_vec(),
                start_seq: input_start_seq + candidate_index as u64,
                end_seq: input_start_seq + ei as u64 + 1,
            });
        }
        offset = ei + 1;
    }

    ScanResult {
        queries,
        state: EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
    }
}

fn find_byte(hay: &[u8], byte: u8, from: usize) -> Option<usize> {
    if from > hay.len() {
        return None;
    }
    hay[from..].iter().position(|&b| b == byte).map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(data: &[u8], start_seq: u64, end_seq: u64) -> TerminalReplyQuerySequence {
        TerminalReplyQuerySequence {
            data: data.to_vec(),
            start_seq,
            end_seq,
        }
    }

    // --- terminal-reply-query-scan.test.ts oracle ---

    #[test]
    fn records_reply_eliciting_queries_with_output_high_water_sequence() {
        let data = b"before\x1b[6nafter\x1b[?2031h";
        let result =
            scan_terminal_reply_query_sequences(data, 100, &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE);
        assert_eq!(
            result.queries,
            vec![
                seq(b"\x1b[6n", 106, 110),
                seq(b"\x1b[?2031h", 115, 123),
            ]
        );
        assert_eq!(result.state, EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE);
    }

    #[test]
    fn assembles_a_query_split_across_contiguous_pty_chunks() {
        let first = scan_terminal_reply_query_sequences(
            b"\x1b[?",
            20,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        let second = scan_terminal_reply_query_sequences(b"2026$p", 23, &first.state);
        assert_eq!(first.queries, Vec::new());
        assert_eq!(second.queries, vec![seq(b"\x1b[?2026$p", 20, 29)]);
    }

    #[test]
    fn drops_a_partial_query_when_output_sequence_continuity_is_lost() {
        let first = scan_terminal_reply_query_sequences(
            b"\x1b[?",
            20,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        // chunk_start_seq 30 != 20 + 3 → continuity lost, pending dropped.
        let second = scan_terminal_reply_query_sequences(b"2026$p", 30, &first.state);
        assert_eq!(second.queries, Vec::new());
    }

    // --- C2: OSC match / DCS ST-only / device-attributes + mode CSI ---

    #[test]
    fn c2_osc_color_query_endseq_is_exclusive() {
        // `\x1b]11;?\x07` len 7 → end_seq exclusive == start + 7.
        let result = scan_terminal_reply_query_sequences(
            b"\x1b]11;?\x07",
            0,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        assert_eq!(result.queries, vec![seq(b"\x1b]11;?\x07", 0, 7)]);
    }

    #[test]
    fn c2_dcs_st_terminated_query_accepted() {
        // DCS `$q`/`+q` bodies terminated by ST are accepted (scan asymmetry).
        let result = scan_terminal_reply_query_sequences(
            b"\x1bP$qm\x1b\\",
            0,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        assert_eq!(result.queries, vec![seq(b"\x1bP$qm\x1b\\", 0, 7)]);
        // A DCS with a non-query body is not recorded.
        let none = scan_terminal_reply_query_sequences(
            b"\x1bP1$rm\x1b\\",
            0,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        assert_eq!(none.queries, Vec::new());
    }

    #[test]
    fn c2_device_attributes_and_mode_queries_match() {
        for q in [&b"\x1b[c"[..], b"\x1b[>0c", b"\x1b[?2026$p", b"\x1b[18t"] {
            let r = scan_terminal_reply_query_sequences(
                q,
                0,
                &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
            );
            assert_eq!(r.queries.len(), 1, "expected match for {q:?}");
            assert_eq!(r.queries[0].data, q.to_vec());
        }
    }

    // --- C6: byte-space continuity across a multibyte chunk split ---

    #[test]
    fn c6_byte_continuity_across_multibyte_chunk_split() {
        // "한" is 3 UTF-8 bytes. First chunk ends mid-nothing but carries an
        // incomplete CSI; second chunk continues in BYTE space.
        let first = scan_terminal_reply_query_sequences(
            "한\x1b[?".as_bytes(),
            0,
            &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE,
        );
        // "한"=3 bytes, then \x1b[? at bytes 3..6. Incomplete → pending starts at 3.
        assert_eq!(first.queries, Vec::new());
        assert_eq!(first.state.pending, b"\x1b[?".to_vec());
        assert_eq!(first.state.pending_start_seq, Some(3));
        // Next chunk starts at byte 6 (== 3 + pending.len 3) → continuity holds.
        let second = scan_terminal_reply_query_sequences(b"2026$p", 6, &first.state);
        assert_eq!(second.queries, vec![seq(b"\x1b[?2026$p", 3, 12)]);
    }

    // --- C1: incomplete CSI capped to 4096 bytes; no panic on non-ASCII ---

    #[test]
    fn c1_incomplete_csi_capped_to_4096_bytes() {
        let mut data = b"\x1b[".to_vec();
        data.extend(std::iter::repeat_n(b'1', 5000)); // never final
        let result =
            scan_terminal_reply_query_sequences(&data, 0, &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE);
        assert_eq!(result.queries, Vec::new());
        assert_eq!(result.state.pending.len(), 4096);
        assert_eq!(result.state.pending_start_seq, Some(0));
    }

    #[test]
    fn c1_non_ascii_payload_never_panics() {
        let data = "한국어\x1b[6n중국어".as_bytes();
        let result =
            scan_terminal_reply_query_sequences(data, 0, &EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE);
        // "한국어" = 9 bytes → \x1b[6n at bytes 9..13.
        assert_eq!(result.queries, vec![seq(b"\x1b[6n", 9, 13)]);
    }
}
