//! GitHub project reference input byte-length bound — verbatim port of
//! Orca's `src/shared/github-project-ref-input.ts` (@ v1.4.146-rc.0).
//!
//! Guards a pasted GitHub Projects URL/reference (e.g.
//! `https://github.com/orgs/acme/projects/42/views/3`) against an oversized
//! paste, and gates a submit button on "non-trivial and within bounds".
//!
//! Contract decisions ported from `docs/superpowers/plans/2026-07-27-github-input-bounds.md`
//! (K-numbers refer to that plan's §1; K1/K2/K5/K6/K7/K11 mirror the sibling
//! module [`crate::github_work_items_query_bounds`] — see its doc comment for
//! the full proofs, repeated here only where this module adds something new):
//!
//! - **K1/K2** — [`is_github_project_ref_input_too_large`] inlines the same
//!   collapse as the sibling module: `input.len() as f64 > max_bytes`, with
//!   `max_bytes: Option<f64>` (a `u64` would invert the `NaN`/negative
//!   contract — see the sibling module's K2 for the full argument).
//! - **K3** — `hasBoundedGitHubProjectRefInputText`'s `/\S/.test(input)` is
//!   ECMAScript `\S` (unanchored, no `/u` flag): "at least one non-whitespace
//!   character anywhere". Ported as
//!   `input.chars().any(|c| !is_js_whitespace(c))`, reusing
//!   [`crate::js_ws::is_js_whitespace`] — the one sanctioned intra-crate
//!   dependency in this pair of modules. Neither `char::is_whitespace` nor
//!   `str::trim().is_empty()` reproduce ECMAScript `\S` exactly: `"\u{FEFF}"`
//!   (BOM) is ECMAScript whitespace (→ `false`, no non-whitespace char) but
//!   not Unicode `White_Space` (a naive `char::is_whitespace`-based scan
//!   would wrongly say `true`); `"\u{0085}"` (NEL) is Unicode `White_Space`
//!   but NOT ECMAScript whitespace (→ `true`) — a naive port would wrongly
//!   say `false`. `trim().is_empty()` is wrong in the same two spots (wrong
//!   whitespace set), even though "all chars are whitespace" and "no char is
//!   non-whitespace" are logically the same claim for this predicate.
//! - **K4** — ⚠ [`has_bounded_github_project_ref_input_text`]'s `&&` has a
//!   cap term (`!is_github_project_ref_input_too_large(input, None)`) that is
//!   **dead in the oracle**: its only over-limit fixture
//!   (`' '.repeat(MAX+1)`) is *also* whitespace-only, so both operands
//!   independently evaluate to `false` and deleting the cap term still
//!   passes all 4 oracle assertions for this function. The cap term is kept
//!   (matching the source literally, and because the two operands are not
//!   provably redundant in general — see [`pin_bounded_check_rejects_a_long_non_whitespace_input`]
//!   below, which is the only pin that actually exercises it: "over cap AND
//!   non-whitespace").
//! - **K5** — the `&&`'s operand order is a pure equivalence (both operands
//!   are pure, total, side-effect-free, panic-free), not a mutation target;
//!   documented, not tested for order-swap.
//! - **K6** — [`GITHUB_PROJECT_REF_INPUT_MAX_BYTES`] and
//!   [`GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR`] are symbolic-only upstream
//!   — the literal `2048` and the exact error string appear nowhere else in
//!   the source repo (not even imported by the test file) — both pinned
//!   directly.
//! - **K7** — a multibyte pin at the *exact* cap boundary (not merely over
//!   it), to catch a width-specific off-by-one.
//! - **K8** — [`get_github_project_ref_input_byte_length`] has exactly one
//!   upstream fixture (`'\u{e9}'` → `2`), which every plausible length
//!   definition (`text.len()`, `chars().map(len_utf8).sum()`,
//!   `encode_utf16().count() + 1`, a 2-byte-only lookup table) would also
//!   satisfy — four extra pins disambiguate. ⚠ This export has **zero
//!   production callers** upstream (grepped repo-wide); it is ported for
//!   parity with the TS module's public surface (the
//!   `pi_overlay_ui_settings` precedent: port completely, wire nowhere) and
//!   is not re-exported to any consumer in this crate beyond `lib.rs`.
//! - **K9** — this module never trims (mirrors `clipboard_text`'s S8).
//!   Upstream callers trim **inconsistently**: `client.ts:1351` trims before
//!   capping, `github.ts:2740` caps the raw string, `ProjectPicker.tsx:338`
//!   passes the raw string, `project-view.ts:1703` passes a trimmed string.
//!   This asymmetry lives entirely in the (unported) call sites; this module
//!   is recorded as never trimming and that is not changed here.
//! - **K10** — [`GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR`] is a fixed,
//!   payload-free string constant — metadata about *why* input was rejected,
//!   never the rejected input itself (mirrors `clipboard_text`'s S9; the
//!   upstream IPC `validation_error` payload asserts the same).
//! - **K11** — no renderer-side duplicate exists for this module (unlike the
//!   sibling); nothing skipped here.

use crate::js_ws::is_js_whitespace;

/// Maximum accepted byte length for a pasted GitHub project reference
/// (2 KiB). K6: symbolic-only upstream — pin the literal, not just the name.
pub const GITHUB_PROJECT_REF_INPUT_MAX_BYTES: f64 = 2.0 * 1024.0;

/// Error text for an oversized GitHub project reference. K6/K10: a fixed,
/// payload-free constant — never includes the rejected input.
pub const GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR: &str =
    "Project reference is too large to resolve.";

/// The UTF-8 byte length of `input`. K8: zero production callers upstream —
/// ported for parity with the TS module's public surface, wired nowhere.
pub fn get_github_project_ref_input_byte_length(input: &str) -> u64 {
    input.len() as u64
}

/// `true` if `input`'s UTF-8 byte length exceeds `max_bytes`
/// ([`GITHUB_PROJECT_REF_INPUT_MAX_BYTES`] when `None`).
///
/// K1/K2: collapsed inline form of the shared `clipboard-text.ts` delegate —
/// see [`crate::github_work_items_query_bounds`]'s module doc for the full
/// proof across the `f64` domain.
pub fn is_github_project_ref_input_too_large(input: &str, max_bytes: Option<f64>) -> bool {
    let max_bytes = max_bytes.unwrap_or(GITHUB_PROJECT_REF_INPUT_MAX_BYTES);
    input.len() as f64 > max_bytes
}

/// `true` if `input` is within the byte cap (using the **default** cap only
/// — the TS caller passes no `maxBytes` argument, so this never takes one
/// either) and contains at least one ECMAScript non-whitespace character
/// (K3).
///
/// K4: the cap term is dead in the oracle (see module doc) but kept for
/// fidelity to the source and because it is not provably redundant in
/// general — see [`pin_bounded_check_rejects_a_long_non_whitespace_input`].
pub fn has_bounded_github_project_ref_input_text(input: &str) -> bool {
    !is_github_project_ref_input_too_large(input, None) && input.chars().any(|c| !is_js_whitespace(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: github-project-ref-input.test.ts (5/5).

    #[test]
    fn allows_normal_project_references_below_the_byte_budget() {
        assert!(!is_github_project_ref_input_too_large(
            "https://github.com/orgs/acme/projects/42/views/3",
            None
        ));
    }

    #[test]
    fn measures_utf8_bytes_instead_of_javascript_string_length() {
        assert_eq!(get_github_project_ref_input_byte_length("\u{e9}"), 2);
    }

    #[test]
    fn rejects_oversized_pasted_project_references() {
        assert!(!is_github_project_ref_input_too_large(
            &"x".repeat(GITHUB_PROJECT_REF_INPUT_MAX_BYTES as usize),
            None
        ));
        assert!(is_github_project_ref_input_too_large(
            &"x".repeat(GITHUB_PROJECT_REF_INPUT_MAX_BYTES as usize + 1),
            None
        ));
    }

    #[test]
    fn rejects_multibyte_project_references_whose_character_count_is_below_the_limit() {
        let reference = "\u{1f600}".repeat((GITHUB_PROJECT_REF_INPUT_MAX_BYTES as usize / 4) + 1);
        assert!(reference.chars().count() < GITHUB_PROJECT_REF_INPUT_MAX_BYTES as usize);
        assert!(is_github_project_ref_input_too_large(&reference, None));
    }

    #[test]
    fn rejects_oversized_whitespace_before_submit_checks_trim_the_reference() {
        let oversized_whitespace = " ".repeat(GITHUB_PROJECT_REF_INPUT_MAX_BYTES as usize + 1);

        assert!(is_github_project_ref_input_too_large(
            &oversized_whitespace,
            None
        ));
        assert!(!has_bounded_github_project_ref_input_text(
            &oversized_whitespace
        ));
        assert!(has_bounded_github_project_ref_input_text("  acme/42  "));
        assert!(!has_bounded_github_project_ref_input_text("   "));
    }

    // Mandatory extra pins (oracle-silent — plan §2):

    /// K6: the byte cap and error string really are the pinned literals
    /// (symbolic-only upstream — neither appears elsewhere in the repo,
    /// not even in the test file's imports).
    #[test]
    fn pin_constants_are_the_literal_values() {
        assert_eq!(GITHUB_PROJECT_REF_INPUT_MAX_BYTES, 2048.0);
        assert_eq!(
            GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR,
            "Project reference is too large to resolve."
        );
    }

    /// K1/K2: every `max_bytes` arm across the `f64` domain, for both
    /// byte-cap predicates.
    #[test]
    fn pin_max_bytes_option_covers_every_js_numeric_branch() {
        assert!(!is_github_project_ref_input_too_large("short", None));
        assert!(is_github_project_ref_input_too_large(&"x".repeat(2049), None));

        // NaN / +Infinity: cap disabled entirely.
        assert!(!is_github_project_ref_input_too_large(
            &"x".repeat(50_000),
            Some(f64::NAN)
        ));
        assert!(!is_github_project_ref_input_too_large(
            &"x".repeat(50_000),
            Some(f64::INFINITY)
        ));
        // -1: rejects even the empty string.
        assert!(is_github_project_ref_input_too_large("", Some(-1.0)));
        // 0: rejects any non-empty input, accepts empty.
        assert!(!is_github_project_ref_input_too_large("", Some(0.0)));
        assert!(is_github_project_ref_input_too_large("a", Some(0.0)));
        // 2.5: fractional cap, compared directly.
        assert!(!is_github_project_ref_input_too_large("ab", Some(2.5)));
        assert!(is_github_project_ref_input_too_large("abc", Some(2.5)));
    }

    /// K3: the exact ECMAScript `\S` divergence points, plus a handful of
    /// ordinary Unicode space separators that must still read as
    /// all-whitespace.
    #[test]
    fn pin_non_whitespace_check_uses_ecmascript_whitespace_set() {
        // U+FEFF (BOM/ZWNBSP): ECMAScript whitespace -> no non-whitespace char.
        assert!(!has_bounded_github_project_ref_input_text("\u{FEFF}"));
        // U+0085 (NEL): NOT ECMAScript whitespace -> counts as non-whitespace.
        assert!(has_bounded_github_project_ref_input_text("\u{0085}"));
        // Ordinary Unicode space separators: all-whitespace -> false.
        assert!(!has_bounded_github_project_ref_input_text("\u{00A0}"));
        assert!(!has_bounded_github_project_ref_input_text("\u{2028}"));
        assert!(!has_bounded_github_project_ref_input_text("\u{3000}"));
    }

    /// K4: pins the one cell of the coverage matrix the cap term alone
    /// decides ("over cap AND non-whitespace") — deleting
    /// `!is_github_project_ref_input_too_large(input, None)` from the `&&`
    /// would make this pass when it must fail.
    #[test]
    fn pin_bounded_check_rejects_a_long_non_whitespace_input() {
        assert!(!has_bounded_github_project_ref_input_text(&"x".repeat(2049)));
    }

    /// K7: a multibyte input pinned at the *exact* byte cap, not merely over
    /// it — catches a width-specific off-by-one.
    #[test]
    fn pin_multibyte_input_exactly_at_the_byte_cap() {
        // "é" is 2 UTF-8 bytes; 1024 * 2 = 2048 = the exact cap.
        let exactly_at_cap = "\u{e9}".repeat(1024);
        assert_eq!(exactly_at_cap.len(), 2048);
        assert!(!is_github_project_ref_input_too_large(
            &exactly_at_cap,
            None
        ));

        let one_byte_over = format!("{exactly_at_cap}x");
        assert!(is_github_project_ref_input_too_large(&one_byte_over, None));
    }

    /// K8: `get_github_project_ref_input_byte_length` for four widths — the
    /// upstream oracle only exercises one 2-byte fixture, which every
    /// plausible (wrong) length definition would also satisfy.
    #[test]
    fn pin_byte_length_covers_every_utf8_width() {
        assert_eq!(get_github_project_ref_input_byte_length(""), 0);
        assert_eq!(get_github_project_ref_input_byte_length("\u{ac00}"), 3); // "가"
        assert_eq!(get_github_project_ref_input_byte_length("\u{1f600}"), 4); // "😀"
        assert_eq!(
            get_github_project_ref_input_byte_length("a\u{ac00}\u{1f600}"),
            8
        );
    }

    /// K10: the error string is a fixed constant, never the rejected input,
    /// regardless of what was rejected.
    #[test]
    fn pin_error_message_never_includes_the_rejected_input() {
        let payload = "secret-project-ref-value";
        assert!(is_github_project_ref_input_too_large(payload, Some(1.0)));
        assert!(!GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR.contains(payload));
    }
}
