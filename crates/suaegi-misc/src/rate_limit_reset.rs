//! Rate-limit reset/expiry countdown copy — verbatim port of Orca's
//! `src/shared/rate-limit-reset-format.ts` (@ v1.4.150-rc.0).
//!
//! Pure: `now` is an injected parameter, never `Date.now()`. Inputs are `f64`
//! epoch-ms so the `Number.isFinite` guards and the `[NaN, +inf] -> null` case
//! port faithfully; `Math.floor`/`%` are computed on `f64`, and the formatted
//! integer values are cast to `i64` so no `.0` leaks into the string.
//!
//! Preserved quirks: the days-branch **drops minutes entirely** (`6d 7h 30m`
//! formats as `"6d 7h"`); the tick delay adds `+1ms`; the tick unit flips to
//! hours at exactly `DAY_MS` (`>=`).

const MINUTE_MS: f64 = 60_000.0;
const HOUR_MS: f64 = 60.0 * MINUTE_MS;
const DAY_MS: f64 = 24.0 * HOUR_MS;

/// Compact human duration for a rate-limit window, flooring to whole units:
/// `"47m"`, `"3h 54m"`, `"6d 7h"`. Non-positive delta → `"now"`.
pub fn format_reset_duration(ms: f64) -> String {
    if ms <= 0.0 {
        return "now".to_string();
    }
    let total_mins = (ms / 60_000.0).floor();
    if total_mins < 60.0 {
        return format!("{}m", total_mins as i64);
    }
    let hours = (total_mins / 60.0).floor();
    let mins = total_mins % 60.0;
    if hours >= 24.0 {
        let days = (hours / 24.0).floor();
        let rem_hours = hours % 24.0;
        // NOTE (verbatim quirk): the days branch drops `mins` — `6d 7h 30m` -> "6d 7h".
        return if rem_hours > 0.0 {
            format!("{}d {}h", days as i64, rem_hours as i64)
        } else {
            format!("{}d", days as i64)
        };
    }
    if mins > 0.0 {
        format!("{}h {}m", hours as i64, mins as i64)
    } else {
        format!("{}h", hours as i64)
    }
}

/// `"Resets in 3h 54m"` / `"Resets now"` for a window's time-until-reset (ms).
pub fn format_reset_countdown(ms: f64) -> String {
    let duration = format_reset_duration(ms);
    if duration == "now" {
        "Resets now".to_string()
    } else {
        format!("Resets in {duration}")
    }
}

/// Delay (ms) until the soonest reset countdown label would change, or `None`
/// when no future reset needs a tick. Skips non-finite / past / equal reset
/// times; tick unit is hours at/above a day out, minutes otherwise; `+1ms` so
/// the timeout fires just past the boundary; returns the minimum across resets.
pub fn get_reset_countdown_next_tick_delay(now: f64, reset_times: &[f64]) -> Option<f64> {
    let mut next_delay: Option<f64> = None;
    for &reset_at in reset_times {
        if !reset_at.is_finite() || reset_at <= now {
            continue;
        }
        let remaining_ms = reset_at - now;
        let tick_unit_ms = if remaining_ms >= DAY_MS { HOUR_MS } else { MINUTE_MS };
        let delay_ms = (remaining_ms % tick_unit_ms) + 1.0;
        next_delay = Some(match next_delay {
            None => delay_ms,
            Some(prev) => prev.min(delay_ms),
        });
    }
    next_delay
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f64 = 60_000.0;
    const HOUR: f64 = 60.0 * MIN;
    const DAY: f64 = 24.0 * HOUR;

    // Oracle: rate-limit-reset-format.test.ts

    #[test]
    fn returns_now_for_non_positive_deltas() {
        assert_eq!(format_reset_duration(0.0), "now");
        assert_eq!(format_reset_duration(-1.0), "now");
    }

    #[test]
    fn floors_to_whole_units_and_drops_zero_remainders() {
        assert_eq!(format_reset_duration(47.0 * MIN), "47m");
        assert_eq!(format_reset_duration(3.0 * HOUR + 54.0 * MIN), "3h 54m");
        assert_eq!(format_reset_duration(2.0 * HOUR), "2h");
        assert_eq!(format_reset_duration(6.0 * DAY + 7.0 * HOUR), "6d 7h");
        assert_eq!(format_reset_duration(7.0 * DAY), "7d");
    }

    #[test]
    fn countdown_prefixes_the_duration_or_reports_resets_now() {
        assert_eq!(format_reset_countdown(0.0), "Resets now");
        assert_eq!(format_reset_countdown(3.0 * HOUR + 54.0 * MIN), "Resets in 3h 54m");
        assert_eq!(format_reset_countdown(6.0 * DAY + 7.0 * HOUR), "Resets in 6d 7h");
    }

    const NOW: f64 = 1_000_000_000.0;

    #[test]
    fn next_tick_returns_none_when_nothing_to_count_down() {
        assert_eq!(get_reset_countdown_next_tick_delay(NOW, &[]), None);
        assert_eq!(get_reset_countdown_next_tick_delay(NOW, &[NOW - MIN, NOW]), None);
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[f64::NAN, f64::INFINITY]),
            None
        );
    }

    #[test]
    fn next_tick_wakes_just_after_the_next_minute_boundary_under_a_day() {
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[NOW + 90.0 * MIN + 30_000.0]),
            Some(30_000.0 + 1.0)
        );
    }

    #[test]
    fn next_tick_ticks_on_hour_boundaries_a_day_or_more_away() {
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[NOW + 2.0 * DAY + 3.0 * HOUR + 15.0 * MIN]),
            Some(15.0 * MIN + 1.0)
        );
    }

    #[test]
    fn next_tick_returns_the_soonest_delay_across_multiple_resets() {
        let soon = NOW + 5.0 * MIN + 10_000.0;
        let later = NOW + 42.0 * MIN + 40_000.0;
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[later, soon]),
            Some(10_000.0 + 1.0)
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// The days branch drops minutes: `6d 7h 30m` must format as `"6d 7h"`.
    #[test]
    fn pin_days_branch_drops_minutes() {
        assert_eq!(
            format_reset_duration(6.0 * DAY + 7.0 * HOUR + 30.0 * MIN),
            "6d 7h"
        );
    }

    /// 24h boundary: hours becomes 24 -> days unit with zero remHours -> "1d".
    #[test]
    fn pin_twenty_four_hour_boundary() {
        assert_eq!(format_reset_duration(24.0 * HOUR), "1d");
    }

    /// A zero-remainder tick still adds +1 (=> 1), no special-casing.
    #[test]
    fn pin_zero_remainder_tick_is_one() {
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[NOW + 90.0 * MIN]),
            Some(1.0)
        );
    }

    /// Exactly DAY_MS out uses the hours unit (`>=` boundary): remainder 0 -> 1.
    #[test]
    fn pin_exactly_day_ms_uses_hours_unit() {
        assert_eq!(
            get_reset_countdown_next_tick_delay(NOW, &[NOW + DAY]),
            Some(1.0)
        );
    }
}
