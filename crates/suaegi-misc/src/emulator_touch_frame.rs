//! VERBATIM port of Orca's `src/shared/emulator-touch-frame.ts` (18 lines),
//! at v1.4.146-rc.0. Encodes a `serve-sim` touch-simulation message: a single
//! tag byte followed by the UTF-8 bytes of `JSON.stringify(touch)`. Variable
//! length (`1 + n`), no fixed layout — there is no `DataView`, no multi-byte
//! integer, and no endianness anywhere in this wire shape (unlike the
//! neighbouring `suaegi-screencast` / `suaegi-termstream` protocol modules;
//! do not import their `to_uint32` / `to_le_bytes` / endianness machinery
//! here, none of it applies).
//!
//! Oracle: `emulator-touch-frame.test.ts` (15 lines, one case). ⚠ That oracle
//! pins almost nothing: it parses the payload back to a JSON value before
//! comparing (`JSON.parse(...).toEqual(...)`), so key order and exact number
//! formatting are both invisible to it, and its only fixture (`x: 0.25,
//! y: 0.75`) happens to format identically under ECMAScript's
//! `Number::toString` and Rust's `ryu`. The decisions below are pinned with
//! extra byte-exact tests the oracle cannot express.
//!
//! # Decisions (see the plan's §1-§3 for the full rationale)
//! - **E1 — ECMAScript number formatting, not `ryu`.** `JSON.stringify` uses
//!   `Number::toString`: `1` prints as `"1"` (not `"1.0"`), `-0` as `"0"`,
//!   non-finite values as the bare token `null`, and `1e21` as `"1e+21"`.
//!   [`format_ecmascript_float`] below is a private per-module copy of the
//!   same function living in `suaegi-screencast::format_ecmascript_float`
//!   and `suaegi-termstream`'s copy (this crate's charter forbids reaching
//!   across crates for it — every module gets its own copy). Non-finite
//!   handling follows `suaegi-termstream/src/lib.rs:375-380`: `is_finite()`
//!   guards the call, `null` otherwise. This divergence is production-
//!   reachable on every screen-edge touch (upstream's `clampUnit` returns
//!   exactly `0`/`1` at the edges) and every edge swipe (`edge` is always the
//!   integer `3`) — not a hypothetical.
//! - **E2 — key order is `type, x, y[, edge]`.** `JSON.stringify` preserves
//!   insertion order, and upstream production uses *two* different orders:
//!   the type declaration / CLI path (`cli/handlers/emulator.ts:112`) uses
//!   `type, x, y[, edge]`; the renderer / live-touch path
//!   (`emulator-screen-gesture.ts:62`, `emulator-device-frame.tsx`) spreads
//!   `{...point, type}`, producing `x, y, type[, edge]`. A Rust struct has
//!   exactly one field order, so it cannot reproduce both. This module
//!   pins the type-declaration order because it matches both the type
//!   declaration (`emulator-touch-frame.ts:4-7`) and the module's own oracle
//!   fixture. The renderer path's `x, y, type` order is NOT reproduced here
//!   — if a caller needs to match that path's exact bytes, it cannot with
//!   this function. This is a byte-level divergence, not a behavioral one
//!   (any conforming JSON parser is order-blind), but it is pinned anyway
//!   because the receiver is a third-party binary (`serve-sim`) whose
//!   tolerance for key order cannot be verified from this repo.
//! - **E3 — the tag literal.** The module's own oracle only asserts
//!   `frame[0] === SERVE_SIM_TOUCH_MESSAGE_TAG`, a tautology that stays green
//!   even if the constant is changed to `0x99`; the literal `0x03` is pinned
//!   only by a DIFFERENT, out-of-scope test
//!   (`emulator-gesture-sender.test.ts:19`). Both the constant value and
//!   `frame[0]`'s literal value are asserted directly below. The structural
//!   twin `emulator-keyboard-frame.ts` has the identical defect with tag
//!   `0x06` (noted here, not fixed — out of scope).
//! - **E4 — `edge: Option<f64>`, `None` omits the key entirely.** This
//!   mirrors `JSON.stringify`'s treatment of an `undefined`-valued object
//!   property: the key is dropped, not emitted as `null`. There are zero
//!   fixtures for `edge` anywhere in the upstream repo; presence, absence,
//!   and its integer formatting (E1) are all pinned as extra tests here.
//! - **E5 — all three variants serialize as strings.** `begin` and `end` are
//!   dark in the module's own oracle (only `'move'` appears there), but both
//!   occur constantly in production (`emulator-device-frame.tsx`). No
//!   integer discriminant is introduced for any variant.
//! - **E6 — no validation or clamping is added.** This module has no notion
//!   of coordinate range; clamping lives upstream in
//!   `emulator-screen-gesture.ts:45-47` and validation in
//!   `cli/handlers/emulator.ts:57-61,95-102`.
//! - **E7 — no JSON string-escaping machinery is needed, and that is a
//!   closed-world fact, not an oversight.** The only string-valued field is
//!   [`ServeSimTouchType`], a closed 3-element ASCII enum
//!   (`"begin" | "move" | "end"`) — there is no code path that can produce a
//!   quote, backslash, control character, or non-ASCII byte in this payload.
//!   If a future edit adds a free-form string field to this frame, that
//!   invariant breaks and proper JSON string escaping (as done in
//!   `suaegi-termstream`'s `encode_json_string`) becomes necessary.
//! - **E8 — encode only, no decoder.** There is no decoder anywhere in this
//!   repo for this wire shape; an external `serve-sim@^0.1.40` process
//!   decodes these frames. Do not add one.
//!
//! `emulator-keyboard-frame.ts` (129 lines, tag `0x06`) is this module's
//! structural twin. If it is ever ported, reconsider promoting the pair to a
//! dedicated `suaegi-servesim` leaf crate rather than adding a second module
//! here.

/// The touch gesture phase. All three variants serialize as JSON strings
/// (`"begin"` / `"move"` / `"end"`) — see module doc E5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeSimTouchType {
    Begin,
    Move,
    End,
}

impl ServeSimTouchType {
    /// The exact JSON-string content (without quotes) for this variant.
    pub fn as_str(self) -> &'static str {
        match self {
            ServeSimTouchType::Begin => "begin",
            ServeSimTouchType::Move => "move",
            ServeSimTouchType::End => "end",
        }
    }
}

/// Tag byte identifying a `serve-sim` touch-simulation frame. Pinned as the
/// literal `0x03` — see module doc E3 for why the module's own oracle can't
/// enforce that by itself.
pub const SERVE_SIM_TOUCH_MESSAGE_TAG: u8 = 0x03;

/// A single simulated touch event, encoded to `serve-sim` as
/// `[TAG, ...UTF-8 JSON bytes]` by [`encode_serve_sim_touch_frame`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServeSimTouchFrame {
    pub touch_type: ServeSimTouchType,
    pub x: f64,
    pub y: f64,
    pub edge: Option<f64>,
}

/// Encode `touch` as a `serve-sim` touch-simulation frame: the tag byte
/// [`SERVE_SIM_TOUCH_MESSAGE_TAG`] followed by the UTF-8 bytes of
/// `JSON.stringify(touch)`, in key order `type, x, y[, edge]` (E2).
pub fn encode_serve_sim_touch_frame(touch: &ServeSimTouchFrame) -> Vec<u8> {
    let mut json = String::new();
    json.push('{');
    json.push_str("\"type\":\"");
    json.push_str(touch.touch_type.as_str());
    json.push_str("\",\"x\":");
    json.push_str(&format_json_number(touch.x));
    json.push_str(",\"y\":");
    json.push_str(&format_json_number(touch.y));
    if let Some(edge) = touch.edge {
        json.push_str(",\"edge\":");
        json.push_str(&format_json_number(edge));
    }
    json.push('}');

    let mut frame = Vec::with_capacity(1 + json.len());
    frame.push(SERVE_SIM_TOUCH_MESSAGE_TAG);
    frame.extend_from_slice(json.as_bytes());
    frame
}

/// `JSON.stringify`'s rendering of a JS number: ECMAScript decimal text for
/// finite values (see [`format_ecmascript_float`]), the bare token `null`
/// for `NaN`/`±Infinity` (`termstream`-style, `suaegi-termstream/src/lib.rs:375-380`).
fn format_json_number(value: f64) -> String {
    if value.is_finite() {
        format_ecmascript_float(value)
    } else {
        "null".to_string()
    }
}

// ---------------------------------------------------------------------------
// Private per-module copy of ECMAScript `Number::toString` formatting. Same
// function as `suaegi-screencast::format_ecmascript_float` /
// `suaegi-termstream`'s copy — deliberately duplicated per this crate's
// dependency-free, no-cross-crate-reuse charter (see module doc E1).
// ---------------------------------------------------------------------------

/// ECMA-262 §6.1.6.1.20 `Number::toString(x, 10)`, restricted to the finite
/// values that can come out of JSON. Rust's `f64::to_string()` diverges on
/// two axes this function corrects: it never emits exponential notation
/// (`1e21` -> Rust's `"1000000000000000000000"`), and it prints `"-0"` for
/// negative zero instead of `"0"`.
///
/// Approach: Rust's `{:e}` formatting (`LowerExp`) already computes the
/// shortest round-tripping decimal digit string — the same guarantee
/// `Number::toString` relies on — so this function only needs to re-thread
/// ECMA's placement rules (plain decimal vs. exponential, and where the
/// decimal point / zero-padding goes) around those digits, rather than
/// deriving them itself.
fn format_ecmascript_float(value: f64) -> String {
    if value == 0.0 {
        // Covers +0.0 and -0.0: IEEE-754 equality treats them as equal, and
        // ECMAScript's Number::toString maps BOTH to the string "0".
        return "0".to_string();
    }
    if value < 0.0 {
        return format!("-{}", format_ecmascript_float(-value));
    }

    // `value` is finite, strictly positive here.
    let exponential = format!("{value:e}");
    let (mantissa, exponent_str) = exponential
        .split_once('e')
        .expect("LowerExp output always contains an 'e'");
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let digit_count = digits.len() as i64;
    let exponent: i64 = exponent_str
        .parse()
        .expect("LowerExp exponent is a valid integer");
    // `n` per ECMA-262: s * 10^(n - k) == value, where s = digits (k digits).
    let n = exponent + 1;

    if digit_count <= n && n <= 21 {
        let trailing_zeros = "0".repeat((n - digit_count) as usize);
        format!("{digits}{trailing_zeros}")
    } else if 0 < n && n <= 21 {
        let split_at = n as usize;
        format!("{}.{}", &digits[..split_at], &digits[split_at..])
    } else if -6 < n && n <= 0 {
        let leading_zeros = "0".repeat((-n) as usize);
        format!("0.{leading_zeros}{digits}")
    } else {
        let displayed_exponent = n - 1;
        let sign = if displayed_exponent >= 0 { '+' } else { '-' };
        if digit_count == 1 {
            format!("{digits}e{sign}{}", displayed_exponent.abs())
        } else {
            format!(
                "{}.{}e{sign}{}",
                &digits[..1],
                &digits[1..],
                displayed_exponent.abs()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Port of `emulator-touch-frame.test.ts` ("encodes serve-sim touch
    /// messages with the binary touch tag"). The upstream oracle parses
    /// before comparing; here we additionally assert the exact byte string,
    /// since `x: 0.25, y: 0.75` happen to format identically under both
    /// ECMAScript and `ryu` formatting and would not otherwise catch a
    /// `serde_json`-based port (see module doc E1).
    #[test]
    fn oracle_encodes_move_with_binary_touch_tag() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.25,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);

        assert_eq!(frame[0], SERVE_SIM_TOUCH_MESSAGE_TAG);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0.25,\"y\":0.75}".to_vec()
        );
    }

    // -- E1: ECMAScript number formatting, exact bytes, no parsing ----------

    #[test]
    fn integer_value_has_no_trailing_decimal_point() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 1.0,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":1,\"y\":0.75}".to_vec()
        );
    }

    #[test]
    fn zero_formats_as_bare_zero() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.0,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0,\"y\":0.75}".to_vec()
        );
    }

    #[test]
    fn negative_zero_formats_as_bare_zero_not_minus_zero() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: -0.0,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0,\"y\":0.75}".to_vec()
        );
    }

    #[test]
    fn integer_edge_has_no_trailing_decimal_point() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.25,
            y: 0.75,
            edge: Some(3.0),
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0.25,\"y\":0.75,\"edge\":3}".to_vec()
        );
    }

    #[test]
    fn non_finite_coordinate_formats_as_bare_null_token() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: f64::NAN,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":null,\"y\":0.75}".to_vec()
        );
    }

    #[test]
    fn large_magnitude_uses_ecmascript_exponential_notation() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 1e21,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":1e+21,\"y\":0.75}".to_vec()
        );
    }

    // -- E2: key order is type, x, y[, edge], byte-exact ---------------------

    #[test]
    fn key_order_is_type_then_x_then_y_then_edge() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Begin,
            x: 0.25,
            y: 0.75,
            edge: Some(3.0),
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"begin\",\"x\":0.25,\"y\":0.75,\"edge\":3}".to_vec()
        );
    }

    // -- E3: tag literal ------------------------------------------------------

    #[test]
    fn tag_constant_is_the_literal_0x03() {
        assert_eq!(SERVE_SIM_TOUCH_MESSAGE_TAG, 0x03);
    }

    #[test]
    fn frame_first_byte_is_the_tag_at_offset_zero() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.0,
            y: 0.0,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(frame[0], 0x03);
    }

    // -- E4: edge presence / absence -----------------------------------------

    #[test]
    fn edge_none_omits_the_key_entirely() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.25,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0.25,\"y\":0.75}".to_vec()
        );
        assert!(!String::from_utf8(frame).unwrap().contains("edge"));
    }

    #[test]
    fn edge_some_emits_the_key_as_a_trailing_member() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.25,
            y: 0.75,
            edge: Some(3.0),
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(
            frame,
            b"\x03{\"type\":\"move\",\"x\":0.25,\"y\":0.75,\"edge\":3}".to_vec()
        );
    }

    // -- E5: all three variants serialize as strings -------------------------

    #[test]
    fn begin_variant_serializes_as_the_string_begin() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Begin,
            x: 0.0,
            y: 0.0,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(frame, b"\x03{\"type\":\"begin\",\"x\":0,\"y\":0}".to_vec());
    }

    #[test]
    fn move_variant_serializes_as_the_string_move() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.0,
            y: 0.0,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(frame, b"\x03{\"type\":\"move\",\"x\":0,\"y\":0}".to_vec());
    }

    #[test]
    fn end_variant_serializes_as_the_string_end() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::End,
            x: 0.0,
            y: 0.0,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        assert_eq!(frame, b"\x03{\"type\":\"end\",\"x\":0,\"y\":0}".to_vec());
    }

    // -- frame shape: length 1 + n, UTF-8 payload starts at offset 1 --------

    #[test]
    fn frame_length_is_one_plus_json_byte_length() {
        let touch = ServeSimTouchFrame {
            touch_type: ServeSimTouchType::Move,
            x: 0.25,
            y: 0.75,
            edge: None,
        };
        let frame = encode_serve_sim_touch_frame(&touch);
        let json = b"{\"type\":\"move\",\"x\":0.25,\"y\":0.75}";
        assert_eq!(frame.len(), 1 + json.len());
        assert_eq!(&frame[1..], json);
    }
}
