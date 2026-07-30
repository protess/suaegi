//! VERBATIM port of Orca's `src/shared/terminal-stream-protocol.ts` (109
//! lines, v1.4.146-rc.0). A binary wire protocol carrying terminal
//! output/input/resize/snapshot/metadata frames between the Electron main
//! process and the renderer: a 16-byte little-endian header
//! (kind/version/opcode/pad/streamId/seq-high/seq-low) followed by an
//! opaque payload tail, plus JSON and raw-text codecs for that payload.
//!
//! Ported: `O:1-3` header constants (see [`TERMINAL_STREAM_KIND`] /
//! [`TERMINAL_STREAM_VERSION`] / `HEADER_BYTES`), `O:5-25`
//! [`TerminalStreamOpcode`], `O:27-32` [`TerminalStreamFrame`], `O:34-47`
//! [`encode_terminal_stream_frame`], `O:49-69` [`decode_terminal_stream_frame`],
//! `O:71-73` [`encode_terminal_stream_json`], `O:75-81`
//! [`decode_terminal_stream_json`], `O:83-85` [`encode_terminal_stream_text`],
//! `O:87-89` [`decode_terminal_stream_text`], `O:91-109` `isTerminalStreamOpcode`
//! (see `TryFrom<u8> for TerminalStreamOpcode`).
//!
//! # Traps (see the plan's §2 for the full rationale; each corrects a choice
//! # the sibling `suaegi-screencast` port made that is WRONG for this protocol)
//!
//! - **R1**: decode returns an OWNED COPY of the payload, not a view —
//!   `O:67`'s `bytes.slice(HEADER_BYTES)` copies (unlike the sibling's
//!   `subarray`, which is a view). The oracle has zero buffer-identity
//!   assertions, so a view-returning port would pass all 7 cases, but
//!   production (`remote-runtime-terminal-multiplexer.ts:530,534`) retains
//!   the payload across event-loop turns over a reused socket buffer — a
//!   view would silently observe later mutations. [`TerminalStreamFrame`]
//!   therefore has no lifetime parameter and `payload: Vec<u8>`; pinned by
//!   mutating the source buffer after decode in
//!   `r1_decoded_payload_is_unaffected_by_mutating_the_source_buffer_afterward`.
//! - **R2**: `seq` is a 64-bit value split into two 32-bit LE words, and the
//!   HIGH word sits at the LOWER offset (`O:43`/`O:61`, offset 8), the LOW
//!   word at the higher offset (`O:44`/`O:62`, offset 12). Each word is
//!   little-endian internally, but the two words are placed in BIG-endian
//!   order relative to each other — this is neither `u64::to_le_bytes` nor
//!   `u64::to_be_bytes`. Every oracle `seq` is under 2^32, so the high word
//!   is always zero and word-swapping, using `u64::to_le_bytes` directly, or
//!   dropping the high word entirely would all pass 7/7. Pinned in
//!   `r2_seq_word_layout_high_word_at_lower_offset` with both word slices and
//!   a negative assertion that bytes `[8..16]` are NOT `seq.to_le_bytes()`.
//! - **R3**: all six multi-byte header fields (`streamId` at `O:41`/`O:65`,
//!   the two `seq` words at `O:43-44`/`O:61-62`) are explicitly
//!   little-endian (`DataView`'s `littleEndian` argument is `true`
//!   everywhere). `DataView` defaults to big-endian, and the "network byte
//!   order" reflex for a wire protocol is `to_be_bytes` — exactly backwards
//!   here. Every oracle value is under 256, so a big-endian port would pass
//!   7/7. Pinned in `r3_stream_id_encodes_byte_exact_little_endian`.
//! - **R4**: decode has EXACTLY THREE rejection paths (`O:50-52` length,
//!   `O:54-56` kind/version, `O:58-60` opcode). Byte 3 (`O:40`'s pad,
//!   written as `0` on encode) is NEVER read on decode — validating it would
//!   pass the oracle (encode always writes 0) while rejecting legitimate
//!   third-party frames. Bytes `[12..16]` are the seq LOW word (`O:44`),
//!   not a reserved field — do not invent a reserved-field check. There is
//!   no length-prefixed sub-field in this protocol at all (unlike the
//!   sibling's metadata length), so there is no `checked_add` / overrun
//!   check to port. Pinned in `r4_byte_three_is_a_pad_never_validated_on_decode`,
//!   `r4_bytes_twelve_to_sixteen_are_the_seq_low_word_not_a_reserved_field`,
//!   and `r4_no_length_field_a_one_byte_payload_decodes_without_an_overrun_check`.
//! - **R5**: there are FIFTEEN opcodes (`O:5-25`), validated by set
//!   membership (`O:91-109`'s 15-arm `||` chain), not a range check — the
//!   source comments (`O:18-19`, `O:21-22`) flag that future opcodes may be
//!   non-contiguous. Implemented as `TryFrom<u8>` with 15 explicit arms
//!   rather than `1..=15`. The oracle only pins one unknown-opcode value
//!   (99, `T:154`), leaving the `0` and `16` boundaries dark; pinned here in
//!   `r5_opcode_zero_is_rejected` / `r5_opcode_sixteen_is_rejected`, plus a
//!   full 15-opcode round-trip table.
//! - **R6**: `streamId` and `seq` have DIFFERENT numeric rules, four lines
//!   apart. `seq` clamps: `Math.max(0, Math.floor(frame.seq))` (`O:42`) —
//!   never negative. `streamId` is a bare `ToUint32` via `setUint32`'s
//!   implicit conversion (`O:41`): truncate toward zero, wrap modulo 2^32,
//!   `NaN`/`±Infinity` -> 0, and NO clamp, so `-1` wraps to `0xFFFF_FFFF`.
//!   Rust's `f64 as u32` SATURATES where JS wraps, so both `to_uint32` (for
//!   `streamId`) and the seq-word split reduce through `% 4_294_967_296.0`
//!   rather than a raw `as u32` cast. Pinned in `r6_stream_id_negative_one_wraps_to_u32_max`,
//!   `r6_stream_id_fraction_truncates_toward_zero`,
//!   `r6_stream_id_nan_and_infinity_encode_to_zero`,
//!   `r6_stream_id_at_or_above_two_pow_32_wraps` (all contrasted with seq's
//!   clamp-and-floor behavior on the same inputs).
//! - **R7**: `decodeTerminalStreamJson` (`O:75-81`) has NO shape check —
//!   unlike the sibling's `decodeFrameMetadata`, there is no
//!   `!raw || typeof raw !== 'object' || Array.isArray(raw)` guard, so
//!   `"[1,2]"`, `"42"`, `"true"`, and `"\"a string\""` all succeed.
//!   [`decode_terminal_stream_json`] therefore does NOT match on
//!   `serde_json::Value::Object`. Separately, `JSON.parse("null")` and a
//!   caught parse exception are indistinguishable in TS — both produce the
//!   literal value `null` (`T:76`'s catch block also returns via the
//!   implicit `null`). This port collapses both into `None`, which is
//!   faithful to that ambiguity (see the comment on the function itself).
//! - **R8**: there is also a TEXT codec (`O:83-89`, absent from the
//!   sibling), the hot path for terminal `Output` frames. `TextDecoder`
//!   (default options: `fatal: false`, `ignoreBOM: false`) is lossy —
//!   invalid UTF-8 becomes U+FFFD, never throws — and strips one leading
//!   BOM. Both [`decode_terminal_stream_json`] and
//!   [`decode_terminal_stream_text`] apply `String::from_utf8_lossy` plus an
//!   explicit strip of one leading `'\u{FEFF}'`. Pinned in
//!   `r8_text_decode_strips_a_leading_bom`, `r8_text_decode_replaces_invalid_utf8_with_u_fffd`,
//!   `r8_json_decode_strips_a_leading_bom`, `r8_json_decode_lossily_repairs_invalid_utf8_inside_a_string`.
//! - **R9**: `seq` is recombined in `f64` (`O:66`: `high * 0x100000000 +
//!   low`, ordinary JS float arithmetic) — a `u64` return would be MORE
//!   precise than the source and therefore divergent for values the source
//!   itself cannot represent exactly. [`TerminalStreamFrame::seq`] is `f64`
//!   on both the encode and decode side.
//! - **R10**: two byte-fidelity hazards on JSON encode, invisible to an
//!   oracle that only compares parsed values via `toEqual`/`toMatchObject`:
//!   `serde_json::Value`'s object map is a `BTreeMap` and would re-sort keys
//!   where `JSON.stringify` (`O:72`) preserves insertion order; and `ryu`
//!   prints `"120.0"`/`"-0.0"` where JS prints `"120"`/`"0"` and never
//!   switches to exponential notation before 1e21. [`TerminalStreamJsonValue`]
//!   is a hand-rolled, order-preserving value type (object fields are
//!   `Vec<(String, _)>`, not a map) whose encoder runs numbers through
//!   [`format_ecmascript_float`] (copied verbatim again — see that
//!   function's own docs; this is now a per-module copy in three crates,
//!   the established pattern in this repo for a private upstream helper).
//!   Unlike the sibling protocol, THIS protocol has no length-prefixed
//!   sub-field ahead of the JSON payload, so a format difference changes the
//!   payload bytes but never breaks framing.
//! - **R11**: `mobile/src/transport/terminal-stream-protocol.ts` is a
//!   DIVERGENT DUPLICATE with only 7 opcodes (`Output`, `SnapshotStart`,
//!   `SnapshotChunk`, `SnapshotEnd`, `Resized`, `Error`, `Metadata` — no
//!   `Input`/`Resize`/`Subscribe`/`Unsubscribe`/`SnapshotRequest`/`Ack`/
//!   `ClaimViewport`/`OutputSpan`, and no JSON/text codec exports at all).
//!   The 16-byte header framing is byte-identical to `shared/`'s. This is a
//!   COMPATIBILITY FACT to record, not a discrepancy to reconcile: this
//!   crate ports the 15-opcode `shared/` version; a frame using an opcode
//!   outside the mobile 7-opcode subset simply fails to decode on a mobile
//!   client, by mobile's own design.
// ---------------------------------------------------------------------------
// Header constants — O:1-3
// ---------------------------------------------------------------------------

const TERMINAL_STREAM_KIND: u8 = 0x74;
const TERMINAL_STREAM_VERSION: u8 = 1;
const HEADER_BYTES: usize = 16;

// ---------------------------------------------------------------------------
// Opcodes — O:5-25, O:91-109 (R5: 15-way set membership, not a range check)
// ---------------------------------------------------------------------------

/// `O:5-25`. Fifteen opcodes, validated by set membership (`O:91-109`), not a
/// contiguous range — see R5. `Ack` and `ClaimViewport` deliberately renumber
/// around already-shipped mobile opcodes (see the source comments preserved
/// below); a future opcode is not guaranteed to extend the range
/// contiguously, which is exactly why this port uses an explicit 15-arm
/// `TryFrom` instead of `1..=15`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TerminalStreamOpcode {
    Output = 1,
    SnapshotStart = 2,
    SnapshotChunk = 3,
    SnapshotEnd = 4,
    Resized = 5,
    Error = 6,
    Input = 7,
    Resize = 8,
    Subscribe = 9,
    Unsubscribe = 10,
    SnapshotRequest = 11,
    Metadata = 12,
    // Why 13: Metadata=12 shipped to mobile clients in v1.4.120; Ack
    // (branch-only remote-multiplex flow control) renumbers to stay
    // wire-compatible.
    Ack = 13,
    // Why 14: Ack already occupies 13 on current clients; older runtimes
    // ignore this opcode and still receive the compatibility Resize frame
    // behind it.
    ClaimViewport = 14,
    OutputSpan = 15,
}

impl TryFrom<u8> for TerminalStreamOpcode {
    type Error = ();

    // `Result<Self, Self::Error>` is ambiguous here because the enum has its
    // own `Error` variant (`O:11`) shadowing the associated type name; spell
    // the associated type out fully instead of shortening to `Self::Error`.
    fn try_from(value: u8) -> Result<Self, <Self as TryFrom<u8>>::Error> {
        // O:91-109 `isTerminalStreamOpcode`: an explicit 15-way membership
        // check, not `(1..=15).contains(&value)` — see R5's module doc.
        match value {
            1 => Ok(Self::Output),
            2 => Ok(Self::SnapshotStart),
            3 => Ok(Self::SnapshotChunk),
            4 => Ok(Self::SnapshotEnd),
            5 => Ok(Self::Resized),
            6 => Ok(Self::Error),
            7 => Ok(Self::Input),
            8 => Ok(Self::Resize),
            9 => Ok(Self::Subscribe),
            10 => Ok(Self::Unsubscribe),
            11 => Ok(Self::SnapshotRequest),
            12 => Ok(Self::Metadata),
            13 => Ok(Self::Ack),
            14 => Ok(Self::ClaimViewport),
            15 => Ok(Self::OutputSpan),
            _ => Err(()),
        }
    }
}

// ---------------------------------------------------------------------------
// Frame type — O:27-32
// ---------------------------------------------------------------------------

/// `O:27-32`. Both `streamId` and `seq` are `f64`, matching the TS `number`
/// type on both sides of the wire (R6, R9): encode accepts an arbitrary
/// (possibly negative, fractional, non-finite) float and applies the
/// field's own numeric rule; decode returns an exact `u32`/recombined
/// 64-bit value losslessly promoted to `f64`. `payload` is always an owned
/// `Vec<u8>` (R1) — never a borrowed view, even on decode.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalStreamFrame {
    pub opcode: TerminalStreamOpcode,
    pub stream_id: f64,
    pub seq: f64,
    pub payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Numeric conversions — O:41-44, O:61-62 (R6)
// ---------------------------------------------------------------------------

/// Bare ECMA-262 `ToUint32`: truncate toward zero, wrap modulo 2^32,
/// `NaN`/`±Infinity` -> 0. NO clamp — used for `streamId` (`O:41`'s
/// `setUint32(4, frame.streamId, true)` performs this conversion
/// implicitly) and, after clamp/floor is already applied by the caller, for
/// the `seq` words too (`O:43-44`'s `setUint32` calls do the same
/// conversion on already-non-negative-integer inputs).
///
/// Rust's `f64 as u32` SATURATES on out-of-range values; JS's `ToUint32`
/// WRAPS. `% 4_294_967_296.0` on the truncated value reproduces the wrap
/// (exact in `f64` for these magnitudes, same reasoning as the sibling
/// port's `encode_seq`), with a sign fixup since Rust's `%` keeps the
/// dividend's sign but `ToUint32`'s result is always in `[0, 2^32)`.
fn to_uint32(value: f64) -> u32 {
    if !value.is_finite() {
        // Covers NaN, +Infinity, -Infinity: all -> 0 per ECMA-262 ToUint32.
        return 0;
    }
    let truncated = value.trunc();
    let mut wrapped = truncated % 4_294_967_296.0;
    if wrapped < 0.0 {
        wrapped += 4_294_967_296.0;
    }
    wrapped as u32
}

/// `O:42-44`: `const seq = Math.max(0, Math.floor(frame.seq))`, then the high
/// and low 32-bit words are written via `setUint32` (which itself applies
/// `ToUint32`, see [`to_uint32`]). Unlike `streamId`, this DOES clamp to
/// `>= 0` before the uint32 conversion — R6's point that the two fields
/// have different numeric rules.
fn encode_seq_words(seq: f64) -> (u32, u32) {
    if seq.is_nan() {
        // `Math.max(0, NaN)` is `NaN` in JS (unlike Rust's NaN-ignoring
        // `f64::max`), and `ToUint32(NaN)` is 0 either way — short-circuit
        // to match JS's NaN-propagating `Math.max` semantics exactly.
        return (0, 0);
    }
    let floored = seq.floor();
    // `f64::max` here is safe: `floored` is not NaN (the NaN case returned
    // above), so Rust's NaN-ignoring `max` and JS's NaN-propagating
    // `Math.max` agree for every remaining input.
    let clamped = floored.max(0.0);
    if !clamped.is_finite() {
        // Only reachable when `seq` was `+Infinity` (`Math.floor(+Infinity)`
        // is `+Infinity`, and clamping keeps it there); `ToUint32(+Infinity)`
        // is 0 for both words.
        return (0, 0);
    }
    let high = to_uint32((clamped / 4_294_967_296.0).floor());
    let low = to_uint32(clamped);
    (high, low)
}

// ---------------------------------------------------------------------------
// Frame codec — O:34-69
// ---------------------------------------------------------------------------

/// `O:34-47`.
pub fn encode_terminal_stream_frame(frame: &TerminalStreamFrame) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES + frame.payload.len());
    out.push(TERMINAL_STREAM_KIND);
    out.push(TERMINAL_STREAM_VERSION);
    out.push(frame.opcode as u8);
    // O:40: pad byte, always written as 0. R4: never read back on decode.
    out.push(0);
    // R3: little-endian throughout.
    out.extend_from_slice(&to_uint32(frame.stream_id).to_le_bytes());
    // R2: seq splits into two 32-bit LE words, HIGH word first (lower
    // offset) — neither `to_le_bytes` nor `to_be_bytes` on the u64 as a
    // whole would produce this layout.
    let (high, low) = encode_seq_words(frame.seq);
    out.extend_from_slice(&high.to_le_bytes());
    out.extend_from_slice(&low.to_le_bytes());
    out.extend_from_slice(&frame.payload);
    out
}

/// `O:49-69`. Exactly three rejection paths (R4): length, kind/version,
/// opcode. Total — never panics.
pub fn decode_terminal_stream_frame(bytes: &[u8]) -> Option<TerminalStreamFrame> {
    // R4 rejection (1 of 3): strict `<`; a frame of exactly 16 bytes PASSES
    // this check (an empty payload). No length field exists in this
    // protocol at all, so there is no analogous overrun check to invent.
    if bytes.len() < HEADER_BYTES {
        return None;
    }
    // R4 rejection (2 of 3).
    if bytes[0] != TERMINAL_STREAM_KIND || bytes[1] != TERMINAL_STREAM_VERSION {
        return None;
    }
    // R4 rejection (3 of 3): the only other rejection path. Byte 3 (the pad
    // written by encode) is intentionally never inspected here.
    let opcode = TerminalStreamOpcode::try_from(bytes[2]).ok()?;
    // R3: little-endian throughout.
    let stream_id = u32::from_le_bytes(bytes[4..8].try_into().expect("slice of len 4"));
    // R2: bytes [8..12] are the seq HIGH word (lower offset), [12..16] the
    // LOW word — NOT a reserved field (unlike the sibling protocol).
    let high = u32::from_le_bytes(bytes[8..12].try_into().expect("slice of len 4"));
    let low = u32::from_le_bytes(bytes[12..16].try_into().expect("slice of len 4"));
    Some(TerminalStreamFrame {
        opcode,
        stream_id: stream_id as f64,
        // R9: recombined in f64 (`O:66`: `high * 0x100000000 + low`), not a
        // u64 — a u64 would be more precise than the JS source and diverge.
        seq: (high as f64) * 4_294_967_296.0 + (low as f64),
        // R1: an owned copy of the payload tail, not a view into `bytes`.
        payload: bytes[HEADER_BYTES..].to_vec(),
    })
}

// ---------------------------------------------------------------------------
// JSON value type — hand-rolled for order-preserving encode (R10)
// ---------------------------------------------------------------------------

/// Mirrors the `unknown` that `encodeTerminalStreamJson` (`O:71-73`) accepts.
/// Object fields are an ordered `Vec<(String, _)>`, NOT a map — `serde_json`
/// would need `preserve_order` (an `IndexMap`-backed `Value::Object`) to
/// keep `JSON.stringify`'s insertion order, and this crate never enables
/// that feature (see `Cargo.toml`; it would unify workspace-wide). This type
/// exists ONLY for the encode side; decode uses plain `serde_json::Value`
/// (see [`decode_terminal_stream_json`]), matching the sibling port's
/// precedent that decode never needs to observe key order.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalStreamJsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<TerminalStreamJsonValue>),
    Object(Vec<(String, TerminalStreamJsonValue)>),
}

fn encode_json_string(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn encode_json_value(value: &TerminalStreamJsonValue, out: &mut String) {
    match value {
        TerminalStreamJsonValue::Null => out.push_str("null"),
        TerminalStreamJsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        TerminalStreamJsonValue::Number(n) => {
            if n.is_finite() {
                // R10: ECMAScript `Number::toString` formatting, not `ryu`.
                out.push_str(&format_ecmascript_float(*n));
            } else {
                // `JSON.stringify(NaN)` / `JSON.stringify(Infinity)` -> `"null"`.
                out.push_str("null");
            }
        }
        TerminalStreamJsonValue::String(s) => encode_json_string(s, out),
        TerminalStreamJsonValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_json_value(item, out);
            }
            out.push(']');
        }
        TerminalStreamJsonValue::Object(fields) => {
            out.push('{');
            for (i, (key, val)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                encode_json_string(key, out);
                out.push(':');
                encode_json_value(val, out);
            }
            out.push('}');
        }
    }
}

// ---------------------------------------------------------------------------
// JSON codec — O:71-81 (R7, R8, R10)
// ---------------------------------------------------------------------------

/// `O:71-73`: `new TextEncoder().encode(JSON.stringify(value))`. Hand-rolled
/// per R10 (order-preserving keys, ECMAScript float formatting) rather than
/// `serde_json::to_vec`.
pub fn encode_terminal_stream_json(value: &TerminalStreamJsonValue) -> Vec<u8> {
    let mut out = String::new();
    encode_json_value(value, &mut out);
    out.into_bytes()
}

/// `O:75-81`. R7: NO shape check — every JSON value (object, array, number,
/// string, boolean) succeeds, not just objects; do not add a
/// `serde_json::Value::Object` match. R8: `TextDecoder`'s lossy decode +
/// leading-BOM strip runs before `JSON.parse`.
///
/// `JSON.parse("null")` and a caught parse exception are indistinguishable
/// in TS — `T:76`'s `try { JSON.parse(...) } catch { return null }` returns
/// the literal value `null` either way, and the four real call sites treat
/// them identically. This collapses both into `None`, which is faithful to
/// that ambiguity, not an extra shape check.
pub fn decode_terminal_stream_json(payload: &[u8]) -> Option<serde_json::Value> {
    // R8: lossy UTF-8 + BOM strip, same as the text codec below.
    let text = String::from_utf8_lossy(payload);
    let stripped = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    match serde_json::from_str::<serde_json::Value>(stripped) {
        Ok(serde_json::Value::Null) => None,
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Text codec — O:83-89 (R8)
// ---------------------------------------------------------------------------

/// `O:83-85`: `new TextEncoder().encode(value)`. `value` is already a valid
/// Rust `&str` (guaranteed valid UTF-8), so this is a plain byte copy.
pub fn encode_terminal_stream_text(value: &str) -> Vec<u8> {
    value.as_bytes().to_vec()
}

/// `O:87-89`: `new TextDecoder().decode(payload)`. R8: lossy (invalid UTF-8
/// -> U+FFFD, never throws/panics) and strips one leading BOM.
pub fn decode_terminal_stream_text(payload: &[u8]) -> String {
    let text = String::from_utf8_lossy(payload);
    match text.strip_prefix('\u{FEFF}') {
        Some(stripped) => stripped.to_string(),
        None => text.into_owned(),
    }
}

// ---------------------------------------------------------------------------
// R10 — ECMAScript Number::toString for the Float case
//
// Copied VERBATIM from `suaegi-mcp/src/json.rs:266` (`format_ecmascript_float`
// is a private fn there, not `pub`, so it cannot be imported — see plan
// precedent `suaegi-workname/Cargo.toml:22-24` copying `js_ws` for the same
// reason; this is now the THIRD per-module copy, alongside
// `suaegi-screencast`). Kept byte-for-byte identical, including its doc
// comment, so any future upstream fix in `suaegi-mcp` is easy to diff
// against this copy.
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

    fn frame(
        opcode: TerminalStreamOpcode,
        stream_id: f64,
        seq: f64,
        payload: Vec<u8>,
    ) -> TerminalStreamFrame {
        TerminalStreamFrame {
            opcode,
            stream_id,
            seq,
            payload,
        }
    }

    fn obj(fields: Vec<(&str, TerminalStreamJsonValue)>) -> TerminalStreamJsonValue {
        TerminalStreamJsonValue::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn num(n: f64) -> TerminalStreamJsonValue {
        TerminalStreamJsonValue::Number(n)
    }

    fn s(v: &str) -> TerminalStreamJsonValue {
        TerminalStreamJsonValue::String(v.to_string())
    }

    // =======================================================================
    // Oracle: T:13-157, all 7 cases
    // =======================================================================

    // -- oracle: T:13-28 ------------------------------------------------------

    #[test]
    fn oracle_round_trips_fixed_width_binary_frame_headers_and_payloads() {
        let payload = encode_terminal_stream_text("hello terminal");
        let encoded =
            encode_terminal_stream_frame(&frame(TerminalStreamOpcode::Output, 42.0, 9.0, payload));

        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");

        assert_eq!(decoded.opcode, TerminalStreamOpcode::Output);
        assert_eq!(decoded.stream_id, 42.0);
        assert_eq!(decoded.seq, 9.0);
        assert_eq!(
            decode_terminal_stream_text(&decoded.payload),
            "hello terminal"
        );
    }

    // -- oracle: T:30-45 ------------------------------------------------------

    #[test]
    fn oracle_round_trips_snapshot_metadata_json_payloads() {
        let payload = encode_terminal_stream_json(&obj(vec![
            ("kind", s("scrollback")),
            ("cols", num(49.0)),
            ("rows", num(28.0)),
        ]));
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::SnapshotStart,
            7.0,
            1.0,
            payload,
        ));

        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        let json =
            decode_terminal_stream_json(&decoded.payload).expect("payload must parse as JSON");

        assert_eq!(
            json,
            serde_json::json!({ "kind": "scrollback", "cols": 49, "rows": 28 })
        );
    }

    // -- oracle: T:47-69 ------------------------------------------------------

    #[test]
    fn oracle_round_trips_terminal_input_and_resize_frames() {
        let input_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Input,
            11.0,
            1.0,
            encode_terminal_stream_text("a"),
        ));
        let input = decode_terminal_stream_frame(&input_encoded).expect("valid input frame");

        let resize_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Resize,
            11.0,
            2.0,
            encode_terminal_stream_json(&obj(vec![("cols", num(120.0)), ("rows", num(40.0))])),
        ));
        let resize = decode_terminal_stream_frame(&resize_encoded).expect("valid resize frame");

        assert_eq!(input.opcode, TerminalStreamOpcode::Input);
        assert_eq!(decode_terminal_stream_text(&input.payload), "a");
        assert_eq!(resize.opcode, TerminalStreamOpcode::Resize);
        assert_eq!(
            decode_terminal_stream_json(&resize.payload).expect("resize payload must parse"),
            serde_json::json!({ "cols": 120, "rows": 40 })
        );
    }

    // -- oracle: T:71-83 ------------------------------------------------------

    #[test]
    fn oracle_round_trips_terminal_metadata_frames() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Metadata,
            11.0,
            4.0,
            encode_terminal_stream_json(&obj(vec![("cwd", s("/repo/src"))])),
        ));
        let metadata = decode_terminal_stream_frame(&encoded).expect("valid metadata frame");

        assert_eq!(metadata.opcode, TerminalStreamOpcode::Metadata);
        assert_eq!(
            decode_terminal_stream_json(&metadata.payload).expect("metadata payload must parse"),
            serde_json::json!({ "cwd": "/repo/src" })
        );
    }

    // -- oracle: T:85-124 -------------------------------------------------------

    #[test]
    fn oracle_round_trips_multiplex_subscribe_snapshot_request_and_unsubscribe_frames() {
        let subscribe_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Subscribe,
            0.0,
            1.0,
            encode_terminal_stream_json(&obj(vec![
                ("streamId", num(12.0)),
                ("terminal", s("terminal-1")),
                (
                    "viewport",
                    obj(vec![("cols", num(120.0)), ("rows", num(40.0))]),
                ),
            ])),
        ));
        let subscribe = decode_terminal_stream_frame(&subscribe_encoded).expect("valid subscribe");

        let unsubscribe_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Unsubscribe,
            12.0,
            2.0,
            Vec::new(),
        ));
        let unsubscribe =
            decode_terminal_stream_frame(&unsubscribe_encoded).expect("valid unsubscribe");

        let snapshot_request_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::SnapshotRequest,
            12.0,
            3.0,
            Vec::new(),
        ));
        let snapshot_request = decode_terminal_stream_frame(&snapshot_request_encoded)
            .expect("valid snapshot request");

        assert_eq!(subscribe.opcode, TerminalStreamOpcode::Subscribe);
        let subscribe_json =
            decode_terminal_stream_json(&subscribe.payload).expect("subscribe payload must parse");
        // Oracle uses `toMatchObject` (subset match); assert the two named
        // fields it checks rather than the full (superset) object.
        assert_eq!(subscribe_json["streamId"], serde_json::json!(12));
        assert_eq!(subscribe_json["terminal"], serde_json::json!("terminal-1"));
        assert_eq!(
            snapshot_request.opcode,
            TerminalStreamOpcode::SnapshotRequest
        );
        assert_eq!(snapshot_request.stream_id, 12.0);
        assert_eq!(unsubscribe.opcode, TerminalStreamOpcode::Unsubscribe);
        assert_eq!(unsubscribe.stream_id, 12.0);
    }

    // -- oracle: T:126-139 -----------------------------------------------------

    #[test]
    fn oracle_round_trips_output_acknowledgement_frames() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Ack,
            12.0,
            4.0,
            encode_terminal_stream_json(&obj(vec![("bytes", num(4096.0))])),
        ));
        let ack = decode_terminal_stream_frame(&encoded).expect("valid ack frame");

        assert_eq!(ack.opcode, TerminalStreamOpcode::Ack);
        assert_eq!(ack.stream_id, 12.0);
        assert_eq!(
            decode_terminal_stream_json(&ack.payload).expect("ack payload must parse"),
            serde_json::json!({ "bytes": 4096 })
        );
    }

    // -- oracle: T:141-156 -----------------------------------------------------

    #[test]
    fn oracle_rejects_unknown_frame_versions_and_opcodes() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));

        let mut bad_version = encoded.clone();
        bad_version[1] = 99;
        assert_eq!(decode_terminal_stream_frame(&bad_version), None);

        let mut bad_opcode = encoded.clone();
        bad_opcode[2] = 99;
        assert_eq!(decode_terminal_stream_frame(&bad_opcode), None);
    }

    // =======================================================================
    // Hand-written pins — oracle-silent traps (R1-R11)
    // =======================================================================

    // -- R1: owned copy, not a view -------------------------------------------

    #[test]
    fn r1_decoded_payload_is_unaffected_by_mutating_the_source_buffer_afterward() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            vec![7, 8, 9],
        ));
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.payload, vec![7, 8, 9]);

        // Mutate the ORIGINAL wire buffer's payload region after decode. A
        // view-returning port (e.g. `&'a [u8]` borrowed from `encoded`)
        // would observe this mutation; an owned `Vec<u8>` copy must not.
        let payload_start = encoded.len() - 3;
        encoded[payload_start..].copy_from_slice(&[0, 0, 0]);

        assert_eq!(
            decoded.payload,
            vec![7, 8, 9],
            "decoded payload must be an independent copy, unaffected by later \
             mutation of the source buffer (R1) — a view would fail this"
        );
    }

    // -- R2: seq word layout, high word at the LOWER offset -------------------

    #[test]
    fn r2_seq_word_layout_high_word_at_lower_offset() {
        // seq = 0x0000_0005_0000_0006: high word = 5, low word = 6.
        let seq = 5.0 * 4_294_967_296.0 + 6.0;
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            0.0,
            seq,
            Vec::new(),
        ));

        assert_eq!(&encoded[8..12], &[5, 0, 0, 0], "high word at offset 8");
        assert_eq!(&encoded[12..16], &[6, 0, 0, 0], "low word at offset 12");

        // Negative assertion: this is NOT `seq.to_le_bytes()` on the u64 as
        // a whole (which would place the LOW word first, at offset 8).
        let as_u64_le = (seq as u64).to_le_bytes();
        assert_ne!(
            &encoded[8..16],
            &as_u64_le[..],
            "seq's two words are NOT laid out as a plain `u64::to_le_bytes` \
             would produce (R2) — the high word sits at the lower offset, \
             not the low word"
        );

        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.seq, seq);
    }

    // -- R3: byte-exact little-endian ------------------------------------------

    #[test]
    fn r3_stream_id_encodes_byte_exact_little_endian() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            f64::from(0x0102_0304u32),
            0.0,
            Vec::new(),
        ));
        assert_eq!(&encoded[4..8], &[0x04, 0x03, 0x02, 0x01]);
    }

    // -- R4: exactly three rejection paths -------------------------------------

    #[test]
    fn r4_byte_three_is_a_pad_never_validated_on_decode() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));
        encoded[3] = 0xFF;
        assert!(
            decode_terminal_stream_frame(&encoded).is_some(),
            "byte 3 is a pad, written as 0 by encode but never read back on \
             decode (R4) — a corrupted pad byte must still decode"
        );
    }

    #[test]
    fn r4_bytes_twelve_to_sixteen_are_the_seq_low_word_not_a_reserved_field() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            0.0,
            Vec::new(),
        ));
        encoded[12..16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let decoded = decode_terminal_stream_frame(&encoded)
            .expect("bytes [12..16] are the seq low word, not a reserved field (R4)");
        assert_eq!(decoded.seq, 4_294_967_295.0);
    }

    #[test]
    fn r4_no_length_field_a_one_byte_payload_decodes_without_an_overrun_check() {
        // No length-prefixed sub-field exists in this protocol at all
        // (unlike the sibling's metadata length + checked_add + overrun
        // check) — a bare 17-byte frame (16-byte header + 1 payload byte)
        // decodes with no such check to invent.
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            vec![0xAB],
        ));
        assert_eq!(encoded.len(), 17);
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.payload, vec![0xAB]);
    }

    // -- R5: fifteen opcodes round-trip; boundaries dark in the oracle --------

    #[test]
    fn r5_all_fifteen_opcodes_round_trip() {
        let opcodes = [
            TerminalStreamOpcode::Output,
            TerminalStreamOpcode::SnapshotStart,
            TerminalStreamOpcode::SnapshotChunk,
            TerminalStreamOpcode::SnapshotEnd,
            TerminalStreamOpcode::Resized,
            TerminalStreamOpcode::Error,
            TerminalStreamOpcode::Input,
            TerminalStreamOpcode::Resize,
            TerminalStreamOpcode::Subscribe,
            TerminalStreamOpcode::Unsubscribe,
            TerminalStreamOpcode::SnapshotRequest,
            TerminalStreamOpcode::Metadata,
            TerminalStreamOpcode::Ack,
            TerminalStreamOpcode::ClaimViewport,
            TerminalStreamOpcode::OutputSpan,
        ];
        assert_eq!(opcodes.len(), 15);
        for (i, opcode) in opcodes.into_iter().enumerate() {
            let byte = (i + 1) as u8;
            assert_eq!(opcode as u8, byte, "opcode discriminant must equal {byte}");
            let encoded = encode_terminal_stream_frame(&frame(opcode, 1.0, 1.0, Vec::new()));
            assert_eq!(encoded[2], byte);
            let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
            assert_eq!(decoded.opcode, opcode);
        }
    }

    #[test]
    fn r5_opcode_zero_is_rejected() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));
        encoded[2] = 0;
        assert_eq!(decode_terminal_stream_frame(&encoded), None);
    }

    #[test]
    fn r5_opcode_sixteen_is_rejected() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));
        encoded[2] = 16;
        assert_eq!(decode_terminal_stream_frame(&encoded), None);
    }

    #[test]
    fn r5_zero_and_fifteen_byte_inputs_are_rejected_exactly_sixteen_is_accepted() {
        assert_eq!(decode_terminal_stream_frame(&[]), None);
        let fifteen_bytes = vec![0u8; 15];
        assert_eq!(decode_terminal_stream_frame(&fifteen_bytes), None);

        let sixteen_bytes = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));
        assert_eq!(sixteen_bytes.len(), 16);
        let decoded =
            decode_terminal_stream_frame(&sixteen_bytes).expect("exactly 16 bytes must decode");
        assert_eq!(decoded.payload, Vec::<u8>::new());
    }

    // -- R6: streamId vs seq numeric rules -------------------------------------

    #[test]
    fn r6_stream_id_negative_one_wraps_to_u32_max() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            -1.0,
            -1.0,
            Vec::new(),
        ));
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        // streamId: no clamp, wraps to 0xFFFF_FFFF.
        assert_eq!(decoded.stream_id, 4_294_967_295.0);
        // seq: clamps to >= 0 before anything else, so -1 becomes 0 — the
        // opposite of streamId's wrap for the identical input.
        assert_eq!(decoded.seq, 0.0);
    }

    #[test]
    fn r6_stream_id_fraction_truncates_toward_zero() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            9.9,
            9.9,
            Vec::new(),
        ));
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        // streamId: truncates toward zero (9.9 -> 9).
        assert_eq!(decoded.stream_id, 9.0);
        // seq: floors (9.9 -> 9) — same result here, but via a different
        // rule (floor, not trunc; they only coincide for positive inputs).
        assert_eq!(decoded.seq, 9.0);
    }

    #[test]
    fn r6_stream_id_nan_and_infinity_encode_to_zero() {
        let nan_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            f64::NAN,
            1.0,
            Vec::new(),
        ));
        let nan_decoded = decode_terminal_stream_frame(&nan_encoded).expect("valid frame");
        assert_eq!(nan_decoded.stream_id, 0.0);

        let inf_encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            f64::INFINITY,
            1.0,
            Vec::new(),
        ));
        let inf_decoded = decode_terminal_stream_frame(&inf_encoded).expect("valid frame");
        assert_eq!(inf_decoded.stream_id, 0.0);
    }

    #[test]
    fn r6_stream_id_at_or_above_two_pow_32_wraps() {
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            2f64.powi(32) + 5.0,
            1.0,
            Vec::new(),
        ));
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        // Rust `f64 as u32` would SATURATE to u32::MAX; ToUint32 WRAPS to 5.
        assert_eq!(decoded.stream_id, 5.0);
        assert_ne!(decoded.stream_id, f64::from(u32::MAX));
    }

    // -- R7: no shape check on JSON decode -------------------------------------

    #[test]
    fn r7_array_json_payload_succeeds() {
        let payload = b"[1,2]".to_vec();
        assert_eq!(
            decode_terminal_stream_json(&payload),
            Some(serde_json::json!([1, 2]))
        );
    }

    #[test]
    fn r7_number_json_payload_succeeds() {
        let payload = b"42".to_vec();
        assert_eq!(
            decode_terminal_stream_json(&payload),
            Some(serde_json::json!(42))
        );
    }

    #[test]
    fn r7_boolean_json_payload_succeeds() {
        let payload = b"true".to_vec();
        assert_eq!(
            decode_terminal_stream_json(&payload),
            Some(serde_json::json!(true))
        );
    }

    #[test]
    fn r7_null_json_payload_and_parse_failure_both_collapse_to_none() {
        // `JSON.parse("null")` and a caught parse exception both surface as
        // the literal value `null` in TS — deliberately indistinguishable.
        assert_eq!(decode_terminal_stream_json(b"null"), None);
        assert_eq!(decode_terminal_stream_json(b"not json at all{{{"), None);
    }

    // -- R8: BOM + invalid UTF-8, both codecs -----------------------------------

    #[test]
    fn r8_text_decode_strips_a_leading_bom() {
        let mut payload = "\u{FEFF}".as_bytes().to_vec();
        payload.extend_from_slice(b"hello");
        assert_eq!(decode_terminal_stream_text(&payload), "hello");
    }

    #[test]
    fn r8_text_decode_replaces_invalid_utf8_with_u_fffd() {
        assert_eq!(decode_terminal_stream_text(&[0xFF]), "\u{FFFD}");
    }

    #[test]
    fn r8_json_decode_strips_a_leading_bom() {
        let mut payload = "\u{FEFF}".as_bytes().to_vec();
        payload.extend_from_slice(br#"{"a":1}"#);
        assert_eq!(
            decode_terminal_stream_json(&payload),
            Some(serde_json::json!({ "a": 1 }))
        );
    }

    #[test]
    fn r8_json_decode_lossily_repairs_invalid_utf8_inside_a_string() {
        let mut payload = br#"{"note":""#.to_vec();
        payload.push(0xFF);
        payload.extend_from_slice(br#"","a":5}"#);
        let decoded = decode_terminal_stream_json(&payload).expect("must still parse");
        assert_eq!(decoded["a"], serde_json::json!(5));
        assert_eq!(decoded["note"], serde_json::json!("\u{FFFD}"));
    }

    // -- R10: key order + ECMAScript float formatting on the wire --------------

    #[test]
    fn r10_object_keys_encode_in_insertion_order_not_sorted() {
        // Reverse-alphabetical insertion order; a `BTreeMap`-backed
        // serializer would re-sort these to `a,b,c`.
        let bytes = encode_terminal_stream_json(&obj(vec![
            ("zebra", num(1.0)),
            ("mango", num(2.0)),
            ("apple", num(3.0)),
        ]));
        assert_eq!(bytes, br#"{"zebra":1,"mango":2,"apple":3}"#);
    }

    #[test]
    fn r10_integer_valued_float_encodes_without_a_trailing_point_zero() {
        let bytes = encode_terminal_stream_json(&num(120.0));
        assert_eq!(
            bytes, b"120",
            "ryu would print \"120.0\"; JS prints \"120\""
        );
    }

    #[test]
    fn r10_negative_zero_encodes_as_bare_zero() {
        let bytes = encode_terminal_stream_json(&num(-0.0));
        assert_eq!(bytes, b"0", "ryu would print \"-0.0\"; JS prints \"0\"");
    }

    #[test]
    fn r10_1e21_switches_to_exponential_notation() {
        let bytes = encode_terminal_stream_json(&num(1e21));
        assert_eq!(bytes, b"1e+21");
    }

    // -- corrupted kind byte ----------------------------------------------------

    #[test]
    fn corrupted_kind_byte_is_rejected() {
        let mut encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            1.0,
            Vec::new(),
        ));
        encoded[0] = 0x75;
        assert_eq!(decode_terminal_stream_frame(&encoded), None);
    }

    // -- R9: seq recombination stays in f64 range -------------------------------

    #[test]
    fn r9_seq_recombines_as_f64_not_u64() {
        // A seq value whose high word is nonzero, confirming the full
        // 64-bit range round-trips through f64 recombination.
        let seq = 3.0 * 4_294_967_296.0 + 100.0;
        let encoded = encode_terminal_stream_frame(&frame(
            TerminalStreamOpcode::Output,
            1.0,
            seq,
            Vec::new(),
        ));
        let decoded = decode_terminal_stream_frame(&encoded).expect("valid frame");
        assert_eq!(decoded.seq, seq);
    }

    // =======================================================================
    // Copied verbatim from `suaegi-mcp/src/json.rs` — numeric formatter tests
    // =======================================================================

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
    fn w5_negative_zero_is_the_string_zero() {
        assert_eq!(ecmascript_float(-0.0_f64), "0");
    }

    #[test]
    fn w5_100_has_no_decimal_point() {
        assert_eq!(ecmascript_float(100.0), "100");
    }

    #[test]
    fn w5_1_5_keeps_a_single_fractional_digit() {
        assert_eq!(ecmascript_float(1.5), "1.5");
    }
}
