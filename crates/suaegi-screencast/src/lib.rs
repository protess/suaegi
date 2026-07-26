//! VERBATIM port of Orca's `src/shared/browser-screencast-protocol.ts` (143
//! lines). A binary wire protocol carrying browser CDP screencast frames
//! (kind/opcode/format/seq header, JSON metadata, raw image tail) from the
//! Electron main process to the renderer.
//!
//! Ported: `O:1-3` header constants (see [`BROWSER_SCREENCAST_KIND`] /
//! [`BROWSER_SCREENCAST_VERSION`] / `HEADER_BYTES`), `O:4-14` `METADATA_KEYS`
//! (see [`METADATA_KEYS`]), `O:16-18` [`BrowserScreencastOpcode`], `O:20`
//! [`BrowserScreencastFormat`], `O:22-32` [`BrowserScreencastFrameMetadata`],
//! `O:34-40` [`BrowserScreencastFrame`], `O:42-44` `formatToByte` (private,
//! see `format_to_byte`), `O:46-54` `byteToFormat` (private, see
//! `byte_to_format`), `O:56-58` `encodeJson` (private, see
//! `encode_frame_metadata`), `O:60-66` `decodeJson` (private, see
//! `decode_json`), `O:68-70` `isFiniteNumber` (inlined as an `f64::is_finite`
//! check), `O:72-86` `decodeFrameMetadata` (private, see
//! `decode_frame_metadata`), `O:88-102` [`encode_browser_screencast_frame`],
//! `O:104-143` [`decode_browser_screencast_frame`].
//!
//! Real producer cross-checked: `src/main/browser/browser-screencast-stream.ts:56-66`
//! (`readFrameMetadata`) builds its metadata object key-by-key in exactly the
//! `METADATA_KEYS` order reproduced below (F4) — the oracle fixture's key
//! order differs, but the oracle never inspects metadata bytes, only parsed
//! values, so that's harmless.
//!
//! # Traps (see the plan's §1 for the full rationale)
//! - **F1**: EVERY multi-byte header field is little-endian — `O:96-98` /
//!   `O:122-124` pass `littleEndian = true` explicitly to every
//!   `setUint32`/`getUint32` call. `DataView`'s default is big-endian, and
//!   the wire-protocol reflex to reach for `to_be_bytes` ("network byte
//!   order") is exactly backwards here: a big-endian port would still pass
//!   all 8 oracle cases (every oracle seq is 42 or 1, every length is 2 or
//!   19 bytes — single-byte values look identical in either endianness).
//!   Implemented with `u32::to_le_bytes`/`from_le_bytes` throughout; pinned
//!   byte-exact below.
//! - **F2**: header offset `[12..16]` is RESERVED, not an image length
//!   (`O:124-126`, `O:98`). This protocol length-prefixes only the metadata;
//!   the image runs to the end of the input (`O:141`'s `subarray(imageStart)`
//!   has no end argument). No image-length check is invented. A nonzero
//!   reserved field is itself a rejection.
//! - **F3**: decode returns a BORROWED VIEW, not a copy — `O:141` uses
//!   `subarray`, and the oracle (`T:132-134`) asserts
//!   `decoded.image.buffer === encoded.buffer`. [`decode_browser_screencast_frame`]
//!   therefore returns `Option<BrowserScreencastFrame<'_>>` with
//!   `image: &'a [u8]` borrowed from the input, never a `Vec<u8>` copy.
//! - **F4**: `METADATA_KEYS` is DECODE-ONLY (`O:79`'s loop is the only use
//!   site) — encode is a raw `JSON.stringify` of the caller's object
//!   (`O:57`, `O:89`), so the emitted key order is the caller's insertion
//!   order, not `METADATA_KEYS`. [`BrowserScreencastFrameMetadata`]'s 9
//!   `Option<f64>` fields are declared in `METADATA_KEYS` order, which
//!   matches the real producer exactly (see module docs above); the
//!   oracle's own fixture order differs but the oracle is semantics-only.
//! - **F5**: metadata serialization is HAND-ROLLED, not `serde_json` — its
//!   `ryu` float formatting diverges from ECMAScript `Number::toString`:
//!   `1280.0` -> `"1280.0"` (JS: `"1280"`), `-0.0` -> `"-0.0"` (JS: `"0"`),
//!   and `serde_json` never switches to exponential notation (JS does at
//!   1e21). Every such difference changes the u32 length field at offset 8,
//!   producing a different frame on the wire. `encode_frame_metadata` emits
//!   `{`, the present fields in declaration order as `"key":<num>` joined by
//!   `,`, then `}`; numbers go through `format_ecmascript_float` (copied
//!   below, see its own docs).
//! - **F6**: absent => key GONE; non-finite => key PRESENT with value
//!   `null`. `JSON.stringify` omits `undefined`-valued keys but emits `null`
//!   for `NaN`/`±Infinity` (`O:96` in the stream producer feeds arbitrary
//!   floats through unchanged; the oracle's `T:110` filters `NaN` on the
//!   *decode* side, which is a separate, non-finite-drop rule — see
//!   `decode_frame_metadata`). Modeled as: `None` => omit; `Some(v)` with
//!   `!v.is_finite()` => emit `"key":null`; otherwise the formatted number.
//! - **F7**: `TextDecoder` is LOSSY and strips a leading BOM (`O:62`,
//!   default options: `fatal: false`, `ignoreBOM: false`). Invalid UTF-8
//!   becomes U+FFFD (never throws); a leading U+FEFF is stripped before
//!   `JSON.parse`. Implemented with `String::from_utf8_lossy` (never
//!   `str::from_utf8` or `serde_json::from_slice`) plus an explicit strip of
//!   one leading `'\u{FEFF}'`. Known, accepted divergence in the opposite
//!   direction: `JSON.parse` accepts lone-surrogate escapes (e.g.
//!   `"\ud800"`) that `serde_json` rejects outright; unreachable for
//!   metadata *values* (which must be numbers to survive the finite-number
//!   filter) but reachable for unknown keys, which are dropped anyway.
//! - **F8**: exactly 7 rejection paths; the comparison operators ARE the
//!   contract (checked in this exact order, matching `O:105-135`):
//!   1. `bytes.len() < HEADER_BYTES` — strict `<`; a frame of exactly 16
//!      bytes PASSES this check (though see the pin below for what a real
//!      16-byte frame does further down the pipeline).
//!   2. `kind != 0x62` OR `version != 1`.
//!   3. `opcode != Frame`.
//!   4. format byte is neither 1 nor 2.
//!   5. `reserved != 0`.
//!   6. `image_start > bytes.len()` — strict `>`; `image_start == len`
//!      (an empty image) PASSES.
//!   7. metadata JSON is a parse failure, or parses to something other than
//!      a JSON object (`null`, `0`, `false`, `""`, a number, a string,
//!      `true`, or an array all fall here). **`{}` is SUCCESS** — no
//!      `is_empty()` guard is added; several oracle cases depend on this.
//! - **F9**: `seq` encoding clamps to >= 0, floors, then WRAPS modulo 2^32;
//!   Rust's `as u32` on a float SATURATES instead, which is wrong here.
//!   `O:96`: `Math.max(0, Math.floor(seq)) >>> 0`. `5e9 as u32` in Rust
//!   would saturate to `4294967295`, but JS wraps it to `705032704`.
//!   Implemented as: NaN => 0; else floor, clamp to >= 0.0, and if the
//!   result is still non-finite (i.e. `+Infinity`) => 0; else reduce modulo
//!   2^32 in `f64` (exact for this range) before casting to `u32`. Decode
//!   does NOT invert this and does NOT validate `seq` — it returns the raw
//!   header `u32` unchanged (`O:122`).
//! - **F10**: `HEADER_BYTES + metadata_len` uses CHECKED arithmetic.
//!   `metadata_len` can be attacker-controlled up to `0xFFFF_FFFF`; on a
//!   32-bit target an unchecked `usize` add overflows (debug panic, release
//!   wraparound making `image_start` a small in-bounds number that then
//!   *passes* check 6 and slices a plausible-looking frame out of
//!   garbage). Implemented with `checked_add`, returning `None` on
//!   overflow regardless of target pointer width.
//! - **F11**: the format mapping is ASYMMETRIC. Encode (`format_to_byte`) is
//!   a catch-all: `'png' => 2`, everything else => 1 (`O:43`). Decode
//!   (`byte_to_format`) is strict: only 1 and 2 map to a format; everything
//!   else is `None` (`O:47-52`). A 2-variant Rust enum reproduces the
//!   encode catch-all naturally (an exhaustive match with no wildcard
//!   needed); the decode side stays a strict allowlist.
//! - **F12**: decode respects a nonzero `byteOffset` into a larger backing
//!   buffer (`O:108`'s `DataView` is constructed from `bytes.byteOffset`).
//!   Rust `&[u8]` gives this for free — pinned below with a real subslice.
// ---------------------------------------------------------------------------
// Header constants — O:1-14
// ---------------------------------------------------------------------------

const BROWSER_SCREENCAST_KIND: u8 = 0x62;
const BROWSER_SCREENCAST_VERSION: u8 = 1;
const HEADER_BYTES: usize = 16;

/// `O:4-14`. DECODE-ONLY (F4) — encode does not consult this at all; see the
/// module docs.
const METADATA_KEYS: [&str; 9] = [
    "offsetTop",
    "pageScaleFactor",
    "deviceWidth",
    "deviceHeight",
    "imageWidth",
    "imageHeight",
    "scrollOffsetX",
    "scrollOffsetY",
    "timestamp",
];

// ---------------------------------------------------------------------------
// Public types — O:16-40
// ---------------------------------------------------------------------------

/// `O:16-18`. The TS enum has exactly one member; kept as a Rust enum (not a
/// unit struct) so a corrupted opcode byte has somewhere to fail to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BrowserScreencastOpcode {
    Frame = 1,
}

/// `O:20`. TS models this as a string union (`'jpeg' | 'png'`); the 2-variant
/// enum enumerates the same closed set. See F11 for the asymmetric byte
/// mapping this participates in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserScreencastFormat {
    Jpeg,
    Png,
}

/// `O:22-32`. Every field is `Option<f64>`: `None` mirrors an absent /
/// non-finite-and-dropped TS field. Field DECLARATION ORDER matches
/// `METADATA_KEYS` (F4), which is also the real producer's insertion order —
/// see the module docs for why this matters for encode's wire bytes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BrowserScreencastFrameMetadata {
    pub offset_top: Option<f64>,
    pub page_scale_factor: Option<f64>,
    pub device_width: Option<f64>,
    pub device_height: Option<f64>,
    pub image_width: Option<f64>,
    pub image_height: Option<f64>,
    pub scroll_offset_x: Option<f64>,
    pub scroll_offset_y: Option<f64>,
    pub timestamp: Option<f64>,
}

/// `O:34-40`. `seq` is `f64` on both sides of the wire, matching the TS
/// `number` type: encode accepts an arbitrary (possibly negative, fractional,
/// non-finite) float and clamps/floors/wraps it (F9); decode returns the raw
/// header `u32` promoted losslessly to `f64` (every `u32` value is exactly
/// representable). `image` is a borrowed slice (F3), never an owned copy.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserScreencastFrame<'a> {
    pub opcode: BrowserScreencastOpcode,
    pub seq: f64,
    pub format: BrowserScreencastFormat,
    pub metadata: BrowserScreencastFrameMetadata,
    pub image: &'a [u8],
}

// ---------------------------------------------------------------------------
// Format byte mapping — O:42-54 (F11: asymmetric)
// ---------------------------------------------------------------------------

fn format_to_byte(format: BrowserScreencastFormat) -> u8 {
    // O:42-44 `formatToByte`: `format === 'png' ? 2 : 1` is a catch-all —
    // every format that is not 'png' becomes 1 (jpeg). This 2-variant enum
    // already enumerates the full TS union exhaustively, so the match below
    // reproduces the catch-all without a wildcard arm.
    match format {
        BrowserScreencastFormat::Png => 2,
        BrowserScreencastFormat::Jpeg => 1,
    }
}

fn byte_to_format(value: u8) -> Option<BrowserScreencastFormat> {
    // O:46-54 `byteToFormat`. F11: strict allowlist, asymmetric with
    // `format_to_byte` above.
    match value {
        1 => Some(BrowserScreencastFormat::Jpeg),
        2 => Some(BrowserScreencastFormat::Png),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// seq encoding — O:96 (F9)
// ---------------------------------------------------------------------------

fn encode_seq(seq: f64) -> u32 {
    // O:96: `Math.max(0, Math.floor(seq)) >>> 0`.
    if seq.is_nan() {
        return 0;
    }
    let floored = seq.floor();
    let clamped = floored.max(0.0);
    if !clamped.is_finite() {
        // `+Infinity` survives `floor`/`max`; ToUint32(Infinity) is 0 in JS.
        return 0;
    }
    // `clamped` is now a non-negative finite whole number. `%` on f64 is
    // exact here (both operands are exactly representable / the dividend's
    // low bits are exactly zero at magnitudes beyond f64 precision, same as
    // JS's mathematical-value-based ToUint32), so the cast below never
    // truncates a fractional part.
    (clamped % 4_294_967_296.0) as u32
}

// ---------------------------------------------------------------------------
// Metadata codec — O:56-86 (F5, F6, F7, F8 sub-case 7)
// ---------------------------------------------------------------------------

fn encode_frame_metadata(metadata: &BrowserScreencastFrameMetadata) -> Vec<u8> {
    // O:56-58 `encodeJson` + O:88-89 (`JSON.stringify(frame.metadata)`), but
    // hand-rolled per F5: `serde_json`'s ryu float formatting is wire-visibly
    // wrong. Field order below is `METADATA_KEYS` order (F4), matching the
    // real producer's insertion order.
    let fields: [(&str, Option<f64>); 9] = [
        (METADATA_KEYS[0], metadata.offset_top),
        (METADATA_KEYS[1], metadata.page_scale_factor),
        (METADATA_KEYS[2], metadata.device_width),
        (METADATA_KEYS[3], metadata.device_height),
        (METADATA_KEYS[4], metadata.image_width),
        (METADATA_KEYS[5], metadata.image_height),
        (METADATA_KEYS[6], metadata.scroll_offset_x),
        (METADATA_KEYS[7], metadata.scroll_offset_y),
        (METADATA_KEYS[8], metadata.timestamp),
    ];
    let mut out = String::from("{");
    let mut wrote_field = false;
    for (key, value) in fields {
        // F6: `None` => the key is omitted entirely (mirrors `JSON.stringify`
        // dropping `undefined`-valued keys).
        let Some(v) = value else { continue };
        if wrote_field {
            out.push(',');
        }
        wrote_field = true;
        // Keys are fixed ASCII identifiers; no escaping is needed.
        out.push('"');
        out.push_str(key);
        out.push_str("\":");
        if v.is_finite() {
            out.push_str(&format_ecmascript_float(v));
        } else {
            // F6: non-finite (NaN / +-Infinity) => key PRESENT, value `null`
            // (mirrors `JSON.stringify`'s NaN/Infinity -> `null` behavior).
            out.push_str("null");
        }
    }
    out.push('}');
    out.into_bytes()
}

fn decode_json(bytes: &[u8]) -> Option<serde_json::Value> {
    // O:60-66 `decodeJson`. F7: `TextDecoder` (default options) is lossy
    // (invalid UTF-8 -> U+FFFD, never throws) and strips one leading BOM
    // before `JSON.parse`.
    let text = String::from_utf8_lossy(bytes);
    let stripped = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    serde_json::from_str(stripped).ok()
}

fn decode_frame_metadata(bytes: &[u8]) -> Option<BrowserScreencastFrameMetadata> {
    // O:72-86 `decodeFrameMetadata`.
    let raw = decode_json(bytes)?;
    // F8 sub-case 7: `!raw || typeof raw !== 'object' || Array.isArray(raw)`.
    // Every JSON value other than an object (null, booleans, numbers,
    // strings, arrays) collapses to a single rejection here; `{}` — an
    // empty but still-an-object value — is the one case that survives, and
    // deliberately has no `is_empty()` guard (several oracle cases rely on
    // this).
    let object = match raw {
        serde_json::Value::Object(map) => map,
        _ => return None,
    };
    let mut metadata = BrowserScreencastFrameMetadata::default();
    for key in METADATA_KEYS {
        let Some(value) = object.get(key) else {
            continue;
        };
        // `isFiniteNumber`: `typeof value === 'number' && Number.isFinite(value)`.
        // Unlike JS's `Number()`, `serde_json` REJECTS an out-of-range
        // exponent (e.g. `1e400`) at PARSE time (`number out of range`)
        // rather than producing an infinite `f64` — confirmed against this
        // workspace's locked `serde_json` version. So a `serde_json::Number`
        // that made it this far is always finite, and the `is_finite()`
        // check below is DEFENSIVE, not load-bearing: it cannot currently be
        // false (mutation-tested — replacing it with `if true` kills no
        // test).
        //
        // Known, accepted divergence from Orca: `JSON.parse` on a document
        // with an out-of-range exponent still SUCCEEDS, yielding `Infinity`
        // for that field, so Orca's `isFiniteNumber` drops just that one key
        // and decodes the rest of the metadata normally. Here the same
        // exponent fails the whole-document `serde_json::from_str` in
        // `decode_json`, so `decode_frame_metadata` returns `None` and
        // `decode_browser_screencast_frame` rejects the ENTIRE frame (F8
        // sub-case 7) — see
        // `f7_out_of_range_exponent_rejects_the_whole_frame_unlike_orca`.
        // Accepted rather than worked around: matching Orca here would mean
        // hand-rolling a lenient number parser, which would reintroduce
        // exactly the `JSON.parse`-fidelity risk that motivated using
        // `serde_json` (rather than a hand-rolled parser) for decode in the
        // first place.
        if let Some(n) = value.as_f64() {
            if n.is_finite() {
                set_metadata_field(&mut metadata, key, n);
            }
        }
    }
    Some(metadata)
}

fn set_metadata_field(metadata: &mut BrowserScreencastFrameMetadata, key: &str, value: f64) {
    match key {
        "offsetTop" => metadata.offset_top = Some(value),
        "pageScaleFactor" => metadata.page_scale_factor = Some(value),
        "deviceWidth" => metadata.device_width = Some(value),
        "deviceHeight" => metadata.device_height = Some(value),
        "imageWidth" => metadata.image_width = Some(value),
        "imageHeight" => metadata.image_height = Some(value),
        "scrollOffsetX" => metadata.scroll_offset_x = Some(value),
        "scrollOffsetY" => metadata.scroll_offset_y = Some(value),
        "timestamp" => metadata.timestamp = Some(value),
        _ => unreachable!("METADATA_KEYS lists exactly these 9 keys"),
    }
}

// ---------------------------------------------------------------------------
// Encode — O:88-102
// ---------------------------------------------------------------------------

pub fn encode_browser_screencast_frame(frame: &BrowserScreencastFrame<'_>) -> Vec<u8> {
    let metadata_bytes = encode_frame_metadata(&frame.metadata);
    let mut out = Vec::with_capacity(HEADER_BYTES + metadata_bytes.len() + frame.image.len());
    out.push(BROWSER_SCREENCAST_KIND);
    out.push(BROWSER_SCREENCAST_VERSION);
    out.push(frame.opcode as u8);
    out.push(format_to_byte(frame.format));
    // F1: every multi-byte field is little-endian.
    out.extend_from_slice(&encode_seq(frame.seq).to_le_bytes());
    out.extend_from_slice(&(metadata_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved, always 0 (F2)
    out.extend_from_slice(&metadata_bytes);
    out.extend_from_slice(frame.image);
    out
}

// ---------------------------------------------------------------------------
// Decode — O:104-143
// ---------------------------------------------------------------------------

pub fn decode_browser_screencast_frame(bytes: &[u8]) -> Option<BrowserScreencastFrame<'_>> {
    // F8(1): strict `<` — exactly `HEADER_BYTES` bytes PASSES this check.
    if bytes.len() < HEADER_BYTES {
        return None;
    }
    // F8(2).
    if bytes[0] != BROWSER_SCREENCAST_KIND || bytes[1] != BROWSER_SCREENCAST_VERSION {
        return None;
    }
    // F8(3).
    if bytes[2] != BrowserScreencastOpcode::Frame as u8 {
        return None;
    }
    // F8(4).
    let format = byte_to_format(bytes[3])?;
    // F1: little-endian throughout.
    let seq = u32::from_le_bytes(bytes[4..8].try_into().expect("slice of len 4"));
    let metadata_len = u32::from_le_bytes(bytes[8..12].try_into().expect("slice of len 4"));
    let reserved = u32::from_le_bytes(bytes[12..16].try_into().expect("slice of len 4"));
    // F8(5) / F2: reserved must be exactly 0, and is NEVER an image length.
    if reserved != 0 {
        return None;
    }
    // F10: checked arithmetic — `metadata_len` is attacker-controlled up to
    // `u32::MAX`; an unchecked add could overflow a 32-bit `usize`.
    let image_start = HEADER_BYTES.checked_add(metadata_len as usize)?;
    // F8(6): strict `>` — `image_start == bytes.len()` (an empty image)
    // PASSES this check.
    if image_start > bytes.len() {
        return None;
    }
    let metadata_bytes = &bytes[HEADER_BYTES..image_start];
    // F8(7).
    let metadata = decode_frame_metadata(metadata_bytes)?;
    Some(BrowserScreencastFrame {
        opcode: BrowserScreencastOpcode::Frame,
        // Decode does not invert F9's clamp/floor/wrap and does not validate
        // `seq` — the raw header value is returned unchanged (`O:122`).
        seq: seq as f64,
        format,
        metadata,
        // F3: a borrowed view over the input, sliced to the end (F2) — never
        // a copy.
        image: &bytes[image_start..],
    })
}

// ---------------------------------------------------------------------------
// W5 — ECMAScript Number::toString for the Float case
//
// Copied VERBATIM from `suaegi-mcp/src/json.rs:266` (`format_ecmascript_float`
// is a private fn there, not `pub`, so it cannot be imported — see plan
// precedent `suaegi-workname/Cargo.toml:22-24` copying `js_ws` for the same
// reason). Kept byte-for-byte identical, including its doc comment, so any
// future upstream fix in `suaegi-mcp` is easy to diff against this copy.
// ---------------------------------------------------------------------------

/// ECMA-262 §6.1.6.1.20 `Number::toString(x, 10)`, restricted to the finite
/// values that can come out of JSON (`serde_json` never yields NaN/Infinity).
/// Rust's `f64::to_string()` diverges on two axes this function corrects:
/// it never emits exponential notation (`1e21` -> Rust's
/// `"1000000000000000000000"`), and it prints `"-0"` for negative zero
/// instead of `"0"`.
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

    fn frame<'a>(seq: f64, image: &'a [u8]) -> BrowserScreencastFrame<'a> {
        BrowserScreencastFrame {
            opcode: BrowserScreencastOpcode::Frame,
            seq,
            format: BrowserScreencastFormat::Jpeg,
            metadata: BrowserScreencastFrameMetadata::default(),
            image,
        }
    }

    // -- oracle: T:9-37 round-trip --------------------------------------

    #[test]
    fn oracle_round_trips_frame_metadata_and_image_bytes() {
        let encoded = encode_browser_screencast_frame(&BrowserScreencastFrame {
            opcode: BrowserScreencastOpcode::Frame,
            seq: 42.0,
            format: BrowserScreencastFormat::Jpeg,
            metadata: BrowserScreencastFrameMetadata {
                device_width: Some(1280.0),
                device_height: Some(720.0),
                page_scale_factor: Some(1.0),
                timestamp: Some(123.0),
                ..Default::default()
            },
            image: &[1, 2, 3, 4],
        });

        let decoded = decode_browser_screencast_frame(&encoded).expect("valid frame");

        assert_eq!(decoded.opcode, BrowserScreencastOpcode::Frame);
        assert_eq!(decoded.seq, 42.0);
        assert_eq!(decoded.format, BrowserScreencastFormat::Jpeg);
        assert_eq!(decoded.metadata.device_width, Some(1280.0));
        assert_eq!(decoded.metadata.device_height, Some(720.0));
        assert_eq!(decoded.metadata.page_scale_factor, Some(1.0));
        assert_eq!(decoded.metadata.timestamp, Some(123.0));
        assert_eq!(decoded.metadata.offset_top, None);
        assert_eq!(decoded.image, &[1, 2, 3, 4]);
    }

    // -- oracle: T:39-41 --------------------------------------------------

    #[test]
    fn oracle_rejects_unrelated_binary_frames() {
        // Named for the four-byte input, but it is the length guard (F8(1))
        // that actually rejects it, not any "unrelated frame" heuristic.
        assert_eq!(decode_browser_screencast_frame(&[0, 1, 2, 3]), None);
    }

    // -- oracle: T:43-58 it.each(version/opcode/format) --------------------

    #[test]
    fn oracle_rejects_frames_with_an_unsupported_version_byte() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[1] = 2;
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    #[test]
    fn oracle_rejects_frames_with_an_unsupported_opcode_byte() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[2] = 9;
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    #[test]
    fn oracle_rejects_frames_with_an_unsupported_format_byte() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[3] = 9;
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- oracle: T:60-75 ----------------------------------------------------

    #[test]
    fn oracle_rejects_frames_whose_metadata_length_exceeds_the_payload() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        let len = encoded.len() as u32;
        encoded[8..12].copy_from_slice(&len.to_le_bytes());
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- oracle: T:77-88 ------------------------------------------------------

    #[test]
    fn oracle_rejects_frames_with_nonzero_reserved_header_bytes() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[12] = 1;
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- oracle: T:90-100 -----------------------------------------------------

    #[test]
    fn oracle_rejects_non_object_metadata() {
        // TS encodes `metadata: []`; our typed struct can't express that
        // directly, so build the frame bytes by hand with a `[]` metadata
        // payload instead (same wire shape encode would have produced).
        let metadata_bytes = b"[]";
        let mut encoded = vec![
            BROWSER_SCREENCAST_KIND,
            BROWSER_SCREENCAST_VERSION,
            BrowserScreencastOpcode::Frame as u8,
            1, // jpeg
        ];
        encoded.extend_from_slice(&1u32.to_le_bytes()); // seq
        encoded.extend_from_slice(&(metadata_bytes.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes()); // reserved
        encoded.extend_from_slice(metadata_bytes);
        encoded.extend_from_slice(&[1]); // image

        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- oracle: T:102-121 -----------------------------------------------------

    #[test]
    fn oracle_keeps_only_finite_numeric_metadata_fields() {
        // TS sets `deviceWidth: '1280'` (a string) and an unknown `extra` key;
        // our typed struct can't express a wrong-typed field, so this is
        // built by hand at the JSON-bytes level, exercising the same
        // `decodeFrameMetadata` filter oracle T:102-121 exercises.
        // `NaN` is not valid JSON syntax; TS's `JSON.stringify` never emits a
        // bare `NaN` literal either (it becomes `null`, per F6) — the
        // equivalent input for the *decode* side is a literal JSON `null`.
        let metadata_json =
            br#"{"deviceWidth":"1280","deviceHeight":720,"pageScaleFactor":null,"scrollOffsetX":15,"extra":42}"#;
        let mut encoded = vec![
            BROWSER_SCREENCAST_KIND,
            BROWSER_SCREENCAST_VERSION,
            BrowserScreencastOpcode::Frame as u8,
            1, // jpeg
        ];
        encoded.extend_from_slice(&1u32.to_le_bytes()); // seq
        encoded.extend_from_slice(&(metadata_json.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes()); // reserved
        encoded.extend_from_slice(metadata_json);
        encoded.extend_from_slice(&[1]); // image

        let decoded = decode_browser_screencast_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.metadata.device_width, None); // string value dropped
        assert_eq!(decoded.metadata.device_height, Some(720.0));
        assert_eq!(decoded.metadata.page_scale_factor, None); // null dropped
        assert_eq!(decoded.metadata.scroll_offset_x, Some(15.0));
        // unknown "extra" key has no corresponding field at all — nothing to
        // assert beyond the four fields above already covering the full
        // struct via Default for the rest.
        assert_eq!(decoded.metadata.offset_top, None);
        assert_eq!(decoded.metadata.image_width, None);
        assert_eq!(decoded.metadata.image_height, None);
        assert_eq!(decoded.metadata.scroll_offset_y, None);
        assert_eq!(decoded.metadata.timestamp, None);
    }

    // -- oracle: T:123-135 (F3) ------------------------------------------------

    #[test]
    fn oracle_decodes_image_bytes_as_a_view_over_the_original_frame_buffer() {
        let encoded = encode_browser_screencast_frame(&frame(1.0, &[7, 8, 9]));
        let decoded = decode_browser_screencast_frame(&encoded).expect("valid frame");
        // F3: `image` must be a borrowed subslice of `encoded`, not a copy —
        // assert pointer identity against the source buffer, not just value
        // equality (a `Vec<u8>` copy would pass a plain `==` check).
        assert_eq!(
            decoded.image.as_ptr(),
            encoded[encoded.len() - 3..].as_ptr()
        );
        assert_eq!(decoded.image, &[7, 8, 9]);
    }

    // ======================================================================
    // Hand-written pins — oracle-silent traps (see plan §2 "추가 핀")
    // ======================================================================

    // -- F1: byte-exact little-endian --------------------------------------

    #[test]
    fn f1_seq_encodes_byte_exact_little_endian() {
        let encoded = encode_browser_screencast_frame(&BrowserScreencastFrame {
            opcode: BrowserScreencastOpcode::Frame,
            seq: f64::from(0x01020304u32),
            format: BrowserScreencastFormat::Jpeg,
            metadata: BrowserScreencastFrameMetadata::default(),
            image: &[],
        });
        assert_eq!(&encoded[4..8], &[0x04, 0x03, 0x02, 0x01]);
    }

    #[test]
    fn f1_metadata_length_spans_two_bytes_little_endian() {
        // Force a metadata length >= 256 so the length field is not
        // single-byte-ambiguous between LE and BE (a big-endian port would
        // still pass any test where every meaningful byte is <= 0xFF and in
        // byte 0 of the u32).
        let encoded = encode_browser_screencast_frame(&BrowserScreencastFrame {
            opcode: BrowserScreencastOpcode::Frame,
            seq: 1.0,
            format: BrowserScreencastFormat::Jpeg,
            metadata: BrowserScreencastFrameMetadata {
                // Long, many-significant-digit values so the serialized
                // metadata clears 256 bytes and the length field genuinely
                // spans two bytes (not just byte 0 of the u32).
                timestamp: Some(1.2345678901234568e29),
                offset_top: Some(1.2345678901234568e29),
                page_scale_factor: Some(1.2345678901234568e29),
                device_width: Some(1.2345678901234568e29),
                device_height: Some(1.2345678901234568e29),
                image_width: Some(1.2345678901234568e29),
                image_height: Some(1.2345678901234568e29),
                scroll_offset_x: Some(1.2345678901234568e29),
                scroll_offset_y: Some(1.2345678901234568e29),
            },
            image: &[],
        });
        let metadata_len = u32::from_le_bytes(encoded[8..12].try_into().unwrap());
        assert!(
            metadata_len >= 256,
            "test fixture must produce a two-byte-spanning length, got {metadata_len}"
        );
        // Recompute expected LE bytes directly and compare against the wire.
        assert_eq!(&encoded[8..12], &metadata_len.to_le_bytes());
        // And explicitly: this is NOT big-endian (would require byte 8 to
        // be the high byte, which for any len < 0x0100_0000 is always 0).
        assert_eq!(encoded[8], (metadata_len & 0xFF) as u8);
    }

    // -- F8(1): exactly 16 bytes ---------------------------------------------

    #[test]
    fn f8_1_exactly_sixteen_byte_frame_is_rejected_via_empty_metadata_json_parse_failure() {
        // A real 16-byte frame has metadata_len = 0 (no bytes follow the
        // header at all): the metadata slice is empty, and an empty string
        // is not valid JSON, so `decode_json` returns `None` and the WHOLE
        // decode fails via F8(7) (JSON parse failure) — NOT via the length
        // guard F8(1), which this frame passes (16 is not < 16). This is the
        // "indirect" rejection path the plan calls out.
        let mut header = Vec::with_capacity(16);
        header.push(BROWSER_SCREENCAST_KIND);
        header.push(BROWSER_SCREENCAST_VERSION);
        header.push(BrowserScreencastOpcode::Frame as u8);
        header.push(1); // jpeg
        header.extend_from_slice(&1u32.to_le_bytes()); // seq
        header.extend_from_slice(&0u32.to_le_bytes()); // metadata_len = 0
        header.extend_from_slice(&0u32.to_le_bytes()); // reserved
        assert_eq!(header.len(), 16);

        assert_eq!(decode_browser_screencast_frame(&header), None);
    }

    #[test]
    fn f8_1_exactly_sixteen_byte_frame_with_empty_object_metadata_is_accepted() {
        // Contrast with the above: if the caller DOES supply `{}` as
        // metadata (2 extra bytes, so an 18-byte frame, not 16), decode
        // succeeds — confirming F8(1)'s strict `<` is not itself the
        // blocker for a bare 16-byte input; a genuinely empty metadata JSON
        // string is.
        let encoded = encode_browser_screencast_frame(&frame(1.0, &[]));
        assert_eq!(encoded.len(), 18); // 16 header + "{}" + 0 image bytes
        let decoded = decode_browser_screencast_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.metadata, BrowserScreencastFrameMetadata::default());
        assert_eq!(decoded.image, &[] as &[u8]);
    }

    // -- F8(6): image_start == len (empty image) -----------------------------

    #[test]
    fn f8_6_image_start_equal_to_length_empty_image_is_accepted() {
        let encoded = encode_browser_screencast_frame(&frame(1.0, &[]));
        let decoded = decode_browser_screencast_frame(&encoded).expect("empty image must decode");
        assert_eq!(decoded.image, &[] as &[u8]);
    }

    // -- F8(2): kind byte alone rejected (oracle never touches byte 0) ------

    #[test]
    fn f8_2_wrong_kind_byte_alone_is_rejected() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[0] = 0x63; // valid version/opcode/format/reserved otherwise
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- F5: numeric formatting in the wire bytes ----------------------------

    #[test]
    fn f5_device_width_1280_serializes_to_bare_integer_not_1280_point_0() {
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata {
            device_width: Some(1280.0),
            ..Default::default()
        });
        assert_eq!(bytes, b"{\"deviceWidth\":1280}");
    }

    #[test]
    fn f5_negative_zero_serializes_as_zero() {
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata {
            offset_top: Some(-0.0),
            ..Default::default()
        });
        assert_eq!(bytes, b"{\"offsetTop\":0}");
    }

    #[test]
    fn f5_1e21_switches_to_exponential_with_explicit_plus_sign() {
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata {
            timestamp: Some(1e21),
            ..Default::default()
        });
        assert_eq!(bytes, b"{\"timestamp\":1e+21}");
    }

    // -- F6: absent vs non-finite ---------------------------------------------

    #[test]
    fn f6_absent_field_is_omitted_entirely() {
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata::default());
        assert_eq!(bytes, b"{}");
    }

    #[test]
    fn f6_non_finite_field_emits_key_present_with_null_value() {
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata {
            page_scale_factor: Some(f64::NAN),
            ..Default::default()
        });
        assert_eq!(bytes, b"{\"pageScaleFactor\":null}");
    }

    // -- F7: lossy decode + BOM stripping -------------------------------------

    #[test]
    fn f7_invalid_utf8_inside_metadata_decodes_lossily_and_still_parses() {
        // `{"note":<0xFF>"}` — 0xFF is not valid UTF-8 anywhere in that
        // position; `from_utf8_lossy` replaces it with U+FFFD, which then
        // sits inside a JSON string value (harmless: the key we care about,
        // `deviceWidth`, is untouched and still parses).
        let mut metadata_json = br#"{"note":""#.to_vec();
        metadata_json.push(0xFF);
        metadata_json.extend_from_slice(br#"","deviceWidth":5}"#);

        let mut encoded = vec![
            BROWSER_SCREENCAST_KIND,
            BROWSER_SCREENCAST_VERSION,
            BrowserScreencastOpcode::Frame as u8,
            1,
        ];
        encoded.extend_from_slice(&1u32.to_le_bytes());
        encoded.extend_from_slice(&(metadata_json.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes());
        encoded.extend_from_slice(&metadata_json);

        let decoded = decode_browser_screencast_frame(&encoded).expect("must still parse");
        assert_eq!(decoded.metadata.device_width, Some(5.0));
    }

    #[test]
    fn f7_leading_bom_is_stripped_and_still_parses() {
        let mut metadata_json = "\u{FEFF}".as_bytes().to_vec();
        metadata_json.extend_from_slice(br#"{"deviceWidth":9}"#);

        let mut encoded = vec![
            BROWSER_SCREENCAST_KIND,
            BROWSER_SCREENCAST_VERSION,
            BrowserScreencastOpcode::Frame as u8,
            1,
        ];
        encoded.extend_from_slice(&1u32.to_le_bytes());
        encoded.extend_from_slice(&(metadata_json.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&0u32.to_le_bytes());
        encoded.extend_from_slice(&metadata_json);

        let decoded =
            decode_browser_screencast_frame(&encoded).expect("BOM-prefixed JSON must parse");
        assert_eq!(decoded.metadata.device_width, Some(9.0));
    }

    /// Pins a known, accepted divergence from Orca (see the comment above
    /// the `is_finite()` check in `decode_frame_metadata`): Orca's
    /// `JSON.parse('{"deviceWidth":1e400}')` SUCCEEDS, yielding
    /// `deviceWidth: Infinity`, so Orca's `isFiniteNumber` filter drops just
    /// that one key and still decodes the frame with the rest of its
    /// metadata intact. This port's `serde_json` instead REJECTS `1e400` as
    /// an out-of-range number at parse time, so the whole metadata document
    /// fails to parse and the ENTIRE frame is rejected — not just the one
    /// field. This test pins THIS port's (divergent) behavior, not Orca's.
    #[test]
    fn f7_out_of_range_exponent_rejects_the_whole_frame_unlike_orca() {
        fn build_frame(metadata_json: &[u8]) -> Vec<u8> {
            let mut encoded = vec![
                BROWSER_SCREENCAST_KIND,
                BROWSER_SCREENCAST_VERSION,
                BrowserScreencastOpcode::Frame as u8,
                1,
            ];
            encoded.extend_from_slice(&1u32.to_le_bytes());
            encoded.extend_from_slice(&(metadata_json.len() as u32).to_le_bytes());
            encoded.extend_from_slice(&0u32.to_le_bytes());
            encoded.extend_from_slice(metadata_json);
            encoded.extend_from_slice(b"img");
            encoded
        }

        let out_of_range = build_frame(br#"{"deviceWidth":1e400}"#);
        assert_eq!(
            decode_browser_screencast_frame(&out_of_range),
            None,
            "an out-of-range exponent must fail serde_json's whole-document \
             parse and reject the entire frame, unlike Orca's JSON.parse \
             (which would succeed and just drop the one key)"
        );

        let in_range = build_frame(br#"{"deviceWidth":1e308}"#);
        let decoded = decode_browser_screencast_frame(&in_range)
            .expect("1e308 is within f64 range and must decode successfully");
        assert_eq!(decoded.metadata.device_width, Some(1e308));
    }

    // -- F9: seq clamp/floor/wrap exact values --------------------------------

    #[test]
    fn f9_seq_negative_one_clamps_to_zero() {
        assert_eq!(encode_seq(-1.0), 0);
    }

    #[test]
    fn f9_seq_fraction_floors_down() {
        assert_eq!(encode_seq(42.9), 42);
    }

    #[test]
    fn f9_seq_nan_encodes_to_zero() {
        assert_eq!(encode_seq(f64::NAN), 0);
    }

    #[test]
    fn f9_seq_infinity_encodes_to_zero() {
        assert_eq!(encode_seq(f64::INFINITY), 0);
    }

    #[test]
    fn f9_seq_two_pow_32_wraps_to_zero() {
        assert_eq!(encode_seq(2f64.powi(32)), 0);
    }

    #[test]
    fn f9_seq_five_billion_wraps_not_saturates() {
        // The load-bearing assertion: `5e9 as u32` in Rust would saturate to
        // `u32::MAX` (4294967295); JS's `>>> 0` wraps modulo 2^32, giving
        // 705032704. If this ever regresses to `as u32`, this pin fails.
        assert_eq!(encode_seq(5e9), 705_032_704);
        assert_ne!(encode_seq(5e9), u32::MAX);
    }

    // -- F10: checked arithmetic, no panic ------------------------------------

    #[test]
    fn f10_metadata_len_u32_max_returns_none_without_panic() {
        let mut encoded = vec![0u8; 20];
        encoded[0] = BROWSER_SCREENCAST_KIND;
        encoded[1] = BROWSER_SCREENCAST_VERSION;
        encoded[2] = BrowserScreencastOpcode::Frame as u8;
        encoded[3] = 1;
        encoded[4..8].copy_from_slice(&1u32.to_le_bytes());
        encoded[8..12].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        encoded[12..16].copy_from_slice(&0u32.to_le_bytes());

        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- F11: png round-trip + strict decode allowlist ------------------------

    #[test]
    fn f11_png_round_trips_through_encode_and_decode() {
        let encoded = encode_browser_screencast_frame(&BrowserScreencastFrame {
            opcode: BrowserScreencastOpcode::Frame,
            seq: 1.0,
            format: BrowserScreencastFormat::Png,
            metadata: BrowserScreencastFrameMetadata::default(),
            image: &[9, 9],
        });
        assert_eq!(encoded[3], 2); // png byte
        let decoded = decode_browser_screencast_frame(&encoded).expect("valid png frame");
        assert_eq!(decoded.format, BrowserScreencastFormat::Png);
    }

    #[test]
    fn f11_format_byte_three_is_rejected() {
        let mut encoded = encode_browser_screencast_frame(&frame(1.0, &[1]));
        encoded[3] = 3;
        assert_eq!(decode_browser_screencast_frame(&encoded), None);
    }

    // -- F12: nonzero-offset subslice -----------------------------------------

    #[test]
    fn f12_decoding_a_nonzero_offset_subslice_works() {
        let inner = encode_browser_screencast_frame(&frame(7.0, &[5, 6]));
        let mut buffer = vec![0xAAu8; 5];
        buffer.extend_from_slice(&inner);
        buffer.extend_from_slice(&[0xBB; 3]);

        let view = &buffer[5..5 + inner.len()];
        let decoded = decode_browser_screencast_frame(view).expect("offset slice must decode");
        assert_eq!(decoded.seq, 7.0);
        assert_eq!(decoded.image, &[5, 6]);
    }

    // ======================================================================
    // Copied verbatim from `suaegi-mcp/src/json.rs` — numeric formatter tests
    // ======================================================================

    fn ecmascript_float(value: f64) -> String {
        format_ecmascript_float(value)
    }

    #[test]
    fn w5_1e21_switches_to_exponential_with_explicit_plus_sign() {
        assert_eq!(ecmascript_float(1e21), "1e+21");
    }

    #[test]
    fn w5_1e_minus_7_switches_to_exponential() {
        assert_eq!(ecmascript_float(1e-7), "1e-7");
    }

    #[test]
    fn w5_1e_minus_6_stays_plain_decimal() {
        assert_eq!(ecmascript_float(1e-6), "0.000001");
    }

    #[test]
    fn w5_1e20_stays_plain_decimal_integer() {
        assert_eq!(ecmascript_float(1e20), "100000000000000000000");
    }

    #[test]
    fn w5_negative_zero_is_the_string_zero() {
        assert_eq!(ecmascript_float(-0.0_f64), "0");
    }

    #[test]
    fn w5_0_1_is_plain_decimal() {
        assert_eq!(ecmascript_float(0.1), "0.1");
    }

    #[test]
    fn w5_100_has_no_decimal_point() {
        assert_eq!(ecmascript_float(100.0), "100");
    }

    #[test]
    fn w5_1_5_keeps_a_single_fractional_digit() {
        assert_eq!(ecmascript_float(1.5), "1.5");
    }

    #[test]
    fn w5_5e_minus_324_is_the_smallest_denormal() {
        assert_eq!(ecmascript_float(5e-324), "5e-324");
    }

    #[test]
    fn w5_seventeen_significant_digits_round_trip_in_exponential_form() {
        assert_eq!(
            ecmascript_float(1.2345678901234568e29),
            "1.2345678901234568e+29"
        );
    }

    // -- F4: producer key order, byte-exact -----------------------------------

    #[test]
    fn f4_metadata_keys_serialize_in_producer_declaration_order_regardless_of_set_order() {
        // Set the LAST declared field (`timestamp`) and the FIRST
        // (`offsetTop`) in reverse order in the struct literal; the emitted
        // bytes must still follow `METADATA_KEYS` / struct-declaration order
        // (offsetTop before timestamp), matching the real producer
        // (`browser-screencast-stream.ts:56-66`), not insertion order (Rust
        // struct literals have no observable "insertion order" at runtime,
        // but this pins that the serializer itself is order-fixed, not
        // iterating some incidental hash order).
        let bytes = encode_frame_metadata(&BrowserScreencastFrameMetadata {
            timestamp: Some(2.0),
            offset_top: Some(1.0),
            ..Default::default()
        });
        assert_eq!(bytes, b"{\"offsetTop\":1,\"timestamp\":2}");
    }
}
