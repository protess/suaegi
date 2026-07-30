//! Leading agent-status decoration stripping — verbatim port of Orca's
//! `src/shared/agent-title-decoration.ts` (@ v1.4.146-rc.0).
//!
//! Source pattern: `/^(?:[✳✦⏲◇✋⠀-⣿]+|[.*]\s)\s*/`, applied via
//! `title.replace(RE, '').trimStart()`.
//!
//! ⚠ The glyph set (✳ U+2733, ✦ U+2726, ⏲ U+23F2, ◇ U+25C7, ✋ U+270B, plus the
//! braille range U+2800..=U+28FF for spinner frames) is duplicated in
//! `suaegi-app::agent_status::title` — that crate depends on `suaegi_term`
//! and does not depend on `suaegi-misc`, so sharing this constant across
//! crates would invert the dependency direction. Per-module duplication is
//! this repo's norm; do not "deduplicate" this into a cross-crate edge.
//!
//! There is no pre-trim: `^` is anchored to byte 0 of the raw input (the
//! `.trim()`-like cleanup — `.trimStart()` — happens only *after* the
//! replace). So `" ✳ Pi"` (leading space before the glyph) fails to match at
//! all — the glyph survives, and only the leading space is removed by the
//! trailing `.trimStart()`, giving `"✳ Pi"`. A port that trims before
//! matching "fixes" a behavior Orca does not have; there is zero coverage in
//! Orca for that shape, so this must be reasoned from the source, not copied
//! from a test.
//!
//! The regex's trailing `\s*` and the `.trimStart()` call both strip the same
//! ECMAScript whitespace set from the same position (whatever the alternation
//! consumed), so they are fused here into one [`js_trim_start`] call rather
//! than modeled as two separate steps. This means `.trimStart()` cannot be
//! dropped without changing behavior: when *neither* alternative matches (so
//! the whole regex fails and `.replace()` is a no-op), `.trimStart()` is the
//! *only* thing that still runs, e.g. `"  npm run dev"` → `"npm run dev"`.
//! `js_trim_start` is not promoted to `js_ws` (that would force refactoring
//! `suaegi-quickcmd`'s local `js_trim_end` too) — it is a private two-liner
//! here, matching that sibling.
//!
//! The replace has no `/g` and the pattern is only ever tried once at
//! position 0 (never re-applied to the result), so at most one leading
//! decoration is stripped: `"✳. ✦"` → `". ✦"` (the second decoration
//! survives; the `\s*`/`trimStart` after the first glyph run cannot consume
//! `"."`, so the match stops there).

use crate::js_ws::is_js_whitespace;

/// `stripLeadingAgentTitleDecorationOrEmpty`: strip a single leading
/// decoration run (one or more status glyphs, OR exactly one `.`/`*`
/// followed by exactly one ECMAScript-whitespace character) plus any
/// ECMAScript whitespace immediately after it, then apply ECMAScript
/// `trimStart` once more (a no-op when a decoration was found; the load-
/// bearing case is when none was, see module docs). May return `""`.
pub fn strip_leading_agent_title_decoration_or_empty(title: &str) -> String {
    let branch_len = match_leading_decoration_branch(title).unwrap_or(0);
    js_trim_start(&title[branch_len..]).to_string()
}

/// `stripLeadingAgentTitleDecoration`: like
/// [`strip_leading_agent_title_decoration_or_empty`], but never returns
/// empty — a title that is *only* a status glyph keeps its original
/// (untrimmed, undecoration-stripped) text instead of collapsing to blank.
pub fn strip_leading_agent_title_decoration(title: &str) -> String {
    let stripped = strip_leading_agent_title_decoration_or_empty(title);
    if stripped.is_empty() {
        title.to_string()
    } else {
        stripped
    }
}

/// Local ECMAScript trim-start, built on [`is_js_whitespace`] — NOT
/// `str::trim_start` (see `crate::js_ws` module docs for the U+FEFF/U+0085
/// divergence). Mirrors `suaegi-quickcmd`'s local `js_trim_end`.
fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(|ch: char| is_js_whitespace(ch))
}

/// True for a status glyph: ✳, ✦, ⏲, ◇, ✋, or a braille spinner frame in
/// U+2800..=U+28FF.
fn is_agent_title_decoration_glyph(c: char) -> bool {
    matches!(
        c,
        '\u{2733}' | '\u{2726}' | '\u{23F2}' | '\u{25C7}' | '\u{270B}'
    ) || ('\u{2800}'..='\u{28FF}').contains(&c)
}

/// Attempts `(?:[✳✦⏲◇✋⠀-⣿]+|[.*]\s)` at the start of `title`: either a
/// greedy run of one or more status glyphs, or exactly one `.`/`*` followed
/// by exactly one required ECMAScript-whitespace character. Returns the byte
/// length consumed on success.
fn match_leading_decoration_branch(title: &str) -> Option<usize> {
    let mut chars = title.chars();
    let first = chars.next()?;
    if is_agent_title_decoration_glyph(first) {
        let mut end = first.len_utf8();
        for c in title[end..].chars() {
            if is_agent_title_decoration_glyph(c) {
                end += c.len_utf8();
            } else {
                break;
            }
        }
        Some(end)
    } else if first == '.' || first == '*' {
        let second = chars.next()?;
        if is_js_whitespace(second) {
            Some(first.len_utf8() + second.len_utf8())
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        strip_leading_agent_title_decoration, strip_leading_agent_title_decoration_or_empty,
    };

    // Oracle: agent-title-decoration.test.ts

    #[test]
    fn strips_claudes_idle_glyph() {
        assert_eq!(
            strip_leading_agent_title_decoration("✳ Claude Code"),
            "Claude Code"
        );
    }

    #[test]
    fn strips_claudes_working_idle_text_prefixes() {
        assert_eq!(
            strip_leading_agent_title_decoration(". working on the fix"),
            "working on the fix"
        );
        assert_eq!(
            strip_leading_agent_title_decoration("* Claude Code"),
            "Claude Code"
        );
    }

    #[test]
    fn strips_a_leading_braille_spinner() {
        assert_eq!(strip_leading_agent_title_decoration("⠋ Pi"), "Pi");
    }

    #[test]
    fn leaves_an_undecorated_title_untouched() {
        assert_eq!(
            strip_leading_agent_title_decoration("Dolphin-2"),
            "Dolphin-2"
        );
        assert_eq!(
            strip_leading_agent_title_decoration("npm run dev"),
            "npm run dev"
        );
    }

    #[test]
    fn keeps_the_original_when_the_title_is_only_a_status_glyph() {
        assert_eq!(strip_leading_agent_title_decoration("✳"), "✳");
        assert_eq!(strip_leading_agent_title_decoration("✳ "), "✳ ");
    }

    #[test]
    fn can_strip_to_empty_when_the_caller_supplies_its_own_fallback_label() {
        assert_eq!(strip_leading_agent_title_decoration_or_empty("✳"), "");
        assert_eq!(strip_leading_agent_title_decoration_or_empty("✳ "), "");
    }

    // Indirect oracle: mobile-terminal-tab-agent.test.ts:80,88 — the repo's
    // only ✦ coverage, and the only coverage of the launch-owned-tab path.

    #[test]
    fn indirect_oracle_mobile_terminal_tab_agent() {
        // :80 — strips leading agent decorations when an icon is shown.
        assert_eq!(
            strip_leading_agent_title_decoration("✦ Gemini CLI"),
            "Gemini CLI"
        );
        // :88 — strips decorations for launch-owned terminal tabs before hooks arrive.
        assert_eq!(strip_leading_agent_title_decoration("✳ working"), "working");
    }

    // Mandatory extra pins (oracle-silent):

    /// W2: `.trimStart()` is load-bearing precisely when the alternation does
    /// NOT match at all — this is the only witness in the repo that can kill
    /// dropping it (the pattern's own trailing `\s*` is redundant with it and
    /// is NOT mutation-tested; see module docs and plan §2).
    #[test]
    fn pin_trim_start_is_load_bearing_when_nothing_matches() {
        assert_eq!(
            strip_leading_agent_title_decoration("  npm run dev"),
            "npm run dev"
        );
    }

    /// W10: there is no pre-trim — a leading space before the glyph blocks
    /// the match entirely, so the glyph survives; only the space (via the
    /// trailing `trimStart`) is removed.
    #[test]
    fn pin_no_pre_trim_leading_space_blocks_glyph_match() {
        assert_eq!(strip_leading_agent_title_decoration(" ✳ Pi"), "✳ Pi");
    }

    /// W13: `[.*]` is two literal characters (not a wildcard/quantifier), and
    /// the required whitespace is exactly one character — not `\s*`. A
    /// non-whitespace follower means no match at all (untouched, not even
    /// trimmed, since there's nothing to trim).
    #[test]
    fn pin_dot_star_are_literal_and_need_exactly_one_following_whitespace() {
        assert_eq!(strip_leading_agent_title_decoration(".x"), ".x");
        assert_eq!(strip_leading_agent_title_decoration("*y"), "*y");
        assert_eq!(strip_leading_agent_title_decoration("."), ".");
        assert_eq!(strip_leading_agent_title_decoration("*"), "*");
        // Longer follow-on text so a "required whitespace is optional" mutant
        // is not accidentally masked by the never-empty fallback (".x"/"*y"
        // alone happen to collapse to empty either way).
        assert_eq!(strip_leading_agent_title_decoration(".xy"), ".xy");
        assert_eq!(strip_leading_agent_title_decoration("*yz"), "*yz");
    }

    /// `.`/`*` are literal characters, not wildcards — an ordinary letter
    /// followed by a space must not enter this branch at all.
    #[test]
    fn pin_dot_star_branch_does_not_accept_arbitrary_leading_characters() {
        assert_eq!(strip_leading_agent_title_decoration("a b"), "a b");
    }

    /// W13: once the single required whitespace character is consumed by the
    /// branch, any further whitespace is swept by the trailing trim — tab and
    /// NBSP both count as ECMAScript whitespace for the required character.
    #[test]
    fn pin_extra_whitespace_after_the_required_one_is_swept_by_trim() {
        assert_eq!(strip_leading_agent_title_decoration("*  y"), "y");
        assert_eq!(strip_leading_agent_title_decoration("*\tx"), "x");
        assert_eq!(strip_leading_agent_title_decoration("*\u{00A0}x"), "x");
    }

    /// W12: every glyph in the class matches individually — ⏲, ◇, ✋ have
    /// zero coverage anywhere else in Orca.
    #[test]
    fn pin_every_gemini_glyph_matches() {
        assert_eq!(strip_leading_agent_title_decoration("✦ Task"), "Task");
        assert_eq!(strip_leading_agent_title_decoration("⏲ Task"), "Task");
        assert_eq!(strip_leading_agent_title_decoration("◇ Task"), "Task");
        assert_eq!(strip_leading_agent_title_decoration("✋ Task"), "Task");
    }

    /// W12: the braille range's exact endpoints match...
    #[test]
    fn pin_braille_range_endpoints_match() {
        assert_eq!(
            strip_leading_agent_title_decoration("\u{2800} Task"),
            "Task"
        );
        assert_eq!(
            strip_leading_agent_title_decoration("\u{28FF} Task"),
            "Task"
        );
    }

    /// ...and the characters immediately outside the range do not (an
    /// off-by-one range port would wrongly accept these).
    #[test]
    fn pin_just_outside_braille_range_does_not_match() {
        assert_eq!(
            strip_leading_agent_title_decoration("\u{27FF} Task"),
            "\u{27FF} Task"
        );
        assert_eq!(
            strip_leading_agent_title_decoration("\u{2900} Task"),
            "\u{2900} Task"
        );
    }

    /// W11: never-empty returns the raw original, not a trimmed one — a
    /// braille-only "blank" glyph (U+2800, invisible) still triggers the
    /// never-empty fallback rather than collapsing.
    #[test]
    fn pin_never_empty_returns_raw_untrimmed_original() {
        assert_eq!(strip_leading_agent_title_decoration("  "), "  ");
        assert_eq!(strip_leading_agent_title_decoration("\u{2800}"), "\u{2800}");
        assert_eq!(strip_leading_agent_title_decoration(". "), ". ");
        assert_eq!(strip_leading_agent_title_decoration("✳✦✳ "), "✳✦✳ ");
    }

    /// W14: the alternation's glyph branch is greedy across mixed glyphs
    /// (single match consumes all three), but the replace happens only once.
    #[test]
    fn pin_replace_happens_exactly_once() {
        assert_eq!(strip_leading_agent_title_decoration("✳✦✳ x"), "x");
        // The trailing `\s*` cannot cross the literal `.`, so the second
        // decoration is never reached by a second pass — there isn't one.
        assert_eq!(strip_leading_agent_title_decoration("✳. ✦"), ". ✦");
    }

    // W1 — ECMAScript whitespace at both remaining sites (the branch's
    // single required `\s`, and the fused trailing `\s*`/`trimStart`), each
    // with a U+FEFF and a U+0085 witness.

    /// Site: the branch's single required `\s` after `.`/`*`. FEFF qualifies
    /// (JS whitespace) so the branch matches; NEL does not, so neither
    /// alternative matches and the string is returned untouched.
    #[test]
    fn pin_w1_dot_star_required_whitespace() {
        assert_eq!(strip_leading_agent_title_decoration(".\u{FEFF}x"), "x");
        assert_eq!(
            strip_leading_agent_title_decoration(".\u{0085}x"),
            ".\u{0085}x"
        );
    }

    /// Site: the fused trailing `\s*`/`trimStart` after a matched glyph run.
    /// FEFF is swept away; NEL is ECMAScript-preserved (even though Rust's
    /// `char::is_whitespace` would wrongly strip it).
    #[test]
    fn pin_w1_trailing_trim_after_glyph() {
        assert_eq!(strip_leading_agent_title_decoration("✳\u{FEFF}x"), "x");
        assert_eq!(
            strip_leading_agent_title_decoration("✳\u{0085}Pi"),
            "\u{0085}Pi"
        );
    }
}
