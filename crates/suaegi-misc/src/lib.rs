//! `suaegi-misc` — a batch of eighteen small, self-contained pure helpers
//! ported verbatim from Orca's `src/shared/*`. Baselines differ per module:
//! the original sixteen are @ v1.4.150-rc.0, and [`terminal_line_height`] /
//! [`ui_language`] are @ v1.4.146-rc.0. None import anything (no clock, no
//! fs, no base64, no hashing); each has a Vitest oracle ported bit-for-bit,
//! plus the oracle-silent "extra pins" that guard the real JS↔Rust
//! divergences (ECMAScript whitespace, ASCII-digit / lowercase-UUID rules,
//! UTF-16-vs-byte scan caps, the never-panic OSC trim, and NaN-absorbing vs.
//! NaN-propagating min/max).
//!
//! # Modules
//! - [`clipboard_text`] — clipboard text byte-length limits (UTF-8 byte
//!   measurement vs. JS UTF-16 code-unit fast path, `...WithYield` reshaped
//!   to synchronous + injected callback, `Option<f64>` JS numeric coercion
//!   for `max_bytes`/`stop_after_bytes`, disjoint case-sensitive error-message
//!   predicates, never trims).
//! - [`codex_auth_errors`] — Codex CLI auth-error detection/extraction
//!   (10 `/i` regexes expanded to 15 ASCII-lowercased literals, CSI-only ANSI
//!   strip, UTF-16 4,000-unit cap, `<=`-guarded line iterator distinct from
//!   `process_output_field_scanner`'s `<` sibling).
//! - [`harness_injected_user_turns`] — harness-injected user-turn text
//!   classification (hand-scanned leading-tag-name match against 19 known
//!   tags, 7 prefix literals, `channel` deliberately excluded from the tag
//!   set).
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
//! - [`protocol_compat`] — runtime/mobile protocol compat verdicts (signed
//!   versions, per-reason enum variants, load-bearing check order).
//! - [`powershell_argument`] — PowerShell literal/native-argv quoting
//!   (hand-scanned backslash-run doubling, no regex).
//! - [`process_output_field_scanner`] — lazy LF/CRLF/CR line iteration + bounded
//!   whitespace-separated field extraction (UTF-16 scan cap, `js_ws` reused).
//! - [`command_token_scanner`] — bounded first-command-token extraction with
//!   quote-fallback semantics, path basename, and whole-token containment
//!   (UTF-16 scan cap, `js_ws` reused).
//! - [`remote_runtime_error`] — remote runtime client error classification
//!   (case-sensitive code set vs. case-insensitive message-fragment scan).
//! - [`tailnet_address`] — Tailnet/CGNAT IPv4 detection (ASCII-digit octets,
//!   leading zeros allowed, `100.64.0.0/10` range check).
//! - [`terminal_line_height`] — terminal line-height clamp (`Option<f64>`
//!   modeling `unknown`, `is_finite` guard kept for `±Infinity` even though
//!   Rust's NaN-absorbing `f64::min`/`max` make the oracle blind to it, no
//!   rounding).
//! - [`ui_language`] — UI language selection (`enum` + sentinel `System`
//!   variant, exact-string closed-set membership, no trim/lowercase/locale-tag
//!   splitting borrowed from the sibling `ui-locale` semantics).

pub mod clipboard_text;
pub mod codex_auth_errors;
pub mod command_token_scanner;
pub mod harness_injected_user_turns;
pub mod image_data_uri;
pub mod js_ws;
pub mod markdown_toc_width;
pub mod osc_title_scan;
pub mod powershell_argument;
pub mod process_output_field_scanner;
pub mod protocol_compat;
pub mod rate_limit_reset;
pub mod remote_runtime_error;
pub mod stable_pane_id;
pub mod tailnet_address;
pub mod terminal_line_height;
pub mod ui_language;
pub mod usage_percentage;

pub use clipboard_text::{
    assert_clipboard_text_within_limit, assert_clipboard_text_within_limit_with_yield,
    assert_clipboard_text_write_within_limit, assert_clipboard_text_write_within_limit_with_yield,
    get_clipboard_text_byte_length, get_clipboard_text_read_max_bytes,
    get_clipboard_text_write_max_bytes, is_clipboard_text_byte_length_over_limit,
    is_clipboard_text_byte_length_over_limit_with_yield, is_clipboard_text_too_large_message,
    is_clipboard_text_write_too_large_message, measure_clipboard_text_byte_length,
    measure_clipboard_text_byte_length_with_yield, ClipboardTextByteLengthMeasurement,
    ClipboardTextError, CLIPBOARD_TEXT_MEASURE_YIELD_CODE_UNITS, CLIPBOARD_TEXT_READ_MAX_BYTES,
    CLIPBOARD_TEXT_TOO_LARGE_ERROR, CLIPBOARD_TEXT_WRITE_MAX_BYTES,
    CLIPBOARD_TEXT_WRITE_TOO_LARGE_ERROR,
};
pub use codex_auth_errors::{
    extract_codex_auth_error, is_codex_auth_error, iterate_codex_output_lines, CodexOutputLines,
};
pub use command_token_scanner::{
    command_contains_token, get_command_token_path_basename, get_first_command_token,
    COMMAND_TOKEN_SCAN_MAX_CHARS,
};
pub use harness_injected_user_turns::{
    is_known_harness_injected_user_turn_text, HARNESS_INJECTED_TURN_PREFIXES,
    KNOWN_HARNESS_TAG_NAMES,
};
pub use image_data_uri::build_image_data_uri;
pub use js_ws::{is_js_whitespace, js_trim};
pub use markdown_toc_width::{
    clamp_markdown_toc_panel_width, compute_max_markdown_toc_panel_width,
    MARKDOWN_TOC_PANEL_DEFAULT_WIDTH, MARKDOWN_TOC_PANEL_MAX_WIDTH,
    MARKDOWN_TOC_PANEL_MIN_EDITOR_WIDTH, MARKDOWN_TOC_PANEL_MIN_WIDTH,
};
pub use osc_title_scan::extract_osc_title_scan_tail;
pub use powershell_argument::{quote_powershell_literal, quote_powershell_native_argument};
pub use process_output_field_scanner::{
    get_process_output_fields, iterate_process_output_lines, ProcessOutputLines,
    PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS,
};
pub use protocol_compat::{
    describe_runtime_compat_block, evaluate_compat, evaluate_runtime_compat, CompatInput,
    CompatVerdict, RuntimeCompatInput, RuntimeCompatVerdict,
};
pub use rate_limit_reset::{
    format_reset_countdown, format_reset_duration, get_reset_countdown_next_tick_delay,
};
pub use remote_runtime_error::{
    is_recoverable_remote_runtime_connection_error, RemoteRuntimeClientErrorLike,
};
pub use stable_pane_id::{
    is_stable_pane_id, is_terminal_leaf_id, make_pane_key, parse_legacy_numeric_pane_key,
    parse_pane_key, LegacyNumericPaneKey, MakePaneKeyError, ParsedPaneKey,
};
pub use tailnet_address::is_tailnet_ipv4_address;
pub use terminal_line_height::{
    normalize_terminal_line_height, MAX_TERMINAL_LINE_HEIGHT, MIN_TERMINAL_LINE_HEIGHT,
};
pub use ui_language::{
    normalize_ui_language, UiLanguage, UI_LANGUAGE_CHINESE, UI_LANGUAGE_ENGLISH,
    UI_LANGUAGE_JAPANESE, UI_LANGUAGE_KOREAN, UI_LANGUAGE_SPANISH, UI_LANGUAGE_SYSTEM,
};
pub use usage_percentage::{
    clamp_used_percent, get_displayed_usage_percentage, normalize_usage_percentage_display,
    UsagePercentageDisplay, DEFAULT_USAGE_PERCENTAGE_DISPLAY,
};
