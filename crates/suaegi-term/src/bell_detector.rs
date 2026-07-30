//! Port of Orca `shared/terminal-bell-detector.ts` (@ v1.4.150-rc.0).
//!
//! Stateful BEL (0x07) detector that ignores BEL bytes occurring inside OSC
//! escape sequences (where BEL is a string terminator, not a bell). State is
//! kept across chunks because a PTY OSC sequence may span multiple reads.
//! CAN (0x18) / SUB (0x1A) cancel an in-progress escape per ECMA-48.
//!
//! Byte-native: every inspected byte is ASCII (BEL/CAN/SUB/ESC/`]`/`\`), and
//! UTF-8 is self-synchronizing, so a `&[u8]` scan matches the JS UTF-16 scan
//! exactly and never panics.

const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const ESC: u8 = 0x1b;

/// Stateful BEL detector shared between the transport processor and the per-PTY
/// side-effect tracker so bell semantics never drift between them.
#[derive(Clone, Debug, Default)]
pub struct BellDetector {
    pending_escape: bool,
    in_osc: bool,
    pending_osc_escape: bool,
}

impl BellDetector {
    /// A fresh detector in the ground state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset state — callers invoke this whenever the underlying byte stream is
    /// replaced (PTY detach/attach) so mid-escape state does not leak.
    pub fn reset(&mut self) {
        self.pending_escape = false;
        self.in_osc = false;
        self.pending_osc_escape = false;
    }

    /// Whether `data` contains a real terminal bell (BEL not consumed as an OSC
    /// terminator). `contains_osc_introducer` lets a caller that already scanned
    /// for `ESC ]` share the result instead of paying a second pass.
    pub fn chunk_contains_bell(
        &mut self,
        data: &[u8],
        contains_osc_introducer: Option<bool>,
    ) -> bool {
        if !self.in_osc && !self.pending_escape && !data.contains(&BEL) {
            // Why: CSI/plain chunks with no BEL and no OSC start cannot affect
            // bell state; avoid walking every byte of normal terminal output.
            let has_osc_introducer =
                contains_osc_introducer.unwrap_or_else(|| contains_osc_introducer_bytes(data));
            if !has_osc_introducer {
                self.pending_escape = data.last() == Some(&ESC);
                return false;
            }
        }

        for &char in data {
            if self.in_osc {
                if char == CAN || char == SUB {
                    // ECMA-48 escape-cancel: abort the in-progress OSC so a
                    // malformed/truncated OSC does not swallow the next BEL.
                    self.in_osc = false;
                    self.pending_osc_escape = false;
                    continue;
                }
                if self.pending_osc_escape {
                    self.pending_osc_escape = char == ESC;
                    if char == b'\\' {
                        self.in_osc = false;
                        self.pending_osc_escape = false;
                    }
                    continue;
                }
                if char == BEL {
                    self.in_osc = false;
                    continue;
                }
                self.pending_osc_escape = char == ESC;
                continue;
            }

            if self.pending_escape {
                if char == CAN || char == SUB {
                    self.pending_escape = false;
                    continue;
                }
                self.pending_escape = false;
                if char == b']' {
                    self.in_osc = true;
                    self.pending_osc_escape = false;
                } else if char == ESC {
                    self.pending_escape = true;
                } else if char == BEL {
                    // A bare ESC is not a valid introducer for any sequence that
                    // consumes a following BEL — treat it as a real bell.
                    return true;
                }
                continue;
            }

            if char == ESC {
                self.pending_escape = true;
                continue;
            }

            if char == BEL {
                return true;
            }
        }

        false
    }
}

/// Whether `data` contains the OSC introducer `ESC ]`.
fn contains_osc_introducer_bytes(data: &[u8]) -> bool {
    data.windows(2).any(|w| w == b"\x1b]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_ansi_chunks_without_losing_later_real_bells() {
        let mut detector = BellDetector::new();
        assert!(!detector.chunk_contains_bell(b"\x1b[32mbuild\x1b[0m output", None));
        assert!(detector.chunk_contains_bell(b"\x07", None));
    }

    #[test]
    fn keeps_split_osc_state_so_title_terminators_are_not_reported_as_bells() {
        let mut detector = BellDetector::new();
        assert!(!detector.chunk_contains_bell(b"\x1b]0;Codex working", None));
        // This BEL is the OSC terminator — not a bell.
        assert!(!detector.chunk_contains_bell(b"\x07", None));
        // Now a real bell.
        assert!(detector.chunk_contains_bell(b"\x07", None));
    }

    #[test]
    fn treats_bell_after_a_split_non_osc_escape_as_a_real_bell() {
        let mut detector = BellDetector::new();
        assert!(!detector.chunk_contains_bell(b"\x1b", None));
        assert!(detector.chunk_contains_bell(b"\x07", None));
    }

    // --- extra pins ---

    #[test]
    fn st_terminated_osc_then_bell_is_a_real_bell() {
        let mut detector = BellDetector::new();
        // OSC terminated by ST (ESC \), then a real BEL.
        assert!(detector.chunk_contains_bell(b"\x1b]0;title\x1b\\\x07", None));
    }

    #[test]
    fn can_aborts_a_stuck_osc_so_next_bel_is_real() {
        let mut detector = BellDetector::new();
        assert!(!detector.chunk_contains_bell(b"\x1b]0;stuck", None));
        // CAN cancels the OSC; the following BEL is a real bell.
        assert!(detector.chunk_contains_bell(b"\x18\x07", None));
    }

    #[test]
    fn osc_introducer_hint_is_honored_over_scanning() {
        let mut detector = BellDetector::new();
        // Hint says an OSC introducer is present, forcing the byte-walk even
        // though this chunk has no BEL; state carries the OSC into the next.
        assert!(!detector.chunk_contains_bell(b"\x1b]0;t", Some(true)));
        assert!(!detector.chunk_contains_bell(b"\x07", None)); // OSC terminator
    }

    #[test]
    fn reset_clears_pending_osc_state() {
        let mut detector = BellDetector::new();
        assert!(!detector.chunk_contains_bell(b"\x1b]0;working", None));
        detector.reset();
        // After reset, a BEL is a real bell (no stuck OSC state).
        assert!(detector.chunk_contains_bell(b"\x07", None));
    }

    #[test]
    fn non_ascii_payload_never_panics_and_bel_still_detected() {
        let mut detector = BellDetector::new();
        // Multibyte UTF-8 in an OSC title, terminated, then a real bell.
        let data = "\x1b]0;한국어\x07".as_bytes();
        assert!(!detector.chunk_contains_bell(data, None)); // BEL terminates OSC
        assert!(detector.chunk_contains_bell("중국어\x07".as_bytes(), None));
    }
}
