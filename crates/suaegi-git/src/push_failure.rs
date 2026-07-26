//! Port of Orca `shared/source-control-push-failure.ts` (@ v1.4.150-rc.0),
//! classification/normalization half only (`:3-6, 13-26, 28-66, 68-135,
//! 137-169`). The AI prompt builder (`:8-11, 171-272`) is out of scope — a
//! later PR (M2), since it needs new `GitStatusEntry`/`GitStagingArea` types
//! and an `area` derivation the current `working_tree_status` can't express.
//!
//! # Ported quirks / divergences (decisions L1-L11, plan
//! `docs/superpowers/plans/2026-07-26-push-failure-m1.md`)
//!
//! - **L1 (highest risk):** every case-insensitive pattern in the source is a
//!   JS `/i` regex on a **non-`u`** literal — i.e. ECMAScript
//!   `Canonicalize`, which is **ASCII-only** case folding (it does NOT fold
//!   U+212A KELVIN SIGN to `k`, or U+017F LATIN SMALL LETTER LONG S to `s`).
//!   Rust `(?i)` is Unicode simple case folding and WOULD fold those. Ported
//!   with **`(?i-u:...)`** instead. Do not "simplify" this to
//!   `.to_lowercase()` and `contains()` (that's full Unicode folding, a
//!   different mechanism used by `suaegi-forge::classify` for unrelated
//!   reasons — copying it here would silently change behavior on non-ASCII
//!   input).
//! - **L2:** character classes are converted one at a time, never via a
//!   blanket `(?-u)`:
//!   - `\d` → `[0-9]` (Rust `\d` is Unicode `Nd`, matching e.g. Arabic-Indic
//!     digits).
//!   - `\b` → kept as `\b` inside an `(?i-u:...)` scope, where the disabled
//!     `u` flag makes it the ASCII word boundary JS uses; written as
//!     `(?-u:\b)` at point of use for readers who skim past the outer group.
//!   - `\s` → the explicit ECMAScript whitespace set (see [`WS_CLASS`]).
//!     Neither Rust `\s` (Unicode `White_Space`, has U+0085, lacks U+FEFF)
//!     nor `(?-u:\s)` (ASCII-only, lacks NBSP/FEFF/etc.) match JS `\s`.
//!   - `.` in `.*` → the explicit non-line-terminator set (see
//!     [`DOT_CLASS`]). JS `.` excludes LF/CR/U+2028/U+2029; Rust `.` excludes
//!     only `\n`.
//! - **L3:** length limits are counted in **chars**, following the
//!   `commit_message_prompt.rs` C1 precedent (Orca measures UTF-16 code
//!   units; exact UTF-16 fidelity is overkill for this heuristic). All
//!   slicing goes through [`take_chars`], which is char-boundary safe (never
//!   panics on non-ASCII input near the cap). The empty check runs BEFORE
//!   the length check in [`has_expanded_push_failure_details`], preserving
//!   source order.
//! - **L4:** whitespace predicate/trim reuse `suaegi_misc::{is_js_whitespace,
//!   js_trim}` rather than a 7th hand-rolled copy of the same codepoint
//!   table (verified identical to the source's `:155-169` table).
//! - **L5:** [`ANSI_PATTERN`] and [`CONTROL_PATTERN`] are ported verbatim,
//!   including the deliberately narrow CSI final-byte class
//!   `[0-9A-PR-TZcf-nq-uy=><~]` (NOT the general `[@-~]`) and the C1 CSI
//!   introducer `\x9b` alongside ESC. `CONTROL_PATTERN` deliberately excludes
//!   TAB, LF, and U+00A0. Do not reuse `suaegi-gen-prompt`'s
//!   `strip_ansi_control_sequences` — it doesn't handle `\x9b`, uses the wide
//!   final-byte class, and doesn't support the `ESC (`/`#`/`;`/`?` prefixes;
//!   it is a different, non-equivalent function.
//! - **L6:** [`normalize_push_failure`]'s 5 steps run in a load-bearing
//!   order: char-cap → strip ANSI → `\r\n?` → `\n` → strip CONTROL → js_trim.
//!   ANSI stripping must precede CR normalization, and CR normalization must
//!   precede CONTROL stripping (the CONTROL class excludes `\x0d`, so a
//!   lone CR would survive if not normalized first).
//! - **L7:** lines are split on **LF only** (`split('\n')`), matching the
//!   source's manual `charCodeAt !== 10` scan (its test spies on
//!   `String.prototype.split` never being called — a JS-implementation
//!   detail; `split('\n')` is the faithful Rust equivalent of the scan, not
//!   of avoiding a stdlib method). Do NOT use `.lines()` (different trailing
//!   `\r` contract). U+2028 does NOT split lines here.
//! - **L8:** [`is_push_hook_failure`] has 7 branches in this exact order:
//!   empty → false; EXCLUSION match → false (wins unconditionally, even over
//!   a lint+context or runner+context match); inline `hook declined to push`
//!   (no `\b`) → true; [`PUSH_HOOK_PATTERN`] → true; runner AND context →
//!   true; lint AND context → true; else false. Matching is against the
//!   whole normalized blob, not per line. `PUSH_CONTEXT_PATTERN`'s spaces are
//!   literal single spaces (not `\s+`).
//! - **L9:** [`REMOTE_PUSH_EXCLUSION_PATTERN`] transcribes all 21
//!   alternatives verbatim, including two dead ones (`failed to push all
//!   needed submodules` and `unable to push submodule` are fully subsumed by
//!   the earlier `submodule` alternative) — kept for upstream-diff fidelity,
//!   not cleaned up. There is no bare `failed to push` alternative.
//! - **L10:** [`summarize_push_failure`] has 4 branches, lint beats hook: no
//!   lines → "Push failed."; any LINT line → "Lint failed during push.";
//!   any HOOK or RUNNER line → "Pre-push hook failed."; else the first line.
//!   It deliberately does not consult the exclusion list — gating (e.g. not
//!   summarizing an auth failure as a hook failure) is the caller's
//!   responsibility.
//! - **L11:** the low-signal (npm noise) filter in [`get_meaningful_lines`]
//!   only runs when at least one signal line is present; with no signal
//!   line, the lines are returned unfiltered. If filtering would remove
//!   every line, the unfiltered lines are returned instead. The oracle has
//!   zero coverage of this function; all three paths are pinned below.

use regex::Regex;
use std::sync::LazyLock;
use suaegi_misc::{is_js_whitespace, js_trim};

const FALLBACK_PUSH_FAILURE_SUMMARY: &str = "Push failed.";
const LINT_PUSH_FAILURE_SUMMARY: &str = "Lint failed during push.";
const PRE_PUSH_FAILURE_SUMMARY: &str = "Pre-push hook failed.";

/// Source `:6`. Scan cap for push-failure classification/normalization, in
/// **chars** (L3), not UTF-16 code units.
pub const PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS: usize = 64 * 1024;

/// The explicit ECMAScript `\s` set (L2): WhiteSpace + LineTerminator,
/// notably including U+FEFF and excluding U+0085 (see `suaegi_misc::js_ws`
/// for the equivalent per-codepoint predicate; this is the regex-class form
/// needed inline in `LOW_SIGNAL_LINE_PATTERN`).
const WS_CLASS: &str = r"[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";

/// The explicit JS `.` set (L2): anything except LF, CR, U+2028, U+2029.
const DOT_CLASS: &str = r"[^\n\r\u{2028}\u{2029}]";

// Source `:13-15`. Global (not case-insensitive). `\d` -> `[0-9]` (L2).
// Introducer handles both ESC (`\x1b`) and the C1 CSI `\x9b` (L5).
static ANSI_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\x1b\x9b][\[\]()#;?]*(?:(?:(?:[a-zA-Z0-9]*(?:;[a-zA-Z0-9]*)*)?\x07)|(?:(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-PR-TZcf-nq-uy=><~]))",
    )
    .unwrap()
});

// Source `:16-18`. Deliberately excludes TAB (`\x09`), LF (`\x0a`), CR
// (`\x0d`), and U+00A0 (L5).
static CONTROL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]").unwrap());

// Not present as a named const in the source (`:32`'s `/\r\n?/g` literal) —
// CRLF or lone CR -> LF, run before CONTROL stripping (L6).
static CRLF_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n?").unwrap());

// Source `:19-20`. Anchored at the start of a (single, already-trimmed) line.
//
// NOTE on flag scoping: `(?-u:...)` (non-unicode mode) restricts the *str*
// regex engine to ASCII-only content inside that group — a literal/escape
// above `\x7f` (e.g. `\u{a0}` in `WS_CLASS`, or `\u{2028}` in `DOT_CLASS`)
// is a hard compile error ("Unicode not allowed here") under `-u`, because
// non-unicode mode is byte-oriented and a lone byte >= 0x80 can't be valid
// UTF-8. So — unlike the pure-ASCII patterns below — `(?i-u:...)` here is
// scoped tightly around just the ASCII keyword alternatives (and `\b`
// stays in its own `(?-u:\b)`), while `WS_CLASS`/`DOT_CLASS` sit in the
// surrounding default (unicode) scope where their high codepoints are
// legal. This changes nothing observable: case-folding and ASCII word
// boundaries never applied to the whitespace/dot classes in the JS source
// either.
static LOW_SIGNAL_LINE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?:(?i-u:npm){ws}+(?i-u:warn|warning)(?-u:\b){dot}*(?i-u:env|config)|(?i-u:npm){ws}+(?i-u:notice)(?-u:\b)|(?i-u:husky){ws}+-{ws}+(?i-u:deprecated)(?-u:\b))",
        ws = WS_CLASS,
        dot = DOT_CLASS,
    ))
    .unwrap()
});

// Source `:21`.
static PUSH_HOOK_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u:(?-u:\b)(?:pre-push|prepush)(?-u:\b))").unwrap());

// Source `:22`.
static PUSH_HOOK_RUNNER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u:(?-u:\b)(?:husky|lint-staged|lefthook)(?-u:\b))").unwrap());

// Source `:23`. Spaces are literal single spaces, not `\s+` (L8).
static PUSH_CONTEXT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u:(?-u:\b)(?:failed to push|hook declined to push|git push)(?-u:\b))").unwrap()
});

// Source `:24`.
static LINT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u:(?-u:\b)(?:eslint|oxlint|lint-staged|lint)(?-u:\b))").unwrap()
});

// Source `:78`. Inline use in `isPushHookFailure`, deliberately has NO `\b`
// (L8 branch 3).
static HOOK_DECLINED_INLINE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i-u:hook declined to push)").unwrap());

// Source `:25-26`. 21 alternatives verbatim, no `\b` (pure substring), no
// blanket cleanup of the two dead alternatives (L9).
static REMOTE_PUSH_EXCLUSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // concat! (not a multi-line raw string): a raw string's `\` followed by a
    // newline is NOT a line continuation (raw strings do no escape
    // processing at all) — it would insert a literal backslash + newline
    // into the pattern. concat! joins these plain literals at compile time
    // with no such hazard.
    Regex::new(concat!(
        "(?i-u:",
        "authentication failed",
        "|repository not found",
        "|not a git repository",
        "|does not appear to be a git repository",
        "|permission denied",
        "|protected branch",
        "|pre-receive hook declined",
        "|non-fast-forward",
        "|fetch first",
        "|updates were rejected",
        "|stale info",
        "|submodule",
        "|failed to push all needed submodules",
        "|unable to push submodule",
        "|unable to access",
        "|could not resolve host",
        "|network is unreachable",
        "|connection timed out",
        "|failed to connect",
        "|rpc failed",
        "|remote end hung up",
        ")",
    ))
    .unwrap()
});

/// Take the first `n` chars of `s`, char-boundary safe (L3). Never panics,
/// including on non-ASCII input straddling the cap.
fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Source `:28-35`. 5 ordered steps (L6): char-cap, strip ANSI, normalize
/// CR/CRLF to LF, strip CONTROL, js_trim.
fn normalize_push_failure(raw: &str) -> String {
    let capped = take_chars(raw, PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS);
    let no_ansi = ANSI_PATTERN.replace_all(capped, "");
    let lf_only = CRLF_PATTERN.replace_all(&no_ansi, "\n");
    let no_control = CONTROL_PATTERN.replace_all(&lf_only, "");
    js_trim(&no_control).to_string()
}

/// Source `:52-66`. Splits on LF ONLY (L7) — never `.lines()` — then
/// js_trims each piece and drops empties.
fn get_push_failure_normalized_lines(normalized: &str) -> Vec<String> {
    normalized
        .split('\n')
        .map(js_trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Source `:37-50` (L11). The low-signal filter only runs once a signal
/// line is present; if filtering would remove every line, the unfiltered
/// lines are returned instead.
fn get_meaningful_lines(raw: &str) -> Vec<String> {
    let normalized = normalize_push_failure(raw);
    let lines = get_push_failure_normalized_lines(&normalized);

    let has_signal_line = lines.iter().any(|line| {
        PUSH_HOOK_PATTERN.is_match(line)
            || PUSH_HOOK_RUNNER_PATTERN.is_match(line)
            || LINT_PATTERN.is_match(line)
    });

    if !has_signal_line {
        return lines;
    }

    let filtered: Vec<String> = lines
        .iter()
        .filter(|line| !LOW_SIGNAL_LINE_PATTERN.is_match(line))
        .cloned()
        .collect();

    if !filtered.is_empty() {
        filtered
    } else {
        lines
    }
}

/// Source `:68-95` (L8). 7 branches, matched against the whole normalized
/// blob (not per line); the EXCLUSION branch wins unconditionally.
pub fn is_push_hook_failure(raw: &str) -> bool {
    let normalized = normalize_push_failure(raw);
    if normalized.is_empty() {
        return false;
    }

    if REMOTE_PUSH_EXCLUSION_PATTERN.is_match(&normalized) {
        return false;
    }

    if HOOK_DECLINED_INLINE_PATTERN.is_match(&normalized) {
        return true;
    }

    if PUSH_HOOK_PATTERN.is_match(&normalized) {
        return true;
    }

    if PUSH_HOOK_RUNNER_PATTERN.is_match(&normalized) && PUSH_CONTEXT_PATTERN.is_match(&normalized)
    {
        return true;
    }

    if LINT_PATTERN.is_match(&normalized) && PUSH_CONTEXT_PATTERN.is_match(&normalized) {
        return true;
    }

    false
}

/// Source `:97-99`.
pub fn sanitize_push_failure_details(raw: &str) -> String {
    normalize_push_failure(raw)
}

/// Source `:101-117` (L10). Lint beats hook; does not consult the exclusion
/// list — gating is the caller's responsibility.
pub fn summarize_push_failure(raw: &str) -> String {
    let lines = get_meaningful_lines(raw);

    if lines.is_empty() {
        return FALLBACK_PUSH_FAILURE_SUMMARY.to_string();
    }

    if lines.iter().any(|line| LINT_PATTERN.is_match(line)) {
        return LINT_PUSH_FAILURE_SUMMARY.to_string();
    }

    if lines
        .iter()
        .any(|line| PUSH_HOOK_PATTERN.is_match(line) || PUSH_HOOK_RUNNER_PATTERN.is_match(line))
    {
        return PRE_PUSH_FAILURE_SUMMARY.to_string();
    }

    lines
        .first()
        .cloned()
        .unwrap_or_else(|| FALLBACK_PUSH_FAILURE_SUMMARY.to_string())
}

/// Source `:119-135` (L3: empty check before length check).
pub fn has_expanded_push_failure_details(raw: &str, summary: &str) -> bool {
    let normalized_raw = normalize_push_failure(raw);
    let normalized_summary = normalize_push_failure(summary);

    if normalized_raw.is_empty() {
        return false;
    }

    if raw.chars().count() > PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS {
        return true;
    }

    fold_push_failure_comparison_whitespace(&normalized_raw)
        != fold_push_failure_comparison_whitespace(&normalized_summary)
}

/// Source `:137-169`. The source's hand-rolled `isPushFailureComparisonWhitespace`
/// codepoint set is identical to `suaegi_misc::is_js_whitespace` (L4) — reused
/// rather than re-implemented.
fn fold_push_failure_comparison_whitespace(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut pending_space = false;
    for ch in value.chars() {
        if is_js_whitespace(ch) {
            pending_space = !result.is_empty();
            continue;
        }
        if pending_space {
            result.push(' ');
            pending_space = false;
        }
        result.push(ch);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- T1-T7: ported oracle tests (source-control-push-failure.test.ts) ---

    /// T1: explicit pre-push hook failure -> true / "Pre-push hook failed."
    #[test]
    fn t1_detects_explicit_pre_push_hook_failures() {
        let raw =
            "error: failed to push some refs to 'origin'\nhusky - pre-push hook exited with code 1";
        assert!(is_push_hook_failure(raw));
        assert_eq!(summarize_push_failure(raw), "Pre-push hook failed.");
    }

    /// T2: lint failure during push -> true / "Lint failed during push."
    #[test]
    fn t2_detects_lint_failures_during_push() {
        let raw = [
            "git push failed: Command failed: git push origin main",
            "error: failed to push some refs to origin",
            "eslint found 3 errors",
        ]
        .join("\n");
        assert!(is_push_hook_failure(&raw));
        assert_eq!(summarize_push_failure(&raw), "Lint failed during push.");
    }

    /// T3: auth failure is not treated as a push hook failure.
    #[test]
    fn t3_does_not_treat_auth_failures_as_push_hook_failures() {
        let raw = "git push failed: Command failed: git push origin main\nremote: Repository not found.\nfatal: Authentication failed";
        assert!(!is_push_hook_failure(raw));
    }

    /// T4/T4a: protected/pre-receive/non-fast-forward/transport/submodule
    /// failures are excluded, even when a lint/context or runner/context
    /// signal is also present (exclusion wins — T4a).
    #[test]
    fn t4_exclusions_win_over_hook_or_lint_context_signals() {
        let negatives = [
            "git push failed: Command failed: git push origin main\nremote: error: GH006: Protected branch update failed for refs/heads/main.\nremote: lint status check is required",
            "git push failed: Command failed: git push origin main\nremote: pre-receive hook declined\nremote: eslint failed in hosted checks",
            "git push failed: Command failed: git push origin main\n! [rejected] main -> main (non-fast-forward)\nerror: failed to push some refs",
            "git push failed: Command failed: git push origin main\nfatal: unable to access https://example.com/repo.git: Could not resolve host",
            "git push failed: Command failed: git push --recurse-submodules\nUnable to push submodule 'vendor/lib'\nfatal: failed to push all needed submodules",
        ];
        for raw in negatives {
            assert!(!is_push_hook_failure(raw), "expected false for {raw:?}");
        }
    }

    /// T5: ANSI + BEL are stripped from both the sanitized details and the
    /// input used for summarization; lint beats hook in the summary.
    #[test]
    fn t5_strips_ansi_and_control_before_details_and_comparison() {
        let raw = "\u{1b}[31mhusky - pre-push hook failed\u{1b}[0m\u{7}\neslint failed";
        assert_eq!(
            sanitize_push_failure_details(raw),
            "husky - pre-push hook failed\neslint failed"
        );
        assert_eq!(summarize_push_failure(raw), "Lint failed during push.");
    }

    /// T6: expanded details reporting — richer raw vs a terse summary is
    /// "expanded"; empty raw is never expanded.
    #[test]
    fn t6_reports_whether_expanded_details_add_information() {
        assert!(has_expanded_push_failure_details(
            "husky - pre-push hook\neslint found 2 errors\nfull output",
            "Lint failed during push."
        ));
        assert!(!has_expanded_push_failure_details("", "Push failed."));
    }

    /// T7/T17: pathological single-line logs are bounded by the char cap;
    /// summarization never needs to call `split` on the raw untruncated
    /// input (Rust equivalent: `normalize_push_failure` only ever sees the
    /// capped prefix).
    #[test]
    fn t7_bounds_summary_analysis_for_pathological_single_line_logs() {
        let raw = "x".repeat(PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS + 10_000);
        assert_eq!(
            summarize_push_failure(&raw),
            "x".repeat(PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS)
        );
        assert!(has_expanded_push_failure_details(&raw, "Push failed."));
    }

    // --- Additional pins (oracle-silent decisions L1-L11) ---

    /// L1 crux pin: U+212A KELVIN SIGN standing in for `k` must NOT satisfy
    /// case-insensitive matching against `husky`/`pre-push` — proving the
    /// patterns fold ASCII-only, not full Unicode simple case folding (which
    /// WOULD fold U+212A to `k`).
    #[test]
    fn l1_kelvin_sign_does_not_fold_to_k() {
        // "pre-pus\u{212A}" would equal "pre-push" under Unicode simple case
        // folding (KELVIN SIGN -> K -> k), but must NOT match here.
        let raw = format!("pre-pus{}", '\u{212A}');
        assert!(!PUSH_HOOK_PATTERN.is_match(&raw));

        let husky_raw = format!("hus{}y - pre-push hook failed", '\u{212A}');
        assert!(!PUSH_HOOK_RUNNER_PATTERN.is_match(&husky_raw));
        // But the un-substituted, correctly-cased/ASCII variant still matches.
        assert!(PUSH_HOOK_RUNNER_PATTERN.is_match("HUSKY - pre-push hook failed"));
    }

    /// L2 pin: the ANSI pattern's `[0-9]` (from `\d`) must not match an
    /// Arabic-Indic digit (Rust `\d` is Unicode `Nd` and would).
    #[test]
    fn l2_ansi_digit_class_is_ascii_only() {
        // U+0661 ARABIC-INDIC DIGIT ONE in the CSI parameter position: the
        // whole sequence must fail to match (and thus survive stripping),
        // because `[0-9]` rejects it where Unicode `\d` would not.
        let raw = format!("\u{1b}[{}mtext", '\u{661}');
        assert!(!ANSI_PATTERN.is_match(&raw));
    }

    /// L2 pin: `\b` word-boundary semantics — `pre-pushed` does NOT match
    /// (boundary falls inside a word char), `lint-staged` DOES.
    #[test]
    fn l2_word_boundary_ascii_semantics() {
        assert!(!PUSH_HOOK_PATTERN.is_match("pre-pushed"));
        assert!(PUSH_HOOK_RUNNER_PATTERN.is_match("lint-staged"));
    }

    /// L2 pin: the explicit `\s` set includes U+FEFF (as in JS) and excludes
    /// U+0085 (unlike Rust's Unicode `\s`), matched against
    /// `LOW_SIGNAL_LINE_PATTERN`'s `npm\s+warn...env`.
    #[test]
    fn l2_whitespace_class_feff_yes_nel_no() {
        let with_feff = format!("npm{}warn thing env", '\u{FEFF}');
        assert!(LOW_SIGNAL_LINE_PATTERN.is_match(&with_feff));

        let with_nel = format!("npm{}warn thing env", '\u{0085}');
        assert!(!LOW_SIGNAL_LINE_PATTERN.is_match(&with_nel));
    }

    /// L2 pin: the explicit `.` set (`DOT_CLASS`) must not cross U+2028 —
    /// `npm warn <U+2028> env` must NOT match because JS `.` excludes LS.
    #[test]
    fn l2_dot_class_does_not_cross_line_separator() {
        let raw = format!("npm warn thing{}env", '\u{2028}');
        assert!(!LOW_SIGNAL_LINE_PATTERN.is_match(&raw));

        let raw_same_line = "npm warn thing and env";
        assert!(LOW_SIGNAL_LINE_PATTERN.is_match(raw_same_line));
    }

    /// L3 pin: non-ASCII input straddling the char cap must not panic, and
    /// the empty check in `has_expanded_push_failure_details` precedes the
    /// length check (an all-ANSI input beyond the cap normalizes to empty
    /// and must report `false`, not `true`, even though `raw.len() >
    /// PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS`).
    #[test]
    fn l3_non_ascii_near_cap_does_not_panic_and_empty_precedes_length() {
        // Multi-byte chars straddling the char boundary at the cap.
        let raw: String = "é".repeat(PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS + 5);
        // Must not panic:
        let _ = normalize_push_failure(&raw);
        let _ = is_push_hook_failure(&raw);
        let _ = sanitize_push_failure_details(&raw);
        let _ = summarize_push_failure(&raw);

        // All-ANSI input, well beyond the char cap in raw.len(), normalizes
        // to empty -> `has_expanded_push_failure_details` must be false,
        // proving the empty check runs before the length check.
        let ansi_only: String = "\u{1b}[0m".repeat(PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS);
        assert!(ansi_only.chars().count() > PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS);
        assert!(!has_expanded_push_failure_details(
            &ansi_only,
            "Push failed."
        ));
    }

    /// L6 pin: `\r\n` and a lone `\r` both normalize to `\n`.
    #[test]
    fn l6_crlf_and_lone_cr_normalize_to_lf() {
        assert_eq!(normalize_push_failure("a\r\nb"), "a\nb");
        assert_eq!(normalize_push_failure("a\rb"), "a\nb");
    }

    /// L7 pin: U+2028 (LINE SEPARATOR) is NOT a line splitter here — only
    /// LF is. A single logical "line" containing U+2028 stays fused, so
    /// wrapping context on both sides of it must be visible to a single
    /// blob-level match.
    #[test]
    fn l7_line_separator_is_not_a_line_splitter() {
        let raw = format!("husky{}pre-push hook failed", '\u{2028}');
        let normalized = normalize_push_failure(&raw);
        let lines = get_push_failure_normalized_lines(&normalized);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], format!("husky{}pre-push hook failed", '\u{2028}'));
    }

    /// L9 pin: previously-untested exclusion alternatives.
    #[test]
    fn l9_untested_exclusions() {
        assert!(REMOTE_PUSH_EXCLUSION_PATTERN.is_match("error: stale info"));
        assert!(REMOTE_PUSH_EXCLUSION_PATTERN.is_match("fatal: failed to connect to host"));
        assert!(REMOTE_PUSH_EXCLUSION_PATTERN.is_match("fatal: rpc failed; curl 56"));
        assert!(REMOTE_PUSH_EXCLUSION_PATTERN.is_match("remote end hung up unexpectedly"));
    }

    /// L10 pin: the `lines[0]` fallback — no lint/hook/runner signal, so the
    /// first meaningful line is returned verbatim.
    #[test]
    fn l10_falls_back_to_first_line() {
        let raw = "some unrelated push failure line\nanother detail line";
        assert_eq!(
            summarize_push_failure(raw),
            "some unrelated push failure line"
        );
    }

    /// L11 pin, path 1: a signal line is present, so the low-signal (npm
    /// noise) filter runs and removes the noise line.
    #[test]
    fn l11_filters_low_signal_lines_when_signal_present() {
        let raw = "npm warn config thing env\nhusky - pre-push hook failed";
        let lines = get_meaningful_lines(raw);
        assert_eq!(lines, vec!["husky - pre-push hook failed".to_string()]);
    }

    /// L11 pin, path 2: no signal line anywhere, so the low-signal filter is
    /// skipped entirely and npm noise lines are returned as-is.
    #[test]
    fn l11_skips_filter_when_no_signal_present() {
        let raw = "npm warn config thing env\nnpm notice some notice";
        let lines = get_meaningful_lines(raw);
        assert_eq!(
            lines,
            vec![
                "npm warn config thing env".to_string(),
                "npm notice some notice".to_string(),
            ]
        );
    }

    /// L11 pin, path 3: a signal line is present AND it is also the only
    /// line, and it happens to match the low-signal (npm/husky-deprecated
    /// noise) pattern itself — filtering removes the only line, so the
    /// result falls back to the unfiltered lines instead of an empty vec.
    #[test]
    fn l11_falls_back_to_unfiltered_when_filter_removes_everything() {
        // Contains `husky` (PUSH_HOOK_RUNNER_PATTERN -> has_signal_line
        // true) AND matches `husky\s+-\s+deprecated\b`
        // (LOW_SIGNAL_LINE_PATTERN) -> filtering would remove the only line
        // -> falls back to the unfiltered lines.
        let raw = "husky - deprecated: run the following";
        let lines = get_meaningful_lines(raw);
        assert_eq!(lines, vec![raw.to_string()]);
    }

    /// Whitespace-fold pin: identical content differing only in whitespace
    /// (including a fold-relevant codepoint) reports no expanded details.
    #[test]
    fn whitespace_fold_identical_content_differing_only_in_whitespace() {
        let raw = "husky   -   pre-push\u{00A0}hook   failed";
        let summary = "husky - pre-push hook failed";
        assert!(!has_expanded_push_failure_details(raw, summary));
    }

    /// L5 pin: the CSI final-byte class is the deliberately NARROW
    /// `[0-9A-PR-TZcf-nq-uy=><~]`, not the general `[@-~]`. `Q` is excluded
    /// from the narrow class and has no digit run to fall back on, so
    /// `ESC[Q` fails to match `ANSI_PATTERN` at all (the digit-run before
    /// the final byte is optional, so a bare digit like `0` would itself
    /// satisfy the final-byte class -- `Q` cannot) -- only the lone ESC
    /// byte is later removed by `CONTROL_PATTERN`, leaving `[Qkeep` behind.
    /// Contrast with `m`, which IS in the narrow class, so `ESC[0m` is
    /// fully stripped by `ANSI_PATTERN` itself.
    // Why: guards against widening the CSI final-byte class to `[@-~]`,
    // which would additionally swallow the `Q`-terminated sequence.
    #[test]
    fn l5_ansi_final_byte_class_is_narrow_not_general() {
        assert_eq!(sanitize_push_failure_details("\u{1b}[Qkeep"), "[Qkeep");
        assert_eq!(sanitize_push_failure_details("\u{1b}[0mkeep"), "keep");
    }

    /// L5 pin: the C1 CSI introducer `\x9b` (alongside ESC) must be handled
    /// by `ANSI_PATTERN` itself. If the introducer class dropped `\x9b`,
    /// `CONTROL_PATTERN` would still remove the lone `\x9b` byte (it falls
    /// in `\x7f-\x9f`) but would leave the trailing `0m` behind -- the two
    /// outcomes are distinguishable.
    // Why: guards against narrowing the introducer class `[\x1b\x9b]` to
    // `[\x1b]`, which would leave `0m` as leftover text instead of stripping
    // the whole C1 CSI sequence.
    #[test]
    fn l5_c1_csi_introducer_u009b_is_stripped_as_ansi() {
        assert_eq!(sanitize_push_failure_details("\u{9b}0mtext"), "text");
    }

    /// L11 pin: isolates the low-signal gate from the wipe-out fallback.
    /// No signal line is present anywhere (gate should apply and skip
    /// filtering entirely), but the input also contains an ordinary
    /// (non-low-signal) line, so a deleted gate would filter the npm line
    /// out and leave a non-empty single-line result -- the wipe-out
    /// fallback (protection 2) would NOT kick in to mask the deletion,
    /// unlike the existing all-low-signal test.
    // Why: guards against deleting the `if !has_signal_line` gate, which
    // the existing all-low-signal test cannot detect because its wiped-out
    // result is rescued back to the same unfiltered lines either way.
    #[test]
    fn l11_gate_is_not_masked_by_wipeout_fallback() {
        let raw = "npm warn config thing env\nsomething ordinary happened";
        let lines = get_meaningful_lines(raw);
        assert_eq!(
            lines,
            vec![
                "npm warn config thing env".to_string(),
                "something ordinary happened".to_string(),
            ]
        );
    }

    /// Defense-in-depth pin: JS `\b` is an ASCII word boundary; Rust's
    /// default Unicode `\b` treats accented letters as word chars too. A
    /// non-ASCII character directly adjacent to the `pre-push` keyword
    /// discriminates the two: under ASCII `\b` there IS a boundary between
    /// `h` and `é` (so the pattern still matches); under a Unicode `\b`
    /// there would NOT be one (both are "word" chars), so it would fail to
    /// match.
    // Why: guards against the `(?-u:\b)` ASCII word-boundary scoping being
    // widened to Unicode, which would stop matching keywords immediately
    // followed by accented/non-ASCII letters.
    #[test]
    fn l2_word_boundary_ascii_with_non_ascii_neighbour() {
        assert!(PUSH_HOOK_PATTERN.is_match("pre-pushé"));
    }
}
