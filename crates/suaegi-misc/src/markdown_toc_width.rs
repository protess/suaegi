//! Markdown TOC panel width clamp — verbatim port of Orca's
//! `src/shared/markdown-toc-panel-width.ts` (@ v1.4.150-rc.0).
//!
//! **No rounding anywhere** — decimals pass straight through min/max, so this is
//! `f64` end to end. `width: unknown` is modeled as `Option<f64>` (`None` =
//! non-number → fallback; `Some(non-finite)` → fallback). `container_width:
//! Option<f64>` (`None` → MAX; `Some` → `computeMax`, whose own guard maps
//! `<= 0`/non-finite → MAX). The second `clamp` argument is always a *container*
//! width, never a precomputed max.

pub const MARKDOWN_TOC_PANEL_MIN_WIDTH: f64 = 200.0;
pub const MARKDOWN_TOC_PANEL_DEFAULT_WIDTH: f64 = 240.0;
pub const MARKDOWN_TOC_PANEL_MIN_EDITOR_WIDTH: f64 = 320.0;
pub const MARKDOWN_TOC_PANEL_MAX_WIDTH: f64 = 600.0;

/// `Math.min(600, Math.max(200, containerWidth - 320))`, with non-finite or
/// non-positive container width short-circuiting to MAX (600).
pub fn compute_max_markdown_toc_panel_width(container_width: f64) -> f64 {
    if !container_width.is_finite() || container_width <= 0.0 {
        return MARKDOWN_TOC_PANEL_MAX_WIDTH;
    }
    MARKDOWN_TOC_PANEL_MAX_WIDTH.min(
        (container_width - MARKDOWN_TOC_PANEL_MIN_EDITOR_WIDTH).max(MARKDOWN_TOC_PANEL_MIN_WIDTH),
    )
}

/// `Math.min(maxWidth, Math.max(200, width))`. Non-number / non-finite `width`
/// → `fallback` (Orca default 240). `container_width` `None` → MAX; `Some(cw)`
/// → `computeMax(cw)`.
pub fn clamp_markdown_toc_panel_width(
    width: Option<f64>,
    container_width: Option<f64>,
    fallback: f64,
) -> f64 {
    let width = match width {
        Some(w) if w.is_finite() => w,
        _ => return fallback,
    };
    let max_width = match container_width {
        Some(cw) => compute_max_markdown_toc_panel_width(cw),
        None => MARKDOWN_TOC_PANEL_MAX_WIDTH,
    };
    max_width.min(width.max(MARKDOWN_TOC_PANEL_MIN_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Oracle: markdown-toc-panel-width.test.ts

    #[test]
    fn clamps_widths_into_the_supported_range() {
        assert_eq!(
            clamp_markdown_toc_panel_width(None, None, MARKDOWN_TOC_PANEL_DEFAULT_WIDTH),
            MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
        );
        assert_eq!(
            clamp_markdown_toc_panel_width(Some(100.0), None, MARKDOWN_TOC_PANEL_DEFAULT_WIDTH),
            MARKDOWN_TOC_PANEL_MIN_WIDTH
        );
        assert_eq!(
            clamp_markdown_toc_panel_width(Some(900.0), None, MARKDOWN_TOC_PANEL_DEFAULT_WIDTH),
            MARKDOWN_TOC_PANEL_MAX_WIDTH
        );
    }

    #[test]
    fn respects_the_remaining_editor_width_when_a_container_size_is_known() {
        assert_eq!(compute_max_markdown_toc_panel_width(700.0), 380.0);
        assert_eq!(
            clamp_markdown_toc_panel_width(
                Some(500.0),
                Some(700.0),
                MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
            ),
            380.0
        );
        assert_eq!(
            clamp_markdown_toc_panel_width(
                Some(350.0),
                Some(700.0),
                MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
            ),
            350.0
        );
    }

    #[test]
    fn treats_the_second_argument_as_container_width_not_a_precomputed_max() {
        let max_for_700 = compute_max_markdown_toc_panel_width(700.0); // 380
                                                                       // Re-fed as a container: computeMax(380) = min(600, max(200, 60)) = 200.
        assert_eq!(
            clamp_markdown_toc_panel_width(
                Some(350.0),
                Some(max_for_700),
                MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
            ),
            200.0
        );
        assert_eq!(
            clamp_markdown_toc_panel_width(
                Some(350.0),
                Some(700.0),
                MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
            ),
            350.0
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// Decimals pass through untouched — no rounding.
    #[test]
    fn pin_decimal_pass_through() {
        assert_eq!(
            clamp_markdown_toc_panel_width(
                Some(350.5),
                Some(700.0),
                MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
            ),
            350.5
        );
        assert_eq!(compute_max_markdown_toc_panel_width(700.5), 380.5);
    }

    /// computeMax guard maps 0 / negative / NaN container widths to MAX (600).
    #[test]
    fn pin_compute_max_non_positive_and_nan() {
        assert_eq!(compute_max_markdown_toc_panel_width(0.0), 600.0);
        assert_eq!(compute_max_markdown_toc_panel_width(-5.0), 600.0);
        assert_eq!(compute_max_markdown_toc_panel_width(f64::NAN), 600.0);
    }

    /// NaN width → fallback (the non-finite arm of the unknown guard).
    #[test]
    fn pin_nan_width_falls_back() {
        assert_eq!(
            clamp_markdown_toc_panel_width(Some(f64::NAN), None, MARKDOWN_TOC_PANEL_DEFAULT_WIDTH),
            MARKDOWN_TOC_PANEL_DEFAULT_WIDTH
        );
    }
}
