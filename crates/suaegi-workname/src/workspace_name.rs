//! `workspace-name.ts` — the slug / title-case / intent pipeline that turns a
//! prompt, linked work item, or Linear issue into a `{ display_name, seed_name }`
//! pair (or a git-safe seed slug).
//!
//! # Documented divergences (plan Codex decisions — NOT bugs)
//! - **C1 full lowercase.** [`slugify_for_workspace_name`] lowercases with
//!   `char::to_lowercase()` before the ASCII whitelist, so `İ` → `i`, `İ K` →
//!   `i-k` (pinned). A `to_ascii_lowercase` port would drop them.
//! - **C2 apostrophe hand-scan.** Rust `regex` has no lookaround, and the two
//!   apostrophe helpers use `(?=…)`. We hand-scan scalars, classifying a neighbor
//!   as letter/number via a compiled `^[\p{L}\p{N}]$` (EXACT General Category —
//!   NOT `char::is_alphabetic()`, which also matches Other_Alphabetic such as
//!   combining marks). Smart quotes U+2018/U+2019 are normalized to `'` first.
//! - **C3 no-`/\s+/`.** Whitespace folding/tokenizing is delegated to
//!   [`crate::text_scanner`]'s hand-rolled scanner; no `/\s+/` regex is used
//!   (oracle spy contract).
//! - **C4 ASCII locks.** Every `\d` is `[0-9]`, every `\b` is `(?-u:\b)`. The
//!   `detectIntentAction` patterns are matched against a lowercased copy of the
//!   input (emulating JS non-Unicode `/i`) so the negated ASCII boundary class
//!   `[^a-z0-9_-]` stays exact. The dynamic `\b<number>\b` and `^<identifier>…`
//!   regexes escape their interpolant with `regex::escape` (JS `escapeRegExp`).
//! - **C6 UTF-16 → scalar.** `titleCaseWord`'s `apostropheParts[0].length === 1`
//!   and `compactWorkItemTitle`'s `[^:]{1,32}` are UTF-16 code-unit bounds; we use
//!   scalar counts (narrow divergence for astral input; never panics).

use std::sync::OnceLock;

use regex::Regex;
use suaegi_workref::format_identifier_first;

use crate::js_ws::{js_trim, WS_CLASS};
use crate::text_scanner::{
    collect_compact_workspace_words, fold_workspace_name_whitespace_to_hyphen,
};

/// `STOP_WORDS` (`:84-100`), 15 entries.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "for", "from", "in", "is", "it", "of", "on", "or", "the", "this", "to",
    "with",
];

/// A resolved workspace name: the human display label + the git-safe seed slug.
/// Mirrors `WorkspaceIntentName` (`:55-58`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIntentName {
    pub display_name: String,
    pub seed_name: String,
}

/// The work-item type discriminant (`:47`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemType {
    Issue,
    Pr,
    Mr,
}

/// `WorkspaceIntentWorkItem` (`:46-53`). `provider` is carried for fidelity but is
/// never read by this cluster (research §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIntentWorkItem {
    pub item_type: WorkItemType,
    pub number: i64,
    pub title: String,
    pub provider: Option<String>,
    pub linear_identifier: Option<String>,
    pub jira_identifier: Option<String>,
}

// ---- compiled static patterns (compile-once) -------------------------------

struct Patterns {
    slash: Regex,
    non_ws: Regex,
    multi_hyphen: Regex,
    multi_dot: Regex,
    lead_trail: Regex,
    trail_sep: Regex,
    title_prefix: Regex,
    hash_prefix: Regex,
    paren_num: Regex,
    bare_hash: Regex,
    colon_prefix: Regex,
    acronym: Regex,
    ticket: Regex,
    acronym_poss: Regex,
    ln: Regex,
    action_labels: Vec<(Regex, &'static str)>,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let ws = format!("[{WS_CLASS}]"); // JS `\s` (C3/C4)
        let b = r"(?:^|[^a-z0-9_-])";
        let e = r"(?:$|[^a-z0-9_-])";
        // Action patterns run against a *lowercased* copy of the input (see
        // `detect_intent_action`), so keywords stay lowercase and no `(?i)` is
        // needed — keeping the negated ASCII boundary class exact under JS
        // non-Unicode `/i` semantics.
        let action_specs: Vec<(String, &'static str)> = vec![
            (format!("{b}(?:fix(?:e[sd])?|resolve|repair){e}"), "Fix"),
            (format!("{b}(?:debug|diagnose){e}"), "Debug"),
            (
                format!("{b}(?:review|look{ws}+over|inspect|check|safe|safety){e}"),
                "Review",
            ),
            (format!("{b}(?:implement|build|ship){e}"), "Implement"),
            (
                format!("{b}(?:investigate|understand|triage){e}"),
                "Investigate",
            ),
            (format!("{b}(?:add|create){e}"), "Add"),
            (format!("{b}(?:update|change){e}"), "Update"),
            (format!("{b}(?:refactor|simplify){e}"), "Refactor"),
            (format!("{b}(?:test|verify|validate){e}"), "Test"),
        ];
        let action_labels = action_specs
            .into_iter()
            .map(|(pat, label)| (Regex::new(&pat).unwrap(), label))
            .collect();
        Patterns {
            slash: Regex::new(r"[\\/]+").unwrap(),
            non_ws: Regex::new(r"[^a-z0-9._-]+").unwrap(),
            multi_hyphen: Regex::new(r"-+").unwrap(),
            multi_dot: Regex::new(r"\.{2,}").unwrap(),
            lead_trail: Regex::new(r"^[.-]+|[.-]+$").unwrap(),
            trail_sep: Regex::new(r"[-._]+$").unwrap(),
            title_prefix: Regex::new(&format!(
                r"(?i)^(?:issue|pr|pull request|mr|merge request){ws}*[#!]?[0-9]+{ws}*[:-]{ws}*"
            ))
            .unwrap(),
            hash_prefix: Regex::new(&format!(r"^#[0-9]+{ws}*[:-]{ws}*")).unwrap(),
            paren_num: Regex::new(r"\([#!]?[0-9]+\)").unwrap(),
            bare_hash: Regex::new(r"(?-u:\b)#[0-9]+(?-u:\b)").unwrap(),
            colon_prefix: Regex::new(&format!(r"^[^:]{{1,32}}:{ws}*")).unwrap(),
            acronym: Regex::new(r"^[A-Z]{2,}[0-9]*$").unwrap(),
            // ASCII-locked case-insensitive (C4): `(?i-u:…)` so Kelvin/ſ never fold.
            ticket: Regex::new(r"(?i-u:^[A-Z]+-[0-9]+$)").unwrap(),
            acronym_poss: Regex::new(r"^([A-Z]{2,}[0-9]*)'([sS])$").unwrap(),
            ln: Regex::new(r"^[\p{L}\p{N}]$").unwrap(),
            action_labels,
        }
    })
}

// ---- case helpers (C1 full case mapping) -----------------------------------

fn lower(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}
fn upper(s: &str) -> String {
    s.chars().flat_map(char::to_uppercase).collect()
}

// ---- apostrophe helpers (C2 hand-scan, EXACT GC classification) ------------

/// `[\p{L}\p{N}]` — EXACT General Category letter/number. NOT
/// `char::is_alphabetic()` (that also matches Other_Alphabetic combining marks).
fn is_ln(ch: char) -> bool {
    let mut buf = [0u8; 4];
    patterns().ln.is_match(ch.encode_utf8(&mut buf))
}

/// `normalizeApostrophes` (`:7-9`): U+2018/U+2019 → ASCII `'`.
fn normalize_apostrophes(input: &str) -> String {
    input.replace(&['\u{2018}', '\u{2019}'][..], "'")
}

/// Drop each `'` for which `should_drop(prev, next)` holds; other chars pass
/// through. `prev`/`next` are the immediate scalar neighbors (`None` at the
/// string boundary) — the hand-rolled equivalent of a group-1 / lookahead pass.
fn scan_apostrophes(
    input: &str,
    should_drop: impl Fn(Option<char>, Option<char>) -> bool,
) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '\'' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();
            if should_drop(prev, next) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// `removeIntraWordApostrophes` (`:13-15`): drop `'` when both neighbors are L/N
/// (`([\p{L}\p{N}])'(?=[\p{L}\p{N}])` → `$1`).
fn remove_intra_word_apostrophes(input: &str) -> String {
    let normalized = normalize_apostrophes(input);
    scan_apostrophes(&normalized, |prev, next| {
        matches!(prev, Some(p) if is_ln(p)) && matches!(next, Some(n) if is_ln(n))
    })
}

/// `stripDanglingDisplayApostrophes` (`:17-21`): two sequential passes — remove a
/// leading dangling `'` (start-or-non-LN before, L/N after), then a trailing
/// dangling `'` (L/N before, end-or-non-LN after).
fn strip_dangling_display_apostrophes(input: &str) -> String {
    let normalized = normalize_apostrophes(input);
    // Pass 1: `(^|[^\p{L}\p{N}])'(?=[\p{L}\p{N}])` → `$1`.
    let pass1 = scan_apostrophes(&normalized, |prev, next| {
        (prev.is_none() || matches!(prev, Some(p) if !is_ln(p)))
            && matches!(next, Some(n) if is_ln(n))
    });
    // Pass 2: `([\p{L}\p{N}])'(?=$|[^\p{L}\p{N}])` → `$1`.
    scan_apostrophes(&pass1, |prev, next| {
        matches!(prev, Some(p) if is_ln(p))
            && (next.is_none() || matches!(next, Some(n) if !is_ln(n)))
    })
}

// ---- slugify (C1) ----------------------------------------------------------

/// `slugifyForWorkspaceName` (`:23-39`). Full lowercase (C1), fold whitespace via
/// the hand-rolled scanner (C3), ASCII whitelist, collapse hyphens/dots, strip
/// edge separators, cap at 48 scalars, re-strip a trailing separator.
pub fn slugify_for_workspace_name(input: &str) -> String {
    let p = patterns();
    let step1 = remove_intra_word_apostrophes(input);
    let trimmed = js_trim(&step1);
    let lowered = lower(trimmed);
    let no_slash = p.slash.replace_all(&lowered, "-").into_owned();
    let folded = fold_workspace_name_whitespace_to_hyphen(&no_slash);
    let s = p.non_ws.replace_all(&folded, "-").into_owned();
    let s = p.multi_hyphen.replace_all(&s, "-").into_owned();
    let s = p.multi_dot.replace_all(&s, ".").into_owned();
    let s = p.lead_trail.replace_all(&s, "").into_owned();
    // At this point the string is `[a-z0-9._-]` (ASCII), so the first 48 scalars
    // equal the first 48 UTF-16 code units (JS `.slice(0, 48)`).
    let capped: String = s.chars().take(48).collect();
    p.trail_sep.replace_all(&capped, "").into_owned()
}

// ---- linked work-item title cleanup ----------------------------------------

/// `getLinkedWorkItemTitleSubject` (`:60-68`). Strip a leading `Issue #N:`/`#N:`
/// prefix, parenthesized `(#N)`, and bare `#N`, then trim.
fn get_linked_work_item_title_subject(title: &str) -> String {
    let p = patterns();
    let trimmed = js_trim(title);
    let s = p.title_prefix.replace(trimmed, "").into_owned();
    let s = p.hash_prefix.replace(&s, "").into_owned();
    let s = p.paren_num.replace_all(&s, "").into_owned();
    let s = p.bare_hash.replace_all(&s, "").into_owned();
    js_trim(&s).to_string()
}

/// `getLinkedWorkItemSuggestedName` (`:41-44`).
pub fn get_linked_work_item_suggested_name(title: &str) -> String {
    let subject = get_linked_work_item_title_subject(title);
    let seed = if subject.is_empty() {
        js_trim(title).to_string()
    } else {
        subject
    };
    slugify_for_workspace_name(&seed)
}

// ---- title-casing (C4/C6) --------------------------------------------------

/// `titleCaseWord` (`:111-126`). Acronyms / ticket keys → UPPER; acronym
/// possessive → `API's`; single-letter contraction → `I'm`; else capitalize.
fn title_case_word(word: &str) -> String {
    let p = patterns();
    let normalized = normalize_apostrophes(word);
    if p.acronym.is_match(&normalized) || p.ticket.is_match(&normalized) {
        return upper(&normalized);
    }
    if let Some(c) = p.acronym_poss.captures(&normalized) {
        return format!("{}'s", upper(&c[1]));
    }
    let lowered = lower(&normalized);
    let parts: Vec<&str> = lowered.split('\'').collect();
    // JS `apostropheParts[0].length === 1` is a UTF-16 count; scalar count here
    // (C6 narrow divergence for astral first chars).
    if parts.len() == 2 && parts[0].chars().count() == 1 && !parts[1].is_empty() {
        return format!("{}'{}", upper(parts[0]), parts[1]);
    }
    crate::branch_name::upper_first(&lowered)
}

/// `compactWords` (`:128-137`): scan up to `max_words` visible words (stripping
/// dangling apostrophes first) then title-case and join.
fn compact_words(input: &str, max_words: usize) -> String {
    let stripped = strip_dangling_display_apostrophes(input);
    let words = collect_compact_workspace_words(&stripped, max_words, STOP_WORDS);
    words
        .iter()
        .map(|w| title_case_word(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `compactWorkItemTitle` (`:143-160`). Strip number/prefix noise (including the
/// dynamic `\b<number>\b` and `^<identifier>` regexes, C4-escaped), then compact
/// to 3 words.
fn compact_work_item_title(title: &str, item: &WorkspaceIntentWorkItem) -> String {
    let p = patterns();
    let identifier = item
        .linear_identifier
        .as_deref()
        .or(item.jira_identifier.as_deref());
    let trimmed = js_trim(title);
    let s = p.title_prefix.replace(trimmed, "").into_owned();
    let s = p.paren_num.replace_all(&s, "").into_owned();
    let s = p.colon_prefix.replace(&s, "").into_owned();
    let mut without_prefix = js_trim(&s).to_string();
    if item.number > 0 {
        // Dynamic `\b[#!]?<number>\b` (C4: `regex::escape` + `(?-u:\b)`). The
        // number is integer-typed so escape is a no-op here, kept for fidelity.
        let re = Regex::new(&format!(
            r"(?-u:\b)[#!]?{}(?-u:\b)",
            regex::escape(&item.number.to_string())
        ))
        .unwrap();
        without_prefix = js_trim(&re.replace_all(&without_prefix, "")).to_string();
    }
    if let Some(id) = identifier {
        // Dynamic `^<escapeRegExp(identifier)>\s*[:-]?\s*` (C4-escaped, `(?i)`).
        let re = Regex::new(&format!(
            "(?i)^{}[{WS_CLASS}]*[:-]?[{WS_CLASS}]*",
            regex::escape(id)
        ))
        .unwrap();
        without_prefix = js_trim(&re.replace(&without_prefix, "")).to_string();
    }
    let source = if without_prefix.is_empty() {
        title
    } else {
        &without_prefix
    };
    compact_words(source, 3)
}

// ---- identity / intent -----------------------------------------------------

/// `detectIntentAction` (`:102-109`). First matching `ACTION_LABELS` entry wins
/// (order = priority). Matched against a lowercased copy so the ASCII boundary
/// class stays exact (JS non-Unicode `/i`).
fn detect_intent_action(source_text: &str) -> Option<&'static str> {
    let lowered = lower(source_text);
    for (re, label) in &patterns().action_labels {
        if re.is_match(&lowered) {
            return Some(label);
        }
    }
    None
}

/// `workItemIdentity` (`:162-176`).
fn work_item_identity(item: &WorkspaceIntentWorkItem) -> String {
    if let Some(l) = &item.linear_identifier {
        return upper(l);
    }
    if let Some(j) = &item.jira_identifier {
        return upper(j);
    }
    match item.item_type {
        WorkItemType::Pr => format!("PR {}", item.number),
        WorkItemType::Mr => format!("MR {}", item.number),
        WorkItemType::Issue => format!("Issue {}", item.number),
    }
}

/// `defaultActionForWorkItem` (`:196-198`).
fn default_action_for_work_item(item: &WorkspaceIntentWorkItem) -> Option<&'static str> {
    match item.item_type {
        WorkItemType::Pr | WorkItemType::Mr => Some("Review"),
        WorkItemType::Issue => None,
    }
}

/// Join the non-empty parts with a single space (JS `[..].filter(Boolean).join(' ')`).
fn join_non_empty(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `getLinkedWorkItemWorkspaceName` (`:178-194`).
pub fn get_linked_work_item_workspace_name(
    item: &WorkspaceIntentWorkItem,
) -> Option<WorkspaceIntentName> {
    let identifier = item
        .linear_identifier
        .as_deref()
        .or(item.jira_identifier.as_deref());
    let subject0 = get_linked_work_item_title_subject(&item.title);
    let mut subject = if subject0.is_empty() {
        js_trim(&item.title).to_string()
    } else {
        subject0
    };
    if let Some(id) = identifier {
        let re = Regex::new(&format!(
            "(?i)^{}[{WS_CLASS}]*[:-]?[{WS_CLASS}]*",
            regex::escape(id)
        ))
        .unwrap();
        subject = js_trim(&re.replace(&subject, "")).to_string();
    }
    let display_name = {
        let joined = join_non_empty(&[identifier.unwrap_or(""), subject.as_str()]);
        if joined.is_empty() {
            work_item_identity(item)
        } else {
            joined
        }
    };
    let seed_name = slugify_for_workspace_name(&display_name);
    if seed_name.is_empty() {
        None
    } else {
        Some(WorkspaceIntentName {
            display_name,
            seed_name,
        })
    }
}

/// `getWorkspaceIntentName` (`:205-243`). Resolve the single human intent label +
/// git-safe seed. Consumes `format_identifier_first` from `suaegi-workref`.
pub fn get_workspace_intent_name(
    source_text: Option<&str>,
    work_item: Option<&WorkspaceIntentWorkItem>,
    fallback_name: Option<&str>,
) -> Option<WorkspaceIntentName> {
    let source_text = source_text.map(js_trim).unwrap_or("");
    let mut display_name = String::new();

    if let Some(item) = work_item {
        let action =
            detect_intent_action(source_text).or_else(|| default_action_for_work_item(item));
        let identity = work_item_identity(item);
        if let Some(action) = action {
            display_name = format_identifier_first(&identity, action);
        } else {
            let subject = compact_work_item_title(&item.title, item);
            display_name = join_non_empty(&[identity.as_str(), subject.as_str()]);
        }
    } else if !source_text.is_empty() {
        display_name = compact_words(source_text, 5);
    }

    if display_name.is_empty() {
        if let Some(fb) = fallback_name {
            let trimmed = js_trim(fb);
            if !trimmed.is_empty() {
                display_name = trimmed.to_string();
            }
        }
    }
    if display_name.is_empty() {
        return None;
    }

    let seed_name = slugify_for_workspace_name(&display_name);
    if seed_name.is_empty() {
        None
    } else {
        Some(WorkspaceIntentName {
            display_name,
            seed_name,
        })
    }
}

/// `getLinearIssueWorkspaceName` (`:245-258`). Keep the identifier in the seed,
/// de-duplicating it if the title already leads with it, then re-slugify (≤48).
pub fn get_linear_issue_workspace_name(identifier: &str, title: &str) -> String {
    let key = slugify_for_workspace_name(identifier);
    let title_slug = get_linked_work_item_suggested_name(title);
    if key.is_empty() {
        return title_slug;
    }
    let deduped = if title_slug == key {
        String::new()
    } else if title_slug.starts_with(&format!("{key}-")) {
        // key is a slug (ASCII) → byte offset is char-boundary-safe.
        title_slug[key.len() + 1..].to_string()
    } else {
        title_slug
    };
    slugify_for_workspace_name(&join_hyphen(&[key.as_str(), deduped.as_str()]))
}

/// `[..].filter(Boolean).join('-')`.
fn join_hyphen(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("-")
}

/// `resolveWorkspaceCreateName` (`:260-265`). Return the trimmed draft when
/// non-empty, else the fallback. Does NOT sanitize the draft (host worktree
/// sanitizer owns that) — Japanese / `feature/…` drafts pass through.
pub fn resolve_workspace_create_name(draft: Option<&str>, fallback: &str) -> String {
    match draft {
        Some(d) => {
            let trimmed = js_trim(d);
            if trimmed.is_empty() {
                fallback.to_string()
            } else {
                trimmed.to_string()
            }
        }
        None => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        item_type: WorkItemType,
        number: i64,
        title: &str,
        jira: Option<&str>,
        linear: Option<&str>,
    ) -> WorkspaceIntentWorkItem {
        WorkspaceIntentWorkItem {
            item_type,
            number,
            title: title.to_string(),
            provider: None,
            linear_identifier: linear.map(str::to_string),
            jira_identifier: jira.map(str::to_string),
        }
    }

    fn name(display: &str, seed: &str) -> WorkspaceIntentName {
        WorkspaceIntentName {
            display_name: display.to_string(),
            seed_name: seed.to_string(),
        }
    }

    // ---- Oracle: slugifyForWorkspaceName (workspace-name.test.ts:15-42) ----

    #[test]
    fn slug_short_ascii_git_ref_safe() {
        assert_eq!(
            slugify_for_workspace_name("../../Fix mobile Tasks 🚀"),
            "fix-mobile-tasks"
        );
        assert_eq!(
            slugify_for_workspace_name("feature/add issue drawer"),
            "feature-add-issue-drawer"
        );
        assert_eq!(slugify_for_workspace_name(&"a".repeat(80)), "a".repeat(48));
    }

    #[test]
    fn slug_removes_intra_word_apostrophes() {
        assert_eq!(
            slugify_for_workspace_name("Can't enable browser notifications"),
            "cant-enable-browser-notifications"
        );
        assert_eq!(
            slugify_for_workspace_name("Can\u{2019}t enable browser notifications"),
            "cant-enable-browser-notifications"
        );
    }

    #[test]
    fn slug_folds_pasted_whitespace() {
        let n = format!("Fix{}\nPasted\tWorkspace", '\u{00A0}');
        assert_eq!(slugify_for_workspace_name(&n), "fix-pasted-workspace");
    }

    // ---- C1 pin: full lowercase in slugify (İ K → i-k) ----

    #[test]
    fn c1_slug_dotted_capital_i_and_kelvin() {
        assert_eq!(slugify_for_workspace_name("İ K"), "i-k");
        assert_eq!(slugify_for_workspace_name("\u{212A}"), "k");
    }

    // ---- C2 pins: apostrophe hand-scan (exact GC classification) ----

    #[test]
    fn c2_remove_intra_word_apostrophes() {
        assert_eq!(remove_intra_word_apostrophes("rock'n'roll"), "rocknroll");
        assert_eq!(remove_intra_word_apostrophes("Can't"), "Cant");
        assert_eq!(remove_intra_word_apostrophes("'Hello"), "'Hello"); // leading kept
        assert_eq!(remove_intra_word_apostrophes("Hello'"), "Hello'"); // trailing kept
        assert_eq!(remove_intra_word_apostrophes("''"), "''");
        // Non-Latin L/N neighbors: Greek letters and digits both count as \p{L}\p{N}.
        assert_eq!(
            remove_intra_word_apostrophes("\u{03A9}'\u{03A9}"),
            "\u{03A9}\u{03A9}"
        );
        assert_eq!(remove_intra_word_apostrophes("1'2"), "12");
    }

    /// Other_Alphabetic proof: U+0345 (combining greek ypogegrammeni) has
    /// `char::is_alphabetic() == true` but General Category `Mn` (NOT `\p{L}`).
    /// The EXACT-GC classifier must treat it as NON-L/N, so the apostrophe is
    /// kept. A `char::is_alphabetic()` port would wrongly drop it.
    #[test]
    fn c2_other_alphabetic_neighbor_is_not_ln() {
        assert!(!is_ln('\u{0345}'));
        assert!(char::is_alphabetic('\u{0345}')); // documents the divergence
        assert!(is_ln('\u{03A9}')); // a real Greek letter IS \p{L}
        assert!(is_ln('1'));
        assert!(!is_ln(' '));
        // prev neighbor is the combining mark (non-L/N) → apostrophe kept.
        assert_eq!(remove_intra_word_apostrophes("\u{0345}'b"), "\u{0345}'b");
    }

    #[test]
    fn c2_strip_dangling_display_apostrophes() {
        assert_eq!(strip_dangling_display_apostrophes("'Hello"), "Hello");
        assert_eq!(strip_dangling_display_apostrophes("Hello'"), "Hello");
        assert_eq!(strip_dangling_display_apostrophes("('Hello')"), "(Hello)");
        assert_eq!(
            strip_dangling_display_apostrophes("rock'n'roll"),
            "rock'n'roll"
        ); // intra kept
        assert_eq!(strip_dangling_display_apostrophes("''"), "''");
    }

    // ---- Oracle: getLinkedWorkItemSuggestedName (:44-53) ----

    #[test]
    fn suggested_name_removes_issue_and_pr_numbers() {
        assert_eq!(
            get_linked_work_item_suggested_name("Issue #123: Fix mobile Tasks"),
            "fix-mobile-tasks"
        );
        assert_eq!(
            get_linked_work_item_suggested_name("Add mobile drawer (#812)"),
            "add-mobile-drawer"
        );
    }

    /// C4 [0-9] lock (load-bearing): with ASCII `\d`, an `Issue ١٢٣:` prefix
    /// (Arabic-Indic digits) does NOT match the number-prefix strip, so `Issue`
    /// survives into the slug. A `[0-9]->\d` regression would strip the prefix and
    /// yield `fix-mobile-tasks` instead.
    #[test]
    fn c4_non_ascii_digit_prefix_is_not_stripped() {
        assert_eq!(
            get_linked_work_item_suggested_name("Issue \u{0661}\u{0662}\u{0663}: Fix mobile Tasks"),
            "issue-fix-mobile-tasks"
        );
    }

    // ---- Oracle: getLinkedWorkItemWorkspaceName (:55-83) ----

    #[test]
    fn linked_workspace_name_uses_resolved_title() {
        assert_eq!(
            get_linked_work_item_workspace_name(&item(
                WorkItemType::Pr,
                2049,
                "Fix pasted URL workspace names",
                None,
                None
            )),
            Some(name(
                "Fix pasted URL workspace names",
                "fix-pasted-url-workspace-names"
            ))
        );
    }

    #[test]
    fn linked_workspace_name_keeps_external_identifier() {
        assert_eq!(
            get_linked_work_item_workspace_name(&item(
                WorkItemType::Issue,
                0,
                "PROJ-7 Fix flaky import",
                Some("PROJ-7"),
                None
            )),
            Some(name("PROJ-7 Fix flaky import", "proj-7-fix-flaky-import"))
        );
    }

    /// C4 dynamic-regex escape: a metachar in the provider identifier is escaped
    /// (`escapeRegExp` / `regex::escape`), so `A.B` strips only a literal `A.B`
    /// prefix — never `AxB`. Unescaped, `^A.B` would match `AxB` and corrupt the
    /// subject.
    #[test]
    fn c4_identifier_metachar_is_escaped() {
        assert_eq!(
            get_linked_work_item_workspace_name(&item(
                WorkItemType::Issue,
                0,
                "AxB fix thing",
                Some("A.B"),
                None
            )),
            Some(name("A.B AxB fix thing", "a.b-axb-fix-thing"))
        );
    }

    // ---- Oracle: getWorkspaceIntentName (:85-252) ----

    #[test]
    fn intent_uses_explicit_action_for_linked_issue() {
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/mvanhorn/cli-printing-press/issues/2635 and fix it"),
                Some(&item(
                    WorkItemType::Issue,
                    2635,
                    "scorer/dogfood: live acceptance can't authenticate via the CLI's config/cookie credentials (scoped-home is env-only)",
                    None,
                    None
                )),
                None
            ),
            Some(name("Issue 2635 - Fix", "issue-2635-fix"))
        );
    }

    #[test]
    fn intent_defaults_pr_and_mr_to_review() {
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/acme/app/pull/1234 and check whether this is safe"),
                Some(&item(
                    WorkItemType::Pr,
                    1234,
                    "Refactor account settings panel",
                    None,
                    None
                )),
                None
            ),
            Some(name("PR 1234 - Review", "pr-1234-review"))
        );
        assert_eq!(
            get_workspace_intent_name(
                Some("fix https://gitlab.com/acme/app/-/merge_requests/77"),
                Some(&item(WorkItemType::Mr, 77, "Resolve sync race", None, None)),
                None
            ),
            Some(name("MR 77 - Fix", "mr-77-fix"))
        );
    }

    #[test]
    fn intent_compresses_subject_when_no_action() {
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/acme/app/issues/9876"),
                Some(&item(
                    WorkItemType::Issue,
                    9876,
                    "Make importer handle archived rows",
                    None,
                    None
                )),
                None
            ),
            Some(name(
                "Issue 9876 Make Importer Handle",
                "issue-9876-make-importer-handle"
            ))
        );
    }

    #[test]
    fn intent_keeps_contractions_readable() {
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/acme/app/issues/4802"),
                Some(&item(
                    WorkItemType::Issue,
                    4802,
                    "Can't enable browser notifications from within a browser tab",
                    None,
                    None
                )),
                None
            ),
            Some(name(
                "Issue 4802 Can't Enable Browser",
                "issue-4802-cant-enable-browser"
            ))
        );
    }

    #[test]
    fn intent_single_letter_contractions() {
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/acme/app/issues/17"),
                Some(&item(
                    WorkItemType::Issue,
                    17,
                    "i'm blocked on notifications",
                    None,
                    None
                )),
                None
            ),
            Some(name(
                "Issue 17 I'm Blocked Notifications",
                "issue-17-im-blocked-notifications"
            ))
        );
        assert_eq!(
            get_workspace_intent_name(
                Some("https://github.com/acme/app/issues/18"),
                Some(&item(
                    WorkItemType::Issue,
                    18,
                    "i'll update login",
                    None,
                    None
                )),
                None
            ),
            Some(name(
                "Issue 18 I'll Update Login",
                "issue-18-ill-update-login"
            ))
        );
    }

    #[test]
    fn intent_does_not_treat_auto_slug_as_intent() {
        assert_eq!(
            get_workspace_intent_name(
                Some("issue-123-fix-navbar"),
                Some(&item(
                    WorkItemType::Issue,
                    456,
                    "Make importer handle archived rows",
                    None,
                    None
                )),
                None
            ),
            Some(name(
                "Issue 456 Make Importer Handle",
                "issue-456-make-importer-handle"
            ))
        );
    }

    #[test]
    fn intent_external_identifier_without_duplication() {
        assert_eq!(
            get_workspace_intent_name(
                None,
                Some(&item(
                    WorkItemType::Issue,
                    0,
                    "PROJ-7 Fix flaky import",
                    Some("PROJ-7"),
                    None
                )),
                None
            ),
            Some(name("PROJ-7 Fix Flaky Import", "proj-7-fix-flaky-import"))
        );
    }

    #[test]
    fn intent_summarizes_unlinked_text() {
        assert_eq!(
            get_workspace_intent_name(Some("add keyboard shortcut settings"), None, None),
            Some(name(
                "Add Keyboard Shortcut Settings",
                "add-keyboard-shortcut-settings"
            ))
        );
    }

    #[test]
    fn intent_compacts_pasted_text_with_url() {
        let source = format!(
            "https://github.com/acme/app/issues/123\nadd{}keyboard\tshortcut settings",
            '\u{00A0}'
        );
        assert_eq!(
            get_workspace_intent_name(Some(&source), None, None),
            Some(name(
                "Add Keyboard Shortcut Settings",
                "add-keyboard-shortcut-settings"
            ))
        );
    }

    // ---- Oracle: getLinearIssueWorkspaceName (:254-281) ----

    #[test]
    fn linear_keeps_identifier_in_seed() {
        assert_eq!(
            get_linear_issue_workspace_name("ENG-42", "Ship Linear parity"),
            "eng-42-ship-linear-parity"
        );
    }

    #[test]
    fn linear_does_not_duplicate_identifier() {
        assert_eq!(
            get_linear_issue_workspace_name("ENG-42", "ENG-42 Ship Linear parity"),
            "eng-42-ship-linear-parity"
        );
    }

    #[test]
    fn linear_keeps_within_limit() {
        let seed = get_linear_issue_workspace_name(
            "ENG-42",
            "Implement a very long Linear issue title that should be truncated",
        );
        assert!(seed.len() <= 48);
        assert!(seed.starts_with("eng-42-"));
    }

    // ---- Oracle: resolveWorkspaceCreateName (:283-303) ----

    #[test]
    fn resolve_preserves_explicit_names() {
        assert_eq!(
            resolve_workspace_create_name(Some("feature/something"), "issue-123"),
            "feature/something"
        );
        assert_eq!(
            resolve_workspace_create_name(Some("日本語 テスト"), "issue-123"),
            "日本語 テスト"
        );
    }

    #[test]
    fn resolve_uses_fallback_when_blank() {
        assert_eq!(resolve_workspace_create_name(Some("   "), "pr-9"), "pr-9");
        assert_eq!(resolve_workspace_create_name(None, "issue-4"), "issue-4");
    }
}
