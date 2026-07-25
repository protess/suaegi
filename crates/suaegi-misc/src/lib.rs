//! `suaegi-misc` — a batch of six small, self-contained pure helpers ported
//! verbatim from Orca's `src/shared/*` (@ v1.4.150-rc.0). None import anything
//! (no clock, no fs, no base64, no hashing); each has a Vitest oracle ported
//! bit-for-bit, plus the oracle-silent "extra pins" that guard the real
//! JS↔Rust divergences (ECMAScript whitespace, ASCII-digit / lowercase-UUID
//! rules, and the never-panic OSC trim).
//!
//! # Modules
//! - [`usage_percentage`] — consumption-meter percentage (clamp → round →
//!   complement order, #7574).
//! - [`rate_limit_reset`] — rate-limit countdown copy (floor/modulo, the
//!   days-branch minute drop, `+1ms` tick).
//! - [`markdown_toc_width`] — TOC panel width clamp (no rounding; decimals pass
//!   through).
//! - [`image_data_uri`] — inline `data:` URI (JS-whitespace strip, no base64
//!   encoding, case-sensitive `image/`).
//! - [`osc_title_scan`] — terminal OSC title scan-tail (byte-safe, char-boundary
//!   snapped `> 4096` trim).
//! - [`stable_pane_id`] — pane-key validation (lowercase-exact UUID, ASCII-digit
//!   legacy check, no hashing).
//! - [`js_ws`] — the shared ECMAScript whitespace predicate + trim, reused by
//!   `image_data_uri` and `stable_pane_id`.

pub mod image_data_uri;
pub mod js_ws;
pub mod markdown_toc_width;
pub mod osc_title_scan;
pub mod rate_limit_reset;
pub mod stable_pane_id;
pub mod usage_percentage;

pub use image_data_uri::build_image_data_uri;
pub use js_ws::{is_js_whitespace, js_trim};
pub use markdown_toc_width::{
    clamp_markdown_toc_panel_width, compute_max_markdown_toc_panel_width,
    MARKDOWN_TOC_PANEL_DEFAULT_WIDTH, MARKDOWN_TOC_PANEL_MAX_WIDTH,
    MARKDOWN_TOC_PANEL_MIN_EDITOR_WIDTH, MARKDOWN_TOC_PANEL_MIN_WIDTH,
};
pub use osc_title_scan::extract_osc_title_scan_tail;
pub use rate_limit_reset::{
    format_reset_countdown, format_reset_duration, get_reset_countdown_next_tick_delay,
};
pub use stable_pane_id::{
    is_stable_pane_id, is_terminal_leaf_id, make_pane_key, parse_legacy_numeric_pane_key,
    parse_pane_key, LegacyNumericPaneKey, MakePaneKeyError, ParsedPaneKey,
};
pub use usage_percentage::{
    clamp_used_percent, get_displayed_usage_percentage, normalize_usage_percentage_display,
    UsagePercentageDisplay, DEFAULT_USAGE_PERCENTAGE_DISPLAY,
};
