//! GitHub work-items search query byte-length bound — verbatim port of
//! Orca's `src/shared/github-work-items-query-bounds.ts` (@ v1.4.146-rc.0).
//!
//! Guards a pasted GitHub Issues/PRs search query against an oversized paste
//! by comparing its **UTF-8 byte length** to a cap, one code point at a time
//! being an implementation detail Rust doesn't need (see the collapse note
//! below).
//!
//! Contract decisions ported from `docs/superpowers/plans/2026-07-27-github-input-bounds.md`
//! (K-numbers refer to that plan's §1):
//!
//! - **K1** — the TS delegate this module calls,
//!   `isClipboardTextByteLengthOverLimit(text, maxBytes)`, is
//!   `text.length > maxBytes || measure(text, { stopAfterBytes: maxBytes }).exceededLimit`.
//!   For every code point, its UTF-8 byte count is `>=` its UTF-16 code-unit
//!   count, so `text.length > maxBytes` (the OR's left arm) already implies
//!   the right arm would also report exceeded for a finite `maxBytes`; across
//!   the **entire `f64` domain**, including the non-finite arms
//!   (`NaN` → both `false`; `+Infinity` → both `false`; `-Infinity`/`-1` →
//!   both `true`, even for `""`), the whole expression collapses to exactly
//!   `text.len() as f64 > max_bytes`. This is inlined directly as one line —
//!   it does **not** call [`crate::clipboard_text::is_clipboard_text_byte_length_over_limit`],
//!   whose `u64` signature is the one thing that would break this (see K2).
//!   ⚠ Not representable: JS counts a lone (unpaired) UTF-16 surrogate as 3
//!   bytes (`clipboard-text.ts` `getUtf8ByteLengthForCodePoint`'s `<= 0xffff`
//!   branch, fed a lone surrogate's code-point value); Rust `&str` is
//!   guaranteed valid UTF-8 and cannot contain an unpaired surrogate at all,
//!   so this arm of the JS byte-counter has no Rust counterpart to diverge
//!   from.
//! - **K2** — `max_bytes` is `Option<f64>`, not `u64`. JS `NaN` disables the
//!   cap (falls through the OR as `false`/`false`); `NaN as u64` in Rust is
//!   `0`, which would reject every non-empty query — the opposite contract.
//!   Symmetrically, JS `-1` rejects everything (`0 > -1` is `true` even for
//!   `""`); `-1.0 as u64` saturates to `0`, which would accept everything.
//!   No production or test call site ever actually passes `max_bytes`
//!   (default parameter only), so a `u64` port would still be 100% green —
//!   the signature is the only thing enforcing the contract here.
//! - **K6** — [`GITHUB_WORK_ITEMS_QUERY_MAX_BYTES`] is a symbolic-only
//!   constant upstream (`8 * 1024`); the literal `8192` appears nowhere else
//!   in the source repo, so it is pinned directly as a regression test.
//! - **K11** — `src/renderer/src/store/slices/github-work-items-query-bounds.ts`
//!   is a pure re-export shim over this module (zero new logic, zero new
//!   test coverage in its mirrored `.test.ts`) — not ported; there is no Rust
//!   analogue to a re-export barrel file.

/// Maximum accepted byte length for a pasted GitHub work-items search query
/// (8 KiB). K6: symbolic-only upstream — pin the literal, not just the name.
pub const GITHUB_WORK_ITEMS_QUERY_MAX_BYTES: f64 = 8.0 * 1024.0;

/// `true` if `query`'s UTF-8 byte length exceeds `max_bytes`
/// ([`GITHUB_WORK_ITEMS_QUERY_MAX_BYTES`] when `None`).
///
/// K1: this is the collapsed form of the TS delegate's
/// `text.length > maxBytes || measure(...).exceededLimit` — see the module
/// doc for the full proof across the `f64` domain.
pub fn is_github_work_items_query_too_large(query: &str, max_bytes: Option<f64>) -> bool {
    let max_bytes = max_bytes.unwrap_or(GITHUB_WORK_ITEMS_QUERY_MAX_BYTES);
    query.len() as f64 > max_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: github-work-items-query-bounds.test.ts (3/3) +
    // renderer/.../github-work-items-query-bounds.test.ts (not re-ported,
    // K11: pure re-export shim, zero new coverage).

    #[test]
    fn allows_normal_github_search_syntax() {
        assert!(!is_github_work_items_query_too_large(
            "is:issue is:open label:bug",
            None
        ));
    }

    #[test]
    fn rejects_oversized_pasted_work_item_queries_by_byte_length() {
        assert!(!is_github_work_items_query_too_large(
            &"x".repeat(GITHUB_WORK_ITEMS_QUERY_MAX_BYTES as usize),
            None
        ));
        assert!(is_github_work_items_query_too_large(
            &"x".repeat(GITHUB_WORK_ITEMS_QUERY_MAX_BYTES as usize + 1),
            None
        ));
    }

    #[test]
    fn rejects_multibyte_pasted_queries_whose_character_count_is_below_the_limit() {
        let query = "\u{1f600}".repeat((GITHUB_WORK_ITEMS_QUERY_MAX_BYTES as usize / 4) + 1);
        assert!(query.chars().count() < GITHUB_WORK_ITEMS_QUERY_MAX_BYTES as usize);
        assert!(is_github_work_items_query_too_large(&query, None));
    }

    #[test]
    fn measures_utf8_bytes_for_non_ascii_query_text() {
        assert!(is_github_work_items_query_too_large(
            &"\u{1f600}".repeat(3_000),
            None
        ));
    }

    // Mandatory extra pins (oracle-silent — plan §2):

    /// K6: the byte cap really is 8 KiB (symbolic-only upstream — the
    /// literal `8192` appears nowhere else in the source repo).
    #[test]
    fn pin_max_bytes_constant_is_8_kibibytes() {
        assert_eq!(GITHUB_WORK_ITEMS_QUERY_MAX_BYTES, 8192.0);
    }

    /// K1/K2: every `max_bytes` arm across the `f64` domain, both below and
    /// above the (deliberately tiny, for this pin) cap.
    #[test]
    fn pin_max_bytes_option_covers_every_js_numeric_branch() {
        // None: falls back to the 8 KiB default.
        assert!(!is_github_work_items_query_too_large("short", None));
        assert!(is_github_work_items_query_too_large(
            &"x".repeat(8193),
            None
        ));

        // NaN: disables the cap entirely — even a huge query passes.
        assert!(!is_github_work_items_query_too_large(
            &"x".repeat(50_000),
            Some(f64::NAN)
        ));
        // +Infinity: same as NaN, cap disabled.
        assert!(!is_github_work_items_query_too_large(
            &"x".repeat(50_000),
            Some(f64::INFINITY)
        ));
        // -1: rejects even the empty string.
        assert!(is_github_work_items_query_too_large("", Some(-1.0)));
        // 0: rejects any non-empty query, accepts empty.
        assert!(!is_github_work_items_query_too_large("", Some(0.0)));
        assert!(is_github_work_items_query_too_large("a", Some(0.0)));
        // 2.5: fractional cap, compared directly (no floor here — that's
        // `resolve_clipboard_text_max_bytes`'s job, not this delegate's).
        assert!(!is_github_work_items_query_too_large("ab", Some(2.5)));
        assert!(is_github_work_items_query_too_large("abc", Some(2.5)));
    }

    /// K7: a multibyte input pinned at the *exact* byte cap, not merely over
    /// it — catches a width-specific off-by-one (e.g. counting a 2-byte
    /// character as 3).
    #[test]
    fn pin_multibyte_input_exactly_at_the_byte_cap() {
        // "é" is 2 UTF-8 bytes; 4096 * 2 = 8192 = the exact cap.
        let exactly_at_cap = "\u{e9}".repeat(4096);
        assert_eq!(exactly_at_cap.len(), 8192);
        assert!(!is_github_work_items_query_too_large(&exactly_at_cap, None));

        let one_byte_over = format!("{exactly_at_cap}x");
        assert!(is_github_work_items_query_too_large(&one_byte_over, None));
    }
}
