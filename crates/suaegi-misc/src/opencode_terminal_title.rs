//! OpenCode native-title detection — verbatim port of Orca's
//! `src/shared/opencode-terminal-title.ts` (@ v1.4.146-rc.0).
//!
//! Source pattern: `/^(?:[^|\s]+ \| )?OC\s*\|\s*\S/u` tested against
//! `title?.trim() ?? ''`.
//!
//! Every `\s`/`\S`/`.trim()` here is **ECMAScript** whitespace
//! ([`crate::js_ws::is_js_whitespace`]), not Rust's `char::is_whitespace` /
//! `str::trim` — they diverge at U+FEFF (JS ws, not Unicode `White_Space`) and
//! U+0085 (Unicode `White_Space`, not JS ws). There are 8 distinct `\s`/`\S`/
//! `.trim()` occurrences in the source (4 in the pattern + the outer
//! `.trim()`, times the two files) that must each use the ECMAScript set; the
//! oracle-silent pins below hit each one individually with both codepoints.
//!
//! The optional multiplexer-prefix group `(?:[^|\s]+ \| )?` uses a **literal**
//! `" | "` (exactly one ASCII space each side of the pipe) — NOT `\s*` like
//! the marker pipe that follows `OC`. That asymmetry is real (verified by
//! running the regex in Node): unifying the two pipes to the same class is
//! the most tempting "consistency" fix and is exactly wrong. Because the
//! greedy prefix group can consume `OC` itself as the multiplexer token
//! (e.g. `"OC | x"` tries `token="OC"` first, leaving just `"x"` for the
//! required `OC\s*\|\s*\S` core, which then fails), a correct port must
//! **retry without the prefix** when the prefix-consuming path fails — a
//! single pass is not enough.
//!
//! The `[^|\s]+` vs `[^|\s]*` distinction (`+` vs `*`) is unobservable: an
//! empty token would require the trimmed string to start with `" | "`, which
//! `.trim()` makes impossible (a leading space cannot survive trim). Not
//! mutation-tested — see `suaegi-misc-placement-rule`/plan §2 for why hunting
//! this "SURVIVED" would be chasing an equivalent mutant.

use crate::js_ws::{is_js_whitespace, js_trim};

/// `isOpenCodeNativeTitle`: true iff `title`, after ECMAScript-`trim()` (or
/// `""` for `None`), matches `^(?:[^|\s]+ \| )?OC\s*\|\s*\S` — an optional
/// single-token multiplexer prefix (SSH/tmux framing) followed by the
/// case-sensitive `OC` marker, `\s*`-separated pipe, and at least one
/// non-whitespace character (any character, not just alphanumeric — `\S` is
/// unrestricted).
pub fn is_opencode_native_title(title: Option<&str>) -> bool {
    let base = title.map(js_trim).unwrap_or("");
    if let Some(after_prefix) = strip_multiplexer_prefix(base) {
        if matches_opencode_core(after_prefix) {
            return true;
        }
    }
    matches_opencode_core(base)
}

/// `isMeaningfulOpenCodeTerminalTitle`: a pure alias (`:12-14` adds zero
/// logic). Both names stay live — call sites use one for tab-agent identity
/// and the other for display-title preservation — but neither is ever passed
/// as a function reference, so a thin `#[inline]` delegator is sufficient.
#[inline]
pub fn is_meaningful_opencode_terminal_title(title: Option<&str>) -> bool {
    is_opencode_native_title(title)
}

/// Attempts the optional `(?:[^|\s]+ \| )?` group at the start of `s`: a
/// non-empty run of characters that are neither `|` nor ECMAScript
/// whitespace, followed by the literal 3-character sequence `" | "` (ASCII
/// space, pipe, ASCII space — NOT `\s`). Returns the remainder after the
/// group on success.
fn strip_multiplexer_prefix(s: &str) -> Option<&str> {
    let mut token_end = 0;
    for (i, c) in s.char_indices() {
        if c == '|' || is_js_whitespace(c) {
            break;
        }
        token_end = i + c.len_utf8();
    }
    if token_end == 0 {
        return None; // `+` requires at least one token character
    }
    let rest = &s[token_end..];
    let mut chars = rest.chars();
    if chars.next() == Some(' ') && chars.next() == Some('|') && chars.next() == Some(' ') {
        Some(&rest[3..]) // " | " is 3 ASCII bytes
    } else {
        None
    }
}

/// The mandatory `OC\s*\|\s*\S` core: case-sensitive literal `OC`, ECMAScript
/// `\s*`, literal `|`, ECMAScript `\s*`, then one arbitrary non-whitespace
/// character.
fn matches_opencode_core(s: &str) -> bool {
    let Some(after_oc) = s.strip_prefix("OC") else {
        return false;
    };
    let after_ws1 = after_oc.trim_start_matches(is_js_whitespace);
    let Some(after_pipe) = after_ws1.strip_prefix('|') else {
        return false;
    };
    let after_ws2 = after_pipe.trim_start_matches(is_js_whitespace);
    after_ws2.chars().next().is_some_and(|c| !is_js_whitespace(c))
}

#[cfg(test)]
mod tests {
    use super::{is_meaningful_opencode_terminal_title, is_opencode_native_title};

    // Oracle: opencode-terminal-title.test.ts

    #[test]
    fn recognizes_native_session_titles() {
        assert!(is_meaningful_opencode_terminal_title(Some(
            "OC | Native Stable Session"
        )));
        assert!(is_meaningful_opencode_terminal_title(Some("  OC|Session  ")));
        assert!(is_opencode_native_title(Some(
            "OC | Understand about the plugin"
        )));
        assert!(is_opencode_native_title(Some("tmux | OC | ses_123")));
    }

    #[test]
    fn rejects_generic_incomplete_embedded_and_lookalike_titles() {
        assert!(!is_meaningful_opencode_terminal_title(Some("OpenCode")));
        assert!(!is_meaningful_opencode_terminal_title(Some("OpenCode ready")));
        assert!(!is_meaningful_opencode_terminal_title(Some("OC |")));
        assert!(!is_meaningful_opencode_terminal_title(None));
        // Why: lowercase is not OpenCode's native marker; avoid "oc |" cwd/task noise.
        assert!(!is_opencode_native_title(Some(
            "oc | Understand about the plugin"
        )));
        // Why: mid-title OC must not steal another agent's braille/task frame.
        assert!(!is_opencode_native_title(Some("⠋ Fix foo | OC | bar")));
        assert!(!is_opencode_native_title(Some("my session | OC | task")));
    }

    // Indirect oracle: terminal-title-agent-type.test.ts:57-67 (resolves through
    // isOpenCodeNativeTitle before task-text identities).

    #[test]
    fn indirect_oracle_terminal_title_agent_type() {
        // :62 — a Gemini glyph inside OpenCode session text must not defeat the marker.
        assert!(is_opencode_native_title(Some("OC | ✦ Gemini CLI")));
        // :66 — no spaces at all around the marker pipe is still `\s*` (zero repeats).
        assert!(is_opencode_native_title(Some("OC|compact-session")));
    }

    // Mandatory extra pins (oracle-silent):

    /// W3: the multiplexer-prefix pipe is a literal `" | "` (exactly one ASCII
    /// space each side), while the marker pipe after `OC` is `\s*`. These are
    /// NOT the same rule — false witnesses first.
    #[test]
    fn pin_multiplexer_prefix_pipe_is_literal_not_s_star() {
        assert!(!is_opencode_native_title(Some("tmux  |  OC | x")));
        assert!(!is_opencode_native_title(Some("tmux\t| OC | x")));
        assert!(!is_opencode_native_title(Some("tmux |  OC | x")));
    }

    /// W3 true controls: the marker pipe genuinely is `\s*` (any run of
    /// ECMAScript whitespace, including tabs, on either side).
    #[test]
    fn pin_marker_pipe_is_s_star() {
        assert!(is_opencode_native_title(Some("OC  |  x")));
        assert!(is_opencode_native_title(Some("OC\t|\tx")));
    }

    /// W4: `"OC | x"` only matches after abandoning a first attempt that
    /// (wrongly) consumed `OC` itself as the multiplexer token — a
    /// single-pass port without the retry rejects this.
    #[test]
    fn pin_prefix_group_requires_retry() {
        assert!(is_opencode_native_title(Some("OC | x")));
    }

    /// W5: `\S` is any non-whitespace character, not just alphanumeric.
    #[test]
    fn pin_s_capital_is_unrestricted() {
        assert!(is_opencode_native_title(Some("OC||x")));
        assert!(is_opencode_native_title(Some("OC | |")));
        assert!(is_opencode_native_title(Some("OC | ✳")));
    }

    /// W6: `OC` is case-sensitive and the boundary is exact — `OCX` is not `OC`,
    /// and a second chained multiplexer-style prefix does not create a second
    /// retry attempt (only one optional group exists).
    #[test]
    fn pin_oc_boundary_is_exact() {
        assert!(!is_opencode_native_title(Some("OCX | x")));
        assert!(!is_opencode_native_title(Some("a | b | OC | x")));
    }

    /// W9: `undefined`/`null`/`''`/`'   '` all collapse to `''` via
    /// `title?.trim() ?? ''`, and none of them match.
    #[test]
    fn pin_nullish_and_blank_all_reject() {
        assert!(!is_meaningful_opencode_terminal_title(None));
        assert!(!is_meaningful_opencode_terminal_title(Some("")));
        assert!(!is_meaningful_opencode_terminal_title(Some("   ")));
    }

    // W1 — ECMAScript whitespace at all 8 sites (the outer `.trim()`, the
    // prefix token's `[^|\s]+` exclusion, and the pattern's two `\s*` runs +
    // final `\S`), each with both a U+FEFF (JS ws, not Unicode `White_Space`)
    // and a U+0085 (Unicode `White_Space`, not JS ws) witness.

    /// Site: outer `.trim()`. FEFF is JS whitespace, so a leading FEFF is
    /// trimmed away before matching; NEL is not, so it survives and blocks
    /// the match.
    #[test]
    fn pin_w1_outer_trim() {
        assert!(is_opencode_native_title(Some("\u{FEFF}OC | x")));
        assert!(!is_opencode_native_title(Some("\u{0085}OC | x")));
    }

    /// Site: prefix token's `[^|\s]+` exclusion. FEFF must stop the token
    /// (like a space would), breaking the literal `" | "` that must follow
    /// immediately; NEL is an ordinary token character and does not.
    #[test]
    fn pin_w1_prefix_token_exclusion() {
        assert!(!is_opencode_native_title(Some("a\u{FEFF}b | OC | x")));
        assert!(is_opencode_native_title(Some("a\u{0085}b | OC | x")));
    }

    /// Site: first `\s*` (between `OC` and the marker pipe). FEFF is skipped
    /// so the pipe is still found; NEL is not skipped, so the pipe search
    /// fails immediately.
    #[test]
    fn pin_w1_first_s_star() {
        assert!(is_opencode_native_title(Some("OC\u{FEFF}| x")));
        assert!(!is_opencode_native_title(Some("OC\u{0085}| x")));
    }

    /// Site: second `\s*` (between the marker pipe and `\S`) and the `\S`
    /// check itself. FEFF is skipped, reaching `x` for `\S`; NEL is not
    /// skipped but still satisfies `\S` on its own (it is a non-whitespace
    /// character), including at end of string.
    #[test]
    fn pin_w1_second_s_star_and_s_capital() {
        assert!(is_opencode_native_title(Some("OC|\u{FEFF}x")));
        assert!(is_opencode_native_title(Some("OC|\u{0085}")));
        assert!(is_opencode_native_title(Some("OC|\u{0085}x")));
    }
}
