//! Ephemeral setup-terminal worktree-id branding — verbatim port of Orca's
//! `src/shared/ephemeral-setup-terminal-worktree-id.ts` (@ v1.4.146-rc.0).
//!
//! Inline setup/onboarding terminals have no backing worktree. Branding their
//! per-panel id lets the terminal RPC layer scope them to the floating
//! terminal, instead of leaking an unresolvable selector to a remote runtime
//! (#6789).
//!
//! **Not git**: unlike [`crate::stable_pane_id`] (whose branded-id
//! validate/construct pair this module mirrors structurally), a worktree id
//! here never denotes a real git worktree — it denotes the *absence* of one.
//!
//! # Prefix literal (H7)
//! The prefix constant's literal value is asserted nowhere in the upstream
//! test suite — every oracle assertion goes through the `${PREFIX}` template
//! or through [`brand_ephemeral_setup_terminal_worktree_id`] itself, so
//! changing the literal (even dropping the trailing colon) leaves every
//! oracle case green. The trailing colon is part of the constant. Pin the
//! literal directly.
//!
//! # Round-trip is uninformative (H8)
//! `is(brand(x))` is a tautology of the shared constant: both branches of
//! `brand` are driven by `is` (`:11`), so if the constant's value were wrong,
//! the round-trip would still hold. It is not mutation-hunted here — see the
//! module tests for the direct-literal and direct-behavior pins that replace
//! it as real coverage.
//!
//! # `is` is a bare `startsWith` (H9)
//! [`is_ephemeral_setup_terminal_worktree_id`] validates only the prefix, not
//! the suffix. `is(PREFIX)` (empty suffix) is `true`; so is
//! `is(PREFIX + "a::b")` even though the suffix embeds the `::` worktree-id
//! separator the oracle's "does not introduce `::`" test cares about
//! elsewhere. This is intentional upstream behavior — do not add suffix
//! validation.
//!
//! # `brand` is not injective (H10)
//! `brand("")` takes the "not already branded" branch (`is("")` is `false`)
//! and returns the bare prefix, which itself satisfies `is`. So
//! `brand("")` and `brand(PREFIX)` produce the same value — two different
//! inputs collapse to one output.
//!
//! # Upstream hazard, ported unchanged (H12)
//! A real `panelId` that happens to already start with the prefix is
//! silently treated as pre-branded and returned unbranded-relative-to-itself
//! (i.e. not re-prefixed) by [`brand_ephemeral_setup_terminal_worktree_id`];
//! downstream (`runtime-worktree-selector.ts:21`) this is indistinguishable
//! from a deliberately branded id and gets routed to the floating-terminal
//! scope. There is no guard, comment, or test upstream for this collision.
//! Ported as-is, per the `escape_cmd_set_value` precedent of preserving
//! behavior rather than "fixing" an upstream hazard silently.
//!
//! # No trim (H13)
//! Neither `brand` nor `is` trims its input (`:18`); the caller
//! (`runtime-worktree-selector.ts:21`) is responsible for trimming before
//! calling in. Do not add trimming here.

/// Prefix branding an ephemeral setup-terminal worktree id (`:4`). The
/// trailing colon is part of the literal (H7).
pub const EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX: &str = "ephemeral-setup-terminal:";

/// Brand a per-panel setup-terminal id so the terminal RPC layer routes it to
/// the floating-terminal scope on a runtime. Idempotent for already-branded
/// ids (H8) — but not injective (H10): `brand("")` and
/// `brand(EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX)` both return the bare
/// prefix. A `panel_id` that already starts with the prefix for unrelated
/// reasons is returned unmodified rather than re-prefixed (H12).
pub fn brand_ephemeral_setup_terminal_worktree_id(panel_id: &str) -> String {
    if is_ephemeral_setup_terminal_worktree_id(panel_id) {
        panel_id.to_string()
    } else {
        format!("{EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX}{panel_id}")
    }
}

/// Whether `worktree_id` is a branded ephemeral setup-terminal id. A bare
/// `startsWith` (H9): validates the prefix only, never the suffix — no `::`
/// check, no non-empty-suffix requirement.
pub fn is_ephemeral_setup_terminal_worktree_id(worktree_id: &str) -> bool {
    worktree_id.starts_with(EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::{
        brand_ephemeral_setup_terminal_worktree_id, is_ephemeral_setup_terminal_worktree_id,
        EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX,
    };

    // Oracle: ephemeral-setup-terminal-worktree-id.test.ts

    #[test]
    fn brands_a_panel_id_with_the_ephemeral_prefix() {
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(
                "feature-wall-orchestration-skill-terminal"
            ),
            format!(
                "{EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX}feature-wall-orchestration-skill-terminal"
            )
        );
    }

    #[test]
    fn is_idempotent_for_already_branded_ids() {
        let branded =
            brand_ephemeral_setup_terminal_worktree_id("settings-orchestration-skill-terminal");
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(&branded),
            branded
        );
    }

    #[test]
    fn recognizes_branded_ids_and_rejects_real_worktree_ids() {
        assert!(is_ephemeral_setup_terminal_worktree_id(
            &brand_ephemeral_setup_terminal_worktree_id("feature-tip-cli-skills-terminal")
        ));
        assert!(!is_ephemeral_setup_terminal_worktree_id(
            "repo-1::/work/orca/wt"
        ));
        assert!(!is_ephemeral_setup_terminal_worktree_id(
            "global-floating-terminal"
        ));
    }

    #[test]
    fn does_not_introduce_the_worktree_id_separator() {
        assert!(
            !brand_ephemeral_setup_terminal_worktree_id("onboarding-inline-terminal")
                .contains("::")
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// H7 crux pin: the prefix literal itself, including the trailing colon.
    /// The upstream test suite never asserts this literal — every case goes
    /// through the symbol or `brand()`.
    #[test]
    fn pin_prefix_literal() {
        assert_eq!(
            EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX,
            "ephemeral-setup-terminal:"
        );
    }

    /// H9: `is` validates only the prefix. An empty suffix (the bare prefix)
    /// is accepted...
    #[test]
    fn pin_is_accepts_bare_prefix_with_empty_suffix() {
        assert!(is_ephemeral_setup_terminal_worktree_id(
            EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX
        ));
    }

    /// ...and so is a suffix that embeds the `::` worktree-id separator —
    /// `is` performs no suffix validation at all.
    #[test]
    fn pin_is_accepts_suffix_containing_worktree_separator() {
        assert!(is_ephemeral_setup_terminal_worktree_id(&format!(
            "{EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX}repo-1::/x"
        )));
    }

    /// H10: `brand` is not injective — `brand("")` returns the bare prefix...
    #[test]
    fn pin_brand_empty_string_returns_bare_prefix() {
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(""),
            EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX
        );
    }

    /// ...which is the same value `brand(PREFIX)` produces (idempotence on
    /// the prefix itself), so two distinct inputs collapse to one output.
    #[test]
    fn pin_brand_is_not_injective() {
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(""),
            brand_ephemeral_setup_terminal_worktree_id(EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX)
        );
    }

    /// H11 boundary #1: a true partial prefix with no trailing colon is not
    /// recognized.
    #[test]
    fn pin_partial_prefix_without_colon_is_rejected() {
        assert!(!is_ephemeral_setup_terminal_worktree_id(
            "ephemeral-setup-terminal"
        ));
    }

    /// H11 boundary #2: the prefix appearing mid-string, not at the start, is
    /// not recognized.
    #[test]
    fn pin_prefix_in_the_middle_is_rejected() {
        assert!(!is_ephemeral_setup_terminal_worktree_id(
            "x-ephemeral-setup-terminal:y"
        ));
    }

    /// H11 boundary #3: matching is case-sensitive.
    #[test]
    fn pin_case_sensitive_prefix_is_rejected() {
        assert!(!is_ephemeral_setup_terminal_worktree_id(
            "EPHEMERAL-SETUP-TERMINAL:x"
        ));
    }

    /// H12: a `panel_id` that happens to already start with the prefix is
    /// returned unmodified by `brand` (silently treated as pre-branded) —
    /// the ported-unchanged upstream collision hazard.
    #[test]
    fn pin_panel_id_colliding_with_prefix_is_not_re_branded() {
        let colliding =
            format!("{EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX}not-actually-branded");
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(&colliding),
            colliding
        );
    }

    /// H13: no trimming — leading/trailing whitespace on the input is
    /// preserved in the branded output.
    #[test]
    fn pin_no_trim() {
        assert_eq!(
            brand_ephemeral_setup_terminal_worktree_id(" spaced "),
            format!("{EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX} spaced ")
        );
    }
}
