//! Port of Orca `shared/terminal-kitty-keyboard-mode-tracker.ts` (@ v1.4.150-rc.0).
//!
//! Mirrors the kitty keyboard protocol flag state (CSI > u push, CSI < u pop,
//! CSI = u set) by scanning the raw PTY output stream, replicating xterm's
//! exact stack/screen algorithm including the per-screen flag slots swapped by
//! DECSET/DECRST 47/1047/1049, the full reset on RIS (`ESC c`), and the soft
//! reset on DECSTR (`CSI ! p`).
//!
//! Why a mirror instead of reading xterm's internal state: Orca defensively
//! wipes the renderer terminal's kitty flags at moments when the TUI may have
//! died (Ctrl+C interrupts, reattach resets) while the TUI is usually still
//! alive and expecting protocol-encoded input. This tracker is fed only by
//! application output, so it reflects what the *application* negotiated,
//! independent of renderer-side defensive writes.
//!
//! ## Byte-native divergence: `\x9b` is `C2 9B`, never a lone `9B` (H1)
//!
//! The TS source is a `string` regex containing the JS character `'\x9b'`
//! (U+009B, the C1 CSI introducer). Orca's PTY data reaches this module as a
//! `string` decoded by node-pty with its default `encoding: 'utf8'` (no spawn
//! site in Orca passes an `encoding`, and there is no latin1/binary/TextDecoder
//! anywhere in the PTY path). Under UTF-8, U+009B is the two-byte sequence
//! `C2 9B` — a lone `9B` byte is not valid UTF-8 on its own and node-pty would
//! decode it as U+FFFD, which does not match `'\x9b'` at all.
//!
//! So in this byte-native port, `'\x9b'` is matched as the literal two-byte
//! sequence `\xc2\x9b`, and a lone `0x9b` byte deliberately does **not**
//! match. This is not merely "the byte our decode path produces" — matching a
//! lone `0x9b` would be an active regression, because `0x9B` is a UTF-8
//! *continuation* byte that appears inside ordinary multibyte characters:
//! `⚛` U+269B is `E2 9A 9B`, `♛` U+265B is `E2 99 9B`. A bare-`0x9b` matcher
//! would treat `"⚛>1u"` as `CSI > 1 u` and slice mid-character (see
//! `h1_multibyte_char_containing_0x9b_does_not_match_kitty_sequence` and
//! `h1_bare_0x9b_byte_does_not_match_kitty_sequence` below).
//!
//! Honoring a *real* 8-bit C1 CSI (a genuine lone `0x9b` byte arriving over a
//! non-UTF-8 transport) is a deliberate **non-goal**: Orca's own TS source
//! ignores it too, since node-pty never hands it a raw C1 byte in the first
//! place. Matching only `\xc2\x9b` keeps this port faithful to the oracle
//! while staying safe against the multibyte-collision regression above.
//!
//! ## H3 — not sharing `partial_escape_tail`
//!
//! `extract_scan_tail` below is this module's own string-suffix heuristic
//! (last introducer index + a body character-class check), deliberately
//! *not* [`crate::partial_escape_tail::extract_partial_escape_tail`], which is
//! a full VT500 state machine (8 states, modeling OSC/DCS/CAN/SUB). They
//! diverge concretely: on `"\x1b]0;title"` (an unterminated OSC),
//! `extract_partial_escape_tail` returns the whole OSC, while
//! `extract_scan_tail` here returns empty (its body-validity check rejects
//! OSC bodies — see `is_incomplete_sequence_body`). They only agree by
//! coincidence on the CSI-prefix cases both modules also happen to cover.
//!
//! ## H5 — 32-bit signed bitwise ops vs. a raw `=` assignment
//!
//! JS `|=` and `&= ~` are `ToInt32`-based 32-bit *signed* operations, while a
//! plain `=` assigns a raw `f64`. So `currentFlags` can hold a value like
//! `3e9` right after a `CSI = u` set, and then go *negative* after a
//! subsequent `CSI = u` OR. Storing `u32` throughout would diverge from the
//! oracle at magnitudes `>= 2^31`; storing `i32` throughout would diverge
//! immediately on the `=` (set) path, which must preserve the raw value.
//! This port stores flags as `i64` and applies an explicit [`to_int32`]
//! conversion only at the two bitwise sites (`CSI = u` modes 2 and 3). Real
//! kitty flags are `0..=31` and the only consumer tests `flags > 0`, so this
//! is unobservable in production — but it is an explicit, pinned decision
//! (see `h5_32_bit_wrap_after_or`).

use regex::bytes::{Regex, RegexBuilder};
use std::collections::VecDeque;
use std::sync::LazyLock;

/// Why: PTY/SSH chunks can split an escape sequence before its final byte.
/// Keep parser state far beyond normal sequence lengths while bounding memory.
pub const KITTY_SCAN_TAIL_LIMIT: usize = 4096;

/// Why: mirrors xterm's InputHandler cap so a runaway TUI cannot grow the
/// mirrored stacks unboundedly while the renderer's own stacks stay at 16.
pub const KITTY_STACK_LIMIT: usize = 16;

fn bytes_regex(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .build()
        .expect("static kitty-keyboard-mode-tracker regex")
}

// oxlint/clippy: terminal escape sequences require raw control bytes.
// `\xc2\x9b` is the UTF-8 encoding of U+009B (see the H1 module doc above) —
// NOT a lone 0x9b byte.
static KITTY_MODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    bytes_regex(r"\x1bc|(?:\x1b\[|\xc2\x9b)(?:!p|\?([0-9;]+)([hl])|([<>=])([0-9;]*)u)")
});

static INCOMPLETE_BODY_RE: LazyLock<Regex> = LazyLock::new(|| bytes_regex(r"\A[<>=?]?[0-9;]*\z"));

/// JS `ToInt32`: reduce to the value's low 32 bits and reinterpret as signed.
/// Used only at the two bitwise sites in `apply_kitty_sequence` (H5) — the
/// `=` (set) path assigns the raw parsed value with no truncation.
fn to_int32(value: i64) -> i32 {
    (value as u64 & 0xFFFF_FFFF) as u32 as i32
}

/// Mirrors JS `Number(s)` for the digit-only substrings this module ever
/// feeds it (`[0-9;]*` split on `;`): empty is `0`, not an error (H4) —
/// `Number('') === 0` in JS, while Rust's `"".parse()` returns `Err`. A
/// sibling Orca module (`terminal-private-mode-tracker.ts:30-32`) instead
/// *skips* empty params; this module does not, and the two behaviors are not
/// interchangeable (see `h4_empty_param_forms`).
fn parse_js_number(bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    // The regex guarantees ASCII digits only; a parse failure only occurs on
    // magnitudes exceeding i64, which is far outside any real or pinned
    // kitty/DECSET parameter. Falling back to i64::MAX (rather than 0) avoids
    // manufacturing an accidental collision with 47/1047/1049 or the flag
    // truthiness checks below.
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(i64::MAX)
}

/// Mirrors the kitty keyboard protocol flag state by scanning raw PTY output.
/// See the module docs for the byte-native `\x9b` decision and the full list
/// of asymmetric-reset / truthiness traps this tracker deliberately
/// replicates from xterm/Orca.
#[derive(Debug, Default)]
pub struct KittyKeyboardModeTracker {
    scan_tail: Vec<u8>,
    current_flags: i64,
    main_flags: i64,
    alt_flags: i64,
    main_stack: VecDeque<i64>,
    alt_stack: VecDeque<i64>,
    alternate_screen_active: bool,
    alternate_screen_switch_observed: bool,
}

impl KittyKeyboardModeTracker {
    /// Current effective kitty keyboard flags (0 = protocol inactive).
    pub fn flags(&self) -> i64 {
        self.current_flags
    }

    pub fn is_alternate_screen(&self) -> bool {
        self.alternate_screen_active
    }

    pub fn has_observed_alternate_screen_switch(&self) -> bool {
        self.alternate_screen_switch_observed
    }

    /// Full reset: all eight fields, including the retained scan tail.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn scan(&mut self, data: &[u8]) {
        self.scan_internal(data, false);
    }

    /// Scan bytes replayed from a retained history window (reattach payloads,
    /// relay replays, daemon snapshots). Replays can redeliver the
    /// application's one-time `CSI > u` push — applying it with stack
    /// semantics on every delivery grows the mirrored stack, so the TUI's
    /// eventual single pop lands on a stale frame and Option chords stay
    /// kitty-encoded in a plain shell. Pushes seen during replay therefore
    /// apply as idempotent sets (H12: `replay` gates *only* the stack push;
    /// screen switches, DECSTR, and pops all apply identically to a live
    /// scan). Known limit: a NESTED push/push/pop inside the window collapses
    /// to flags 0 on the pop — unavoidable stackless-replay tradeoff (a
    /// redelivered push is byte-wise indistinguishable from a new one); real
    /// TUIs push once at startup.
    pub fn scan_replay(&mut self, data: &[u8]) {
        self.scan_internal(data, true);
    }

    fn scan_internal(&mut self, data: &[u8], replay: bool) {
        // H9: extract_scan_tail runs BEFORE the regex loop below, and the
        // regex then scans `input` in full, INCLUDING the just-retained
        // tail. The only thing preventing double-application of a retained
        // partial sequence is that `is_incomplete_sequence_body` never
        // accepts a body containing a terminator (`u`/`h`/`l`/`p`) — do not
        // "improve" that predicate to accept terminators.
        let mut input = std::mem::take(&mut self.scan_tail);
        input.extend_from_slice(data);
        self.scan_tail = Self::extract_scan_tail(&input);

        for caps in KITTY_MODE_RE.captures_iter(&input) {
            let whole = caps.get(0).expect("group 0 always present").as_bytes();
            if whole == b"\x1bc" {
                // RIS resets kitty state and returns to the main screen.
                // H10: save the tail, run the FULL reset, restore the tail,
                // then force `observed = true`. `*self = Self::default()`
                // alone would drop the tail permanently.
                let tail = std::mem::take(&mut self.scan_tail);
                self.reset();
                self.scan_tail = tail;
                self.alternate_screen_switch_observed = true;
                continue;
            }
            if whole.ends_with(b"!p") {
                self.apply_soft_reset();
                continue;
            }
            if let Some(params) = caps.get(1) {
                let enabled = caps.get(2).is_some_and(|m| m.as_bytes() == b"h");
                self.apply_screen_switch(params.as_bytes(), enabled);
                continue;
            }
            let prefix = caps
                .get(3)
                .and_then(|m| m.as_bytes().first().copied())
                .expect("non-RIS, non-!p, non-screen-switch match must carry a </>/= prefix");
            let params = caps.get(4).map(|m| m.as_bytes()).unwrap_or(b"");
            self.apply_kitty_sequence(prefix, params, replay);
        }
    }

    /// DECSTR (`CSI ! p`): xterm's soft reset wipes kitty flags and stacks
    /// for both screens via coreService.reset but does not switch buffers —
    /// mirror that so a soft-resetting TUI stops receiving kitty-encoded
    /// Option chords. H10: `alternate_screen_active`,
    /// `alternate_screen_switch_observed`, and `scan_tail` are left
    /// untouched.
    fn apply_soft_reset(&mut self) {
        self.current_flags = 0;
        self.main_flags = 0;
        self.alt_flags = 0;
        self.main_stack.clear();
        self.alt_stack.clear();
    }

    /// H8: no already-active guard, and every parameter in the sequence is
    /// applied in turn — `?1049h` twice swaps twice, and `?1049;47h` swaps
    /// twice within a single sequence. This mirrors xterm deliberately; do
    /// not `break` after the first match and do not add a guard.
    fn apply_screen_switch(&mut self, params: &[u8], enabled: bool) {
        for raw_param in params.split(|&b| b == b';') {
            let param = parse_js_number(raw_param);
            if param != 47 && param != 1047 && param != 1049 {
                continue;
            }
            self.alternate_screen_switch_observed = true;
            // xterm swaps the current flags with the inactive screen's slot
            // on every 47/1047/1049 transition, without an already-active
            // guard — mirror it exactly so this state matches what the
            // renderer encodes.
            if enabled {
                self.main_flags = self.current_flags;
                self.current_flags = self.alt_flags;
                self.alternate_screen_active = true;
            } else {
                self.alt_flags = self.current_flags;
                self.current_flags = self.main_flags;
                self.alternate_screen_active = false;
            }
        }
    }

    fn apply_kitty_sequence(&mut self, prefix: u8, params: &[u8], replay: bool) {
        let parsed: Vec<i64> = params.split(|&b| b == b';').map(parse_js_number).collect();
        match prefix {
            b'>' => {
                // H12: `replay` gates ONLY this push onto the stack.
                if !replay {
                    let current = self.current_flags;
                    let stack = self.active_stack_mut();
                    // H6: `stack.shift()` removes the FRONT — `pop_front`,
                    // never `pop_back`.
                    if stack.len() >= KITTY_STACK_LIMIT {
                        stack.pop_front();
                    }
                    stack.push_back(current);
                }
                // `currentFlags = parsed[0] || 0` runs regardless of replay.
                self.current_flags = parsed.first().copied().unwrap_or(0);
            }
            b'<' => {
                // H14: `parsed[0] || 1` means `CSI < 0 u` pops ONCE, not zero
                // times (0 is falsy in JS, so it falls back to the `|| 1`
                // default). `Math.max(1, ...)` is unreachable in practice
                // (the parameter charset has no `-`) but kept verbatim.
                let parsed0 = parsed.first().copied().unwrap_or(0);
                let count = (if parsed0 != 0 { parsed0 } else { 1 }).max(1);
                let mut popped = 0i64;
                while popped < count {
                    let stack = self.active_stack_mut();
                    if stack.is_empty() {
                        break;
                    }
                    self.current_flags = stack.pop_back().expect("checked non-empty above");
                    popped += 1;
                }
                // H7: zero the flags whenever the stack ENDS empty, even if
                // it was already empty before this pop — no
                // "only if we actually popped" guard.
                if self.active_stack_mut().is_empty() {
                    self.current_flags = 0;
                }
            }
            b'=' => {
                let flags = parsed.first().copied().unwrap_or(0);
                // H11: mode uses truthiness, not `??`. `parsed[1]` picks the
                // mode only when present AND non-zero; otherwise (including
                // `CSI = 5 ; 0 u`, which collapses to mode 1) the default is
                // 1. Modes other than 1/2/3 are silent no-ops (no `else`).
                let mode = if parsed.len() > 1 && parsed[1] != 0 {
                    parsed[1]
                } else {
                    1
                };
                if mode == 1 {
                    // Raw f64-style assignment: no ToInt32 truncation (H5).
                    self.current_flags = flags;
                } else if mode == 2 {
                    self.current_flags = (to_int32(self.current_flags) | to_int32(flags)) as i64;
                } else if mode == 3 {
                    self.current_flags = (to_int32(self.current_flags) & !to_int32(flags)) as i64;
                }
            }
            _ => unreachable!("regex only ever captures '<', '>', or '=' as the kitty prefix"),
        }
    }

    fn active_stack_mut(&mut self) -> &mut VecDeque<i64> {
        if self.alternate_screen_active {
            &mut self.alt_stack
        } else {
            &mut self.main_stack
        }
    }

    /// H9/H13: runs before the regex loop. Finds the last ESC (`0x1b`) or the
    /// last `\xc2\x9b` (H1: the UTF-8 encoding of the JS char `'\x9b'`) and
    /// returns the suffix from there, provided it still looks like the start
    /// of an in-progress sequence. H13: the 4096-byte cap check is strictly
    /// `>` and runs BEFORE the body-validity test.
    fn extract_scan_tail(input: &[u8]) -> Vec<u8> {
        let esc_pos = input.iter().rposition(|&b| b == 0x1b);
        let c1_pos = Self::rfind_c2_9b(input);
        let start = match (esc_pos, c1_pos) {
            (None, None) => return Vec::new(),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (Some(a), Some(b)) => a.max(b),
        };
        let tail = &input[start..];
        if tail.len() > KITTY_SCAN_TAIL_LIMIT {
            return Vec::new();
        }
        if tail == b"\x1b" || tail == b"\x1b[" || tail == b"\xc2\x9b" {
            return tail.to_vec();
        }
        let body: Option<&[u8]> = if let Some(rest) = tail.strip_prefix(b"\x1b[") {
            Some(rest)
        } else {
            tail.strip_prefix(b"\xc2\x9b")
        };
        match body {
            None => Vec::new(),
            Some(b) if Self::is_incomplete_sequence_body(b) => tail.to_vec(),
            Some(_) => Vec::new(),
        }
    }

    /// Last byte-index of the two-byte sequence `\xc2\x9b` (UTF-8 for
    /// U+009B). A lone trailing `0xc2` with no following `0x9b` is not a
    /// match — it is either not this sequence at all, or a chunk boundary
    /// mid-character that `extract_partial_escape_tail`-style multibyte
    /// tracking is explicitly out of scope for here (H3).
    fn rfind_c2_9b(input: &[u8]) -> Option<usize> {
        if input.len() < 2 {
            return None;
        }
        (0..=input.len() - 2).rev().find(|&i| input[i] == 0xc2 && input[i + 1] == 0x9b)
    }

    fn is_incomplete_sequence_body(body: &[u8]) -> bool {
        body == b"!" || INCOMPLETE_BODY_RE.is_match(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Oracle: terminal-kitty-keyboard-mode-tracker.test.ts (13 cases) ----

    #[test]
    fn oracle_starts_inactive_and_ignores_non_kitty_sequences() {
        let mut t = KittyKeyboardModeTracker::default();
        assert_eq!(t.flags(), 0);
        t.scan(b"plain output \x1b[?2004h\x1b[38;5;10mcolored\x1b[0m");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_does_not_treat_csi_u_or_csi_query_u_as_kitty_state() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[u\x1b[?u");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_tracks_push_and_pop_including_pop_to_empty_zeroing() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        assert_eq!(t.flags(), 1);
        t.scan(b"\x1b[>7u");
        assert_eq!(t.flags(), 7);
        t.scan(b"\x1b[<u");
        assert_eq!(t.flags(), 1);

        let mut drained = KittyKeyboardModeTracker::default();
        drained.scan(b"\x1b[=3;1u\x1b[>5u\x1b[<u");
        assert_eq!(drained.flags(), 0);
    }

    #[test]
    fn oracle_applies_set_or_clear_modes_of_csi_equals_u() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[=1;1u");
        assert_eq!(t.flags(), 1);
        t.scan(b"\x1b[=2;2u");
        assert_eq!(t.flags(), 3);
        t.scan(b"\x1b[=1;3u");
        assert_eq!(t.flags(), 2);
        // Mode defaults to 1 (set) when omitted.
        t.scan(b"\x1b[=4u");
        assert_eq!(t.flags(), 4);
    }

    #[test]
    fn oracle_clears_state_for_defensive_reset_and_ris() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan(b"\x1b[<99u\x1b[=0u");
        assert_eq!(t.flags(), 0);

        t.scan(b"\x1b[>1u");
        t.scan(b"\x1bc");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_keeps_per_screen_flags_across_alternate_screen_switches() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        assert_eq!(t.flags(), 1);
        t.scan(b"\x1b[?1049h");
        assert!(t.has_observed_alternate_screen_switch());
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
        t.scan(b"\x1b[>2u");
        assert_eq!(t.flags(), 2);
        t.scan(b"\x1b[?1049l");
        assert!(!t.is_alternate_screen());
        assert_eq!(t.flags(), 1);
    }

    #[test]
    fn oracle_handles_sequences_split_across_chunks_and_c1_csi() {
        // H2: the oracle's `'\x9b<99u'` / `'\x9b>7u'` are 5-char JS strings;
        // each is the JS char U+009B followed by ASCII. Under the PTY's utf8
        // decode (see module docs), U+009B is the two bytes `\xc2\x9b`, so
        // the byte-faithful re-encoding is 6 bytes: `"\u{9b}<99u".as_bytes()`
        // written via a Rust \u escape so the UTF-8 re-encoding is
        // self-evident at the call site (NOT `b"\x9b<99u"`, which would
        // encode a lone 0x9b byte — a different, wrong test; see H1/H2 in the
        // module docs).
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>");
        assert_eq!(t.flags(), 0);
        t.scan(b"1u");
        assert_eq!(t.flags(), 1);
        t.scan("\u{9b}<99u".as_bytes());
        assert_eq!(t.flags(), 0);
        t.scan("\u{9b}>7u".as_bytes());
        assert_eq!(t.flags(), 7);
    }

    #[test]
    fn oracle_caps_the_mirrored_stack_without_losing_current_flags() {
        let mut t = KittyKeyboardModeTracker::default();
        let mut last_sent = 0i64;
        for i in 0..40i64 {
            last_sent = (i % 3) + 1;
            t.scan(format!("\x1b[>{last_sent}u").as_bytes());
        }
        assert_eq!(t.flags(), last_sent);
    }

    #[test]
    fn oracle_reset_returns_to_inactive_state() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u\x1b[?1049h\x1b[>2u");
        t.reset();
        assert_eq!(t.flags(), 0);
        t.scan(b"\x1b[?1049l");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_clears_kitty_state_on_decstr_without_switching_screens() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u\x1b[!p");
        assert_eq!(t.flags(), 0);

        let mut on_alt = KittyKeyboardModeTracker::default();
        on_alt.scan(b"\x1b[>1u\x1b[?1049h\x1b[>2u");
        assert_eq!(on_alt.flags(), 2);
        on_alt.scan(b"\x1b[!p");
        assert_eq!(on_alt.flags(), 0);
        on_alt.scan(b"\x1b[?1049l");
        assert_eq!(on_alt.flags(), 0);
    }

    #[test]
    fn oracle_handles_decstr_split_across_chunks() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u\x1b[!");
        assert_eq!(t.flags(), 1);
        t.scan(b"p");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_applies_replayed_pushes_as_sets() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan_replay(b"\x1b[>1u");
        t.scan_replay(b"\x1b[>1u");
        assert_eq!(t.flags(), 1);
        t.scan(b"\x1b[<u");
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn oracle_replay_scans_arm_a_fresh_tracker_and_honor_pops_inside_window() {
        let mut fresh = KittyKeyboardModeTracker::default();
        fresh.scan_replay(b"\x1b[>1u");
        assert_eq!(fresh.flags(), 1);
        fresh.scan(b"\x1b[<u");
        assert_eq!(fresh.flags(), 0);

        let mut ran_and_exited = KittyKeyboardModeTracker::default();
        ran_and_exited.scan_replay(b"\x1b[>1uoutput\x1b[<u");
        assert_eq!(ran_and_exited.flags(), 0);
    }

    // ---- H<N> pins (oracle-silent branches) ----

    /// H1: the mutation-catcher for the whole \x9b-decision. `⚛` (U+269B) is
    /// `E2 9A 9B` in UTF-8 — its last two bytes are `9A 9B`, NOT `C2 9B`, so
    /// this must not match even though it contains a literal `9b` byte after
    /// `9a`. This case alone does not prove the fix; paired with the next
    /// case (a real `C2 9B`) it pins that the matcher requires exactly `C2`
    /// before `9B`.
    #[test]
    fn h1_multibyte_char_containing_0x9b_does_not_match_kitty_sequence() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan("⚛>1u".as_bytes());
        assert_eq!(t.flags(), 0);
    }

    /// H1: a bare `0x9b` byte (not preceded by `0xc2`) followed by `>1u`
    /// must not match either — only the two-byte `\xc2\x9b` does.
    #[test]
    fn h1_bare_0x9b_byte_does_not_match_kitty_sequence() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(&[0x9b, b'>', b'1', b'u']);
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h2_re_encoded_oracle_c1_decstr() {
        // \u{9b} + "!p" re-encoded per H2: 3 JS chars -> 3 bytes
        // (\xc2\x9b + '!' + 'p' is 4 bytes total including the '!p').
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan("\u{9b}!p".as_bytes());
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h2_re_encoded_oracle_c1_screen_switch() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan("\u{9b}?1049h".as_bytes());
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h2_bare_c1_introducer_retained_as_tail_across_chunks() {
        let mut t = KittyKeyboardModeTracker::default();
        // A lone \xc2\x9b (the C1 CSI introducer) with nothing after it must
        // be retained as a scan tail and complete once the rest arrives.
        t.scan("\u{9b}".as_bytes());
        assert_eq!(t.flags(), 0);
        t.scan(b">5u");
        assert_eq!(t.flags(), 5);
    }

    #[test]
    fn h13_scan_tail_cap_is_strictly_greater_than_4096() {
        // Exactly at the cap: a dangling `CSI >` push totaling exactly
        // KITTY_SCAN_TAIL_LIMIT bytes must still be retained (the cap check
        // is `>`, not `>=`), so appending the terminator in the next chunk
        // completes the push. The digit run is long enough to overflow i64,
        // so `parse_js_number`'s overflow fallback (i64::MAX) doubles as
        // proof the retained digits actually reached the parser.
        let mut at_cap = KittyKeyboardModeTracker::default();
        let mut chunk = b"\x1b[>".to_vec();
        chunk.extend(std::iter::repeat_n(b'1', KITTY_SCAN_TAIL_LIMIT - 3));
        assert_eq!(chunk.len(), KITTY_SCAN_TAIL_LIMIT);
        at_cap.scan(&chunk);
        assert_eq!(at_cap.flags(), 0); // nothing has completed yet
        at_cap.scan(b"u");
        assert_eq!(at_cap.flags(), i64::MAX);

        // One byte over the cap: the tail is dropped entirely, so the lone
        // trailing "u" in the next chunk is scanned with no retained prefix
        // and cannot complete anything.
        let mut over_cap = KittyKeyboardModeTracker::default();
        let mut too_long = b"\x1b[>".to_vec();
        too_long.extend(std::iter::repeat_n(b'1', KITTY_SCAN_TAIL_LIMIT - 2));
        assert_eq!(too_long.len(), KITTY_SCAN_TAIL_LIMIT + 1);
        over_cap.scan(&too_long);
        over_cap.scan(b"u");
        assert_eq!(over_cap.flags(), 0);
    }

    #[test]
    fn h9_body_is_null_paths_do_not_retain_a_tail() {
        // Unterminated OSC: extract_scan_tail's body-prefix check rejects it
        // (body is neither after `\x1b[` nor after `\xc2\x9b`), so nothing is
        // retained, unlike `extract_partial_escape_tail` which would keep
        // the whole thing (H3 divergence documented in the module header).
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b]0;title");
        t.scan(b"\x07\x1b[>1u");
        assert_eq!(t.flags(), 1);

        // `ESC O` (SS3): body is `O`, which starts with neither prefix form,
        // so nothing is retained either.
        let mut t2 = KittyKeyboardModeTracker::default();
        t2.scan(b"\x1bO");
        t2.scan(b"P\x1b[>1u");
        assert_eq!(t2.flags(), 1);
    }

    #[test]
    fn h4_empty_param_forms_parse_as_zero() {
        // CSI > u (no params): parsed = [Number('')] = [0] -> push 0, then
        // `parsed[0] || 0` -> currentFlags = 0.
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>u");
        assert_eq!(t.flags(), 0);

        // CSI = u (no params): flags = 0, mode defaults to 1 (set).
        let mut t2 = KittyKeyboardModeTracker::default();
        t2.scan(b"\x1b[>5u"); // establish a nonzero baseline first
        assert_eq!(t2.flags(), 5);
        t2.scan(b"\x1b[=u");
        assert_eq!(t2.flags(), 0);

        // CSI ? ; h (both DECSET params empty): parsed via split(';') as
        // ["", ""] -> [0, 0], neither is 47/1047/1049, so no screen switch
        // and no observed-switch flag.
        let mut t3 = KittyKeyboardModeTracker::default();
        t3.scan(b"\x1b[?;h");
        assert!(!t3.has_observed_alternate_screen_switch());
        assert!(!t3.is_alternate_screen());
    }

    /// H4, stronger pin: a *leading* empty parameter must map to `0` in
    /// place, not be dropped from the array. `CSI = ; 5 u` splits to
    /// `["", "5"]` -> `[0, 5]`: `flags = parsed[0] = 0`,
    /// `mode = parsed[1] = 5` (out of 1..=3, a no-op) -> currentFlags is
    /// UNCHANGED. A "skip empty params" mutant would instead drop the
    /// leading `""` entirely, shifting `"5"` into index 0 (`parsed = [5]`,
    /// length 1) so `mode` would default to 1 (SET) and wrongly apply
    /// `currentFlags = 5`. The three tests above alone cannot catch that
    /// mutant (skip-vs-zero happens to coincide once the shift lands on a
    /// trailing position); this one isolates the index-shift specifically.
    #[test]
    fn h4_leading_empty_param_does_not_shift_the_mode_index() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[=9u"); // baseline: mode defaults to 1, flags = 9
        assert_eq!(t.flags(), 9);
        t.scan(b"\x1b[=;5u");
        assert_eq!(t.flags(), 9); // unchanged: mode 5 is a silent no-op
    }

    #[test]
    fn h11_mode_zero_collapses_to_set_and_mode_five_is_a_noop() {
        // `CSI = 5 ; 0 u`: parsed = [5, 0]; parsed[1] is falsy (0), so mode
        // collapses to 1 (set), NOT mode 0 / a no-op.
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[=5;0u");
        assert_eq!(t.flags(), 5);

        // Mode 5 is out of range 1-3: silent no-op, flags unchanged.
        let mut t2 = KittyKeyboardModeTracker::default();
        t2.scan(b"\x1b[=9u"); // baseline flags = 9
        assert_eq!(t2.flags(), 9);
        t2.scan(b"\x1b[=1;5u");
        assert_eq!(t2.flags(), 9);
    }

    #[test]
    fn h8_decset_params_47_and_1047_also_trigger_the_screen_switch() {
        let mut t47 = KittyKeyboardModeTracker::default();
        t47.scan(b"\x1b[>1u");
        t47.scan(b"\x1b[?47h");
        assert!(t47.is_alternate_screen());
        assert!(t47.has_observed_alternate_screen_switch());
        assert_eq!(t47.flags(), 0);

        let mut t1047 = KittyKeyboardModeTracker::default();
        t1047.scan(b"\x1b[>1u");
        t1047.scan(b"\x1b[?1047h");
        assert!(t1047.is_alternate_screen());
        assert_eq!(t1047.flags(), 0);
    }

    #[test]
    fn h8_multi_param_sequence_double_swaps_within_one_sequence() {
        // `?1049;47h`: both params are swap-triggering, applied in order,
        // with no already-active guard -> swaps TWICE in one sequence.
        // Both params share the same direction (`h` == enable), so the
        // *current_flags* value after the call is 0 either way (single or
        // double swap) — checking only that would not catch a
        // "break after the first match" mutant. The double swap DOES leave
        // a different `main_flags` behind: the second swap re-reads the
        // already-swapped current_flags (0) into main_flags, overwriting
        // the original 3 that a single swap would have preserved there.
        // Exiting the alternate screen surfaces that overwrite.
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>3u");
        assert_eq!(t.flags(), 3);
        t.scan(b"\x1b[?1049;47h");
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
        t.scan(b"\x1b[?1049l");
        assert!(!t.is_alternate_screen());
        // A break-after-first-match mutant would have left main_flags at 3
        // (only the first param's swap ran) and this would read back 3.
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h8_repeated_enable_sequences_swap_every_time() {
        // Two separate `?1049h` sequences swap twice total (once each), with
        // no already-active guard to skip the second.
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan(b"\x1b[?1049h");
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
        t.scan(b"\x1b[>9u");
        assert_eq!(t.flags(), 9);
        // Second enable: main <- current(9), current <- alt(0). Still
        // "active" per the flag (no guard distinguishes re-entry).
        t.scan(b"\x1b[?1049h");
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h10_ris_preserves_a_pending_tail_across_the_reset() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1bc\x1b[>");
        assert_eq!(t.flags(), 0);
        assert!(t.has_observed_alternate_screen_switch());
        t.scan(b"1u");
        assert_eq!(t.flags(), 1);
    }

    #[test]
    fn h10_ris_forces_observed_alternate_screen_switch_true() {
        let mut t = KittyKeyboardModeTracker::default();
        assert!(!t.has_observed_alternate_screen_switch());
        t.scan(b"\x1bc");
        assert!(t.has_observed_alternate_screen_switch());
    }

    #[test]
    fn h10_decstr_leaves_is_alternate_screen_unchanged() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[?1049h");
        assert!(t.is_alternate_screen());
        t.scan(b"\x1b[!p");
        assert!(t.is_alternate_screen());
    }

    /// H12: production-critical and completely untested upstream —
    /// `scan_replay` must still apply a `?1049h` screen switch identically
    /// to a live scan (only the `>` stack push is gated by `replay`).
    #[test]
    fn h12_scan_replay_applies_screen_switch_identically_to_live_scan() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>4u");
        assert_eq!(t.flags(), 4);
        t.scan_replay(b"\x1b[?1049h");
        assert!(t.is_alternate_screen());
        assert!(t.has_observed_alternate_screen_switch());
        assert_eq!(t.flags(), 0);
    }

    #[test]
    fn h5_32_bit_wrap_after_set_then_or() {
        let mut t = KittyKeyboardModeTracker::default();
        // Raw f64-style assignment: no truncation on the `=` (set) path.
        t.scan(b"\x1b[=4000000000;1u");
        assert_eq!(t.flags(), 4_000_000_000);
        // `|=` (mode 2) applies ToInt32 to both operands first:
        // to_int32(4_000_000_000) == -294_967_296, OR 1 == -294_967_295.
        t.scan(b"\x1b[=1;2u");
        assert_eq!(t.flags(), -294_967_295);
    }

    #[test]
    fn h6_stack_cap_drops_the_front_not_the_back() {
        let mut t = KittyKeyboardModeTracker::default();
        // Push KITTY_STACK_LIMIT + 1 times. Each push stores the flags
        // value from BEFORE that push, so after this loop the (correct,
        // front-evicting) stack holds [0, 1, 2, ..., 14, 15] front-to-back:
        // the initial 0 survives the first 15 pushes (stack not yet full),
        // then the 17th push (i=16) evicts the FRONT (0) and appends 15.
        for i in 0..(KITTY_STACK_LIMIT as i64 + 1) {
            t.scan(format!("\x1b[>{}u", i).as_bytes());
        }
        assert_eq!(t.flags(), KITTY_STACK_LIMIT as i64); // 16

        // A pop_back-eviction mutant would instead evict the value about to
        // become the back before the final push, producing the stack
        // [0, 0, 1, ..., 13, 15] (missing 14, duplicate 0 at front). The
        // FIRST pop (the freshly pushed top, 15) is identical either way and
        // cannot distinguish the mutant; the SECOND pop does: 14 under
        // correct front-eviction vs. 13 under the pop_back mutant.
        t.scan(b"\x1b[<u");
        assert_eq!(t.flags(), KITTY_STACK_LIMIT as i64 - 1); // 15
        t.scan(b"\x1b[<u");
        assert_eq!(t.flags(), KITTY_STACK_LIMIT as i64 - 2); // 14
    }

    #[test]
    fn split_1049h_across_two_chunks() {
        let mut t = KittyKeyboardModeTracker::default();
        t.scan(b"\x1b[>1u");
        t.scan(b"\x1b[?10");
        assert_eq!(t.flags(), 1);
        assert!(!t.is_alternate_screen());
        t.scan(b"49h");
        assert!(t.is_alternate_screen());
        assert_eq!(t.flags(), 0);
    }
}
