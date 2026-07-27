//! ⚠ P1 — Terminal font weight clamp + bold-weight derivation — verbatim port
//! of Orca's `src/shared/terminal-fonts.ts` (@ v1.4.146-rc.0).
//!
//! **`TERMINAL_FONT_WEIGHT_STEP` is exported upstream but used by nothing.**
//! The body is `Math.min(MAX, Math.max(MIN, Math.round(x)))` — round then
//! clamp, no `x / STEP * STEP` snap and no `%`. `normalize(550)` is `550`,
//! not `600` or `500`. Every oracle fixture happens to already be a multiple
//! of 100 (`10`, `1200`, `500`, `800`, `undefined`), so a step-snapping port
//! passes every oracle case; the pins below (`550 -> 550`, `449 -> 449`)
//! exist specifically to kill that mistake. This is production-reachable,
//! not academic: there is no `terminalFontWeight` sanitizer in persistence,
//! and the Ghostty importer writes any finite value through unrounded.
//!
//! **The `is_finite` guard falls back to `DEFAULT` (500), not `MIN` (100)** —
//! unlike the structurally identical [`crate::terminal_line_height`], where
//! the guard's fallback and MIN happen to coincide. A guardless
//! `MAX.min(v.round().max(MIN))` gives `NaN -> 100`, `+Infinity -> 900`,
//! `-Infinity -> 100`; `f64::clamp` gives `NaN -> NaN` and panics if either
//! bound were NaN. The oracle's only fallback fixture is `undefined`, which
//! an `Option<f64>` port answers from its `None` arm without ever running
//! the guard — so `NaN`/`+Infinity`/`-Infinity` (all pinned to `500` below)
//! are the only inputs that separate a guarded port from an unguarded one.
//! Mutation-verified: swapping the guarded min/max chain below for
//! `value.round().clamp(MIN, MAX)` **survives every test in this module**,
//! because the guard has already forced `value` finite by the time this
//! line runs — the two forms are then bit-identical for every reachable
//! input. This is an equivalent mutant *conditional on the guard staying
//! intact*, not a gap in the pins: the real hazard `f64::clamp` warns about
//! only appears if the guard is *also* removed (`F2a` above), which the
//! `NaN`/`±Infinity` pins do kill. Do not read the survival as license to
//! simplify to `f64::clamp` — the guard and the min/max chain are one unit;
//! keep both.
//!
//! In the bold chain `min(MAX, max(BOLD_FLOOR, n + 200))`, the oracle fixture
//! `resolve(500) -> 700` has `500 + 200` coincide exactly with the floor
//! `700`, so the floor and the `+200` delta mask each other: a floorless
//! version, a step function, or any delta in `[100, 200]` with any floor
//! `<= 700` (including no floor at all) all pass that one fixture. The pin
//! `resolve(600) -> {600, 800}` is the single highest-leverage guard against
//! this (kills the step function and any delta other than `+200`);
//! `resolve(100) -> {100, 700}` separately pins the floor.
//!
//! JS `Math.round` is half-toward-`+Infinity`; Rust's `f64::round` is
//! half-away-from-zero. They diverge only on negative half-integers, and
//! every reachable input here (`Math.round(numericFontWeight)` before the
//! `max(MIN)` floor, and `normalizedFontWeight + 200` in the bold chain,
//! which is always positive since `normalizedFontWeight >= MIN = 100`) is
//! never a negative half-integer that survives to be observable post-floor.
//! No input distinguishes the two rounding modes here — kept verbatim per
//! [`crate::usage_percentage`]'s precedent for the same equivalence; do not
//! mutation-hunt it.

/// `unknown` is modeled as `Option<f64>` (`None` = non-number/`undefined`),
/// matching the source's `typeof fontWeight === 'number' ? fontWeight : NaN`
/// coercion: this makes numeric-string coercion structurally impossible,
/// since the source performs none either.
pub const DEFAULT_TERMINAL_FONT_WEIGHT: f64 = 500.0;
pub const TERMINAL_FONT_WEIGHT_MIN: f64 = 100.0;
pub const TERMINAL_FONT_WEIGHT_MAX: f64 = 900.0;
/// Exported upstream (`terminal-fonts.ts:4`) but used by nothing — see the
/// module doc's F1 warning. Kept `pub` for surface parity only.
pub const TERMINAL_FONT_WEIGHT_STEP: f64 = 100.0;

/// Module-private in the source too (`const DEFAULT_TERMINAL_FONT_WEIGHT_BOLD = 700`,
/// not exported) — the floor for the derived bold weight, not a step.
const DEFAULT_TERMINAL_FONT_WEIGHT_BOLD: f64 = 700.0;

/// `Math.min(MAX, Math.max(MIN, Math.round(numericFontWeight)))`, with
/// `None` or non-finite input short-circuiting to `DEFAULT` (not `MIN` —
/// see module doc). No step snapping despite `TERMINAL_FONT_WEIGHT_STEP`
/// existing as a sibling constant.
#[allow(clippy::manual_clamp)]
pub fn normalize_terminal_font_weight(font_weight: Option<f64>) -> f64 {
    let value = match font_weight {
        Some(v) if v.is_finite() => v,
        _ => return DEFAULT_TERMINAL_FONT_WEIGHT,
    };
    TERMINAL_FONT_WEIGHT_MAX.min(value.round().max(TERMINAL_FONT_WEIGHT_MIN))
}

/// `{ fontWeight, fontWeightBold }` pair returned by [`resolve_terminal_font_weights`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalFontWeights {
    pub font_weight: f64,
    pub font_weight_bold: f64,
}

/// Normalizes `font_weight`, then derives the bold weight as
/// `Math.min(MAX, Math.max(BOLD_FLOOR, normalizedFontWeight + 200))` — the
/// floor and the `+200` delta mask each other at `normalize() == 500` (see
/// module doc); `resolve(600) -> {600, 800}` is the pin that tells them apart.
#[allow(clippy::manual_clamp)]
pub fn resolve_terminal_font_weights(font_weight: Option<f64>) -> TerminalFontWeights {
    let normalized_font_weight = normalize_terminal_font_weight(font_weight);
    TerminalFontWeights {
        font_weight: normalized_font_weight,
        font_weight_bold: TERMINAL_FONT_WEIGHT_MAX
            .min((normalized_font_weight + 200.0).max(DEFAULT_TERMINAL_FONT_WEIGHT_BOLD)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: terminal-fonts.test.ts

    #[test]
    fn falls_back_to_the_orca_default_when_the_value_is_missing() {
        assert_eq!(normalize_terminal_font_weight(None), DEFAULT_TERMINAL_FONT_WEIGHT);
    }

    #[test]
    fn clamps_weights_to_the_supported_xterm_range() {
        assert_eq!(normalize_terminal_font_weight(Some(10.0)), 100.0);
        assert_eq!(normalize_terminal_font_weight(Some(1200.0)), 900.0);
    }

    #[test]
    fn keeps_bold_text_heavier_than_the_base_terminal_weight() {
        assert_eq!(
            resolve_terminal_font_weights(Some(500.0)),
            TerminalFontWeights { font_weight: 500.0, font_weight_bold: 700.0 }
        );
        assert_eq!(
            resolve_terminal_font_weights(Some(800.0)),
            TerminalFontWeights { font_weight: 800.0, font_weight_bold: 900.0 }
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// F1: no step snapping. Every oracle input is already a multiple of
    /// 100, so a step-snapping port passes all of them; these are not.
    #[test]
    fn pin_no_step_snapping() {
        assert_eq!(normalize_terminal_font_weight(Some(550.0)), 550.0);
        assert_eq!(normalize_terminal_font_weight(Some(449.0)), 449.0);
    }

    /// F2: the guard's fallback is `DEFAULT` (500), not `MIN` (100). The
    /// oracle's only fallback fixture is `undefined`, which an `Option<f64>`
    /// port answers without ever running the `is_finite` guard — these three
    /// are the only inputs that exercise it.
    #[test]
    fn pin_non_finite_falls_back_to_default_not_min() {
        assert_eq!(normalize_terminal_font_weight(Some(f64::NAN)), DEFAULT_TERMINAL_FONT_WEIGHT);
        assert_eq!(
            normalize_terminal_font_weight(Some(f64::INFINITY)),
            DEFAULT_TERMINAL_FONT_WEIGHT
        );
        assert_eq!(
            normalize_terminal_font_weight(Some(f64::NEG_INFINITY)),
            DEFAULT_TERMINAL_FONT_WEIGHT
        );
    }

    /// F3: rounding is exercised (never witnessed by the all-integer oracle).
    #[test]
    fn pin_rounds_fractional_weights() {
        assert_eq!(normalize_terminal_font_weight(Some(550.6)), 551.0);
        assert_eq!(normalize_terminal_font_weight(Some(550.4)), 550.0);
        assert_eq!(normalize_terminal_font_weight(Some(550.5)), 551.0);
    }

    /// F5: `resolve(600) -> {600, 800}` is the highest-leverage pin in this
    /// module — it kills a step function for the bold weight and any delta
    /// other than exactly `+200`. `resolve(100) -> {100, 700}` separately
    /// pins the `BOLD_FLOOR` (700), which the `resolve(500)` oracle fixture
    /// cannot distinguish from a floorless version.
    #[test]
    fn pin_bold_floor_and_delta_are_independently_observable() {
        assert_eq!(
            resolve_terminal_font_weights(Some(600.0)),
            TerminalFontWeights { font_weight: 600.0, font_weight_bold: 800.0 }
        );
        assert_eq!(
            resolve_terminal_font_weights(Some(100.0)),
            TerminalFontWeights { font_weight: 100.0, font_weight_bold: 700.0 }
        );
    }

    /// F6: `resolve` re-runs normalization on invalid input rather than
    /// skipping it (the oracle's `resolve` fixtures are both already-valid).
    #[test]
    fn pin_resolve_normalizes_invalid_input() {
        assert_eq!(
            resolve_terminal_font_weights(None),
            TerminalFontWeights { font_weight: 500.0, font_weight_bold: 700.0 }
        );
        assert_eq!(
            resolve_terminal_font_weights(Some(1200.0)),
            TerminalFontWeights { font_weight: 900.0, font_weight_bold: 900.0 }
        );
    }

    /// F7: `DEFAULT`/`STEP` are symbol references only in the source (never
    /// pinned to a literal by the oracle) — pin them directly.
    #[test]
    fn pin_default_and_step_literals() {
        assert_eq!(DEFAULT_TERMINAL_FONT_WEIGHT, 500.0);
        assert_eq!(TERMINAL_FONT_WEIGHT_STEP, 100.0);
        assert_eq!(TERMINAL_FONT_WEIGHT_MIN, 100.0);
        assert_eq!(TERMINAL_FONT_WEIGHT_MAX, 900.0);
    }
}
