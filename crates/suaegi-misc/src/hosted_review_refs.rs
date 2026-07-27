//! Hosted-review head/base ref normalization — verbatim port of Orca's
//! `src/shared/hosted-review-refs.ts` (@ v1.4.146-rc.0).
//!
//! Strips the local-branch/remote-tracking wrapping a hosted git provider
//! (GitHub, GitLab, ...) puts on a ref name, so the display name matches what
//! the user typed as a branch.
//!
//! All three JS regexes in the source are `^`-anchored with **no** `/g` flag,
//! so each can replace **at most once**, and only a true prefix match. Rust's
//! `String::replace` is unconditionally global and `Regex::replace` is
//! unanchored by default — both are the wrong tool here. `str::strip_prefix`
//! is used instead everywhere, which is exact-once and anchored by
//! construction. The oracle's four fixtures all happen to have their prefix
//! at position 0 in exact lowercase, so a global/unanchored port would still
//! pass all of them; the extra pins below (`feature/refs/heads/x`,
//! `release/origin/patch`, `origin/origin/main`, `my-origin/main`) are the
//! only witnesses that separate the two behaviors — a real branch name like
//! `release/origin-sync` would be silently mangled by an unanchored port.
//!
//! The `refs/remotes/[^/]+/` step is a two-part scan: strip `refs/remotes/`,
//! then strip up to (and including) the *next* `/`. `[^/]+` requires **at
//! least one** non-slash character in that segment, so `refs/remotes//x`
//! (empty segment) and `refs/remotes/origin` (no trailing slash at all) both
//! fail to match and are returned unchanged.
//!
//! `.trim()` is ECMAScript whitespace, not Rust's `char::is_whitespace` /
//! `str::trim` — see [`crate::js_ws`] for the U+FEFF/U+0085 divergence this
//! crate re-derives.

use crate::js_ws::js_trim;

const REFS_HEADS_PREFIX: &str = "refs/heads/";
const REFS_REMOTES_PREFIX: &str = "refs/remotes/";

/// Strip a single, anchored `refs/remotes/[^/]+/` prefix, if present. The
/// segment between `refs/remotes/` and the next `/` must be non-empty (the
/// `[^/]+` in the original regex), so `refs/remotes//x` and
/// `refs/remotes/origin` (no trailing slash) are both left unchanged.
fn strip_remotes_segment_prefix(s: &str) -> &str {
    let Some(rest) = s.strip_prefix(REFS_REMOTES_PREFIX) else {
        return s;
    };
    match rest.find('/') {
        Some(0) => s,             // empty segment: `refs/remotes//x`
        Some(slash_at) => &rest[slash_at + 1..],
        None => s,                // no trailing slash: `refs/remotes/origin`
    }
}

/// Port of `normalizeHostedReviewHeadRef` (`hosted-review-refs.ts:1-6`).
///
/// Trims ECMAScript whitespace, strips one leading `refs/heads/`, then
/// strips one leading `refs/remotes/<remote>/` — in that order, each at most
/// once.
pub fn normalize_hosted_review_head_ref(r#ref: &str) -> String {
    let trimmed = js_trim(r#ref);
    let after_heads = trimmed.strip_prefix(REFS_HEADS_PREFIX).unwrap_or(trimmed);
    strip_remotes_segment_prefix(after_heads).to_string()
}

/// Port of `normalizeHostedReviewBaseRef` (`hosted-review-refs.ts:8-11`).
///
/// Delegates to [`normalize_hosted_review_head_ref`] first (do not
/// re-implement its logic), then strips one leading `origin/` or `upstream/`.
/// The two steps are order-sensitive in principle (head-normalization runs
/// first), but the upstream oracle never separates them — `origin/refs/heads/x`
/// is the only witness: head-normalization runs first but can't strip
/// anything (the string doesn't start with `refs/heads/` or `refs/remotes/`),
/// so it passes through untouched, and *then* `origin/` is stripped, giving
/// `refs/heads/x`. A port that ran the steps in the opposite order would
/// strip `origin/` first and then (wrongly) strip `refs/heads/` too, giving
/// `x`.
pub fn normalize_hosted_review_base_ref(r#ref: &str) -> String {
    let normalized = normalize_hosted_review_head_ref(r#ref);
    match normalized
        .strip_prefix("origin/")
        .or_else(|| normalized.strip_prefix("upstream/"))
    {
        Some(rest) => rest.to_string(),
        None => normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: hosted-review-refs.test.ts

    #[test]
    fn normalizes_local_and_remote_head_refs_to_branch_names() {
        assert_eq!(
            normalize_hosted_review_head_ref(" refs/heads/feature/create-pr "),
            "feature/create-pr"
        );
        assert_eq!(
            normalize_hosted_review_head_ref("refs/remotes/origin/feature/create-pr"),
            "feature/create-pr"
        );
    }

    #[test]
    fn strips_common_remote_prefixes_from_base_refs() {
        assert_eq!(normalize_hosted_review_base_ref("origin/main"), "main");
        assert_eq!(
            normalize_hosted_review_base_ref("refs/remotes/upstream/release/1.0"),
            "release/1.0"
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// G1: unanchored/global divergence witnesses. Each input has its prefix
    /// candidate NOT at true position 0 (or repeated), so a
    /// `String::replace`/`Regex::replace_all` port would wrongly touch it.
    #[test]
    fn pin_g1_head_ref_prefix_not_at_start_is_left_untouched() {
        assert_eq!(
            normalize_hosted_review_head_ref("feature/refs/heads/x"),
            "feature/refs/heads/x"
        );
    }

    #[test]
    fn pin_g1_base_ref_prefix_occurrences_are_not_globally_stripped() {
        // "origin/" appears mid-string, not as a true prefix: must survive.
        assert_eq!(
            normalize_hosted_review_base_ref("release/origin/patch"),
            "release/origin/patch"
        );
        // Only the leading "origin/" is stripped once, not both occurrences.
        assert_eq!(
            normalize_hosted_review_base_ref("origin/origin/main"),
            "origin/main"
        );
        // "origin/" is a substring starting at index 3, not the true prefix.
        assert_eq!(
            normalize_hosted_review_base_ref("my-origin/main"),
            "my-origin/main"
        );
    }

    /// G2: `[^/]+` requires a non-empty segment; both edge cases fail to
    /// match and are returned unchanged.
    #[test]
    fn pin_g2_remotes_prefix_requires_a_nonempty_segment_and_a_trailing_slash() {
        assert_eq!(
            normalize_hosted_review_head_ref("refs/remotes//x"),
            "refs/remotes//x"
        );
        assert_eq!(
            normalize_hosted_review_head_ref("refs/remotes/origin"),
            "refs/remotes/origin"
        );
    }

    /// G3: ECMAScript whitespace, not Rust's — U+FEFF is trimmed, U+0085 is not.
    #[test]
    fn pin_g3_trim_uses_ecmascript_whitespace_not_rust_whitespace() {
        assert_eq!(
            normalize_hosted_review_head_ref("\u{FEFF}refs/heads/x\u{FEFF}"),
            "x"
        );
        assert_eq!(normalize_hosted_review_head_ref("\u{85}main"), "\u{85}main");
    }

    /// G4: the only witness separating the two normalization-order
    /// possibilities in `normalize_hosted_review_base_ref`.
    #[test]
    fn pin_g4_base_ref_step_order_head_normalization_runs_first() {
        assert_eq!(
            normalize_hosted_review_base_ref("origin/refs/heads/x"),
            "refs/heads/x"
        );
    }

    /// G5: `normalize_hosted_review_base_ref` must delegate to
    /// `normalize_hosted_review_head_ref` — which itself `js_trim`s — rather
    /// than reimplementing equivalent-looking logic that could drop a step
    /// like trimming. Nothing else in this suite exercises whitespace
    /// through the base-ref entry point.
    #[test]
    fn pin_g5_base_ref_delegates_head_normalization_including_trim() {
        assert_eq!(normalize_hosted_review_base_ref(" origin/main "), "main");
    }
}
