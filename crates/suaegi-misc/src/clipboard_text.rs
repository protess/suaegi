//! Clipboard text byte-length limits — verbatim port of Orca's
//! `src/shared/clipboard-text.ts` (@ v1.4.150-rc.0).
//!
//! Guards clipboard reads/writes against oversized payloads by measuring
//! **UTF-8 byte length** (not JS UTF-16 code-unit / Rust scalar count) and
//! comparing it to a configurable limit. `repo-icon.ts` (a sibling module in
//! the same source directory) is deliberately **not** ported here — it needs
//! WHATWG URL parsing for a credential-confusion security boundary, which
//! would require adding a dependency to this otherwise dependency-free crate;
//! that is a separate, later decision.
//!
//! Nine contract decisions were made porting this module (see the plan doc
//! for the full rationale); each is noted at its point of relevance below:
//!
//! - **S1** — the four `...WithYield` TS functions are `async`, but this crate
//!   has no runtime and adds no dependency, so they become **synchronous**
//!   functions taking an injected `yield_to_event_loop: &mut dyn FnMut()`.
//!   The portable content is the **yield cadence calculation** (how often to
//!   call it); actually yielding to an event loop is the **caller's**
//!   responsibility — call this from an async context and have the closure
//!   bridge back into your own executor (e.g. a channel send, or a
//!   `block_on`-friendly waker poke).
//! - **S2** — `max_bytes` inputs are `Option<f64>` to reproduce JS numeric
//!   coercion exactly: `Number.isFinite(v) && v > 0` picks `v.floor()`,
//!   otherwise the fallback is used. So `None`, `NaN`, `±Infinity`, `0.0`, and
//!   negative values all fall back to the caller-supplied default; `0.5`
//!   floors to `0` (an effective limit of zero bytes, rejecting every
//!   non-empty text).
//! - **S3/S4** — errors are exposed both as a typed [`ClipboardTextError`]
//!   enum (this crate's own errors) **and** as free string predicates
//!   ([`is_clipboard_text_too_large_message`],
//!   [`is_clipboard_text_write_too_large_message`]) that substring-match an
//!   arbitrary message, mirroring the TS predicates which classify *any*
//!   `unknown` error by its `.message`. The two predicates are **disjoint**
//!   and **case-sensitive** substring checks — neither message is a substring
//!   of the other. Do NOT reuse [`crate::remote_runtime_error`]'s message
//!   matcher for this: that one lowercases first, which is wrong here.
//! - **S5** — `get_clipboard_text_byte_length` is exactly `text.len()` (the
//!   UTF-8 byte count of a well-formed `&str`). `is_clipboard_text_byte_length_over_limit`
//!   is exactly `text.len() as u64 > max_bytes`. The TS fast path
//!   (`text.length > maxBytes`, a UTF-16 **code-unit** count) is a strictly
//!   looser, *sound* over-approximation there (every code point's UTF-8 byte
//!   count is ≥ its UTF-16 unit count, so `codeUnitCount > maxBytes` implies
//!   `byteCount > maxBytes`) that TS ORs with a full scan only to skip
//!   scanning large inputs; in Rust `text.len()` is already the exact answer
//!   in O(1), so there is nothing left to approximate or scan, and this fast
//!   path is unobservable for the **boolean-returning** functions. The
//!   spy-based oracle case that observes "no scan happened" via
//!   `String.prototype.codePointAt` is skipped for exactly this reason (see
//!   the comment on the skipped test below). The `...WithYield` sibling is
//!   different: see the comment on
//!   [`is_clipboard_text_byte_length_over_limit_with_yield`].
//!   `measure_clipboard_text_byte_length` still needs the incremental
//!   `chars()` + `len_utf8()` loop, because on early stop it must return a
//!   **partial sum that includes the overflowing character** (the oracle
//!   expects `8`, not the pre-overflow `5`, when stopping mid-emoji). The
//!   stop comparison is strict `>`: landing exactly on the limit is not over.
//! - **S6** — `stop_after_bytes` is also `Option<f64>` with JS semantics:
//!   `Number.isFinite(v) && byteLength > v`. `None`/`NaN`/`Infinity` never
//!   stop early; `0.0` stops at the first character; negative values stop
//!   immediately (as soon as any byte has accumulated). An empty string never
//!   enters the loop, so it always yields `{0, false}` regardless of
//!   `stop_after_bytes`.
//! - **S7** — yield cadence: `yield_after_code_units = max(1, opt.unwrap_or(262144))`,
//!   so `0` becomes `1`. After each character, `next_yield_at` is
//!   **recomputed from the current index** (`index + cadence`), not
//!   incremented (`+=`), which is what keeps the cadence from drifting on
//!   astral input. The index advances in **UTF-16 units**
//!   (`ch.len_utf16()`), matching the original JS `for` loop's code-unit
//!   index, and the yield check happens **after** advancing past the
//!   character — this combination is what makes the "524289 units → exactly
//!   2 yields" oracle case come out right.
//! - **S8** — this module **never trims**. The four `assert_*` functions
//!   return the input **verbatim** (as a borrowed `&str` slice of the input)
//!   when it is within limit — no trimming, no truncation, no normalization.
//!   Several neighbouring modules in this crate DO trim (via
//!   [`crate::js_ws::js_trim`]); this one deliberately does not.
//! - **S9** — error messages are fixed, payload-free constants
//!   ([`CLIPBOARD_TEXT_TOO_LARGE_ERROR`], [`CLIPBOARD_TEXT_WRITE_TOO_LARGE_ERROR`]).
//!   They never include the rejected text, only forward-looking metadata.

/// Default byte limit for clipboard **reads**, in bytes (16 MiB).
pub const CLIPBOARD_TEXT_READ_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Default byte limit for clipboard **writes**, in bytes (16 MiB).
pub const CLIPBOARD_TEXT_WRITE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Error message thrown/returned when a clipboard **read** exceeds its limit.
pub const CLIPBOARD_TEXT_TOO_LARGE_ERROR: &str =
    "Clipboard text is too large for this paste target.";

/// Error message thrown/returned when a clipboard **write** exceeds its limit.
pub const CLIPBOARD_TEXT_WRITE_TOO_LARGE_ERROR: &str =
    "Clipboard text is too large to copy safely.";

/// Default yield cadence, in UTF-16 code units, for the `...WithYield`
/// variants (256 KiB worth of code units).
pub const CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS: u64 = 256 * 1024;

/// Result of measuring a text's UTF-8 byte length, possibly stopped early.
///
/// `byte_length` is the number of UTF-8 bytes counted so far. When
/// `exceeded_limit` is `true` and measurement stopped early (see
/// [`measure_clipboard_text_byte_length`]'s `stop_after_bytes`), the count
/// includes the byte length of the character that pushed it over the limit
/// (a **partial sum**, not the pre-overflow total).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardTextByteLengthMeasurement {
    pub byte_length: u64,
    pub exceeded_limit: bool,
}

/// Error from the `assert_*` functions in this module. The `Display` message
/// is one of the two fixed, payload-free constants (S9) — never the rejected
/// text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTextError {
    /// A clipboard **read** exceeded its byte limit.
    TooLarge,
    /// A clipboard **write** exceeded its byte limit.
    WriteTooLarge,
}

impl core::fmt::Display for ClipboardTextError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            ClipboardTextError::TooLarge => CLIPBOARD_TEXT_TOO_LARGE_ERROR,
            ClipboardTextError::WriteTooLarge => CLIPBOARD_TEXT_WRITE_TOO_LARGE_ERROR,
        })
    }
}

impl std::error::Error for ClipboardTextError {}

/// `true` if `stop_after_bytes` is a finite number and `byte_length` has
/// already exceeded it (S6). `None`/non-finite (`NaN`, `±Infinity`) never
/// stop; `Some(0.0)` or negative values stop as soon as any bytes have
/// accumulated.
fn stop_after_bytes_exceeded(stop_after_bytes: Option<f64>, byte_length: u64) -> bool {
    match stop_after_bytes {
        Some(limit) if limit.is_finite() => (byte_length as f64) > limit,
        _ => false,
    }
}

/// Measure the UTF-8 byte length of `text`, optionally stopping early once
/// `stop_after_bytes` (S6 semantics) is exceeded.
///
/// On early stop, `byte_length` is a **partial sum that includes the
/// overflowing character** — e.g. stopping a scan of `"😀".repeat(100)`
/// (4 bytes/char) at `stop_after_bytes: 5.0` yields `{ byte_length: 8,
/// exceeded_limit: true }`, not `5`.
pub fn measure_clipboard_text_byte_length(
    text: &str,
    stop_after_bytes: Option<f64>,
) -> ClipboardTextByteLengthMeasurement {
    let mut byte_length: u64 = 0;
    for ch in text.chars() {
        byte_length += ch.len_utf8() as u64;
        if stop_after_bytes_exceeded(stop_after_bytes, byte_length) {
            return ClipboardTextByteLengthMeasurement {
                byte_length,
                exceeded_limit: true,
            };
        }
    }
    ClipboardTextByteLengthMeasurement {
        byte_length,
        exceeded_limit: false,
    }
}

/// The plain UTF-8 byte length of `text` (S5: exactly `text.len()`).
pub fn get_clipboard_text_byte_length(text: &str) -> u64 {
    text.len() as u64
}

/// Same as [`measure_clipboard_text_byte_length`], but calls
/// `yield_to_event_loop` at a computed cadence while scanning (S1, S7).
///
/// `yield_after_code_units` sets the cadence in UTF-16 code units; `None`
/// falls back to [`CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS`], and the
/// effective cadence is always at least `1` (`0` behaves as `1`, matching the
/// JS `Math.max(1, ...)` guard).
pub fn measure_clipboard_text_byte_length_with_yield(
    text: &str,
    stop_after_bytes: Option<f64>,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> ClipboardTextByteLengthMeasurement {
    let cadence = yield_after_code_units
        .unwrap_or(CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS)
        .max(1);
    let mut next_yield_at = cadence;
    let mut index: u64 = 0;
    let mut byte_length: u64 = 0;

    for ch in text.chars() {
        byte_length += ch.len_utf8() as u64;
        if stop_after_bytes_exceeded(stop_after_bytes, byte_length) {
            return ClipboardTextByteLengthMeasurement {
                byte_length,
                exceeded_limit: true,
            };
        }
        index += ch.len_utf16() as u64;
        if index >= next_yield_at {
            yield_to_event_loop();
            // Recomputed from the CURRENT index, not `+=`, so the cadence
            // does not drift when astral characters advance the index by 2
            // units at a time (S7).
            next_yield_at = index + cadence;
        }
    }
    ClipboardTextByteLengthMeasurement {
        byte_length,
        exceeded_limit: false,
    }
}

/// `true` if `text`'s UTF-8 byte length exceeds `max_bytes` (strict `>`;
/// landing exactly on the limit is not over).
///
/// S5: this is exactly `text.len() as u64 > max_bytes`. The original TS
/// `text.length > maxBytes || measure(...).exceededLimit` OR was an
/// optimization to avoid a full scan of large inputs, using a UTF-16
/// code-unit count as a sound (but looser) pre-check. In Rust, `text.len()`
/// is already the exact UTF-8 byte count computed in O(1) (`&str` tracks its
/// byte length), so there is nothing left to approximate or scan — the
/// UTF-16 fast path is unobservable here and is not ported. The spy-based
/// oracle case asserting `codePointAt` was never called (i.e. "no scan
/// happened") is skipped for the same reason: this implementation never
/// scans regardless of input.
pub fn is_clipboard_text_byte_length_over_limit(text: &str, max_bytes: u64) -> bool {
    text.len() as u64 > max_bytes
}

/// Same as [`is_clipboard_text_byte_length_over_limit`], but yields while
/// scanning multibyte text before rejecting it (S1).
///
/// Unlike the synchronous sibling above, this function's UTF-16 fast path
/// (`text.encode_utf16().count() as u64 > max_bytes`, mirroring the TS
/// `text.length > maxBytes` exactly) is **not** replaceable by
/// `text.len() > max_bytes`: doing so would change *observable* behavior,
/// because it would skip the scan (and every yield call) whenever the exact
/// byte length exceeds the limit — even when the looser UTF-16 count does
/// not. The oracle exercises exactly this gap: `"é".repeat(16)` has 16 UTF-16
/// units (≤ the 31-byte limit, so the fast path here does NOT short-circuit)
/// but 32 UTF-8 bytes (over the limit), so the full scan runs and yields at
/// least once before returning `true`. Using the exact byte length as the
/// fast path would return `true` immediately with zero yields, breaking that
/// case.
pub fn is_clipboard_text_byte_length_over_limit_with_yield(
    text: &str,
    max_bytes: u64,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> bool {
    if text.encode_utf16().count() as u64 > max_bytes {
        return true;
    }
    measure_clipboard_text_byte_length_with_yield(
        text,
        Some(max_bytes as f64),
        yield_after_code_units,
        yield_to_event_loop,
    )
    .exceeded_limit
}

/// Resolve a caller-supplied `max_bytes` option against JS numeric coercion
/// rules (S2): `Number.isFinite(v) && v > 0 ? floor(v) : fallback`.
fn resolve_clipboard_text_max_bytes(max_bytes: Option<f64>, fallback: u64) -> u64 {
    match max_bytes {
        Some(value) if value.is_finite() && value > 0.0 => value.floor() as u64,
        _ => fallback,
    }
}

/// Resolve the effective read byte limit: `max_bytes` if it is a finite,
/// positive number (floored), otherwise `fallback`.
pub fn get_clipboard_text_read_max_bytes(max_bytes: Option<f64>, fallback: u64) -> u64 {
    resolve_clipboard_text_max_bytes(max_bytes, fallback)
}

/// Resolve the effective write byte limit: `max_bytes` if it is a finite,
/// positive number (floored), otherwise `fallback`.
pub fn get_clipboard_text_write_max_bytes(max_bytes: Option<f64>, fallback: u64) -> u64 {
    resolve_clipboard_text_max_bytes(max_bytes, fallback)
}

/// Return `text` verbatim (S8: no trimming) if its byte length is within the
/// resolved read limit, else [`ClipboardTextError::TooLarge`].
pub fn assert_clipboard_text_within_limit(
    text: &str,
    max_bytes: Option<f64>,
) -> Result<&str, ClipboardTextError> {
    let limit = get_clipboard_text_read_max_bytes(max_bytes, CLIPBOARD_TEXT_READ_MAX_BYTES);
    if is_clipboard_text_byte_length_over_limit(text, limit) {
        return Err(ClipboardTextError::TooLarge);
    }
    Ok(text)
}

/// Yielding variant of [`assert_clipboard_text_within_limit`] (S1).
pub fn assert_clipboard_text_within_limit_with_yield<'a>(
    text: &'a str,
    max_bytes: Option<f64>,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> Result<&'a str, ClipboardTextError> {
    let limit = get_clipboard_text_read_max_bytes(max_bytes, CLIPBOARD_TEXT_READ_MAX_BYTES);
    if is_clipboard_text_byte_length_over_limit_with_yield(
        text,
        limit,
        yield_after_code_units,
        yield_to_event_loop,
    ) {
        return Err(ClipboardTextError::TooLarge);
    }
    Ok(text)
}

/// Return `text` verbatim (S8: no trimming) if its byte length is within the
/// resolved write limit, else [`ClipboardTextError::WriteTooLarge`].
pub fn assert_clipboard_text_write_within_limit(
    text: &str,
    max_bytes: Option<f64>,
) -> Result<&str, ClipboardTextError> {
    let limit = get_clipboard_text_write_max_bytes(max_bytes, CLIPBOARD_TEXT_WRITE_MAX_BYTES);
    if is_clipboard_text_byte_length_over_limit(text, limit) {
        return Err(ClipboardTextError::WriteTooLarge);
    }
    Ok(text)
}

/// Yielding variant of [`assert_clipboard_text_write_within_limit`] (S1).
pub fn assert_clipboard_text_write_within_limit_with_yield<'a>(
    text: &'a str,
    max_bytes: Option<f64>,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> Result<&'a str, ClipboardTextError> {
    let limit = get_clipboard_text_write_max_bytes(max_bytes, CLIPBOARD_TEXT_WRITE_MAX_BYTES);
    if is_clipboard_text_byte_length_over_limit_with_yield(
        text,
        limit,
        yield_after_code_units,
        yield_to_event_loop,
    ) {
        return Err(ClipboardTextError::WriteTooLarge);
    }
    Ok(text)
}

/// `true` if `message` contains the fixed read-too-large error text.
/// Case-sensitive substring match (S3/S4) — mirrors the TS
/// `isClipboardTextTooLargeError`, but operating on an already-extracted
/// `&str` message rather than sniffing an `unknown` error value (that
/// sniffing is the caller's job; see [`crate::remote_runtime_error`] for the
/// analogous pattern with a different case-sensitivity rule).
pub fn is_clipboard_text_too_large_message(message: &str) -> bool {
    message.contains(CLIPBOARD_TEXT_TOO_LARGE_ERROR)
}

/// `true` if `message` contains the fixed write-too-large error text.
/// Case-sensitive substring match (S3/S4); disjoint from
/// [`is_clipboard_text_too_large_message`] — neither error message is a
/// substring of the other.
pub fn is_clipboard_text_write_too_large_message(message: &str) -> bool {
    message.contains(CLIPBOARD_TEXT_WRITE_TOO_LARGE_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: clipboard-text.test.ts (spy-based fast-path case at test:82-93
    // intentionally excluded — see the doc comment on
    // `is_clipboard_text_byte_length_over_limit` for why it is unobservable
    // in this port).

    #[test]
    fn measures_utf8_bytes_instead_of_utf16_code_units() {
        assert_eq!(get_clipboard_text_byte_length("a😀"), 5);
    }

    #[test]
    fn can_stop_measuring_once_a_byte_limit_is_exceeded() {
        let full = "😀".repeat(100);
        let measurement = measure_clipboard_text_byte_length(&full, Some(5.0));
        assert_eq!(
            measurement,
            ClipboardTextByteLengthMeasurement {
                byte_length: 8,
                exceeded_limit: true,
            }
        );
        assert!(measurement.byte_length < get_clipboard_text_byte_length(&full));
    }

    #[test]
    fn detects_text_over_a_byte_limit_without_requiring_full_measurement() {
        assert!(is_clipboard_text_byte_length_over_limit(
            &"😀".repeat(100),
            5
        ));
        assert!(!is_clipboard_text_byte_length_over_limit("éé", 4));
    }

    #[test]
    fn yields_while_measuring_large_accepted_clipboard_text() {
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let measurement = measure_clipboard_text_byte_length_with_yield(
            &"x".repeat(32),
            Some(64.0),
            Some(8),
            &mut yield_to_event_loop,
        );
        assert_eq!(
            measurement,
            ClipboardTextByteLengthMeasurement {
                byte_length: 32,
                exceeded_limit: false,
            }
        );
        assert_eq!(calls, 4);
    }

    #[test]
    fn uses_the_default_256k_code_unit_yield_cadence_for_accepted_large_clipboard_text() {
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let text = "x".repeat((CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS * 2 + 1) as usize);
        let measurement = measure_clipboard_text_byte_length_with_yield(
            &text,
            None,
            None,
            &mut yield_to_event_loop,
        );
        assert!(!measurement.exceeded_limit);
        assert_eq!(calls, 2);
    }

    #[test]
    fn yields_while_checking_multibyte_clipboard_limits_before_rejecting() {
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let over_limit = is_clipboard_text_byte_length_over_limit_with_yield(
            &"é".repeat(16),
            31,
            Some(4),
            &mut yield_to_event_loop,
        );
        assert!(over_limit);
        assert_eq!(calls, 3);
    }

    #[test]
    fn lets_each_clipboard_consumer_override_the_shared_default_byte_limits() {
        assert_eq!(
            assert_clipboard_text_within_limit("abc", Some(3.0)),
            Ok("abc")
        );
        assert_eq!(
            assert_clipboard_text_within_limit("abc", Some(2.0)),
            Err(ClipboardTextError::TooLarge)
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit("copy", Some(4.0)),
            Ok("copy")
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit("copy", Some(3.0)),
            Err(ClipboardTextError::WriteTooLarge)
        );

        let mut noop = || {};
        assert_eq!(
            assert_clipboard_text_within_limit_with_yield("async", Some(5.0), None, &mut noop),
            Ok("async")
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit_with_yield(
                "async",
                Some(4.0),
                None,
                &mut noop
            ),
            Err(ClipboardTextError::WriteTooLarge)
        );
    }

    #[test]
    fn rejects_oversized_text_with_a_metadata_only_error() {
        let payload = "secret-token-value";
        let err = assert_clipboard_text_within_limit(payload, Some(4.0)).unwrap_err();
        assert_eq!(err, ClipboardTextError::TooLarge);
        assert!(is_clipboard_text_too_large_message(&err.to_string()));
        assert!(!err.to_string().contains(payload));
    }

    #[test]
    fn rejects_oversized_async_clipboard_reads_and_writes_with_metadata_only_errors() {
        let mut noop = || {};
        let read_payload = "secret-token-value";
        let write_payload = "copied-secret-token-value";

        assert_eq!(
            assert_clipboard_text_within_limit_with_yield(read_payload, Some(4.0), None, &mut noop),
            Err(ClipboardTextError::TooLarge)
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit_with_yield(
                write_payload,
                Some(4.0),
                None,
                &mut noop
            ),
            Err(ClipboardTextError::WriteTooLarge)
        );

        let err =
            assert_clipboard_text_within_limit_with_yield(read_payload, Some(4.0), None, &mut noop)
                .unwrap_err();
        assert!(is_clipboard_text_too_large_message(&err.to_string()));
        assert!(!err.to_string().contains(read_payload));
    }

    #[test]
    fn rejects_oversized_clipboard_writes_with_a_metadata_only_error() {
        let payload = "copied-secret-token-value";
        let err = assert_clipboard_text_write_within_limit(payload, Some(4.0)).unwrap_err();
        assert_eq!(err, ClipboardTextError::WriteTooLarge);
        assert!(is_clipboard_text_write_too_large_message(&err.to_string()));
        // S4: the two predicates are disjoint — a write error never matches
        // the read predicate.
        assert!(!is_clipboard_text_too_large_message(&err.to_string()));
        assert!(!err.to_string().contains(payload));
    }

    // Mandatory extra pins (oracle-silent):

    /// S2: every branch of `get_clipboard_text_read_max_bytes` /
    /// `get_clipboard_text_write_max_bytes` — the oracle never imports either
    /// function, so nothing here is covered elsewhere.
    #[test]
    fn pin_max_bytes_resolution_covers_every_js_numeric_branch() {
        let fallback = CLIPBOARD_TEXT_READ_MAX_BYTES;
        assert_eq!(get_clipboard_text_read_max_bytes(None, fallback), fallback);
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(f64::NAN), fallback),
            fallback
        );
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(f64::INFINITY), fallback),
            fallback
        );
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(f64::NEG_INFINITY), fallback),
            fallback
        );
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(0.0), fallback),
            fallback
        );
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(-5.0), fallback),
            fallback
        );
        // 0.5 floors to 0 — an effective limit of zero, not "no limit".
        assert_eq!(get_clipboard_text_read_max_bytes(Some(0.5), fallback), 0);
        assert_eq!(
            get_clipboard_text_read_max_bytes(Some(1000.7), fallback),
            1000
        );

        let write_fallback = CLIPBOARD_TEXT_WRITE_MAX_BYTES;
        assert_eq!(
            get_clipboard_text_write_max_bytes(None, write_fallback),
            write_fallback
        );
        assert_eq!(
            get_clipboard_text_write_max_bytes(Some(f64::NAN), write_fallback),
            write_fallback
        );
        assert_eq!(
            get_clipboard_text_write_max_bytes(Some(0.0), write_fallback),
            write_fallback
        );
        assert_eq!(
            get_clipboard_text_write_max_bytes(Some(-1.0), write_fallback),
            write_fallback
        );
        assert_eq!(
            get_clipboard_text_write_max_bytes(Some(0.5), write_fallback),
            0
        );
        assert_eq!(
            get_clipboard_text_write_max_bytes(Some(42.9), write_fallback),
            42
        );
    }

    /// S2: the fallback constants really are 16 MiB.
    #[test]
    fn pin_default_fallback_constants_are_16_mebibytes() {
        assert_eq!(CLIPBOARD_TEXT_READ_MAX_BYTES, 16 * 1024 * 1024);
        assert_eq!(CLIPBOARD_TEXT_WRITE_MAX_BYTES, 16 * 1024 * 1024);
    }

    /// S6: `stop_after_bytes` of `None`/`0.0`/negative/`NaN`.
    #[test]
    fn pin_stop_after_bytes_covers_every_js_numeric_branch() {
        // None: never stops, full scan.
        assert_eq!(
            measure_clipboard_text_byte_length("abc", None),
            ClipboardTextByteLengthMeasurement {
                byte_length: 3,
                exceeded_limit: false,
            }
        );
        // 0.0: stops at the first character (1 byte > 0.0).
        assert_eq!(
            measure_clipboard_text_byte_length("abc", Some(0.0)),
            ClipboardTextByteLengthMeasurement {
                byte_length: 1,
                exceeded_limit: true,
            }
        );
        // Negative: stops immediately (1 byte > -5.0).
        assert_eq!(
            measure_clipboard_text_byte_length("abc", Some(-5.0)),
            ClipboardTextByteLengthMeasurement {
                byte_length: 1,
                exceeded_limit: true,
            }
        );
        // NaN is not finite: never stops, full scan.
        assert_eq!(
            measure_clipboard_text_byte_length("abc", Some(f64::NAN)),
            ClipboardTextByteLengthMeasurement {
                byte_length: 3,
                exceeded_limit: false,
            }
        );
    }

    /// S6: `Some(f64::NEG_INFINITY)` is the ONLY value that discriminates the
    /// `is_finite()` guard in `stop_after_bytes_exceeded`. Deleting that guard
    /// (turning `Some(limit) if limit.is_finite() => ...` into bare
    /// `Some(limit) => ...`) passes every other pin in this suite:
    /// - `NaN`: any comparison with NaN is `false`, so `byte_length > NaN`
    ///   never stops the scan either way — the guard is a no-op there.
    /// - `+Infinity`: `byte_length > inf` is always `false`, so again both
    ///   versions never stop.
    /// - a finite negative (e.g. `-5.0`, already pinned above): `is_finite`
    ///   is `true` for it, so BOTH versions stop immediately — no
    ///   observable difference.
    ///
    /// Only `-Infinity` differs: `byte_length > -inf` is `true`, so
    /// *without* the guard the scan would stop after the first byte, while
    /// *with* the guard (matching JS `Number.isFinite(-Infinity) === false`)
    /// it must never stop, behaving exactly like `None`.
    #[test]
    fn pin_stop_after_bytes_neg_infinity_never_stops_like_none() {
        let measurement = measure_clipboard_text_byte_length("abc", Some(f64::NEG_INFINITY));
        assert_eq!(
            measurement,
            ClipboardTextByteLengthMeasurement {
                byte_length: 3,
                exceeded_limit: false,
            }
        );
    }

    /// S6: an empty string never enters the loop, so it is always
    /// `{0, false}` regardless of `stop_after_bytes`.
    #[test]
    fn pin_empty_text_never_exceeds_any_stop_after_bytes() {
        for stop in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
            assert_eq!(
                measure_clipboard_text_byte_length("", stop),
                ClipboardTextByteLengthMeasurement {
                    byte_length: 0,
                    exceeded_limit: false,
                }
            );
        }
    }

    /// S7: cadence `0` behaves as `1`, not as "unset" (which would fall back
    /// to the 256 KiB default). A naive `if opt == Some(0) { default } else
    /// { opt }`-style bug (conflating "explicit zero" with "absent") would
    /// make a 5-character text never reach the cadence at all (0 yields);
    /// the correct `max(1, 0) = 1` yields on every character (5 yields).
    #[test]
    fn pin_zero_yield_cadence_behaves_as_one_not_as_absent() {
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let measurement = measure_clipboard_text_byte_length_with_yield(
            "abcde",
            None,
            Some(0),
            &mut yield_to_event_loop,
        );
        assert!(!measurement.exceeded_limit);
        assert_eq!(calls, 5);
    }

    /// S8: the four `assert_*` functions never trim — leading/trailing
    /// whitespace, including U+FEFF (byte-order mark / ZERO WIDTH NO-BREAK
    /// SPACE), passes through byte-for-byte.
    #[test]
    fn pin_assert_functions_never_trim_whitespace_or_feff() {
        let text = "  \u{FEFF}hello\u{FEFF}  ";
        assert_eq!(assert_clipboard_text_within_limit(text, None), Ok(text));
        assert_eq!(
            assert_clipboard_text_write_within_limit(text, None),
            Ok(text)
        );
        let mut noop = || {};
        assert_eq!(
            assert_clipboard_text_within_limit_with_yield(text, None, None, &mut noop),
            Ok(text)
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit_with_yield(text, None, None, &mut noop),
            Ok(text)
        );
    }

    /// Exactly-at-limit passes (strict `>`, not `>=`) for the `assert_*`
    /// entry points, not just the lower-level predicate.
    #[test]
    fn pin_assert_functions_accept_text_exactly_at_the_limit() {
        assert_eq!(
            assert_clipboard_text_within_limit("éé", Some(4.0)),
            Ok("éé")
        );
        assert_eq!(
            assert_clipboard_text_write_within_limit("éé", Some(4.0)),
            Ok("éé")
        );
    }
}
