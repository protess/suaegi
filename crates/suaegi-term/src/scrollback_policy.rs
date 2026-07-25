//! Port of Orca `shared/terminal-scrollback-policy.ts` (@ v1.4.150-rc.0).
//!
//! Pure numeric policy for terminal scrollback: row normalization/clamping, the
//! output-backlog cap that scales with scrollback, and legacy byte→row bucket
//! migration. JS `unknown` inputs are modeled as `Option<f64>` — `None` for a
//! non-number/undefined value, and `Some(x)` where a non-finite `x` (NaN/±∞) is
//! rejected by the finite check (mirrors `Number.isFinite`).

pub const DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT: i64 = 5_000;
pub const DESKTOP_TERMINAL_SCROLLBACK_ROWS_MIN: i64 = 1_000;
pub const DESKTOP_TERMINAL_SCROLLBACK_ROWS_MAX: i64 = 50_000;
pub const DESKTOP_TERMINAL_SCROLLBACK_ROW_PRESETS: [i64; 4] = [5_000, 10_000, 25_000, 50_000];

pub const LEGACY_TERMINAL_SCROLLBACK_BYTES_1_MB: i64 = 1_000_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BYTES_10_MB: i64 = 10_000_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BYTES_25_MB: i64 = 25_000_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BYTES_50_MB: i64 = 50_000_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BYTES_100_MB: i64 = 100_000_000;

pub const LEGACY_TERMINAL_SCROLLBACK_BUCKET_5K_MAX_BYTES: i64 = 17_500_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BUCKET_10K_MAX_BYTES: i64 = 37_500_000;
pub const LEGACY_TERMINAL_SCROLLBACK_BUCKET_25K_MAX_BYTES: i64 = 75_000_000;

/// `Math.min(MAX, Math.max(min, Math.floor(value)))`. `value` is finite (the
/// caller filters NaN/∞), so the final `as i64` cast is bounded by MAX.
fn clamp_rows(value: f64, min: i64) -> i64 {
    value
        .floor()
        .max(min as f64)
        .min(DESKTOP_TERMINAL_SCROLLBACK_ROWS_MAX as f64) as i64
}

/// Normalize a persisted desktop scrollback-rows setting; non-finite → default.
pub fn normalize_desktop_terminal_scrollback_rows(value: Option<f64>) -> i64 {
    match value {
        Some(v) if v.is_finite() => clamp_rows(v, DESKTOP_TERMINAL_SCROLLBACK_ROWS_MIN),
        _ => DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT,
    }
}

/// Minimum output-backlog cap (2 MiB) regardless of scrollback.
pub const TERMINAL_OUTPUT_BACKLOG_MIN_CAP_CHARS: i64 = 2 * 1024 * 1024;
const OUTPUT_BACKLOG_CHARS_PER_SCROLLBACK_ROW: i64 = 120;

/// The output-backlog cap in chars, scaling with normalized scrollback rows
/// above the 2 MiB floor.
pub fn terminal_output_backlog_cap_chars(scrollback_rows: Option<f64>) -> i64 {
    let rows = normalize_desktop_terminal_scrollback_rows(scrollback_rows);
    TERMINAL_OUTPUT_BACKLOG_MIN_CAP_CHARS.max(rows * OUTPUT_BACKLOG_CHARS_PER_SCROLLBACK_ROW)
}

/// Normalize a snapshot-rows value, preserving the visible-screen-only zero;
/// non-finite → `None` (undefined).
pub fn normalize_desktop_terminal_snapshot_rows(value: Option<f64>) -> Option<i64> {
    match value {
        Some(v) if v.is_finite() => Some(clamp_rows(v, 0)),
        _ => None,
    }
}

/// Migrate a legacy scrollback byte budget to a row count by intent (bucketed),
/// not byte-to-row math. Non-finite or non-positive → default.
pub fn legacy_terminal_scrollback_bytes_to_rows(bytes: Option<f64>) -> i64 {
    let bytes = match bytes {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => return DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT,
    };
    if bytes <= LEGACY_TERMINAL_SCROLLBACK_BYTES_1_MB as f64 {
        return DESKTOP_TERMINAL_SCROLLBACK_ROWS_MIN;
    }
    if bytes < LEGACY_TERMINAL_SCROLLBACK_BUCKET_5K_MAX_BYTES as f64 {
        return DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT;
    }
    if bytes < LEGACY_TERMINAL_SCROLLBACK_BUCKET_10K_MAX_BYTES as f64 {
        return 10_000;
    }
    if bytes < LEGACY_TERMINAL_SCROLLBACK_BUCKET_25K_MAX_BYTES as f64 {
        return 25_000;
    }
    DESKTOP_TERMINAL_SCROLLBACK_ROWS_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_the_desktop_row_defaults_and_presets() {
        assert_eq!(DESKTOP_TERMINAL_SCROLLBACK_ROWS_DEFAULT, 5_000);
        assert_eq!(DESKTOP_TERMINAL_SCROLLBACK_ROWS_MIN, 1_000);
        assert_eq!(DESKTOP_TERMINAL_SCROLLBACK_ROWS_MAX, 50_000);
        assert_eq!(
            DESKTOP_TERMINAL_SCROLLBACK_ROW_PRESETS,
            [5_000, 10_000, 25_000, 50_000]
        );
    }

    #[test]
    fn normalizes_persisted_desktop_rows_without_string_coercion() {
        // undefined and a string both arrive as None (non-number) → default.
        assert_eq!(normalize_desktop_terminal_scrollback_rows(None), 5_000);
        assert_eq!(
            normalize_desktop_terminal_scrollback_rows(Some(f64::NAN)),
            5_000
        );
        assert_eq!(
            normalize_desktop_terminal_scrollback_rows(Some(500.9)),
            1_000
        );
        assert_eq!(
            normalize_desktop_terminal_scrollback_rows(Some(25_000.9)),
            25_000
        );
        assert_eq!(
            normalize_desktop_terminal_scrollback_rows(Some(100_000.0)),
            50_000
        );
    }

    #[test]
    fn normalizes_snapshot_rows_preserving_visible_screen_only_zero() {
        assert_eq!(normalize_desktop_terminal_snapshot_rows(None), None); // undefined & string
        assert_eq!(normalize_desktop_terminal_snapshot_rows(Some(0.0)), Some(0));
        assert_eq!(normalize_desktop_terminal_snapshot_rows(Some(-1.0)), Some(0));
        assert_eq!(
            normalize_desktop_terminal_snapshot_rows(Some(25_000.9)),
            Some(25_000)
        );
        assert_eq!(
            normalize_desktop_terminal_snapshot_rows(Some(100_000.0)),
            Some(50_000)
        );
    }

    #[test]
    fn scales_the_output_backlog_cap_with_scrollback_rows_above_2mb_floor() {
        assert_eq!(
            terminal_output_backlog_cap_chars(None),
            TERMINAL_OUTPUT_BACKLOG_MIN_CAP_CHARS
        );
        assert_eq!(
            terminal_output_backlog_cap_chars(Some(5_000.0)),
            TERMINAL_OUTPUT_BACKLOG_MIN_CAP_CHARS
        );
        // 'garbage' string → None → default 5000 → floor.
        assert_eq!(
            terminal_output_backlog_cap_chars(None),
            TERMINAL_OUTPUT_BACKLOG_MIN_CAP_CHARS
        );
        assert_eq!(terminal_output_backlog_cap_chars(Some(25_000.0)), 3_000_000);
        assert_eq!(terminal_output_backlog_cap_chars(Some(50_000.0)), 6_000_000);
        assert_eq!(
            terminal_output_backlog_cap_chars(Some(1_000_000.0)),
            6_000_000
        );
    }

    #[test]
    fn migrates_legacy_decimal_mb_buckets_by_intent() {
        assert_eq!(legacy_terminal_scrollback_bytes_to_rows(None), 5_000);
        assert_eq!(legacy_terminal_scrollback_bytes_to_rows(Some(0.0)), 5_000);
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(1_000_000.0)),
            1_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(10_000_000.0)),
            5_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(25_000_000.0)),
            10_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(50_000_000.0)),
            25_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(100_000_000.0)),
            50_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(250_000_000.0)),
            50_000
        );
    }

    // --- D3 extra pins: bucket boundary operators (<= vs <) ---

    #[test]
    fn d3_bucket_boundary_values() {
        // Exactly 1 MB (<=) → MIN.
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(1_000_000.0)),
            1_000
        );
        // Exactly 17.5 MB is NOT < 17.5M → falls to the 10k bucket.
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(17_500_000.0)),
            10_000
        );
        // Exactly 37.5 MB → 25k bucket.
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(37_500_000.0)),
            25_000
        );
        // Exactly 75 MB → MAX.
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(75_000_000.0)),
            50_000
        );
    }

    #[test]
    fn d3_nan_and_infinity_are_default() {
        assert_eq!(
            normalize_desktop_terminal_scrollback_rows(Some(f64::INFINITY)),
            5_000
        );
        assert_eq!(
            legacy_terminal_scrollback_bytes_to_rows(Some(f64::NAN)),
            5_000
        );
        assert_eq!(
            normalize_desktop_terminal_snapshot_rows(Some(f64::INFINITY)),
            None
        );
    }
}
