//! Consumption-meter percentage display — verbatim port of Orca's
//! `src/shared/usage-percentage-display.ts` (@ v1.4.150-rc.0).
//!
//! The one load-bearing contract (Orca #7574): the `remaining` complement is
//! taken **after** rounding the used value (`100 - round(x)`), never
//! `round(100 - x)`. At a `.5` fraction those disagree by 1% — see the
//! `remaining` pins. `Math.round` maps to [`f64::round`]: they diverge only at
//! negative `.5`, and every reachable input to `round` here is clamped
//! non-negative first, so the port is exact.

/// Which way the status bar renders a usage meter: the used capacity, or its
/// remaining complement. `unknown` persisted settings default to `Used`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePercentageDisplay {
    Used,
    Remaining,
}

/// Why: missing settings preserve the consumption-meter behavior introduced in #8167.
pub const DEFAULT_USAGE_PERCENTAGE_DISPLAY: UsagePercentageDisplay = UsagePercentageDisplay::Used;

/// `value === 'used' || value === 'remaining' ? value : 'used'`. Exact string
/// match, **no case-folding** (`'Used'`/`'left'`/absent → `Used`). `None` models
/// a non-string / `undefined` persisted value.
pub fn normalize_usage_percentage_display(value: Option<&str>) -> UsagePercentageDisplay {
    match value {
        Some("used") => UsagePercentageDisplay::Used,
        Some("remaining") => UsagePercentageDisplay::Remaining,
        _ => DEFAULT_USAGE_PERCENTAGE_DISPLAY,
    }
}

/// Single clamp+round for bar width and label. Non-finite → 0. Otherwise
/// `Math.max(0, Math.min(100, Math.round(x)))` — round **first**, then clamp.
// Explicit min/max mirrors the JS `Math.min`/`Math.max` structure verbatim; the
// non-finite input is already rejected above, so `f64::clamp` is not substituted.
#[allow(clippy::manual_clamp)]
pub fn clamp_used_percent(used_percent: f64) -> f64 {
    if !used_percent.is_finite() {
        return 0.0;
    }
    used_percent.round().min(100.0).max(0.0)
}

/// The value the status bar shows. Non-finite → 0 (invalid provider data must
/// not read as "100% remaining"). Otherwise clamp into `[0, 100]`, round, then
/// for `Remaining` take `100 - rounded` — the complement is taken **after**
/// rounding (#7574), never `round(100 - x)`.
// Explicit min/max mirrors the JS `Math.min`/`Math.max` structure verbatim; the
// non-finite input is already rejected above, so `f64::clamp` is not substituted.
#[allow(clippy::manual_clamp)]
pub fn get_displayed_usage_percentage(
    used_percent: f64,
    display: UsagePercentageDisplay,
) -> f64 {
    if !used_percent.is_finite() {
        return 0.0;
    }
    // Math.min(100, Math.max(0, usedPercent))
    let bounded_used_percent = used_percent.max(0.0).min(100.0);
    let rounded_used_percent = bounded_used_percent.round();
    match display {
        UsagePercentageDisplay::Used => rounded_used_percent,
        UsagePercentageDisplay::Remaining => 100.0 - rounded_used_percent,
    }
}

#[cfg(test)]
mod tests {
    use super::UsagePercentageDisplay::{Remaining, Used};
    use super::*;

    // Oracle: usage-percentage-display.test.ts

    #[test]
    fn defaults_unknown_persisted_values_to_used() {
        assert_eq!(normalize_usage_percentage_display(None), Used); // undefined
        assert_eq!(normalize_usage_percentage_display(Some("left")), Used);
    }

    #[test]
    fn shows_either_the_provider_value_or_its_complement() {
        assert_eq!(get_displayed_usage_percentage(6.0, Used), 6.0);
        assert_eq!(get_displayed_usage_percentage(6.0, Remaining), 94.0);
    }

    #[test]
    fn rounds_and_bounds_percentages_for_display() {
        // Crux: positive .5 rounds half-up.
        assert_eq!(get_displayed_usage_percentage(20.5, Used), 21.0);
        // #7574: complement is taken from the rounded used value (21) => 79,
        // NOT round(100 - 20.5) = 80.
        assert_eq!(get_displayed_usage_percentage(20.5, Remaining), 79.0);
        assert_eq!(get_displayed_usage_percentage(120.0, Remaining), 0.0);
        assert_eq!(get_displayed_usage_percentage(-20.0, Used), 0.0);
        assert_eq!(get_displayed_usage_percentage(f64::NAN, Remaining), 0.0);
    }

    #[test]
    fn clamps_non_finite_provider_values_to_zero() {
        assert_eq!(clamp_used_percent(f64::NAN), 0.0);
        assert_eq!(clamp_used_percent(f64::INFINITY), 0.0);
        assert_eq!(clamp_used_percent(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn agrees_whether_given_raw_or_preclamped_used_percent() {
        for raw in [20.5, 6.5, 79.5, 0.5, 99.5] {
            for display in [Used, Remaining] {
                assert_eq!(
                    get_displayed_usage_percentage(clamp_used_percent(raw), display),
                    get_displayed_usage_percentage(raw, display)
                );
            }
        }
    }

    // Mandatory extra pins (oracle-silent):

    /// Half-up on a positive .5, independent of the 20.5 pin.
    #[test]
    fn pin_half_up_on_positive_half() {
        assert_eq!(get_displayed_usage_percentage(0.5, Used), 1.0);
    }

    /// round↔clamp order: a negative .5 rounds toward 0 after being absorbed by
    /// the clamp. (f64::round(-0.5) == -1.0, but the clamp floors it to 0.)
    #[test]
    fn pin_negative_half_through_clamp() {
        assert_eq!(clamp_used_percent(-0.5), 0.0);
        assert_eq!(get_displayed_usage_percentage(-0.5, Used), 0.0);
    }
}
