//! Port of Orca `shared/terminal-view-attributes.ts` (@ v1.4.150-rc.0).
//!
//! Payload contract for the renderer→main `pty:terminalViewAttributes` push,
//! plus main/renderer mirrors of xterm's `XParseColor` color-spec grammar so
//! main's responder replies byte-identically to a visible renderer xterm.
//!
//! This module mirrors **xterm's `XParseColor`** grammar (`rgb:h/h/h`..
//! `rgb:hhhh/hhhh/hhhh` and `#RGB|#RRGGBB|#RRRGGGBBB|#RRRRGGGGBBBB`), which is
//! a DIFFERENT grammar from the CSS-color parser in
//! `reply_query::osc_color_reply::css_color_to_osc_rgb`. The two disagree on
//! `#RGB`: this module's `#abc` truncates via a shift (`0xa0/0xb0/0xc0`) while
//! the CSS module duplicates the nibble (`0xaa/0xbb/0xcc`). This module also
//! does NOT trim input, unlike the CSS module's `js_trim`. Do not reuse
//! parsing helpers between the two — see
//! `docs/superpowers/plans/2026-07-26-terminal-view-attributes.md` (R1-R10).

/// 8-bit-per-channel RGB triple — the same resolution xterm's theme service
/// stores internally (`color.toColorRGB`).
pub type TerminalViewRgb = [u8; 3];

/// Full ANSI palette size: theme's 16 named colors + extendedAnsi/default
/// tail, exactly as the renderer ThemeService resolves them.
pub const TERMINAL_VIEW_ANSI_COLOR_COUNT: usize = 256;

/// Terminal cursor rendering style. An INDEPENDENT 3-variant enum — do not
/// map to alacritty's 5-variant `CursorShape` (`grid.rs`): the value sets
/// differ, and `HollowBlock`/`Hidden` have no destination in this contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalViewCursorStyle {
    Bar,
    Block,
    Underline,
}

/// Resolved APP color-scheme mode (the 2031/997 flip source). NOT the DSR
/// `?996n` answer: that is computed from background/foreground relative
/// luminance like a visible xterm, and the two can disagree (e.g. dark
/// terminal theme in light app mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalViewColorSchemeMode {
    Dark,
    Light,
}

/// One app-global snapshot of the renderer's composed terminal appearance —
/// per-pane font zoom never affects these, and terminalColorOverrides /
/// cursor settings are global, so one push covers all PTYs.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalViewAttributes {
    pub foreground: TerminalViewRgb,
    pub background: TerminalViewRgb,
    /// Already blended over the background (xterm ThemeService blends the
    /// cursor color's alpha at theme-set time, e.g. terminalCursorOpacity).
    pub cursor: TerminalViewRgb,
    /// Fixed at 256 (`TERMINAL_VIEW_ANSI_COLOR_COUNT`); the length check
    /// lives in `validate_terminal_view_attributes`, so a short palette is
    /// rejected there rather than representable here.
    pub ansi: [TerminalViewRgb; TERMINAL_VIEW_ANSI_COLOR_COUNT],
    pub color_scheme_mode: TerminalViewColorSchemeMode,
    pub cursor_style: TerminalViewCursorStyle,
    pub cursor_blink: bool,
}

// ---- parse_x_color_spec ----
//
// R1: hand-rolled, no `regex` crate. JS `\d` is ASCII-only; Rust's `\d` in the
// `regex` crate is Unicode `Nd` (e.g. Arabic-Indic digits), so a literal
// transliteration of the source's `X_RGB_SPEC_RE`/`X_HASH_SPEC_RE` would
// accept strings the real xterm.js rejects. The four `rgb:` widths are
// mutually exclusive by digit count, so a length match + `is_ascii_hexdigit`
// check is a direct, safe hand-port (house precedent:
// `reply_query::osc_color_reply` hand-rolls for the same reason).

fn is_ascii_hex_body(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `rgb:h/h/h` .. `rgb:hhhh/hhhh/hhhh` (all three channels the SAME digit
/// count — R4: xterm.js's alternation is width-anchored per branch, so
/// `rgb:f/ff/fff` is rejected even though real X11 `XParseColor` allows mixed
/// widths). R6: participating channels are told apart by `Option`, matching
/// the source's JS-truthiness capture-group dispatch (a captured `"0"` is
/// truthy) without ever converting to a number first — `rgb:0/8/f` must
/// still yield channel 0.
fn parse_rgb_colon_spec(rest: &str) -> Option<TerminalViewRgb> {
    let mut parts = rest.split('/');
    let r = parts.next()?;
    let g = parts.next()?;
    let b = parts.next()?;
    if parts.next().is_some() {
        return None; // more than 3 `/`-separated segments
    }
    let len = r.len();
    if g.len() != len || b.len() != len {
        return None; // R4: widths must match across all three channels
    }
    if !is_ascii_hex_body(r) || !is_ascii_hex_body(g) || !is_ascii_hex_body(b) {
        return None;
    }
    // R3 (rgb: path): base scales by digit count, then round(v / base * 255).
    // f64::round() is half-away-from-zero; JS Math.round is half-toward-+inf.
    // These agree here because an exact `.5` tie is mathematically impossible
    // for these bases (15/255/4095/65535 all divide 255 exactly or produce a
    // ratio whose halfway point never lands on x.5 — see the plan's R3 note).
    let base: u32 = match len {
        1 => 15,
        2 => 255,
        3 => 4095,
        4 => 65535,
        _ => return None,
    };
    let scale = |channel: &str| -> Option<u8> {
        let v = u32::from_str_radix(channel, 16).ok()?;
        Some(((v as f64 / base as f64) * 255.0).round() as u8)
    };
    Some([scale(r)?, scale(g)?, scale(b)?])
}

/// `#RGB | #RRGGBB | #RRRGGGBBB | #RRRRGGGGBBBB`. R5: the hex-body check and
/// the length gate are SEPARATE conditions (both required) — do not merge
/// them into one, since `#abcd` (4 hex digits) is all-hex but length 4 is not
/// in `{3,6,9,12}`, and must still be rejected.
fn parse_hash_spec(rest: &str) -> Option<TerminalViewRgb> {
    let hex_ok = is_ascii_hex_body(rest);
    let len = rest.len();
    if !(hex_ok && matches!(len, 3 | 6 | 9 | 12)) {
        return None;
    }
    // `rest` is verified all-ASCII-hex above, so byte slicing here never
    // crosses a char boundary.
    let bytes = rest.as_bytes();
    let adv = len / 3;
    let mut out: TerminalViewRgb = [0, 0, 0];
    for (i, slot) in out.iter_mut().enumerate() {
        let chunk = &bytes[adv * i..adv * i + adv];
        // SAFETY-free: chunk is a sub-slice of an all-ASCII-hex byte string.
        let text = std::str::from_utf8(chunk).ok()?;
        let c = u32::from_str_radix(text, 16).ok()?;
        // R3 (# path): shifts (truncation), NOT the rgb: path's round/scale.
        // This is the deliberate xterm.js asymmetry: `#fff` -> 240 while
        // `rgb:f/f/f` -> 255. Do not unify the two paths.
        *slot = match adv {
            1 => (c << 4) as u8,
            2 => c as u8,
            3 => (c >> 4) as u8,
            4 => (c >> 8) as u8,
            _ => return None,
        };
    }
    Some(out)
}

/// Mirror of xterm's `XParseColor` `parseColor` (the grammar the renderer
/// accepts for OSC 4/10/11/12 SET payloads): `rgb:h/h/h`..`rgb:hhhh/hhhh/hhhh`
/// and `#RGB|#RRGGBB|#RRRGGGBBB|#RRRRGGGGBBBB`. Anything else (named colors,
/// `rgbi:`) is rejected exactly like the renderer rejects it.
///
/// R7: this module does NOT trim input — `" rgb:f/f/f"` (leading space) is
/// rejected, unlike the neighbouring CSS module's `css_color_to_osc_rgb`
/// (which does trim). R8: case folding uses `to_lowercase()` (full Unicode),
/// because the source calls `String.prototype.toLowerCase()` and its regexes
/// have no `/i` flag; `to_ascii_lowercase()` would be observationally
/// equivalent here (no non-ASCII character folds into `a-f`/`0-9`), but the
/// literal contract is kept.
pub fn parse_x_color_spec(spec: &str) -> Option<TerminalViewRgb> {
    if spec.is_empty() {
        return None;
    }
    let low = spec.to_lowercase();
    if let Some(rest) = low.strip_prefix("rgb:") {
        return parse_rgb_colon_spec(rest);
    }
    if let Some(rest) = low.strip_prefix('#') {
        return parse_hash_spec(rest);
    }
    None
}

/// A single 4-hex-digit channel word: the 8-bit byte's `{:02x}` rendering,
/// doubled (xterm reports 16-bit channels by repeating the 8-bit byte —
/// `XParseColor.toRgbString` with `bits=16`).
///
/// R9: this ~4-line formatter is DUPLICATED here rather than shared with
/// `reply_query::osc_color_reply::rgb_channel_to_word` (which is byte-
/// identical today). Byte-exactness of this module's output is a hard
/// requirement, and sharing would couple it to unrelated CSS-module
/// refactors; keep the two independent but byte-identical. Also: that file's
/// `byte_hex_to_word` preserves the INPUT's original case (`#ABC` ->
/// `AAAA/...`) — never use it here, since this module's output must always
/// be lowercase.
fn channel_word(byte: u8) -> String {
    let hex = format!("{byte:02x}");
    format!("{hex}{hex}")
}

/// Mirror of xterm's `toRgbString(color, 16)` — the exact channel format a
/// visible renderer xterm uses in OSC 4/10/11/12 query replies.
///
/// R9/R10: byte-exact, 18 bytes, always lowercase. Taking `[u8; 3]` (rather
/// than the source's `number[]`) structurally rules out the source's
/// `value > 255` 8-hex-digit-per-channel output path — a deliberate
/// strengthening that a Rust `u8` type makes free.
pub fn format_x_color_rgb_spec(rgb: TerminalViewRgb) -> String {
    format!(
        "rgb:{}/{}/{}",
        channel_word(rgb[0]),
        channel_word(rgb[1]),
        channel_word(rgb[2])
    )
}

/// Value equality over the whole snapshot. Lets main's store treat a re-push
/// of identical attributes (fresh renderer process: second window, reload,
/// macOS re-activation) as a no-op instead of a theme apply. Structural
/// `PartialEq` on `TerminalViewAttributes` already compares the full 256-
/// entry `ansi` array element-by-element, so this is a direct value compare
/// (the source's `a === b` reference-identity fast path has no Rust
/// equivalent to preserve — these are plain values, not shared objects).
pub fn terminal_view_attributes_equal(
    a: &TerminalViewAttributes,
    b: &TerminalViewAttributes,
) -> bool {
    a == b
}

// ---- validate_terminal_view_attributes ----
//
// R2: ported so the oracle's real validation rules stay pinned: channels are
// integers in 0..=255, the palette must be exactly 256 entries, enum
// membership for colorSchemeMode/cursorStyle, and cursorBlink must be a JSON
// *boolean* (the source's `typeof candidate.cursorBlink !== 'boolean'` check
// rejects the numeric `1` even though it is truthy in JS).

/// `isRgbChannel`: an integer in `0..=255`. Using `as_f64()` (rather than
/// `as_i64()`) treats all three `serde_json::Number` representations
/// (unsigned/signed/float) uniformly, matching JS `Number.isInteger` — a
/// float with a nonzero fractional part (e.g. `1.5`) is rejected.
fn is_rgb_channel(value: &serde_json::Value) -> Option<u8> {
    let n = value.as_f64()?;
    if !n.is_finite() || n.fract() != 0.0 {
        return None;
    }
    if !(0.0..=255.0).contains(&n) {
        return None;
    }
    Some(n as u8)
}

/// `validateRgbTriple`: a JSON array of exactly 3 in-range integer channels.
fn validate_rgb_triple(value: &serde_json::Value) -> Option<TerminalViewRgb> {
    let arr = value.as_array()?;
    if arr.len() != 3 {
        return None;
    }
    let r = is_rgb_channel(&arr[0])?;
    let g = is_rgb_channel(&arr[1])?;
    let b = is_rgb_channel(&arr[2])?;
    Some([r, g, b])
}

/// IPC-boundary validation for the `pty:terminalViewAttributes` push. Returns
/// a normalized copy or `None` — main must never store a malformed palette (a
/// wrong color reply is worse than silence, the OSC-11 lesson).
pub fn validate_terminal_view_attributes(
    payload: &serde_json::Value,
) -> Option<TerminalViewAttributes> {
    let obj = payload.as_object()?;

    let foreground = validate_rgb_triple(obj.get("foreground")?)?;
    let background = validate_rgb_triple(obj.get("background")?)?;
    let cursor = validate_rgb_triple(obj.get("cursor")?)?;

    let ansi_array = obj.get("ansi")?.as_array()?;
    if ansi_array.len() != TERMINAL_VIEW_ANSI_COLOR_COUNT {
        return None;
    }
    let mut ansi_vec: Vec<TerminalViewRgb> = Vec::with_capacity(TERMINAL_VIEW_ANSI_COLOR_COUNT);
    for entry in ansi_array {
        ansi_vec.push(validate_rgb_triple(entry)?);
    }
    let ansi: [TerminalViewRgb; TERMINAL_VIEW_ANSI_COLOR_COUNT] = match ansi_vec.try_into() {
        Ok(a) => a,
        Err(_) => return None,
    };

    let color_scheme_mode = match obj.get("colorSchemeMode").and_then(|v| v.as_str()) {
        Some("dark") => TerminalViewColorSchemeMode::Dark,
        Some("light") => TerminalViewColorSchemeMode::Light,
        _ => return None,
    };

    let cursor_style = match obj.get("cursorStyle").and_then(|v| v.as_str()) {
        Some("bar") => TerminalViewCursorStyle::Bar,
        Some("block") => TerminalViewCursorStyle::Block,
        Some("underline") => TerminalViewCursorStyle::Underline,
        _ => return None,
    };

    // typeof check: only a real JSON boolean is accepted; numeric `1` (JS
    // truthy) must still be rejected.
    let cursor_blink = match obj.get("cursorBlink") {
        Some(serde_json::Value::Bool(b)) => *b,
        _ => return None,
    };

    Some(TerminalViewAttributes {
        foreground,
        background,
        cursor,
        ansi,
        color_scheme_mode,
        cursor_style,
        cursor_blink,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- parseXColorSpec oracle: accept (terminal-view-attributes.test.ts) ----

    #[test]
    fn parses_rgb_colon_widths_like_xterm() {
        assert_eq!(parse_x_color_spec("rgb:f/f/f"), Some([255, 255, 255]));
        assert_eq!(parse_x_color_spec("rgb:0/8/f"), Some([0, 136, 255]));
        assert_eq!(parse_x_color_spec("rgb:ff/00/80"), Some([255, 0, 128]));
        assert_eq!(parse_x_color_spec("rgb:fff/000/888"), Some([255, 0, 136]));
        assert_eq!(
            parse_x_color_spec("rgb:ffff/0000/8888"),
            Some([255, 0, 136])
        );
    }

    #[test]
    fn parses_uppercase_rgb_colon_spec() {
        // R8: case folding via to_lowercase(); no /i flag needed since input
        // is lowercased first.
        assert_eq!(parse_x_color_spec("RGB:FF/00/80"), Some([255, 0, 128]));
    }

    #[test]
    fn parses_hash_widths_like_xterm() {
        assert_eq!(parse_x_color_spec("#abc"), Some([0xa0, 0xb0, 0xc0]));
        assert_eq!(parse_x_color_spec("#aabbcc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(parse_x_color_spec("#aaabbbccc"), Some([0xaa, 0xbb, 0xcc]));
        assert_eq!(
            parse_x_color_spec("#aaaabbbbcccc"),
            Some([0xaa, 0xbb, 0xcc])
        );
    }

    // ---- parseXColorSpec oracle: reject ----

    #[test]
    fn rejects_empty_spec() {
        assert_eq!(parse_x_color_spec(""), None);
    }

    #[test]
    fn rejects_named_colors() {
        assert_eq!(parse_x_color_spec("red"), None);
    }

    #[test]
    fn rejects_missing_channel() {
        assert_eq!(parse_x_color_spec("rgb:ff/ff"), None);
    }

    #[test]
    fn rejects_non_hex_rgb_colon_spec() {
        assert_eq!(parse_x_color_spec("rgb:ggg/000/000"), None);
    }

    #[test]
    fn rejects_hash_length_four() {
        // R5: all-hex but length 4 is not a valid XParseColor width.
        assert_eq!(parse_x_color_spec("#abcd"), None);
    }

    #[test]
    fn rejects_rgbi_spec() {
        assert_eq!(parse_x_color_spec("rgbi:1/1/1"), None);
    }

    // ---- Additional pins (oracle silent) ----

    #[test]
    fn r4_pin_rejects_mixed_channel_widths() {
        // No oracle coverage: xterm.js's alternation is width-anchored per
        // branch, so all three channels must share one digit count.
        assert_eq!(parse_x_color_spec("rgb:f/ff/fff"), None);
    }

    #[test]
    fn r7_pin_rejects_leading_whitespace() {
        // This module never trims. A `js_trim`-equipped implementation would
        // wrongly accept this as `rgb:f/f/f`.
        assert_eq!(parse_x_color_spec(" rgb:f/f/f"), None);
    }

    #[test]
    fn rejects_bare_prefixes() {
        assert_eq!(parse_x_color_spec("#"), None);
        assert_eq!(parse_x_color_spec("rgb:"), None);
    }

    #[test]
    fn rejects_long_non_hex_hash_body() {
        assert_eq!(parse_x_color_spec("#abcdefghi"), None); // length 9, contains 'g'/'h'/'i'
    }

    #[test]
    fn r3_pin_hash_and_rgb_colon_scale_asymmetrically() {
        // The counterintuitive core of R3: #fff truncates via shift (240),
        // rgb:f/f/f rounds via division (255). Same digits, different paths.
        assert_eq!(parse_x_color_spec("#fff"), Some([240, 240, 240]));
        assert_eq!(parse_x_color_spec("rgb:f/f/f"), Some([255, 255, 255]));
    }

    #[test]
    fn r6_pin_rgb_colon_all_zero_channels() {
        assert_eq!(parse_x_color_spec("rgb:0/0/0"), Some([0, 0, 0]));
    }

    #[test]
    fn r3_pin_rgb_colon_scale_rounds_a_non_integral_ratio() {
        // Why: every existing rgb: oracle value (f/f/f, ff/00/80, 0/8/f,
        // fff/000/888, ffff/0000/8888) lands on an EXACT integer ratio, so
        // round() and truncation agree and a dropped `.round()` slips past
        // the whole suite. `0x009/4095*255 = 0.5604...` is non-integral with
        // a fractional part >= 0.5: rounding yields 1, truncation yields 0.
        assert_eq!(parse_x_color_spec("rgb:009/000/000"), Some([1, 0, 0]));
    }

    #[test]
    fn r3_pin_hash_three_digit_shift_uses_high_nibble() {
        // Why: the oracle only exercises repeated-digit inputs like #aaabbbccc,
        // where 0xaaa >> 4 == 0xaa coincides with 0xaaa & 0xff == 0xaa (low-
        // byte truncation). Distinct nibbles per channel break the tie:
        // 0x123 >> 4 = 0x12 = 18, but 0x123 as u8 (truncation) = 0x23 = 35.
        assert_eq!(parse_x_color_spec("#123456789"), Some([18, 69, 120]));
    }

    #[test]
    fn r3_pin_hash_four_digit_shift_uses_high_byte() {
        // Why: same coincidence as the 3-digit case, but for #aaaabbbbcccc
        // (0xaaaa >> 8 == 0xaa == 0xaaaa & 0xff). Distinct nibbles per channel
        // break the tie: 0x1234 >> 8 = 0x12 = 18, but 0x1234 as u8
        // (truncation) = 0x34 = 52.
        assert_eq!(
            parse_x_color_spec("#123456789abc"),
            Some([18, 86, 154])
        );
    }

    #[test]
    fn module_separation_rejects_css_only_grammar() {
        // rgb(...) (CSS function syntax) belongs to
        // reply_query::osc_color_reply::css_color_to_osc_rgb, not this
        // XParseColor mirror.
        assert_eq!(parse_x_color_spec("rgb(1,2,3)"), None);
    }

    // ---- formatXColorRgbSpec oracle ----

    #[test]
    fn formats_16_bit_channels_by_doubling_the_byte() {
        assert_eq!(
            format_x_color_rgb_spec([0x1e, 0x1e, 0x2e]),
            "rgb:1e1e/1e1e/2e2e"
        );
        assert_eq!(format_x_color_rgb_spec([0, 8, 255]), "rgb:0000/0808/ffff");
    }

    #[test]
    fn r9_pin_all_zero_channels_format() {
        assert_eq!(format_x_color_rgb_spec([0, 0, 0]), "rgb:0000/0000/0000");
    }

    // ---- validateTerminalViewAttributes oracle ----

    fn valid_payload() -> serde_json::Value {
        let ansi: Vec<serde_json::Value> = (0..256usize)
            .map(|i| json!([(i % 256) as u64, 0, 0]))
            .collect();
        json!({
            "foreground": [1, 2, 3],
            "background": [4, 5, 6],
            "cursor": [7, 8, 9],
            "ansi": ansi,
            "colorSchemeMode": "dark",
            "cursorStyle": "block",
            "cursorBlink": true
        })
    }

    #[test]
    fn accepts_and_normalizes_a_well_formed_payload() {
        let attrs = validate_terminal_view_attributes(&valid_payload());
        assert!(attrs.is_some());
        let attrs = attrs.unwrap();
        assert_eq!(attrs.ansi.len(), 256);
        assert_eq!(attrs.color_scheme_mode, TerminalViewColorSchemeMode::Dark);
    }

    #[test]
    fn rejects_null_payload() {
        assert_eq!(
            validate_terminal_view_attributes(&serde_json::Value::Null),
            None
        );
    }

    #[test]
    fn rejects_missing_foreground() {
        let mut payload = valid_payload();
        payload.as_object_mut().unwrap().remove("foreground");
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_short_triple() {
        let mut payload = valid_payload();
        payload["background"] = json!([1, 2]);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_out_of_range_channel() {
        let mut payload = valid_payload();
        payload["cursor"] = json!([0, 0, 300]);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_non_integer_channel() {
        let mut payload = valid_payload();
        payload["cursor"] = json!([0, 0, 1.5]);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_short_palette() {
        let mut payload = valid_payload();
        let short: Vec<serde_json::Value> = (0..16usize).map(|i| json!([i as u64, 0, 0])).collect();
        payload["ansi"] = json!(short);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_bad_palette_entry() {
        let mut payload = valid_payload();
        let mut ansi = payload["ansi"].as_array().unwrap().clone();
        ansi[255] = json!("red");
        payload["ansi"] = json!(ansi);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_bad_color_scheme_mode() {
        let mut payload = valid_payload();
        payload["colorSchemeMode"] = json!("auto");
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_bad_cursor_style() {
        let mut payload = valid_payload();
        payload["cursorStyle"] = json!("beam");
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    #[test]
    fn rejects_non_boolean_blink() {
        // R2: numeric 1 is JS-truthy but must still be rejected — only a
        // real boolean passes.
        let mut payload = valid_payload();
        payload["cursorBlink"] = json!(1);
        assert_eq!(validate_terminal_view_attributes(&payload), None);
    }

    // ---- terminalViewAttributesEqual oracle ----

    fn snapshot() -> TerminalViewAttributes {
        let ansi_vec: Vec<TerminalViewRgb> =
            (0..256usize).map(|i| [(i % 256) as u8, 0, 0]).collect();
        TerminalViewAttributes {
            foreground: [1, 2, 3],
            background: [4, 5, 6],
            cursor: [7, 8, 9],
            ansi: ansi_vec.try_into().unwrap(),
            color_scheme_mode: TerminalViewColorSchemeMode::Dark,
            cursor_style: TerminalViewCursorStyle::Block,
            cursor_blink: true,
        }
    }

    #[test]
    fn treats_two_independently_built_identical_snapshots_as_equal() {
        assert!(terminal_view_attributes_equal(&snapshot(), &snapshot()));
    }

    #[test]
    fn detects_a_change_in_foreground() {
        let mut b = snapshot();
        b.foreground = [1, 2, 4];
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_background() {
        let mut b = snapshot();
        b.background = [0, 0, 0];
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_cursor() {
        let mut b = snapshot();
        b.cursor = [7, 8, 10];
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_an_ansi_entry() {
        // ansi[200]: proves the full 256-entry tail is compared, not just a
        // prefix.
        let mut b = snapshot();
        b.ansi[200] = [9, 9, 9];
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_color_scheme_mode() {
        let mut b = snapshot();
        b.color_scheme_mode = TerminalViewColorSchemeMode::Light;
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_cursor_style() {
        let mut b = snapshot();
        b.cursor_style = TerminalViewCursorStyle::Bar;
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }

    #[test]
    fn detects_a_change_in_cursor_blink() {
        let mut b = snapshot();
        b.cursor_blink = false;
        assert!(!terminal_view_attributes_equal(&snapshot(), &b));
    }
}
