//! Terminal input byte-budget guards — verbatim port of Orca's
//! `src/shared/terminal-input.ts` (109 lines, @ v1.4.146-rc.0).
//!
//! Guards PTY writes against oversized or unbounded-latency sends: a hard
//! total-size cap ([`TERMINAL_INPUT_MAX_BYTES`], `assert`/`is_too_large`
//! family) and a lazy per-chunk cap ([`TERMINAL_INPUT_CHUNK_MAX_BYTES`],
//! `iterate`/`split` family) so a large paste gets fed to the PTY in
//! bounded-size writes instead of one giant one. Reuses
//! `suaegi-misc::clipboard_text`'s byte-accounting primitives (see that
//! crate's own module doc for its S1–S9 decisions); does not depend on the
//! `suaegi-native-file-drop`-analogous crates.
//!
//! Fourteen contract decisions were made porting this module (`docs/superpowers/plans/2026-07-27-terminal-input.md`);
//! each is noted at its point of relevance below with a `U<N>` tag, plus two
//! provably-equivalent mutations (`E1`/`E2`) called out where they occur.
//!
//! - **U1** — ⚠⚠ Chunk boundaries are Unicode-**scalar**-atomic, but the
//!   *budget* is bytes while the source's own cursor arithmetic is UTF-16
//!   (`TI:95`: `codePoint > 0xffff ? 2 : 1`). A cut can never land inside a
//!   multi-byte UTF-8 sequence or a surrogate pair. In Rust, `char_indices()`
//!   already enumerates one Unicode scalar at a time regardless of its UTF-16
//!   width, so the source's `codeUnitLength`/`nextIndex` bookkeeping (needed
//!   only because JS strings are UTF-16 arrays) has no Rust analogue — we
//!   just track byte offsets from `char_indices()` and slice at recorded
//!   offsets only, never at a raw byte-budget count (`&text[..n]` on an
//!   arbitrary `n` panics on a non-char-boundary). This module does **not**
//!   upgrade to grapheme-cluster atomicity: combining marks, ZWJ sequences,
//!   CRLF pairs, and ANSI escape sequences are deliberately split across
//!   chunk boundaries, matching the source exactly.
//! - **U2** — ⚠ Chunks **can exceed** `max_chunk_bytes`. The loop guard is
//!   `current_bytes > 0 && current_bytes + character_bytes > normalized_max`
//!   (`TI:98`): a single oversized code point is admitted whole into an
//!   otherwise-empty chunk (up to `max(max_chunk_bytes, 4)` bytes), never
//!   split. Downstream PTY-batching consumers already assume this; it is not
//!   a bug to "fix".
//! - **U3** — ⚠ [`isTerminalInputTooLargeWithDeferredMeasurement`]'s return
//!   **shape** is the contract (`TI:56-67`: `boolean | Promise<boolean>`),
//!   not just its resolved value. Collapsing it to a bare `bool` erases which
//!   branch ran. [`TerminalInputTooLargeDecision`] is the 2-variant
//!   analogue: `Immediate(bool)` for the two synchronous branches (`TI:60`'s
//!   immediate reject, `TI:66`'s delegate-to-sync-check), `Deferred(bool)`
//!   for the one branch that actually measures via the yielding path
//!   (`TI:63`) — the *only* arm where `yield_to_event_loop` is ever called.
//! - **U4** — ⚠⚠ `text.length` (UTF-16 code-unit count) is observable in
//!   *some* call sites here but not others — this is NOT a rule to apply
//!   uniformly:
//!   - [`is_terminal_input_too_large`] (`TI:44`): the UTF-16 fast path is
//!     unobservable in Rust for the same reason as
//!     `is_clipboard_text_byte_length_over_limit` (S5) — every code point's
//!     UTF-8 byte count is `>=` its UTF-16 unit count, so
//!     `utf16_len > max_bytes` implies `utf8_len > max_bytes`, and the
//!     trailing term is exactly `utf8_len > max_bytes`. `text.len() as f64 >
//!     max_bytes` is the whole function, in O(1), no scan.
//!   - [`is_terminal_input_too_large_with_deferred_measurement`]'s two
//!     `text.length` checks (`TI:60`, `TI:63`) pick a return **shape** (U3),
//!     which genuinely differs from a byte-length check: `text.encode_utf16().count()`
//!     is used **literally**, not `text.len()`. Reproducing example:
//!     `"é".repeat(200_000)` has 200,000 UTF-16 units (`<=` a 262,144-unit
//!     threshold, so the *shape* is `Immediate`) but 400,000 UTF-8 bytes — a
//!     byte-based port of this check would route it through `Deferred`
//!     instead. The oracle's only large fixture (`'é' × 262145`) exceeds
//!     *both* metrics and cannot distinguish them; the reproducing case is
//!     pinned explicitly below (`pin_routing_diverges_between_utf16_and_utf8_length`).
//! - **U5** — All numeric caps are `Option<f64>` (mirrors `S2` in
//!   `clipboard_text.rs`): `None` uses the built-in default; `Some(v)` is
//!   used **as-is**, with zero JS-style re-normalization at these call
//!   sites (unlike `resolve_clipboard_text_max_bytes`, there is no
//!   `is_finite() && > 0` gate on `max_bytes` here — `TI:31/41/51/58` never
//!   apply one). So `Some(f64::NAN)` accepts everything (`len > NaN` is
//!   always `false`); `Some(-1.0)` rejects even `""` (`0 > -1` is `true`).
//!   A `u64`-typed parameter cannot represent any of this domain.
//! - **U6** — ⚠ `TI:83`'s `normalizedMax` fallback, unlike its sibling
//!   `resolve_clipboard_text_max_bytes` (`CT:108-110`), has **no
//!   `Math.floor`**: `Number.isFinite(x) && x > 0 ? x : 1`. Non-finite
//!   (`NaN`, `±Infinity`) / zero / negative all fall back to `1`; a
//!   fractional `2.5` is kept as `2.5`, not truncated. `normalized_max`
//!   stays `f64` throughout and the loop guard compares
//!   `(current_bytes + character_bytes) as f64 > normalized_max` — no cast
//!   to an integer type anywhere in the comparison.
//!
//!   Two mutations of this fallback are, empirically, **also equivalent**
//!   (confirmed by mutation testing, not just reasoned about — neither
//!   changes any test outcome in this suite):
//!   - Adding `.floor()` to the finite/positive branch (`value.floor()`
//!     instead of `value`): `current_bytes`/`character_bytes` are always
//!     non-negative integers, and for any non-integer `x`, `n > x` and
//!     `n > floor(x)` agree for every integer `n` (there is no integer in
//!     `(floor(x), x)`), so the guard's `f64` comparison can never tell a
//!     floored cap from an unfloored one. Kept unfloored anyway, matching
//!     the source literally and avoiding the need for this proof at every
//!     call site.
//!   - Changing the non-finite/non-positive fallback constant from `1.0` to
//!     `0.0`: the loop guard only ever evaluates its second operand when
//!     `current_bytes > 0`, i.e. `current_bytes >= 1`, and
//!     `character_bytes >= 1` always (`char::len_utf8()` is never `0`), so
//!     the compared sum is always `>= 2` whenever the fallback value (`0`
//!     or `1`, both `< 2`) could matter — both fallbacks are dominated the
//!     same way. The one place a smaller total could apply (a single
//!     one-byte character, whose total is `1`) changes which code path is
//!     taken (the `SingleChunk` fast path vs. falling through the
//!     `Scanning` loop) but not the emitted chunk (both yield the same
//!     single whole-input chunk — see E1). Kept as `1` anyway to match the
//!     source's literal fallback value.
//! - **U7** — ⚠ [`is_terminal_input_too_large_with_yield`] does **not** call
//!   `suaegi_misc::is_clipboard_text_byte_length_over_limit_with_yield`: that
//!   function's Rust signature is `max_bytes: u64`, which cannot express the
//!   `Option<f64>` domain U5 requires (a `NaN`/negative/fractional
//!   `max_bytes` would be silently coerced away by an `as u64` cast at the
//!   call boundary, and the oracle never exercises those values so nothing
//!   would catch it). Instead, `clipboard-text.ts:92-101`'s 5-line body is
//!   inlined here directly against the `f64` cap, while still **reusing**
//!   [`suaegi_misc::measure_clipboard_text_byte_length_with_yield`] for the
//!   actual scan-and-yield cadence (that part has no numeric-domain mismatch
//!   to preserve).
//! - **U8** — S1-style: the TS `async` yielding functions become synchronous
//!   functions taking an injected `yield_to_event_loop: &mut dyn FnMut()`.
//!   Only the yield *cadence* (`CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS`,
//!   256 Ki UTF-16 units) is ported; actually yielding to an event loop is
//!   the caller's responsibility.
//! - **U9** — Laziness here is production **semantics**, not an
//!   optimization: a PTY writer can abandon iteration mid-payload and resume
//!   later between drains. [`TerminalInputChunks`] is a named, lazy
//!   `Iterator<Item = &str>` (precedent:
//!   `suaegi-misc::process_output_field_scanner::ProcessOutputLines`) rather
//!   than a `Vec`-then-`.into_iter()` — a `Vec`-backed port would pass every
//!   oracle case (the oracle only observes emitted values) but silently
//!   destroy the laziness contract production code depends on.
//!   [`split_terminal_input_chunks`] is a literal `.collect()` of the lazy
//!   iterator (`TI:73`).
//! - **U10** — Empty input yields **zero** chunks (`TI:80-82`), not `[""]`.
//!   The oracle never exercises `""`, so this is an extra pin, not an oracle
//!   case.
//! - **U11** — The error carries no payload (`TI:34`: the thrown `Error`'s
//!   message is always the same fixed string, never the rejected text).
//!   [`assert_terminal_input_within_limit`] returns the input **verbatim**
//!   (a borrowed slice, no trimming/truncation) on success (`TI:36`).
//! - **U12** — Every comparison site in this module is a strict `>`; none is
//!   `>=`. Landing exactly on a cap is accepted (16,384 bytes is one whole
//!   chunk; exactly 16 MiB is not too large).
//! - **U13** — [`get_terminal_input_byte_length`] is pure delegation
//!   (`TI:12-14`, no early-stop option passed) — arithmetically identical to
//!   `text.len()` (S5's identity), ported here as a literal call to
//!   [`suaegi_misc::measure_clipboard_text_byte_length`] to mirror the
//!   source's delegation shape.
//! - **U14** — The private `getUtf8ByteLengthForCodePoint` helper
//!   (`TI:16-27`) is byte-for-byte identical to `clipboard-text.ts`'s
//!   private helper of the same shape. In Rust both collapse to
//!   `char::len_utf8()`, so there is nothing left to even duplicate — no
//!   shared helper is extracted (this repo's convention: duplicate small
//!   per-module private helpers rather than reach across crates for them).
//!
//! # Equivalent mutations (documented, not test-hunted)
//!
//! - **E1** — [`TerminalInputChunks`]'s upfront "does the whole text fit in
//!   one chunk?" fast path (`TI:84-88`) is provably redundant: if the total
//!   byte length is `<= normalized_max`, the loop's guard
//!   (`current_bytes + character_bytes > normalized_max`) can never fire
//!   (a running partial sum is always `<=` the total), so the loop falls
//!   through to the tail yield of `text.slice(currentStart)` with
//!   `currentStart == 0`, i.e. `text` itself — the same single chunk the
//!   fast path would have produced directly. The only difference is scan
//!   cost (the fast path's bounded pre-scan vs. the full per-character
//!   loop), which the oracle's `codePointAt`-spy case (`T:47-55`, not
//!   ported — see below) exists to pin in TS and has no Rust analogue.
//!   Kept for structural fidelity to the source; see
//!   `equivalent_fast_path_and_general_loop_agree_when_input_fits_one_chunk`
//!   below, which pins the *output equivalence* (not a killable behavior
//!   difference).
//! - **E2** — The tail guard `currentStart < text.length` (`TI:106`) is
//!   dead code for any non-empty input: the initial `currentStart = 0` is
//!   `< text.length` (empty input already returned at `TI:80-82`), and every
//!   later assignment sets `currentStart = index`, where `index` is always a
//!   `char_indices()`-yielded start-of-character offset — necessarily `<
//!   text.length` for a character that exists in the string. Removing the
//!   guard cannot be observed by any test; it is inert, not undertested.
//!
//! # Skipped oracle cases (`terminal-input.test.ts`)
//!
//! `T:47-55` ("does not scan the full payload before yielding the first
//! chunk") and `T:83-91` ("rejects terminal input whose string length
//! already exceeds the byte limit without scanning") both spy on
//! `String.prototype.codePointAt` to assert a scan bound. Neither has a
//! Rust analogue: this port never calls anything resembling `codePointAt`
//! (Rust's `char_indices()`/`chars()` have no equivalent global hook to
//! spy on), and for the second case specifically,
//! [`is_terminal_input_too_large`] performs no scan at all in Rust (O(1)
//! via `text.len()`, precedent: `clipboard_text.rs`'s analogous skip). Their
//! *intent* — bounded/zero scanning — is exercised structurally: the first
//! by `oracle_iterates_chunks_lazily_without_prebuilding_every_terminal_input_chunk`
//! (explicit incremental `.next()` calls), the second trivially by
//! `is_terminal_input_too_large`'s O(1) shape.

use std::str::CharIndices;
use suaegi_misc::{
    measure_clipboard_text_byte_length, measure_clipboard_text_byte_length_with_yield,
    CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS,
};

/// `TI:7`. Default per-chunk byte budget for [`iterate_terminal_input_chunks`]
/// / [`split_terminal_input_chunks`] (16 KiB). U2: a chunk can still exceed
/// this when a single oversized code point does not fit.
pub const TERMINAL_INPUT_CHUNK_MAX_BYTES: u64 = 16 * 1024;

/// `TI:8`. Default total byte budget for the `is_too_large`/`assert` family
/// (16 MiB).
pub const TERMINAL_INPUT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// `TI:9-10`. The fixed, payload-free error message (U11).
pub const TERMINAL_INPUT_TOO_LARGE_ERROR: &str =
    "Terminal input is too large for a safe terminal send.";

/// Error from [`assert_terminal_input_within_limit`]. Fieldless (U11): the
/// rejected text is never carried in the error, only the fixed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalInputTooLargeError;

impl core::fmt::Display for TerminalInputTooLargeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(TERMINAL_INPUT_TOO_LARGE_ERROR)
    }
}

impl std::error::Error for TerminalInputTooLargeError {}

/// U3: the 2-variant analogue of TS's `boolean | Promise<boolean>` return
/// shape from `isTerminalInputTooLargeWithDeferredMeasurement` (`TI:56-67`).
/// `Immediate` covers both synchronous branches (`TI:60`'s early reject and
/// `TI:66`'s delegate-to-sync-check); `Deferred` is the *only* branch
/// (`TI:63`) where `yield_to_event_loop` is ever invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalInputTooLargeDecision {
    Immediate(bool),
    Deferred(bool),
}

/// `getTerminalInputByteLength` (`TI:12-14`). U13: pure delegation, no
/// early-stop budget passed — arithmetically `text.len()` (S5 identity).
pub fn get_terminal_input_byte_length(text: &str) -> u64 {
    measure_clipboard_text_byte_length(text, None).byte_length
}

/// `assertTerminalInputWithinLimit` (`TI:29-37`). U11: returns `text`
/// **verbatim** (no trimming/truncation) when within limit.
pub fn assert_terminal_input_within_limit(
    text: &str,
    max_bytes: Option<f64>,
) -> Result<&str, TerminalInputTooLargeError> {
    if is_terminal_input_too_large(text, max_bytes) {
        return Err(TerminalInputTooLargeError);
    }
    Ok(text)
}

/// `isTerminalInputTooLarge` (`TI:39-47`). U4: the source ORs a UTF-16
/// `text.length` fast path with a byte-accounting scan; in Rust `text.len()`
/// is already the exact UTF-8 byte count in O(1), so the fast path is
/// unobservable and not ported — see the module doc comment's U4 entry.
pub fn is_terminal_input_too_large(text: &str, max_bytes: Option<f64>) -> bool {
    let max_bytes = max_bytes.unwrap_or(TERMINAL_INPUT_MAX_BYTES as f64);
    // U12: strict `>`. U5: `max_bytes` is used as-is (no finite/positive
    // re-normalization) — `NaN` accepts everything, a negative rejects `""`.
    text.len() as f64 > max_bytes
}

/// `isTerminalInputTooLargeWithYield` (`TI:49-54`). U7: inlines
/// `clipboard-text.ts:92-101` against the `f64` cap rather than calling
/// `suaegi_misc::is_clipboard_text_byte_length_over_limit_with_yield`
/// (whose `u64` signature cannot represent U5's numeric domain) — see the
/// module doc comment's U7 entry. The scan-and-yield cadence itself is
/// reused from `suaegi-misc`.
pub fn is_terminal_input_too_large_with_yield(
    text: &str,
    max_bytes: Option<f64>,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> bool {
    let max_bytes = max_bytes.unwrap_or(TERMINAL_INPUT_MAX_BYTES as f64);
    // U4: this fast path IS a genuine UTF-16 code-unit count, not `text.len()`
    // — see `is_clipboard_text_byte_length_over_limit_with_yield`'s own doc
    // comment for why it cannot be replaced by the exact byte length here.
    if text.encode_utf16().count() as f64 > max_bytes {
        return true;
    }
    measure_clipboard_text_byte_length_with_yield(
        text,
        Some(max_bytes),
        yield_after_code_units,
        yield_to_event_loop,
    )
    .exceeded_limit
}

/// `isTerminalInputTooLargeWithDeferredMeasurement` (`TI:56-67`). U3/U4: see
/// the module doc comment — both `text.length` checks below are literal
/// UTF-16 code-unit counts (`encode_utf16().count()`), not byte lengths, and
/// the branch taken is baked into the returned
/// [`TerminalInputTooLargeDecision`] variant.
pub fn is_terminal_input_too_large_with_deferred_measurement(
    text: &str,
    max_bytes: Option<f64>,
    yield_after_code_units: Option<u64>,
    yield_to_event_loop: &mut dyn FnMut(),
) -> TerminalInputTooLargeDecision {
    let resolved_max_bytes = max_bytes.unwrap_or(TERMINAL_INPUT_MAX_BYTES as f64);
    let utf16_len = text.encode_utf16().count() as f64;

    // `TI:60`: `text.length > maxBytes` — literal UTF-16 count, NOT
    // `text.len()` (U4's reproducing case pins this distinction below).
    if utf16_len > resolved_max_bytes {
        return TerminalInputTooLargeDecision::Immediate(true);
    }
    // `TI:63`: also a literal UTF-16 count against the yield-cadence
    // threshold — the ONLY branch that reaches the yielding path.
    if utf16_len > CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS as f64 {
        return TerminalInputTooLargeDecision::Deferred(is_terminal_input_too_large_with_yield(
            text,
            Some(resolved_max_bytes),
            yield_after_code_units,
            yield_to_event_loop,
        ));
    }
    // `TI:66`: delegates to the plain synchronous check.
    TerminalInputTooLargeDecision::Immediate(is_terminal_input_too_large(
        text,
        Some(resolved_max_bytes),
    ))
}

/// U6: `TI:83`'s `normalizedMax` fallback — unlike
/// `resolve_clipboard_text_max_bytes`, there is **no `Math.floor`** here.
/// Non-finite / zero / negative all fall back to `1`; a finite positive
/// fractional value (e.g. `2.5`) is kept exactly, not truncated.
fn normalize_max_chunk_bytes(max_chunk_bytes: Option<f64>) -> f64 {
    let value = max_chunk_bytes.unwrap_or(TERMINAL_INPUT_CHUNK_MAX_BYTES as f64);
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    }
}

/// `splitTerminalInputChunks` (`TI:69-74`). A literal `.collect()` of
/// [`iterate_terminal_input_chunks`] (U9).
pub fn split_terminal_input_chunks(text: &str, max_chunk_bytes: Option<f64>) -> Vec<&str> {
    iterate_terminal_input_chunks(text, max_chunk_bytes).collect()
}

/// `iterateTerminalInputChunks` (`TI:76-109`). Returns a lazy
/// [`TerminalInputChunks`] iterator (U9) rather than a prebuilt list.
pub fn iterate_terminal_input_chunks(
    text: &str,
    max_chunk_bytes: Option<f64>,
) -> TerminalInputChunks<'_> {
    TerminalInputChunks {
        text,
        normalized_max: normalize_max_chunk_bytes(max_chunk_bytes),
        mode: ChunksMode::Uninitialized,
    }
}

enum ChunksMode<'a> {
    /// Not yet determined whether this is a single-chunk or multi-chunk
    /// scan (or whether the input is empty).
    Uninitialized,
    /// `TI:85-87` (E1): the whole text fits in one chunk.
    SingleChunk,
    /// `TI:90-105`: mid per-character scan. `chars` resumes exactly where
    /// the previous `next()` call left off — no re-scanning from the start.
    Scanning {
        chars: CharIndices<'a>,
        current_start: usize,
        current_bytes: u64,
    },
    Finished,
}

/// Lazy iterator over UTF-8-byte-budgeted, Unicode-scalar-atomic chunks of a
/// terminal input string (U1, U9). Each `next()` call does only the work
/// needed to produce the next chunk; nothing is prebuilt.
pub struct TerminalInputChunks<'a> {
    text: &'a str,
    normalized_max: f64,
    mode: ChunksMode<'a>,
}

impl<'a> Iterator for TerminalInputChunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        loop {
            match std::mem::replace(&mut self.mode, ChunksMode::Finished) {
                ChunksMode::Uninitialized => {
                    // `TI:80-82` (U10): empty input yields zero chunks, not
                    // `[""]`. `self.mode` is already `Finished` above.
                    if self.text.is_empty() {
                        return None;
                    }
                    // `TI:84`: a bounded pre-scan (stops as soon as
                    // `normalized_max` is exceeded), NOT a full scan of
                    // `text` — this is what keeps the "does not scan the
                    // full payload" property true for large inputs (U9).
                    let measurement =
                        measure_clipboard_text_byte_length(self.text, Some(self.normalized_max));
                    if measurement.exceeded_limit {
                        self.mode = ChunksMode::Scanning {
                            chars: self.text.char_indices(),
                            current_start: 0,
                            current_bytes: 0,
                        };
                    } else {
                        // E1: this fast path (`TI:85-87`) is provably
                        // equivalent in OUTPUT to falling through to the
                        // general loop below — see the module doc comment.
                        // Kept for structural fidelity to the source.
                        self.mode = ChunksMode::SingleChunk;
                    }
                    // Loop back around to act on the mode just set.
                }
                ChunksMode::SingleChunk => {
                    // `self.mode` is already `Finished` (single chunk only).
                    return Some(self.text);
                }
                ChunksMode::Scanning {
                    mut chars,
                    mut current_start,
                    mut current_bytes,
                } => {
                    let mut yielded: Option<&'a str> = None;
                    for (index, ch) in chars.by_ref() {
                        // U1: `char_indices()` enumerates one Unicode scalar
                        // at a time regardless of UTF-16 width, so unlike
                        // `TI:95-96` there is no separate
                        // `codeUnitLength`/`nextIndex` to compute — `index`
                        // is already the next recorded byte offset.
                        let character_bytes = ch.len_utf8() as u64;
                        // `TI:98` (U2): only splits when `current_bytes >
                        // 0` — a lone oversized character is admitted whole
                        // into an otherwise-empty chunk, never split.
                        // U12: strict `>`. U6: `normalized_max` compared as
                        // `f64`, no integer cast.
                        if current_bytes > 0
                            && (current_bytes + character_bytes) as f64 > self.normalized_max
                        {
                            // U1: slicing at `current_start`/`index`, both
                            // recorded `char_indices()` byte offsets — never
                            // a raw byte-budget cut, so this can never land
                            // mid-character.
                            yielded = Some(&self.text[current_start..index]);
                            current_start = index;
                            current_bytes = character_bytes;
                            break;
                        }
                        current_bytes += character_bytes;
                    }
                    if let Some(chunk) = yielded {
                        self.mode = ChunksMode::Scanning {
                            chars,
                            current_start,
                            current_bytes,
                        };
                        return Some(chunk);
                    }
                    // `TI:106-107` (E2): `current_start < text.len()` is
                    // provably always true here for non-empty input — see
                    // the module doc comment's E2 entry. Kept for
                    // structural fidelity; it is dead code, not undertested.
                    if current_start < self.text.len() {
                        return Some(&self.text[current_start..]);
                    }
                    return None;
                }
                ChunksMode::Finished => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Oracle: terminal-input.test.ts (test:21-133). T:47-55 and T:83-91
    // (codePointAt-spy cases) are skipped — see the module doc comment's
    // "Skipped oracle cases" section for why.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_keeps_small_terminal_input_as_one_chunk() {
        assert_eq!(
            split_terminal_input_chunks("npm test", None),
            vec!["npm test"]
        );
    }

    #[test]
    fn oracle_splits_by_utf8_bytes_without_splitting_surrogate_pairs() {
        let chunks = split_terminal_input_chunks("ab\u{1F600}cd", Some(4.0));
        assert_eq!(chunks, vec!["ab", "\u{1F600}", "cd"]);
        assert_eq!(chunks.join(""), "ab\u{1F600}cd");
    }

    #[test]
    fn oracle_uses_16kb_as_the_default_terminal_input_chunk_budget() {
        let text = "x".repeat(TERMINAL_INPUT_CHUNK_MAX_BYTES as usize + 1);
        let chunks = split_terminal_input_chunks(&text, None);
        assert_eq!(
            chunks,
            vec![
                "x".repeat(TERMINAL_INPUT_CHUNK_MAX_BYTES as usize).as_str(),
                "x"
            ]
        );
    }

    #[test]
    fn oracle_iterates_chunks_lazily_without_prebuilding_every_terminal_input_chunk() {
        let mut chunks = iterate_terminal_input_chunks("abcdefghij", Some(4.0));
        assert_eq!(chunks.next(), Some("abcd"));
        assert_eq!(chunks.next(), Some("efgh"));
        assert_eq!(chunks.next(), Some("ij"));
        assert_eq!(chunks.next(), None);
    }

    #[test]
    fn oracle_measures_utf8_bytes_for_terminal_input_without_using_utf16_length() {
        assert_eq!(get_terminal_input_byte_length("a\u{1F600}"), 5);
    }

    #[test]
    fn oracle_keeps_a_single_multibyte_terminal_character_intact_when_the_byte_cap_is_smaller() {
        let chunks = split_terminal_input_chunks("\u{1F600}a", Some(1.0));
        assert_eq!(chunks, vec!["\u{1F600}", "a"]);
        assert_eq!(chunks.join(""), "\u{1F600}a");
    }

    #[test]
    fn oracle_rejects_oversized_terminal_input_with_a_metadata_only_error() {
        let secret = "terminal-secret-token";
        let payload = format!("{secret}payload");
        let err = assert_terminal_input_within_limit(&payload, Some(4.0)).unwrap_err();
        assert_eq!(err.to_string(), TERMINAL_INPUT_TOO_LARGE_ERROR);
        assert!(!err.to_string().contains(secret));
    }

    #[test]
    fn oracle_rejects_multibyte_oversized_terminal_input_at_the_byte_boundary() {
        let text = "\u{1F600}".repeat(3);
        assert!(is_terminal_input_too_large(&text, Some(5.0)));
        assert_eq!(
            assert_terminal_input_within_limit(&text, Some(5.0)),
            Err(TerminalInputTooLargeError)
        );
    }

    #[test]
    fn oracle_yields_while_measuring_accepted_large_terminal_input() {
        let text = "\u{e9}".repeat(CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS as usize + 1);
        let max_bytes = Some(text.encode_utf16().count() as f64 * 3.0);
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let result = is_terminal_input_too_large_with_yield(
            &text,
            max_bytes,
            None,
            &mut yield_to_event_loop,
        );
        assert!(!result);
        assert!(calls > 0);
    }

    #[test]
    fn oracle_keeps_deferred_terminal_input_validation_synchronous_for_small_or_obvious_oversized_input(
    ) {
        let mut noop = || {};
        assert_eq!(
            is_terminal_input_too_large_with_deferred_measurement(
                "npm test", None, None, &mut noop
            ),
            TerminalInputTooLargeDecision::Immediate(false)
        );
        assert_eq!(
            is_terminal_input_too_large_with_deferred_measurement(
                &"x".repeat(6),
                Some(5.0),
                None,
                &mut noop
            ),
            TerminalInputTooLargeDecision::Immediate(true)
        );
    }

    #[test]
    fn oracle_returns_a_pending_validation_for_accepted_large_terminal_input() {
        let text = "\u{e9}".repeat(CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS as usize + 1);
        let max_bytes = Some(text.encode_utf16().count() as f64 * 3.0);
        let mut calls = 0u32;
        let mut yield_to_event_loop = || calls += 1;
        let decision = is_terminal_input_too_large_with_deferred_measurement(
            &text,
            max_bytes,
            None,
            &mut yield_to_event_loop,
        );
        assert_eq!(decision, TerminalInputTooLargeDecision::Deferred(false));
        assert!(calls > 0);
    }

    // -----------------------------------------------------------------
    // Mandatory extra pins (oracle-silent — this PR's main value)
    // -----------------------------------------------------------------

    /// The three exported constants, pinned against exact literal values —
    /// the oracle only ever imports them symbolically and never checks a
    /// concrete number/string.
    #[test]
    fn pin_exported_constants_have_exact_literal_values() {
        assert_eq!(TERMINAL_INPUT_CHUNK_MAX_BYTES, 16 * 1024);
        assert_eq!(TERMINAL_INPUT_MAX_BYTES, 16 * 1024 * 1024);
        assert_eq!(
            TERMINAL_INPUT_TOO_LARGE_ERROR,
            "Terminal input is too large for a safe terminal send."
        );
    }

    /// U10: empty input yields ZERO chunks from both entry points, not
    /// `[""]`.
    #[test]
    fn pin_empty_input_yields_zero_chunks_from_both_entry_points() {
        assert_eq!(split_terminal_input_chunks("", None), Vec::<&str>::new());
        assert_eq!(iterate_terminal_input_chunks("", None).next(), None);
    }

    /// U6: every non-finite/non-positive `max_chunk_bytes` fallback (`0`,
    /// negative, `NaN`, `+Infinity`) normalizes to a budget of `1`, which
    /// splits ASCII input one character per chunk — none of them fall back
    /// to "no cap"/the 16 KiB default, and none of them panic.
    #[test]
    fn pin_max_chunk_bytes_non_finite_or_non_positive_falls_back_to_budget_of_one() {
        for bad in [Some(0.0), Some(-5.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert_eq!(split_terminal_input_chunks("abc", bad), vec!["a", "b", "c"]);
        }
    }

    /// U6: a finite positive fractional `max_chunk_bytes` (`2.5`) is
    /// accepted directly, not floored to `2` and not rejected/panicking.
    #[test]
    fn pin_max_chunk_bytes_fractional_value_is_accepted_without_flooring() {
        // 3 one-byte chars, budget 2.5: "aa" (2 <= 2.5) then a 3rd char
        // pushes 2+1=3 > 2.5, so it splits into ["aa", "a"].
        assert_eq!(
            split_terminal_input_chunks("aaa", Some(2.5)),
            vec!["aa", "a"]
        );
    }

    /// U11: the four `assert_*`-shaped call one error path never carries the
    /// rejected text, and the success path returns the input **verbatim**
    /// (byte-for-byte, no trimming) — including at exactly the cap (U12).
    #[test]
    fn pin_assert_returns_input_verbatim_at_exactly_the_cap_and_never_trims() {
        let text = "  \u{FEFF}hi\u{FEFF}  ";
        assert_eq!(
            assert_terminal_input_within_limit(text, Some(text.len() as f64)),
            Ok(text)
        );
    }

    /// U12: all three predicates (`is_terminal_input_too_large`,
    /// `is_terminal_input_too_large_with_yield`, and the deferred
    /// function's own two literal checks) use strict `>` — landing exactly
    /// on the cap is accepted, never rejected.
    #[test]
    fn pin_predicates_use_strict_greater_than_not_greater_or_equal() {
        assert!(!is_terminal_input_too_large("abcde", Some(5.0)));
        assert!(is_terminal_input_too_large("abcdef", Some(5.0)));

        let mut noop = || {};
        assert!(!is_terminal_input_too_large_with_yield(
            "abcde",
            Some(5.0),
            None,
            &mut noop
        ));
        assert!(is_terminal_input_too_large_with_yield(
            "abcdef",
            Some(5.0),
            None,
            &mut noop
        ));

        assert_eq!(
            is_terminal_input_too_large_with_deferred_measurement(
                "abcde",
                Some(5.0),
                None,
                &mut noop
            ),
            TerminalInputTooLargeDecision::Immediate(false)
        );
        assert_eq!(
            is_terminal_input_too_large_with_deferred_measurement(
                "abcdef",
                Some(5.0),
                None,
                &mut noop
            ),
            TerminalInputTooLargeDecision::Immediate(true)
        );
    }

    /// U5: `NaN` accepts everything (even absurdly "large" input by the
    /// UTF-16 metric); a negative cap rejects even `""`.
    #[test]
    fn pin_max_bytes_nan_accepts_everything_negative_rejects_empty_string() {
        assert!(!is_terminal_input_too_large("abcdef", Some(f64::NAN)));
        assert!(is_terminal_input_too_large("", Some(-1.0)));
        assert_eq!(
            assert_terminal_input_within_limit("", Some(-1.0)),
            Err(TerminalInputTooLargeError)
        );
    }

    /// U7/U5: `is_terminal_input_too_large_with_yield` must preserve the
    /// same `NaN`/negative numeric domain as the synchronous check — a
    /// `u64`-typed call to the underlying `suaegi-misc` helper would coerce
    /// these away silently (this is exactly what U7 forbids).
    #[test]
    fn pin_with_yield_max_bytes_nan_accepts_everything_negative_rejects_empty_string() {
        let mut noop = || {};
        assert!(!is_terminal_input_too_large_with_yield(
            "abcdef",
            Some(f64::NAN),
            None,
            &mut noop
        ));
        assert!(is_terminal_input_too_large_with_yield(
            "",
            Some(-1.0),
            None,
            &mut noop
        ));
    }

    /// U4 (`is_terminal_input_too_large`, TI:44): the UTF-8 byte length is
    /// used, NOT the UTF-16 code-unit count. `"é" × 3` is 6 UTF-8 bytes but
    /// only 3 UTF-16 units — a byte-based port correctly reports "too
    /// large" against a cap of 4, while a UTF-16-based port would wrongly
    /// report "not too large" (3 <= 4).
    #[test]
    fn pin_is_too_large_uses_utf8_byte_length_not_utf16_code_unit_count() {
        let text = "\u{e9}".repeat(3);
        assert_eq!(text.len(), 6);
        assert_eq!(text.encode_utf16().count(), 3);
        assert!(is_terminal_input_too_large(&text, Some(4.0)));
    }

    /// U4: the reproducing case that separates a UTF-16-code-unit-based
    /// routing decision from a UTF-8-byte-based one. `"é" × 200_000` has
    /// 200,000 UTF-16 units (`<=` the 262,144-unit yield-cadence threshold,
    /// so the TRUE shape is `Immediate`) but 400,000 UTF-8 bytes (`>` the
    /// threshold if it were compared as bytes, which would wrongly route
    /// through `Deferred` instead). This is the ONE fixture that kills a
    /// byte-based port of `TI:63`; the oracle's own large fixture
    /// (`'é' × 262145`) exceeds both metrics and cannot tell them apart.
    #[test]
    fn pin_routing_diverges_between_utf16_and_utf8_length() {
        let text = "\u{e9}".repeat(200_000);
        assert_eq!(text.encode_utf16().count(), 200_000);
        assert_eq!(text.len(), 400_000);
        let mut noop = || {};
        let decision = is_terminal_input_too_large_with_deferred_measurement(
            &text,
            Some(1_000_000.0),
            None,
            &mut noop,
        );
        assert!(matches!(
            decision,
            TerminalInputTooLargeDecision::Immediate(false)
        ));
    }

    /// U3: each of the three routing arms, plus the fact that
    /// `yield_to_event_loop` is invoked ONLY on the `Deferred` arm.
    #[test]
    fn pin_deferred_decision_shape_and_yield_only_fires_on_the_deferred_arm() {
        // TI:66 arm: Immediate, delegating to the sync check.
        let mut calls = 0u32;
        {
            let mut cb = || calls += 1;
            assert_eq!(
                is_terminal_input_too_large_with_deferred_measurement(
                    "npm test", None, None, &mut cb
                ),
                TerminalInputTooLargeDecision::Immediate(false)
            );
        }
        assert_eq!(calls, 0);

        // TI:60 arm: Immediate(true), zero scanning, zero yields.
        let mut calls = 0u32;
        {
            let mut cb = || calls += 1;
            assert_eq!(
                is_terminal_input_too_large_with_deferred_measurement(
                    &"x".repeat(6),
                    Some(5.0),
                    None,
                    &mut cb
                ),
                TerminalInputTooLargeDecision::Immediate(true)
            );
        }
        assert_eq!(calls, 0);

        // TI:63 arm: Deferred, and this is the only arm that yields.
        let mut calls = 0u32;
        let text = "\u{e9}".repeat(CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS as usize + 1);
        let max_bytes = Some(text.encode_utf16().count() as f64 * 3.0);
        {
            let mut cb = || calls += 1;
            let decision = is_terminal_input_too_large_with_deferred_measurement(
                &text, max_bytes, None, &mut cb,
            );
            assert_eq!(decision, TerminalInputTooLargeDecision::Deferred(false));
        }
        assert!(calls > 0);
    }

    /// U12/U4: the `TI:63` yield-cadence threshold check is strict `>`, not
    /// `>=` — a text whose UTF-16 length lands EXACTLY on
    /// `CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS` takes the `Immediate`
    /// shape (delegating to the synchronous check, zero yields), not
    /// `Deferred`. Every other test in this suite uses a length one past
    /// the threshold, which cannot distinguish `>` from `>=` here.
    #[test]
    fn pin_deferred_yield_cadence_threshold_is_strict_greater_than() {
        let text = "x".repeat(CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS as usize);
        assert_eq!(
            text.encode_utf16().count() as u64,
            CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS
        );
        let mut calls = 0u32;
        let mut cb = || calls += 1;
        let decision = is_terminal_input_too_large_with_deferred_measurement(
            &text,
            Some((CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS * 2) as f64),
            None,
            &mut cb,
        );
        assert_eq!(decision, TerminalInputTooLargeDecision::Immediate(false));
        assert_eq!(calls, 0);
    }

    /// U1: a 2-byte character (`é`) and a 3-byte character (`€`) each land
    /// exactly on a chunk boundary intact — the oracle's own fixtures are
    /// astral (4-byte) only, so a port that special-cases 4-byte characters
    /// but mishandles 2-/3-byte ones would pass the oracle 13/13.
    #[test]
    fn pin_two_and_three_byte_characters_are_never_split_at_a_chunk_boundary() {
        // 'é' is 2 bytes; cap of 1 byte forces it into its own chunk whole
        // (U2), never split into a lone continuation byte.
        let chunks = split_terminal_input_chunks("a\u{e9}b", Some(1.0));
        assert_eq!(chunks, vec!["a", "\u{e9}", "b"]);
        assert_eq!(chunks.join(""), "a\u{e9}b");
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }

        // '€' is 3 bytes; same shape.
        let chunks = split_terminal_input_chunks("a\u{20ac}b", Some(1.0));
        assert_eq!(chunks, vec!["a", "\u{20ac}", "b"]);
        assert_eq!(chunks.join(""), "a\u{20ac}b");
    }

    /// U1: a mixed-width payload (1/2/3/4-byte characters interleaved)
    /// still round-trips exactly under `join("") == input` regardless of
    /// where chunk boundaries fall.
    #[test]
    fn pin_mixed_width_payload_round_trips_under_join() {
        let text = "a\u{e9}\u{20ac}\u{1F600}b\u{e9}c";
        for cap in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0] {
            let chunks = split_terminal_input_chunks(text, Some(cap));
            assert_eq!(chunks.join(""), text, "cap={cap}");
        }
    }

    /// U1: combining marks / ZWJ sequences are NOT kept together as a
    /// grapheme cluster — a base character and its following combining mark
    /// can land in different chunks when the cap forces a split between
    /// them. This module operates on Unicode scalars, not grapheme
    /// clusters.
    #[test]
    fn pin_combining_marks_are_split_across_chunk_boundaries_not_kept_as_a_grapheme() {
        // 'e' + COMBINING ACUTE ACCENT (U+0301) is two scalars (1 byte +
        // 2 bytes); a 1-byte cap forces them into separate chunks.
        let text = "e\u{0301}";
        let chunks = split_terminal_input_chunks(text, Some(1.0));
        assert_eq!(chunks, vec!["e", "\u{0301}"]);
        assert_eq!(chunks.join(""), text);
    }

    /// U2: a single oversized code point is admitted whole into its own
    /// chunk, never split into invalid partial UTF-8 — already exercised by
    /// the ported oracle case, pinned again here with a fresh 3-byte
    /// example (`€`, cap 1) to separate it from the 4-byte-only oracle
    /// fixture.
    #[test]
    fn pin_oversized_chunk_admits_a_single_character_whole() {
        let chunks = split_terminal_input_chunks("\u{20ac}", Some(1.0));
        assert_eq!(chunks, vec!["\u{20ac}"]);
    }

    /// E1: the fast path and the general loop produce IDENTICAL output when
    /// the whole input fits in one chunk — this pins the equivalence itself
    /// (documentation), not a behavior difference a mutation could expose
    /// (see the module doc comment's E1 entry for the proof).
    #[test]
    fn equivalent_fast_path_and_general_loop_agree_when_input_fits_one_chunk() {
        let text = "small input";
        assert_eq!(split_terminal_input_chunks(text, Some(1024.0)), vec![text]);
    }

    /// U9: `split_terminal_input_chunks` (eager `.collect()`) and manually
    /// draining `iterate_terminal_input_chunks` produce the identical
    /// sequence — pins that `.collect()` really is a literal wrapper, not a
    /// different traversal.
    #[test]
    fn pin_split_and_iterate_collect_produce_the_identical_sequence() {
        let text = "ab\u{1F600}cd\u{e9}ef";
        let via_split = split_terminal_input_chunks(text, Some(3.0));
        let via_iterate: Vec<&str> = iterate_terminal_input_chunks(text, Some(3.0)).collect();
        assert_eq!(via_split, via_iterate);
    }

    /// U13: `get_terminal_input_byte_length` on `""` and on 2-/3-byte
    /// characters (the oracle's only fixture is a 4-byte astral one).
    #[test]
    fn pin_byte_length_of_empty_and_narrow_multibyte_text() {
        assert_eq!(get_terminal_input_byte_length(""), 0);
        assert_eq!(get_terminal_input_byte_length("\u{e9}"), 2);
        assert_eq!(get_terminal_input_byte_length("\u{20ac}"), 3);
    }

    /// U8: `is_terminal_input_too_large_with_yield` really does return
    /// `true` (not just `false`) in its shape, and reaches that `true` via
    /// the fast UTF-16 path with zero scanning/yielding — mirrors the
    /// oracle's boolean-`false` case (T:93-109) with the opposite outcome.
    #[test]
    fn pin_with_yield_returns_true_via_the_fast_path_with_no_scanning() {
        let mut calls = 0u32;
        let mut cb = || calls += 1;
        assert!(is_terminal_input_too_large_with_yield(
            &"x".repeat(10),
            Some(5.0),
            None,
            &mut cb
        ));
        assert_eq!(calls, 0);
    }
}
