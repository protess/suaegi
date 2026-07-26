//! Terminal line-height clamp — verbatim port of Orca's
//! `src/shared/terminal-line-height-settings.ts` (@ v1.4.146-rc.0).
//!
//! **No rounding anywhere** — decimals pass straight through min/max, so this
//! is `f64` end to end; the consumer at `persistence.ts:3027-3028` compares
//! the normalized result with `!==` to decide whether to rewrite a settings
//! file, so any rounding or clamp-order change would cause spurious rewrites
//! on every load. `value: unknown` is modeled as `Option<f64>` (`None` =
//! non-number, matching the JS `typeof value !== 'number'` arm) — this makes
//! numeric-string coercion (`Number("2")`, `parseFloat`, ...) structurally
//! impossible, since the source performs none.
//!
//! The `is_finite` guard is kept even though the oracle's 7 cases never
//! witness it: Rust's `f64::min`/`f64::max` *absorb* NaN (IEEE `minNum`)
//! where JS's `Math.min`/`Math.max` *propagate* it, so a guardless transcription
//! of the last line alone would still pass every oracle case by accident. The
//! guard's real job is `±Infinity`, which the min/max chain alone would let
//! through unclamped in one direction. Do not replace the guard + clamp with
//! `f64::clamp`: that method has a third behavior (NaN self stays NaN, and it
//! panics if either bound is NaN), not either of the two above.

/// `MIN` and `MAX` are pinned to the exact upstream literals (`1`/`3`), not
/// merely to the oracle-satisfying ranges `[0.85, 1]`/`[3, 4]` that the 7
/// fixture cases alone would admit.
pub const MIN_TERMINAL_LINE_HEIGHT: f64 = 1.0;
pub const MAX_TERMINAL_LINE_HEIGHT: f64 = 3.0;

/// `Math.min(MAX, Math.max(MIN, value))`, with `None` or non-finite `value`
/// short-circuiting to `MIN` (older or user-edited profiles can bypass the UI
/// clamp, and xterm throws during construction when `lineHeight` is below one).
pub fn normalize_terminal_line_height(value: Option<f64>) -> f64 {
    let value = match value {
        Some(v) if v.is_finite() => v,
        _ => return MIN_TERMINAL_LINE_HEIGHT,
    };
    MAX_TERMINAL_LINE_HEIGHT.min(value.max(MIN_TERMINAL_LINE_HEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: terminal-line-height-settings.test.ts

    #[test]
    fn normalizes_undefined_to_min() {
        assert_eq!(normalize_terminal_line_height(None), MIN_TERMINAL_LINE_HEIGHT);
    }

    #[test]
    fn normalizes_nan_to_min() {
        assert_eq!(normalize_terminal_line_height(Some(f64::NAN)), MIN_TERMINAL_LINE_HEIGHT);
    }

    #[test]
    fn normalizes_below_min_to_min() {
        assert_eq!(normalize_terminal_line_height(Some(0.85)), MIN_TERMINAL_LINE_HEIGHT);
    }

    #[test]
    fn normalizes_min_to_itself() {
        assert_eq!(normalize_terminal_line_height(Some(1.0)), 1.0);
    }

    #[test]
    fn normalizes_mid_range_value_untouched() {
        assert_eq!(normalize_terminal_line_height(Some(1.35)), 1.35);
    }

    #[test]
    fn normalizes_max_to_itself() {
        assert_eq!(normalize_terminal_line_height(Some(3.0)), 3.0);
    }

    #[test]
    fn normalizes_above_max_to_max() {
        assert_eq!(normalize_terminal_line_height(Some(4.0)), MAX_TERMINAL_LINE_HEIGHT);
    }

    // Mandatory extra pins (oracle-silent):

    /// The oracle's 7 cases never witness `±Infinity`: Rust's `f64::min`/`max`
    /// absorb NaN, so a guardless port already passes all 7. `+Infinity` is
    /// the only value that separates a port with the `is_finite` guard from
    /// one without it.
    #[test]
    fn pin_positive_infinity_falls_back_to_min() {
        assert_eq!(normalize_terminal_line_height(Some(f64::INFINITY)), MIN_TERMINAL_LINE_HEIGHT);
    }

    #[test]
    fn pin_negative_infinity_falls_back_to_min() {
        assert_eq!(normalize_terminal_line_height(Some(f64::NEG_INFINITY)), MIN_TERMINAL_LINE_HEIGHT);
    }

    /// MIN/MAX literals, pinned directly (the 7 oracle cases alone admit any
    /// MIN in [0.85, 1] and any MAX in [3, 4]).
    #[test]
    fn pin_min_max_literals() {
        assert_eq!(MIN_TERMINAL_LINE_HEIGHT, 1.0);
        assert_eq!(MAX_TERMINAL_LINE_HEIGHT, 3.0);
    }

    #[test]
    fn pin_value_above_max_range_clamps_to_exactly_max() {
        assert_eq!(normalize_terminal_line_height(Some(3.5)), 3.0);
    }

    #[test]
    fn pin_value_below_min_range_clamps_to_exactly_min() {
        assert_eq!(normalize_terminal_line_height(Some(0.95)), 1.0);
    }

    /// No numeric coercion of strings: the source has zero `Number()` /
    /// `parseFloat` / `parseInt` calls, so a non-numeric `unknown` (modeled
    /// as `None`, since `Option<f64>` makes coercion structurally impossible)
    /// always falls back to MIN rather than being parsed.
    #[test]
    fn pin_non_numeric_falls_back_to_min() {
        assert_eq!(normalize_terminal_line_height(None), MIN_TERMINAL_LINE_HEIGHT);
    }

    /// No rounding: a fractional value passes through bit-identical.
    #[test]
    fn pin_no_rounding_bit_identical_pass_through() {
        assert_eq!(normalize_terminal_line_height(Some(1.35)), 1.35);
    }

    /// `-0.0` clamps up to MIN like any other value below the floor, and is
    /// not treated as a distinct zero-ish sentinel.
    #[test]
    fn pin_negative_zero_clamps_to_min() {
        assert_eq!(normalize_terminal_line_height(Some(-0.0)), 1.0);
    }
}
