//! Port of Orca `shared/terminal-partial-escape-tail.ts` (@ v1.4.150-rc.0).
//!
//! A PTY read can end mid-escape-sequence; those bytes sit in xterm's parser
//! state, not the screen buffer, so a serialized snapshot cannot carry them.
//! Tracking the unparsed trailing partial sequence at the ingest boundary lets
//! snapshot producers append it so the continuation completes exactly as live.
//!
//! Byte-native: this mirrors the VT500 parser states, and every significant
//! byte it inspects is ASCII (ESC/CAN/SUB/BEL and the 0x20–0x7e ranges). UTF-8
//! is self-synchronizing, so a `&[u8]` scan matches the JS UTF-16 scan's end
//! state and returned tail exactly, and slicing is panic-free.

const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const BEL: u8 = 0x07;

/// Cap on the tracked tail. OSC/DCS payloads are unbounded; beyond this we stop
/// tracking and degrade to pre-fix behavior for that pathological stream.
pub const MAX_PARTIAL_ESCAPE_TAIL_LENGTH: usize = 4096;

/// The VT500 parser states that can span a chunk boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScanState {
    Ground,
    Esc,
    EscIntermediate,
    Csi,
    Osc,
    OscEsc,
    StringSeq,
    StringEsc,
}

/// ESC-state transition shared by the fresh-ESC and abort-reprocess paths.
fn state_after_esc_byte(code: u8) -> ScanState {
    if code == 0x5b {
        return ScanState::Csi; // [
    }
    if code == 0x5d {
        return ScanState::Osc; // ]
    }
    // P / X / ^ / _ open DCS / SOS / PM / APC — ST-terminated strings.
    if code == 0x50 || code == 0x58 || code == 0x5e || code == 0x5f {
        return ScanState::StringSeq;
    }
    if (0x20..=0x2f).contains(&code) {
        return ScanState::EscIntermediate;
    }
    if code < 0x20 || code == 0x7f {
        return ScanState::Esc; // C0 executes / DEL ignored mid-sequence
    }
    ScanState::Ground // final byte — two-byte sequence (ESC 7, ESC c, …)
}

/// Return the trailing incomplete escape sequence of `stream` (`&[]` when the
/// stream ends parser-clean). Fold-safe across chunk boundaries:
/// `extract(a + b) == extract(extract(a) + b)`.
pub fn extract_partial_escape_tail(stream: &[u8]) -> &[u8] {
    let mut state = ScanState::Ground;
    let mut start = 0;
    for (i, &code) in stream.iter().enumerate() {
        if state == ScanState::Ground {
            if code == ESC {
                start = i;
                state = ScanState::Esc;
            }
            continue;
        }
        if code == ESC
            && state != ScanState::Osc
            && state != ScanState::StringSeq
            && state != ScanState::OscEsc
            && state != ScanState::StringEsc
        {
            // ESC aborts a pending ESC/CSI sequence and starts a new one.
            start = i;
            state = ScanState::Esc;
            continue;
        }
        // CAN/SUB abort esc/escIntermediate back to ground (csi/osc/string
        // handle it inline in their arms below).
        if (code == CAN || code == SUB)
            && (state == ScanState::Esc || state == ScanState::EscIntermediate)
        {
            state = ScanState::Ground;
            continue;
        }
        match state {
            ScanState::Esc => state = state_after_esc_byte(code),
            ScanState::EscIntermediate => {
                if (0x30..=0x7e).contains(&code) {
                    state = ScanState::Ground;
                }
                // 0x20–0x2f stays; other C0 executes and stays.
            }
            ScanState::Csi => {
                if code == CAN || code == SUB {
                    state = ScanState::Ground;
                } else if (0x40..=0x7e).contains(&code) {
                    state = ScanState::Ground; // final byte completes the CSI
                }
                // params/intermediates (0x20–0x3f), C0, DEL stay in-sequence.
            }
            ScanState::Osc => {
                if code == BEL || code == CAN || code == SUB {
                    state = ScanState::Ground;
                } else if code == ESC {
                    state = ScanState::OscEsc;
                }
            }
            ScanState::OscEsc => {
                if code == 0x5c {
                    state = ScanState::Ground; // ESC \ = ST terminates the OSC
                } else {
                    // The ESC aborted the OSC and opened a new sequence at i-1.
                    start = i - 1;
                    state = if code == ESC {
                        ScanState::Esc
                    } else {
                        state_after_esc_byte(code)
                    };
                }
            }
            ScanState::StringSeq => {
                if code == CAN || code == SUB {
                    state = ScanState::Ground;
                } else if code == ESC {
                    state = ScanState::StringEsc;
                }
            }
            ScanState::StringEsc => {
                if code == 0x5c {
                    state = ScanState::Ground;
                } else {
                    start = i - 1;
                    state = if code == ESC {
                        ScanState::Esc
                    } else {
                        state_after_esc_byte(code)
                    };
                }
            }
            ScanState::Ground => unreachable!("ground handled before the match"),
        }
    }
    if state == ScanState::Ground {
        &[]
    } else {
        &stream[start..]
    }
}

/// Ingest-time fold: advance the tracked tail with one more chunk. Returns `&[]`
/// (tracking abandoned) when the tail exceeds the cap.
pub fn advance_partial_escape_tail(pending_tail: &[u8], chunk: &[u8]) -> Vec<u8> {
    let combined: Vec<u8> = [pending_tail, chunk].concat();
    let tail = extract_partial_escape_tail(&combined);
    if tail.len() > MAX_PARTIAL_ESCAPE_TAIL_LENGTH {
        Vec::new()
    } else {
        tail.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract(s: &[u8]) -> &[u8] {
        extract_partial_escape_tail(s)
    }

    #[test]
    fn returns_empty_for_parser_clean_streams() {
        assert_eq!(extract(b""), b"");
        assert_eq!(extract(b"plain text no escapes"), b"");
        assert_eq!(extract(b"\x1b[38;5;196mred\x1b[0m done"), b"");
        assert_eq!(extract(b"\x1b[2J\x1b[H"), b"");
    }

    #[test]
    fn returns_the_dangling_csi_when_a_chunk_ends_mid_sequence() {
        assert_eq!(extract(b"hello\x1b[3"), b"\x1b[3");
        assert_eq!(extract(b"a\x1b[38;5;"), b"\x1b[38;5;");
        assert_eq!(extract(b"\x1b"), b"\x1b");
        assert_eq!(extract(b"\x1b["), b"\x1b[");
    }

    #[test]
    fn returns_the_dangling_osc_unterminated_sequence() {
        assert_eq!(extract(b"\x1b]0;my-title"), b"\x1b]0;my-title");
        assert_eq!(extract(b"\x1b]0;title\x07after"), b"");
        assert_eq!(extract(b"\x1b]0;title\x1b\\after"), b"");
    }

    #[test]
    fn treats_a_fresh_esc_as_aborting_a_pending_csi() {
        assert_eq!(extract(b"\x1b[3\x1b[0m"), b"");
        assert_eq!(extract(b"\x1b[3\x1b["), b"\x1b[");
    }

    #[test]
    fn treats_can_sub_as_aborting_an_in_progress_escape_back_to_ground() {
        assert_eq!(extract(b"\x1b\x18"), b""); // ESC CAN
        assert_eq!(extract(b"\x1b\x1a"), b""); // ESC SUB
        assert_eq!(extract(b"\x1b \x18"), b""); // ESC SP CAN (escIntermediate)
        assert_eq!(extract(b"\x1b#\x1a"), b""); // ESC # SUB
        assert_eq!(extract(b"\x1b[38;\x18"), b""); // CSI ... CAN
        assert_eq!(extract(b"\x1b]0;title\x18"), b""); // OSC ... CAN
        assert_eq!(extract(b"\x1b\x18\x1b[3"), b"\x1b[3"); // abort then fresh dangling
    }

    #[test]
    fn is_fold_safe_across_chunk_boundaries() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"first\x1b[3", b"8;5;196mred"),
            (b"\x1b", b"[0m"),
            (b"\x1b]0;ti", b"tle\x07"),
            (b"clean", b"\x1b[1"),
            (b"\x1b", b"\x18after"),
            (b"\x1b ", b"\x18after"),
            // Multibyte payload must not break fold-safety either.
            ("\x1b]0;한".as_bytes(), "국\x07".as_bytes()),
        ];
        for (a, b) in cases {
            let folded: Vec<u8> = [extract(a), b].concat();
            assert_eq!(
                extract(&folded),
                extract(&[*a, *b].concat()),
                "fold-safety failed for {a:?} + {b:?}"
            );
        }
    }

    #[test]
    fn advance_accumulates_a_split_sequence_across_chunks() {
        let tail = advance_partial_escape_tail(b"", b"ls\r\n\x1b[3");
        assert_eq!(tail, b"\x1b[3".to_vec());
        let tail = advance_partial_escape_tail(&tail, b"8;5;196m");
        assert_eq!(tail, Vec::<u8>::new()); // completed
    }

    #[test]
    fn advance_abandons_tracking_past_the_cap() {
        let mut huge = b"\x1b]0;".to_vec();
        huge.extend(std::iter::repeat_n(b'x', MAX_PARTIAL_ESCAPE_TAIL_LENGTH + 10));
        assert_eq!(advance_partial_escape_tail(b"", &huge), Vec::<u8>::new());
    }

    #[test]
    fn non_ascii_payload_never_panics() {
        // Multibyte bytes inside a dangling OSC are retained verbatim.
        let s = "\x1b]0;한국어".as_bytes();
        assert_eq!(extract(s), s);
    }
}
