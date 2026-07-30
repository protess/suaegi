//! Port of Orca `shared/terminal-osc-color-reply.ts` (@ v1.4.150-rc.0).
//!
//! OSC 10/11 (foreground/background) color-query parsing and reply building for
//! the transport/daemon path. Orca operates on JS UTF-16 strings; this port is
//! byte-native (`&[u8]`) — every escape grammar it inspects is ASCII framing, so
//! byte translation is behaviorally equivalent AND panic-proof (a `&str[a..b]`
//! at a multibyte boundary would panic; `&[u8]` slicing never does). See
//! `docs/superpowers/plans/2026-07-25-terminal-query-reply.md` (C1–C8).
//!
//! `css_color_to_osc_rgb` is the one function that takes a decoded config
//! string (a CSS color from app config, always valid UTF-8) rather than PTY
//! bytes, so it stays on `&str`.

/// OSC introducer `ESC ]`.
const OSC: &[u8] = b"\x1b]";
/// BEL (0x07) — one-byte OSC string terminator.
const BEL: u8 = 0x07;
/// ST `ESC \` — two-byte OSC string terminator.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// Colors the emulator can answer an OSC 10/11 query with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalOscColorQueryReplyColors {
    pub foreground: Option<String>,
    pub background: Option<String>,
}

/// The two OSC color-query slots Orca answers: 10 (foreground), 11 (background).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOscColorQuerySlot {
    /// OSC 10 — foreground.
    Ten,
    /// OSC 11 — background.
    Eleven,
}

impl TerminalOscColorQuerySlot {
    /// The numeric OSC code (10 or 11).
    pub fn number(self) -> u8 {
        match self {
            TerminalOscColorQuerySlot::Ten => 10,
            TerminalOscColorQuerySlot::Eleven => 11,
        }
    }
}

use TerminalOscColorQuerySlot::{Eleven, Ten};

const SLOTS_10: &[TerminalOscColorQuerySlot] = &[Ten];
const SLOTS_10_11: &[TerminalOscColorQuerySlot] = &[Ten, Eleven];
const SLOTS_11: &[TerminalOscColorQuerySlot] = &[Eleven];

/// Result of scanning for an OSC 10/11 color *query* at a byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOscColorQueryParseResult {
    /// A complete, well-formed query. `end_index` is the EXCLUSIVE byte end.
    Match {
        slots: &'static [TerminalOscColorQuerySlot],
        end_index: usize,
    },
    /// A well-formed prefix that needs more bytes (e.g. a split ST terminator).
    Partial,
    /// Not an OSC color query.
    None,
}

/// Internal result of parsing an OSC string terminator.
#[derive(Clone, Debug, PartialEq, Eq)]
enum TerminatorParseResult {
    /// Terminator present; `end_index` is the EXCLUSIVE byte end.
    Complete { end_index: usize },
    /// Terminator not yet fully present (split ST).
    Partial,
    /// No terminator here.
    None,
}

// ---- JS whitespace (for `css_color_to_osc_rgb`'s `.trim()` / `\s`) ----
// ECMAScript WhiteSpace + LineTerminator (excludes U+0085 / U+180E). Diverges
// from Rust `char::is_whitespace()`, so reproduce it exactly. CSS color config
// realistically only holds ASCII whitespace, but faithfulness is cheap here.
fn is_js_whitespace(c: char) -> bool {
    matches!(c,
        '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{00A0}' | '\u{1680}'
        | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
        | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// Convert a CSS color (`#rgb`, `#rrggbb`, `rgb()`, `rgba()`) to the xterm
/// `rgb:RRRR/GGGG/BBBB` OSC reply body. Returns `None` for empty/unparseable
/// input (mirrors JS string-truthiness: `undefined` and `""` both fail).
pub fn css_color_to_osc_rgb(value: Option<&str>) -> Option<String> {
    let value = value.filter(|s| !s.is_empty())?;
    let trimmed = js_trim(value);

    // Hex: `^#([0-9a-f]{3}|[0-9a-f]{6})$` case-insensitive. Case is PRESERVED in
    // the output (Orca never lowercases the captured hex).
    if let Some(hex) = parse_hex_body(trimmed) {
        let expanded: String = if hex.len() == 3 {
            hex.chars().flat_map(|c| [c, c]).collect()
        } else {
            hex.to_string()
        };
        let b = expanded.as_bytes();
        return Some(format!(
            "rgb:{}/{}/{}",
            byte_hex_to_word(&b[0..2]),
            byte_hex_to_word(&b[2..4]),
            byte_hex_to_word(&b[4..6]),
        ));
    }

    // `^rgba?\(\s*([^)]+)\)$` case-insensitive.
    let body = parse_rgb_body(trimmed)?;
    let [red, green, blue] = parse_css_rgb_channels(body)?;
    Some(format!(
        "rgb:{}/{}/{}",
        rgb_channel_to_word(red),
        rgb_channel_to_word(green),
        rgb_channel_to_word(blue),
    ))
}

/// `byteHexToWord`: a 2-hex-digit byte repeated twice → a 4-hex-digit word.
fn byte_hex_to_word(byte: &[u8]) -> String {
    // `byte` is exactly two ASCII hex chars; repeat verbatim (case preserved).
    let s = std::str::from_utf8(byte).unwrap_or("");
    format!("{s}{s}")
}

/// A channel value (0..=255) formatted as `{:02x}` and repeated twice.
fn rgb_channel_to_word(byte: u8) -> String {
    let hex = format!("{byte:02x}");
    format!("{hex}{hex}")
}

/// Match `^#([0-9a-f]{3}|[0-9a-f]{6})$` (ASCII, case-insensitive); return the
/// captured hex (original case), or `None`.
fn parse_hex_body(s: &str) -> Option<&str> {
    let hex = s.strip_prefix('#')?;
    let len_ok = hex.len() == 3 || hex.len() == 6;
    if len_ok && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(hex)
    } else {
        None
    }
}

/// Match `^rgba?\(\s*([^)]+)\)$` (ASCII case-insensitive prefix); return the
/// captured body (leading JS-whitespace stripped, as `\s*` consumes it).
fn parse_rgb_body(s: &str) -> Option<&str> {
    // Prefix is case-insensitive; the body is digits/punct so case is moot.
    let lower_prefix_len = if starts_with_ascii_ci(s, "rgba(") {
        5
    } else if starts_with_ascii_ci(s, "rgb(") {
        4
    } else {
        return None;
    };
    let rest = &s[lower_prefix_len..];
    let inner = rest.strip_suffix(')')?;
    if inner.contains(')') {
        return None; // `[^)]+` forbids an interior ')'.
    }
    let body = inner.trim_start_matches(is_js_whitespace); // `\s*`
    if body.is_empty() {
        return None; // `[^)]+` requires ≥1 char.
    }
    Some(body)
}

fn starts_with_ascii_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// `parseCssRgbChannels`: first `/`-segment, then 3 comma- or whitespace-
/// separated channels.
fn parse_css_rgb_channels(body: &str) -> Option<[u8; 3]> {
    let color_part = js_trim(body.split('/').next().unwrap_or(""));
    if color_part.is_empty() {
        return None;
    }
    let components: Vec<&str> = if color_part.contains(',') {
        // Comma path keeps empty segments (JS `split(',')`), then `slice(0,3)`.
        color_part.split(',').take(3).collect()
    } else {
        // Whitespace path is `split(/\s+/)`; on trimmed input this is runs of
        // JS-whitespace with no empty edges — filter empties to mimic `+`.
        color_part
            .split(is_js_whitespace)
            .filter(|c| !c.is_empty())
            .take(3)
            .collect()
    };
    if components.len() != 3 {
        return None;
    }
    let mut out = [0u8; 3];
    for (slot, component) in out.iter_mut().zip(components) {
        *slot = parse_css_rgb_channel(js_trim(component))?;
    }
    Some(out)
}

/// `parseCssRgbChannel`: `NN%` (of 255) or plain `NN`, ASCII decimal only.
fn parse_css_rgb_channel(component: &str) -> Option<u8> {
    match component.strip_suffix('%') {
        // `^([0-9]+(?:\.[0-9]+)?)%$`
        Some(pct) if is_ascii_decimal(pct) => {
            let v: f64 = pct.parse().ok()?;
            Some(clamp_byte(v / 100.0 * 255.0))
        }
        // Ends with `%` but not a decimal percent → no match.
        Some(_) => None,
        // `^[0-9]+(?:\.[0-9]+)?$`
        None => {
            if is_ascii_decimal(component) {
                let v: f64 = component.parse().ok()?;
                Some(clamp_byte(v))
            } else {
                None
            }
        }
    }
}

/// Match `^[0-9]+(?:\.[0-9]+)?$` — ASCII digits only (JS `\d` is ASCII; Rust
/// `\d` would be Unicode Nd, so we hand-roll to keep it ASCII).
fn is_ascii_decimal(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    let int_start = i;
    while i < n && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == int_start {
        return false; // need ≥1 integer digit
    }
    if i < n && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return false; // need ≥1 fractional digit
        }
    }
    i == n
}

/// `clampByte`: `min(255, max(0, round(v)))`. JS `Math.round` rounds half toward
/// +∞; for the nonnegative values here that equals Rust `f64::round` (half away
/// from zero).
fn clamp_byte(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// Build the OSC reply for one slot, or `None` if that slot's color is unset/
/// unparseable. Always terminated with ST (`ESC \`).
pub fn terminal_osc_color_query_reply(
    colors: &TerminalOscColorQueryReplyColors,
    slot: TerminalOscColorQuerySlot,
) -> Option<String> {
    let color = match slot {
        Ten => css_color_to_osc_rgb(colors.foreground.as_deref()),
        Eleven => css_color_to_osc_rgb(colors.background.as_deref()),
    }?;
    Some(format!("\x1b]{};{}\x1b\\", slot.number(), color))
}

/// Build replies for every requested slot; `None` if ANY slot is unanswerable
/// (mirrors Orca's `replies.every(isNotNull)`).
pub fn terminal_osc_color_query_replies(
    colors: &TerminalOscColorQueryReplyColors,
    slots: &[TerminalOscColorQuerySlot],
) -> Option<Vec<String>> {
    slots
        .iter()
        .map(|&slot| terminal_osc_color_query_reply(colors, slot))
        .collect()
}

/// Which slots a given query body (`?` or `?;?`) requests for a given slot.
pub fn terminal_osc_color_query_slots_for_body(
    slot: TerminalOscColorQuerySlot,
    body: &[u8],
) -> Option<&'static [TerminalOscColorQuerySlot]> {
    match slot {
        Ten => match body {
            b"?" => Some(SLOTS_10),
            b"?;?" => Some(SLOTS_10_11),
            _ => None,
        },
        Eleven => match body {
            b"?" => Some(SLOTS_11),
            _ => None,
        },
    }
}

fn parse_terminal_osc_terminator(data: &[u8], offset: usize) -> TerminatorParseResult {
    if offset >= data.len() {
        return TerminatorParseResult::Partial;
    }
    if data[offset] == BEL {
        return TerminatorParseResult::Complete {
            end_index: offset + 1,
        };
    }
    if data[offset..].starts_with(STRING_TERMINATOR) {
        return TerminatorParseResult::Complete {
            end_index: offset + STRING_TERMINATOR.len(),
        };
    }
    if data[offset] == 0x1b && offset + 1 >= data.len() {
        // Why: streamed PTY chunks can split the ST terminator between ESC and \.
        return TerminatorParseResult::Partial;
    }
    TerminatorParseResult::None
}

fn complete_terminal_osc_color_query(
    slot: TerminalOscColorQuerySlot,
    body: &[u8],
    terminator: TerminatorParseResult,
) -> TerminalOscColorQueryParseResult {
    let end_index = match terminator {
        TerminatorParseResult::Complete { end_index } => end_index,
        TerminatorParseResult::Partial => return TerminalOscColorQueryParseResult::Partial,
        TerminatorParseResult::None => return TerminalOscColorQueryParseResult::None,
    };
    match terminal_osc_color_query_slots_for_body(slot, body) {
        Some(slots) => TerminalOscColorQueryParseResult::Match { slots, end_index },
        None => TerminalOscColorQueryParseResult::None,
    }
}

fn parse_terminal_osc_color_query_body(
    data: &[u8],
    body_start: usize,
    slot: TerminalOscColorQuerySlot,
) -> TerminalOscColorQueryParseResult {
    if body_start >= data.len() {
        return TerminalOscColorQueryParseResult::Partial;
    }
    if data[body_start] != b'?' {
        return TerminalOscColorQueryParseResult::None;
    }
    let single = parse_terminal_osc_terminator(data, body_start + 1);
    if single != TerminatorParseResult::None {
        return complete_terminal_osc_color_query(slot, b"?", single);
    }
    // Combined `?;?` is only valid for slot 10.
    if slot != Ten || data.get(body_start + 1) != Some(&b';') {
        return TerminalOscColorQueryParseResult::None;
    }
    if body_start + 2 >= data.len() {
        return TerminalOscColorQueryParseResult::Partial;
    }
    if data[body_start + 2] != b'?' {
        return TerminalOscColorQueryParseResult::None;
    }
    complete_terminal_osc_color_query(
        slot,
        b"?;?",
        parse_terminal_osc_terminator(data, body_start + 3),
    )
}

/// Parse a potential OSC 10/11 color query starting at `offset`.
pub fn parse_terminal_osc_color_query(
    data: &[u8],
    offset: usize,
) -> TerminalOscColorQueryParseResult {
    const PREFIXES: [(TerminalOscColorQuerySlot, &[u8]); 2] =
        [(Ten, b"\x1b]10;"), (Eleven, b"\x1b]11;")];
    let tail = data.get(offset..).unwrap_or(&[]);
    let entry = PREFIXES.iter().find(|(_, prefix)| tail.starts_with(prefix));
    let (slot, prefix) = match entry {
        Some(&(slot, prefix)) => (slot, prefix),
        None => {
            // Is the tail a prefix of a known query prefix (mid-prefix split)?
            return if PREFIXES.iter().any(|(_, prefix)| prefix.starts_with(tail)) {
                TerminalOscColorQueryParseResult::Partial
            } else {
                TerminalOscColorQueryParseResult::None
            };
        }
    };
    let body_start = offset + prefix.len();
    parse_terminal_osc_color_query_body(data, body_start, slot)
}

/// Scan `data` for OSC color queries and hand each built reply to `sink`.
/// Returns whether any reply was sent. `sink` is the injected effect (Orca's
/// `sendInput` callback) — the rest of this module is pure.
pub fn send_terminal_osc_color_query_replies(
    data: &[u8],
    colors: &TerminalOscColorQueryReplyColors,
    sink: &mut dyn FnMut(&[u8]),
) -> bool {
    let mut sent = false;
    let mut offset = 0;
    while offset < data.len() {
        let osc_index = match find_subslice(data, OSC, offset) {
            Some(i) => i,
            None => break,
        };
        match parse_terminal_osc_color_query(data, osc_index) {
            TerminalOscColorQueryParseResult::Match { slots, end_index } => {
                if let Some(replies) = terminal_osc_color_query_replies(colors, slots) {
                    for reply in replies {
                        sink(reply.as_bytes());
                    }
                    sent = true;
                }
                offset = end_index;
            }
            TerminalOscColorQueryParseResult::Partial => break,
            TerminalOscColorQueryParseResult::None => offset = osc_index + OSC.len(),
        }
    }
    sent
}

/// Find `needle` in `hay` at or after byte index `from`.
pub(crate) fn find_subslice(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(data: &[u8]) -> TerminalOscColorQueryParseResult {
        parse_terminal_osc_color_query(data, 0)
    }

    // --- parseTerminalOscColorQuery oracle (terminal-osc-color-reply.test.ts) ---

    #[test]
    fn matches_exact_osc_color_queries_terminated_by_st_or_bel() {
        let fg = b"\x1b]10;?\x1b\\";
        let bg = b"\x1b]11;?\x07";
        assert_eq!(
            q(fg),
            TerminalOscColorQueryParseResult::Match {
                slots: SLOTS_10,
                end_index: fg.len()
            }
        );
        assert_eq!(
            q(bg),
            TerminalOscColorQueryParseResult::Match {
                slots: SLOTS_11,
                end_index: bg.len()
            }
        );
    }

    #[test]
    fn matches_complete_combined_foreground_and_background_queries() {
        let query = b"\x1b]10;?;?\x1b\\";
        assert_eq!(
            q(query),
            TerminalOscColorQueryParseResult::Match {
                slots: SLOTS_10_11,
                end_index: query.len()
            }
        );
    }

    #[test]
    fn keeps_a_split_st_terminator_pending() {
        assert_eq!(
            q(b"\x1b]10;?\x1b"),
            TerminalOscColorQueryParseResult::Partial
        );
        assert_eq!(
            q(b"\x1b]10;?;?\x1b"),
            TerminalOscColorQueryParseResult::Partial
        );
    }

    #[test]
    fn rejects_osc_color_commands_that_only_start_like_queries() {
        assert_eq!(
            q(b"\x1b]10;?not-a-query\x1b\\"),
            TerminalOscColorQueryParseResult::None
        );
        assert_eq!(q(b"\x1b]11;?\x1bX"), TerminalOscColorQueryParseResult::None);
        assert_eq!(
            q(b"\x1b]10;?;#123456\x1b\\"),
            TerminalOscColorQueryParseResult::None
        );
        assert_eq!(
            q(b"\x1b]10;?;?;?\x1b\\"),
            TerminalOscColorQueryParseResult::None
        );
        assert_eq!(
            q(b"\x1b]11;?;?\x1b\\"),
            TerminalOscColorQueryParseResult::None
        );
    }

    #[test]
    fn rejects_unsupported_query_shaped_bodies_without_waiting_for_a_terminator() {
        let mut data = b"\x1b]10;?".to_vec();
        data.extend(std::iter::repeat_n(b'x', 10_000));
        assert_eq!(
            parse_terminal_osc_color_query(&data, 0),
            TerminalOscColorQueryParseResult::None
        );
    }

    // --- C1: byte-boundary panic-safety (no oracle coverage) ---

    #[test]
    fn c1_non_ascii_payload_never_panics() {
        // Multibyte UTF-8 in an OSC body; byte slicing must not panic.
        let data = "\x1b]10;?한국어\x1b\\".as_bytes();
        assert_eq!(q(data), TerminalOscColorQueryParseResult::None);
        // Fragment that splits a query prefix mid-way is Partial, not a panic.
        assert_eq!(q(b"\x1b]1"), TerminalOscColorQueryParseResult::Partial);
    }

    // --- C2: BEL vs ST terminator end offsets ---

    #[test]
    fn c2_bel_end_is_plus_one_st_end_is_plus_two() {
        // BEL: `\x1b]11;?\x07` len 7, exclusive end == 7.
        match q(b"\x1b]11;?\x07") {
            TerminalOscColorQueryParseResult::Match { end_index, .. } => assert_eq!(end_index, 7),
            other => panic!("expected match, got {other:?}"),
        }
        // ST: `\x1b]10;?\x1b\\` len 8, exclusive end == 8 (two-byte terminator).
        match q(b"\x1b]10;?\x1b\\") {
            TerminalOscColorQueryParseResult::Match { end_index, .. } => assert_eq!(end_index, 8),
            other => panic!("expected match, got {other:?}"),
        }
    }

    // --- C3: css_color_to_osc_rgb (no oracle coverage) ---

    #[test]
    fn c3_hex_short_and_long_preserve_case() {
        assert_eq!(
            css_color_to_osc_rgb(Some("#abc")).as_deref(),
            Some("rgb:aaaa/bbbb/cccc")
        );
        assert_eq!(
            css_color_to_osc_rgb(Some("#aabbcc")).as_deref(),
            Some("rgb:aaaa/bbbb/cccc")
        );
        assert_eq!(
            css_color_to_osc_rgb(Some("#ABC")).as_deref(),
            Some("rgb:AAAA/BBBB/CCCC")
        );
    }

    #[test]
    fn c3_rgb_decimal_channels() {
        // 40=0x28, 44=0x2c, 52=0x34 → the color used in the query-reply oracle.
        assert_eq!(
            css_color_to_osc_rgb(Some("rgb(40, 44, 52)")).as_deref(),
            Some("rgb:2828/2c2c/3434")
        );
        assert_eq!(
            css_color_to_osc_rgb(Some("rgba(40,44,52,1)")).as_deref(),
            Some("rgb:2828/2c2c/3434")
        );
    }

    #[test]
    fn c3_percent_and_half_rounding() {
        // 100%→255=ff, 0%→0, 50%→127.5→round 128=0x80.
        assert_eq!(
            css_color_to_osc_rgb(Some("rgb(100%, 0%, 50%)")).as_deref(),
            Some("rgb:ffff/0000/8080")
        );
        // .5 halves round half-up (toward +∞), matching JS Math.round.
        assert_eq!(
            css_color_to_osc_rgb(Some("rgb(0.5, 1.5, 2.5)")).as_deref(),
            Some("rgb:0101/0202/0303")
        );
    }

    #[test]
    fn c3_over_range_clamps_to_255() {
        assert_eq!(
            css_color_to_osc_rgb(Some("rgb(300, 0, 0)")).as_deref(),
            Some("rgb:ffff/0000/0000")
        );
    }

    #[test]
    fn c3_trim_uses_js_whitespace() {
        assert_eq!(
            css_color_to_osc_rgb(Some("  #abc  ")).as_deref(),
            Some("rgb:aaaa/bbbb/cccc")
        );
        // U+00A0 (JS whitespace, NOT ascii) must also be trimmed.
        assert_eq!(
            css_color_to_osc_rgb(Some("\u{a0}#abc\u{a0}")).as_deref(),
            Some("rgb:aaaa/bbbb/cccc")
        );
    }

    #[test]
    fn c3_empty_and_unparseable_and_negative_are_none() {
        assert_eq!(css_color_to_osc_rgb(None), None);
        assert_eq!(css_color_to_osc_rgb(Some("")), None);
        assert_eq!(css_color_to_osc_rgb(Some("notacolor")), None);
        assert_eq!(css_color_to_osc_rgb(Some("#12")), None); // wrong hex length
        assert_eq!(css_color_to_osc_rgb(Some("rgb(-5, 0, 0)")), None); // negative
        assert_eq!(css_color_to_osc_rgb(Some("rgb(1, 2)")), None); // too few channels
        assert_eq!(css_color_to_osc_rgb(Some("rgb()")), None); // empty body
    }

    // --- reply builders + slots-for-body ---

    #[test]
    fn builds_replies_with_st_terminator() {
        let colors = TerminalOscColorQueryReplyColors {
            foreground: Some("#abc".into()),
            background: Some("rgb(40,44,52)".into()),
        };
        assert_eq!(
            terminal_osc_color_query_reply(&colors, Ten).as_deref(),
            Some("\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\")
        );
        assert_eq!(
            terminal_osc_color_query_reply(&colors, Eleven).as_deref(),
            Some("\x1b]11;rgb:2828/2c2c/3434\x1b\\")
        );
    }

    #[test]
    fn replies_is_none_if_any_slot_unanswerable() {
        let colors = TerminalOscColorQueryReplyColors {
            foreground: Some("#abc".into()),
            background: None,
        };
        assert_eq!(
            terminal_osc_color_query_replies(&colors, SLOTS_10)
                .as_deref()
                .map(|v| v.to_vec()),
            Some(vec!["\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_string()])
        );
        assert_eq!(terminal_osc_color_query_replies(&colors, SLOTS_10_11), None);
    }

    #[test]
    fn slots_for_body_maps_bodies() {
        assert_eq!(
            terminal_osc_color_query_slots_for_body(Ten, b"?"),
            Some(SLOTS_10)
        );
        assert_eq!(
            terminal_osc_color_query_slots_for_body(Ten, b"?;?"),
            Some(SLOTS_10_11)
        );
        assert_eq!(
            terminal_osc_color_query_slots_for_body(Eleven, b"?"),
            Some(SLOTS_11)
        );
        // Combined body invalid for slot 11.
        assert_eq!(
            terminal_osc_color_query_slots_for_body(Eleven, b"?;?"),
            None
        );
    }

    // --- C8: send helper drives the injected sink ---

    #[test]
    fn c8_send_replies_invokes_sink_and_advances() {
        let colors = TerminalOscColorQueryReplyColors {
            foreground: Some("#abc".into()),
            background: Some("#def".into()),
        };
        let mut out: Vec<Vec<u8>> = Vec::new();
        // Two back-to-back queries: fg (?) then combined (?;?).
        let data = b"\x1b]10;?\x1b\\\x1b]10;?;?\x07";
        let sent = send_terminal_osc_color_query_replies(data, &colors, &mut |r| {
            out.push(r.to_vec());
        });
        assert!(sent);
        assert_eq!(
            out,
            vec![
                b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]11;rgb:dddd/eeee/ffff\x1b\\".to_vec(),
            ]
        );
    }

    #[test]
    fn c8_send_replies_false_when_no_query() {
        let colors = TerminalOscColorQueryReplyColors::default();
        let mut count = 0;
        let sent = send_terminal_osc_color_query_replies(b"plain text", &colors, &mut |_| {
            count += 1;
        });
        assert!(!sent);
        assert_eq!(count, 0);
    }
}
