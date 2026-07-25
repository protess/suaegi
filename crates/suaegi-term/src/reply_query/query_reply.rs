//! Port of Orca `shared/terminal-query-reply.ts` (@ v1.4.150-rc.0).
//!
//! Classifies whether a chunk from the emulator's output stream is a synthetic
//! reply to a terminal query (CPR/DSR, DA, DECRPM, window/cell size, OSC 10/11
//! color, kitty keyboard flags, DCS-framed DECRQSS/XTVERSION) rather than typed
//! input. Latency-critical replies must bypass input coalescing on the remote
//! transport (Orca #7329).
//!
//! Byte-native: the recognition grammars are all ASCII, matched via
//! `regex::bytes` with Unicode mode OFF so `[^\x07\x1b]` etc. are byte classes
//! (equivalent to JS's UTF-16-unit classes for ASCII and panic-free on raw
//! bytes). `\d`→`[0-9]` is moot here since the source already uses `[0-9]`.
//! Anchors use `\A`…`\z` (absolute start/end) to exactly match JS `^`…`$`
//! without the multiline flag.

use regex::bytes::{Regex, RegexBuilder};
use std::sync::LazyLock;

/// ESC (0x1b).
const ESC: u8 = 0x1b;

fn anchored(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .unicode(false)
        .build()
        .expect("static terminal-query-reply regex")
}

/// The seven query-reply grammars, in Orca's `||` order.
static QUERY_REPLY_RES: LazyLock<[Regex; 7]> = LazyLock::new(|| {
    [
        // CPR / DECXCPR / DSR — cursor position + device status reports.
        anchored(r"\A\x1b\[\??[0-9;]*[Rn]\z"),
        // DA1/DA2/DA3 device attributes.
        anchored(r"\A\x1b\[[?>=]?[0-9;]*c\z"),
        // Window/cell pixel-size + text-area-size reports (CSI 4/6/8 … t).
        anchored(r"\A\x1b\[[468];[0-9]+;[0-9]+t\z"),
        // DECRPM mode report (`$y`), private (`?`) or ANSI.
        anchored(r"\A\x1b\[\??[0-9;]*\$y\z"),
        // Kitty keyboard flags report (CSI ? flags u).
        anchored(r"\A\x1b\[\?[0-9]+u\z"),
        // OSC color/title responses: ESC ] Ps ; body (BEL | ESC \).
        anchored(r"\A\x1b\][0-9]+;[^\x07\x1b]*(?:\x07|\x1b\\)\z"),
        // DCS-framed DECRQSS (`ESC P [01] $ r … ST`) / XTVERSION (`ESC P > | … ST`).
        anchored(r"\A\x1bP(?:[01]\$r[^\x1b]*|>\|[^\x1b]*)\x1b\\\z"),
    ]
});

/// True when `data` is a synthetic query reply that must bypass input
/// coalescing. Conservative: only complete, well-formed reply grammars match,
/// so ordinary keystrokes/navigation are never misclassified (with the single
/// documented modified-F3/CPR collision).
pub fn is_terminal_query_reply(data: &[u8]) -> bool {
    if data.len() < 3 || data[0] != ESC {
        return false;
    }
    QUERY_REPLY_RES.iter().any(|re| re.is_match(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- isTerminalQueryReply oracle (terminal-query-reply.test.ts) ---

    #[test]
    fn matches_synthetic_query_replies_that_must_be_sent_immediately() {
        // The full 20-vector "true" set from the oracle.
        let truthy: &[&[u8]] = &[
            b"\x1b[3;1R",    // CPR
            b"\x1b[22;1R",   // CPR
            b"\x1b[0n",      // DSR
            b"\x1b[?1;2c",   // DA
            b"\x1b[?61;4c",  // DA
            b"\x1b[>0;276;0c", // DA
            b"\x1b[6;16;8t",   // window/cell pixel size
            b"\x1b[4;384;640t", // window/cell pixel size
            b"\x1b[?2026;2$y", // DECRPM private
            b"\x1b[4;1$y",     // DECRPM ANSI
            b"\x1b]11;rgb:2828/2c2c/3434\x1b\\", // OSC color (ST)
            b"\x1b]10;rgb:c0c0/c0c0/c0c0\x07",   // OSC color (BEL)
            b"\x1b[?12;5R", // DECXCPR
            b"\x1b[8;24;80t", // text-area size
            b"\x1b[?0u",  // kitty flags
            b"\x1b[?31u", // kitty flags
            b"\x1bP1$r2 q\x1b\\", // DCS DECRQSS
            b"\x1bP1$r0m\x1b\\",  // DCS DECRQSS
            b"\x1bP0$r\x1b\\",    // DCS DECRQSS
            b"\x1bP>|xterm.js(5.6.0)\x1b\\", // DCS XTVERSION
        ];
        assert_eq!(truthy.len(), 20);
        for v in truthy {
            assert!(is_terminal_query_reply(v), "expected reply: {v:?}");
        }
    }

    #[test]
    fn documents_the_accepted_modified_f3_cpr_collision() {
        // Shift+F3 (CSI 1;2R) is byte-identical to a CPR report — accepted.
        assert!(is_terminal_query_reply(b"\x1b[1;2R"));
    }

    #[test]
    fn does_not_match_ordinary_typed_input_or_navigation_sequences() {
        let falsy: &[&[u8]] = &[
            b"yes",
            b"y",
            b"\r",
            b"\x03", // Ctrl-C
            b"\x1b[A",
            b"\x1b[B",
            b"\x1b[C",
            b"\x1b[D",
            b"\x1b[H", // Home
            b"\x1b[F", // End
            b"\x1b[15~",
            b"\x1b[3~", // Delete
            b"\x1b",    // bare Escape
            b"\x1bb",   // Alt+b
            b"\x1bP",   // Alt+Shift+P (prefix of DCS grammar)
            b"\x1b[97;5u", // kitty keystroke
            b"\x1b[13u",   // kitty keystroke
            b"\x1b[1;2P",  // modified F1
            b"\x1b[1;2Q",  // modified F2
            b"\x1b[1;2S",  // modified F4
            b"\x1b[200~",  // bracketed paste start
            b"\x1b[201~",  // bracketed paste end
            b"\x1b]11;rgb:2828/2c2c/3434", // incomplete OSC (no terminator)
            b"\x1bP1$r2 q",                // incomplete DCS (no terminator)
        ];
        assert_eq!(falsy.len(), 24);
        for v in falsy {
            assert!(!is_terminal_query_reply(v), "expected NOT reply: {v:?}");
        }
    }

    // --- C1: byte-native, no panic on non-ASCII / invalid UTF-8 ---

    #[test]
    fn c1_non_ascii_and_invalid_utf8_do_not_panic_and_do_not_match() {
        // An OSC title with multibyte UTF-8 payload is well-formed → matches.
        assert!(is_terminal_query_reply("\x1b]0;한국어\x07".as_bytes()));
        // Invalid UTF-8 bytes inside an OSC body still classify without panic.
        assert!(is_terminal_query_reply(b"\x1b]0;\xff\xfe\x07"));
        // A lone invalid byte is not a reply.
        assert!(!is_terminal_query_reply(b"\xff\xfe\xfd"));
    }

    #[test]
    fn c1_anchors_reject_trailing_garbage() {
        // `\z` must reject anything after the terminator (no multiline `$`).
        assert!(!is_terminal_query_reply(b"\x1b[3;1RX"));
        assert!(!is_terminal_query_reply(b"\x1b]10;?\x07extra"));
    }
}
