//! `display-name-from-work.ts` — compose the sidebar display name for a freshly
//! auto-renamed workspace. `<identifier> - <action>` when the prompt names a
//! review target, else the humanized branch slug.
//!
//! Consumes `suaegi_workref::{extract_work_identifier, format_identifier_first}`
//! and [`crate::branch_name::humanize_branch_slug`].
//!
//! # Documented divergences (plan Codex decisions — NOT bugs)
//! - **C4 ASCII digit lock.** `leadingActionWord`'s `/^\d+$/` and
//!   `collisionSuffixFromLeaf`'s `/^\d+$/` are ASCII-digit checks (`is_ascii_digit`),
//!   so a non-ASCII-numeral slug word is never treated as a skippable digit.
//! - **`Number(rest)` leading-zero strip.** `resolvedLeaf = "fix-007"` yields
//!   suffix `7`, not `007` — the suffix is parsed then re-formatted. Absurdly long
//!   digit runs that overflow `u128` fall back to no-suffix (documented; JS uses
//!   f64 and would keep an imprecise float).

use std::collections::HashSet;

use suaegi_workref::{extract_work_identifier, format_identifier_first};

use crate::branch_name::{humanize_branch_slug, upper_first};

/// `ACTION_STOPWORDS` (`:13-28`), 15 entries — type/function words that are never
/// a useful action verb.
const ACTION_STOPWORDS: &[&str] = &[
    "pr", "mr", "pull", "merge", "request", "requests", "issue", "issues", "the", "a", "an", "to",
    "for", "of",
];

/// `leadingActionWord` (`:32-41`). First slug word that is neither an all-digit
/// token nor an identifier/type token, capitalized. `''` when none remains.
fn leading_action_word(slug: &str, identifier_tokens: &[String]) -> String {
    let mut skip: HashSet<&str> = ACTION_STOPWORDS.iter().copied().collect();
    for t in identifier_tokens {
        skip.insert(t.as_str());
    }
    for word in slug.split('-').filter(|w| !w.is_empty()) {
        // `/^\d+$/` → all-ASCII-digit (C4). Non-ASCII numerals are not digits.
        let all_digits = word.chars().all(|c| c.is_ascii_digit());
        if all_digits || skip.contains(word) {
            continue;
        }
        return upper_first(word);
    }
    String::new()
}

/// `collisionSuffixFromLeaf` (`:45-54`). The `-N` collision suffix appended to
/// `base_slug` in `resolved_leaf`, parsed to a number (leading zeros stripped).
/// `None` when there is no numeric suffix.
fn collision_suffix_from_leaf(base_slug: &str, resolved_leaf: Option<&str>) -> Option<u128> {
    let resolved_leaf = resolved_leaf?;
    if resolved_leaf == base_slug || !resolved_leaf.starts_with(&format!("{base_slug}-")) {
        return None;
    }
    // base_slug is a slug (ASCII) → byte offset is char-boundary-safe.
    let rest = &resolved_leaf[base_slug.len() + 1..];
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
        rest.parse::<u128>().ok() // `Number(rest)`: strips leading zeros
    } else {
        None
    }
}

/// `deriveWorkspaceDisplayName` (`:63-76`).
pub fn derive_workspace_display_name(
    prompt: &str,
    slug: &str,
    resolved_leaf: Option<&str>,
) -> String {
    let identifier = match extract_work_identifier(prompt) {
        None => return humanize_branch_slug(resolved_leaf.unwrap_or(slug)),
        Some(id) => id,
    };
    let action = leading_action_word(slug, &identifier.tokens);
    let base = format_identifier_first(&identifier.label, &action);
    // JS `suffix ? … : base`: `0` is falsy, so a `-0` suffix shows no `(0)`.
    match collision_suffix_from_leaf(slug, resolved_leaf) {
        Some(n) if n != 0 => format!("{base} ({n})"),
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Oracle: deriveWorkspaceDisplayName (display-name-from-work.test.ts) ----

    #[test]
    fn leads_with_identifier_and_single_action_verb() {
        assert_eq!(
            derive_workspace_display_name(
                "Carefully evaluate https://github.com/o/r/pull/1033. Fix the merge conflict.",
                "review-community-pr-conflict",
                None
            ),
            "PR 1033 - Review"
        );
    }

    #[test]
    fn drops_identifier_tokens_the_slug_carried() {
        assert_eq!(
            derive_workspace_display_name(
                "look at this community PR https://github.com/o/r/pull/1094",
                "review-community-pr-1094",
                None
            ),
            "PR 1094 - Review"
        );
    }

    #[test]
    fn uses_namespaced_ticket_bare() {
        assert_eq!(
            derive_workspace_display_name("fix ENG-456 crash", "fix-eng-456-crash", None),
            "ENG-456 - Fix"
        );
    }

    #[test]
    fn identifier_alone_when_no_action_survives() {
        assert_eq!(
            derive_workspace_display_name("PR 12", "pr-12", None),
            "PR 12"
        );
    }

    #[test]
    fn carries_a_collision_suffix() {
        assert_eq!(
            derive_workspace_display_name(
                "review https://github.com/o/r/pull/1033",
                "review-conflict",
                Some("review-conflict-2")
            ),
            "PR 1033 - Review (2)"
        );
    }

    #[test]
    fn falls_back_to_humanized_leaf_without_identifier() {
        assert_eq!(
            derive_workspace_display_name("add a dark mode toggle", "add-dark-mode-toggle", None),
            "Add dark mode toggle"
        );
    }

    #[test]
    fn humanizes_resolved_leaf_with_suffix_on_fallback() {
        assert_eq!(
            derive_workspace_display_name(
                "add a logout button",
                "add-logout-button",
                Some("add-logout-button-2")
            ),
            "Add logout button 2"
        );
    }

    // ---- Codex pins ----

    /// collisionSuffix: leading zeros stripped via `Number(rest)` (`fix-007` → 7).
    #[test]
    fn collision_suffix_strips_leading_zeros() {
        assert_eq!(collision_suffix_from_leaf("fix", Some("fix-007")), Some(7));
        assert_eq!(collision_suffix_from_leaf("fix", Some("fix-2")), Some(2));
        assert_eq!(collision_suffix_from_leaf("fix", Some("fix")), None);
        assert_eq!(collision_suffix_from_leaf("fix", None), None);
        assert_eq!(collision_suffix_from_leaf("fix", Some("fix-abc")), None);
    }

    /// C4 [0-9] lock: a non-ASCII-numeral slug word is NOT treated as a digit, so
    /// it survives as the action word instead of being skipped.
    #[test]
    fn c4_leading_action_word_non_ascii_digit_is_not_a_digit() {
        // `٢` (Arabic-Indic 2) is not `[0-9]` → returned, not skipped.
        assert_eq!(leading_action_word("\u{0662}-fix", &[]), "\u{0662}");
        // ASCII digit IS skipped.
        assert_eq!(leading_action_word("2-fix", &[]), "Fix");
    }
}
