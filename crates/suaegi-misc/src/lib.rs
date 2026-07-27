//! `suaegi-misc` — a batch of thirty-two small, self-contained pure helpers
//! ported verbatim from Orca's `src/shared/*`. Baselines differ per module:
//! the original sixteen are @ v1.4.150-rc.0, and [`terminal_line_height`] /
//! [`ui_language`] / [`opencode_terminal_title`] / [`agent_title_decoration`]
//! / [`pi_overlay_ui_settings`] / [`hosted_review_refs`] /
//! [`base_ref_search_result`] / [`worktree_base_ref`] /
//! [`worktree_submodule_removal`] / [`ephemeral_setup_terminal_worktree_id`]
//! / [`github_work_items_query_bounds`] / [`github_project_ref_input`] /
//! [`gitlab_projects`] / [`updater_windows_signature_check`] /
//! [`agent_notification_id`] / [`orchestration_task_summary`]
//! are @ v1.4.146-rc.0.
//! None import anything (no clock, no fs, no base64, no hashing); each has a
//! Vitest oracle ported bit-for-bit, plus the oracle-silent "extra pins" that
//! guard the real JS↔Rust divergences (ECMAScript whitespace, ASCII-digit /
//! lowercase-UUID rules, UTF-16-vs-byte scan caps, the never-panic OSC trim,
//! NaN-absorbing vs. NaN-propagating min/max, anchored-once-vs-global regex
//! replacement, and — for [`pi_overlay_ui_settings`] — `[[Set]]` key order
//! becoming a real Rust choice instead of a free JS property).
//!
//! # Modules
//! - [`hosted_review_refs`] — hosted-review head/base ref normalization
//!   (anchored-once `strip_prefix` in place of global/unanchored regex ports,
//!   two-step `refs/remotes/[^/]+/` scan requiring a non-empty segment,
//!   `js_trim`, order-sensitive base-ref delegation).
//! - [`base_ref_search_result`] — legacy base-ref search result derivation
//!   (`length >` guard distinguishing exact-prefix input from a real strip,
//!   literal prefix-list pin, derive-function wiring pin).
//! - [`worktree_base_ref`] — `git worktree add` base-ref resolution
//!   (injected `&mut dyn FnMut(&str) -> bool` probe callback replacing the
//!   async original, short-circuiting first-match loop, remotes-before-heads
//!   candidate order, deliberate `bool`-vs-`Result` divergence documented at
//!   the module header).
//! - [`worktree_submodule_removal`] — submodule worktree-removal refusal
//!   detection (structural twin of `remote_runtime_error`'s `…ErrorLike`
//!   input struct; ASCII-only `/i`→`to_ascii_lowercase` case fold per
//!   `codex_auth_errors`; `get_error_text` made `pub` to pin field order and
//!   `\n` join that the unanchored substring match can't otherwise observe).
//! - [`ephemeral_setup_terminal_worktree_id`] — inline setup/onboarding
//!   terminal id branding (structural twin of `stable_pane_id`'s branded-id
//!   validate/construct pair; bare `startsWith` predicate, non-injective
//!   `brand`, no trim, ported-unchanged upstream collision hazard).
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
//! - [`opencode_terminal_title`] — OpenCode native-title detection (hand-scanned
//!   `OC\s*\|\s*\S` marker, literal `" | "` multiplexer-prefix pipe distinct
//!   from the marker's `\s*` pipe, prefix-group retry, pure alias delegator).
//! - [`agent_title_decoration`] — leading agent-status glyph/text stripping
//!   (no pre-trim, exactly-one replacement, never-empty raw fallback, fused
//!   trailing `\s*`/`trimStart`, local `js_trim_start` mirroring
//!   `suaegi-quickcmd`'s `js_trim_end`).
//! - [`pi_overlay_ui_settings`] — Pi overlay UI settings merge (third private
//!   `JsValue`/`JsRecord` copy, `[[Set]]` overwrite-in-place-else-append key
//!   order pinned directly since the oracle's `toEqual` can't see it, array
//!   top-level input guarded against index-key spreading, zero production
//!   callers — ported for clone-completeness only, wired nowhere).
//! - [`github_work_items_query_bounds`] — GitHub work-items search query byte
//!   cap (delegate's `text.length > max || measure(...).exceededLimit`
//!   collapses to one `text.len() as f64 > max_bytes` comparison across the
//!   entire `f64` domain, `Option<f64>` cap, renderer re-export shim not
//!   ported).
//! - [`github_project_ref_input`] — GitHub project reference input byte cap
//!   plus a submit-gate "non-trivial and within bounds" check (same collapse
//!   as its sibling, `js_ws`-based ECMAScript `\S` non-whitespace scan, a
//!   cap term that is dead in the oracle but pinned directly, byte-length
//!   export with zero production callers ported for surface parity only).
//! - [`gitlab_projects`] — GitLab "recent projects" list computation
//!   (caller-formatted `&str` timestamp in place of `toISOString`,
//!   case-sensitive remove-all dedupe, a real `ToIntegerOrInfinity`-based
//!   `Array.prototype.slice(0, max)` helper distinct from `Vec::truncate`,
//!   surviving entries keep their original timestamp — only the new head is
//!   stamped).
//! - [`updater_windows_signature_check`] — Windows updater signature-check
//!   failure classification (a precedent INVERSION: `.toLowerCase()` here is
//!   full-Unicode in both JS and Rust, so `str::to_lowercase()` is correct —
//!   NOT the `to_ascii_lowercase` this crate uses for non-`u` `/…/i`
//!   regex-derived modules; an oracle-unpinned security veto pinned directly
//!   against a message containing both phrases).
//! - [`agent_notification_id`] — agent-event notification id construction
//!   (load-bearing `encodeURIComponent` disambiguating colon-bearing
//!   `worktreeId`/`paneKey` fields, literal id pins since the oracle never
//!   asserts one, `Option<f64>` accepting `stateStartedAt === 0`).
//! - [`orchestration_task_summary`] — `--brief` orchestration task-spec
//!   abbreviation (`\s+`-collapse + trim applied unconditionally, UTF-16
//!   snap-down truncation reusing the `utf16_slice_prefix` technique, 160 as
//!   a ceiling and not an invariant, `js_ws`/local `js_trim_end` for the
//!   three ECMAScript-whitespace sites, closure-based generic passthrough in
//!   place of an unrepresentable `...task` spread).

pub mod agent_notification_id;
pub mod agent_title_decoration;
pub mod base_ref_search_result;
pub mod clipboard_text;
pub mod codex_auth_errors;
pub mod command_token_scanner;
pub mod ephemeral_setup_terminal_worktree_id;
pub mod github_project_ref_input;
pub mod github_work_items_query_bounds;
pub mod gitlab_projects;
pub mod harness_injected_user_turns;
pub mod hosted_review_refs;
pub mod image_data_uri;
pub mod js_ws;
pub mod markdown_toc_width;
pub mod opencode_terminal_title;
pub mod orchestration_task_summary;
pub mod osc_title_scan;
pub mod pi_overlay_ui_settings;
pub mod powershell_argument;
pub mod process_output_field_scanner;
pub mod protocol_compat;
pub mod rate_limit_reset;
pub mod remote_runtime_error;
pub mod stable_pane_id;
pub mod tailnet_address;
pub mod terminal_line_height;
pub mod ui_language;
pub mod updater_windows_signature_check;
pub mod usage_percentage;
pub mod worktree_base_ref;
pub mod worktree_submodule_removal;

pub use agent_notification_id::build_agent_notification_id;
pub use agent_title_decoration::{
    strip_leading_agent_title_decoration, strip_leading_agent_title_decoration_or_empty,
};
pub use base_ref_search_result::{
    derive_legacy_local_branch_name, legacy_base_ref_search_result, BaseRefSearchResult,
};
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
pub use ephemeral_setup_terminal_worktree_id::{
    brand_ephemeral_setup_terminal_worktree_id, is_ephemeral_setup_terminal_worktree_id,
    EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX,
};
pub use github_project_ref_input::{
    get_github_project_ref_input_byte_length, has_bounded_github_project_ref_input_text,
    is_github_project_ref_input_too_large, GITHUB_PROJECT_REF_INPUT_MAX_BYTES,
    GITHUB_PROJECT_REF_INPUT_TOO_LARGE_ERROR,
};
pub use github_work_items_query_bounds::{
    is_github_work_items_query_too_large, GITHUB_WORK_ITEMS_QUERY_MAX_BYTES,
};
pub use gitlab_projects::{
    compute_next_gitlab_recents, GitLabRecentProject, GITLAB_RECENTS_MAX,
};
pub use harness_injected_user_turns::{
    is_known_harness_injected_user_turn_text, HARNESS_INJECTED_TURN_PREFIXES,
    KNOWN_HARNESS_TAG_NAMES,
};
pub use hosted_review_refs::{normalize_hosted_review_base_ref, normalize_hosted_review_head_ref};
pub use image_data_uri::build_image_data_uri;
pub use js_ws::{is_js_whitespace, js_trim};
pub use markdown_toc_width::{
    clamp_markdown_toc_panel_width, compute_max_markdown_toc_panel_width,
    MARKDOWN_TOC_PANEL_DEFAULT_WIDTH, MARKDOWN_TOC_PANEL_MAX_WIDTH,
    MARKDOWN_TOC_PANEL_MIN_EDITOR_WIDTH, MARKDOWN_TOC_PANEL_MIN_WIDTH,
};
pub use opencode_terminal_title::{
    is_meaningful_opencode_terminal_title, is_opencode_native_title,
};
pub use orchestration_task_summary::{
    abbreviate_orchestration_task_spec, abbreviate_orchestration_tasks, AbbreviatedSpec,
    TASK_SPEC_BRIEF_LENGTH,
};
pub use osc_title_scan::extract_osc_title_scan_tail;
pub use pi_overlay_ui_settings::{
    merge_pi_overlay_ui_settings, PI_OVERLAY_CLEAR_ON_SHRINK, PI_OVERLAY_HIDE_THINKING_BLOCK,
};
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
pub use updater_windows_signature_check::{
    is_windows_signature_check_unavailable_failure, is_windows_signature_mismatch_failure,
};
pub use usage_percentage::{
    clamp_used_percent, get_displayed_usage_percentage, normalize_usage_percentage_display,
    UsagePercentageDisplay, DEFAULT_USAGE_PERCENTAGE_DISPLAY,
};
pub use worktree_base_ref::resolve_worktree_add_base_ref;
pub use worktree_submodule_removal::{
    get_error_text, is_submodule_worktree_removal_refusal, GitErrorFields, GitErrorLike,
};
