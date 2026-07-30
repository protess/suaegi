//! Task-list search-query DSL parser — a verbatim port of Orca's
//! `src/shared/task-query.ts` (@ v1.4.150-rc.0). Cited `:line` numbers refer to
//! that file. Behaviour is faithful to the source **including two documented
//! quote-handling defects** (plan Codex decision C3) — see [`serialize_task_query`]
//! and the C3 pins in the tests below.

use crate::js_ws::{is_js_whitespace, js_trim};

/// What kind of item the query targets (`:2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    Issue,
    Pr,
}

/// The state filter (`:3`). `None` means "no state qualifier". Note the `All`
/// variant is distinct from [`Scope::All`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Closed,
    All,
    Merged,
}

/// Parsed representation of a raw search string (`:1-11`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTaskQuery {
    pub scope: Scope,
    pub state: Option<State>,
    pub draft: bool,
    pub assignee: Option<String>,
    pub author: Option<String>,
    pub review_requested: Option<String>,
    pub reviewed_by: Option<String>,
    pub labels: Vec<String>,
    pub free_text: String,
}

impl Default for ParsedTaskQuery {
    /// The initial query object (`:58-68`).
    fn default() -> Self {
        Self {
            scope: Scope::All,
            state: None,
            draft: false,
            assignee: None,
            author: None,
            review_requested: None,
            reviewed_by: None,
            labels: Vec::new(),
            free_text: String::new(),
        }
    }
}

/// The filter key set that [`with_qualifier`] can apply (`:213-220`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskQueryFilterKey {
    Author,
    Assignee,
    ReviewRequested,
    ReviewedBy,
    Labels,
    State,
    Draft,
}

/// The value passed to [`with_qualifier`], mirroring the source's
/// `string | string[] | null` (`:230`). Single-value keys read [`Str`], the
/// `labels` key reads [`List`], and [`Null`] clears a value.
///
/// [`Str`]: QualifierValue::Str
/// [`List`]: QualifierValue::List
/// [`Null`]: QualifierValue::Null
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualifierValue {
    Str(String),
    List(Vec<String>),
    Null,
}

/// One token plus its raw (quote-preserving) form (`:13-16`).
struct SearchQueryToken {
    value: String,
    raw: String,
}

/// Hand-rolled tokenizer state machine (`:18-51`, plan C4).
///
/// Quotes (`"` or `'`) open and close a quoted span; whitespace splits tokens
/// only outside a quote. There is **NO backslash handling** — that omission is
/// the source of the C3 defect and is preserved verbatim (see [`serialize_task_query`]).
///
/// JS indexes the string by UTF-16 code unit; we iterate Unicode scalar values.
/// The delimiter set (whitespace, `"`, `'`) is entirely in the BMP, so the only
/// possible divergence is astral-plane input, which the oracle never exercises.
fn tokenize_search_query_with_raw(raw_query: &str) -> Vec<SearchQueryToken> {
    let mut tokens: Vec<SearchQueryToken> = Vec::new();
    let mut value = String::new();
    let mut raw = String::new();
    let mut quote: Option<char> = None;

    // Mirrors the `flush` closure (`:24-30`): emit a token if either buffer is
    // non-empty (JS truthiness on `value || raw`), then reset both.
    macro_rules! flush {
        () => {
            if !value.is_empty() || !raw.is_empty() {
                tokens.push(SearchQueryToken {
                    value: std::mem::take(&mut value),
                    raw: std::mem::take(&mut raw),
                });
            }
        };
    }

    for ch in raw_query.chars() {
        if is_js_whitespace(ch) && quote.is_none() {
            flush!();
            continue;
        }
        raw.push(ch);
        if (ch == '"' || ch == '\'') && quote.is_none() {
            quote = Some(ch);
            continue;
        }
        if Some(ch) == quote {
            quote = None;
            continue;
        }
        value.push(ch);
    }
    flush!();
    tokens
}

/// Split a raw query into token values, dropping the raw forms (`:53-55`).
pub fn tokenize_search_query(raw_query: &str) -> Vec<String> {
    tokenize_search_query_with_raw(raw_query)
        .into_iter()
        .map(|token| token.value)
        .collect()
}

/// Parse a raw search string into a [`ParsedTaskQuery`] (`:57-163`).
pub fn parse_task_query(raw_query: &str) -> ParsedTaskQuery {
    let mut query = ParsedTaskQuery::default();

    let mut free_text_tokens: Vec<String> = Vec::new();
    let mut saw_issue_scope = false;
    let mut saw_pr_scope = false;

    // `:73` — trim (js_trim, C1) then tokenize.
    for token in tokenize_search_query_with_raw(js_trim(raw_query)) {
        let SearchQueryToken { value: token, raw } = token;
        // `:74` — normalize with to_ascii_lowercase (C2).
        let normalized = token.to_ascii_lowercase();
        if normalized == "is:issue" {
            saw_issue_scope = true;
            query.scope = if saw_pr_scope {
                Scope::All
            } else {
                Scope::Issue
            };
            continue;
        }
        if normalized == "is:pr" || normalized == "is:pull-request" {
            saw_pr_scope = true;
            query.scope = if saw_issue_scope {
                Scope::All
            } else {
                Scope::Pr
            };
            continue;
        }
        if normalized == "is:open" {
            query.state = Some(State::Open);
            continue;
        }
        if normalized == "is:closed" {
            query.state = Some(State::Closed);
            continue;
        }
        if normalized == "is:merged" {
            query.state = Some(State::Merged);
            continue;
        }
        if normalized == "is:draft" {
            query.scope = Scope::Pr;
            query.state = Some(State::Open);
            query.draft = true;
            continue;
        }

        // `:104-106` — split on the first `:`; value is js_trim'd (C1), key is
        // to_ascii_lowercase'd (C2).
        let (raw_key, value) = match token.split_once(':') {
            Some((key, rest)) => (key, js_trim(rest).to_string()),
            None => (token.as_str(), String::new()),
        };
        let key = raw_key.to_ascii_lowercase();
        if value.is_empty() {
            free_text_tokens.push(raw);
            continue;
        }

        if key == "assignee" {
            query.assignee = Some(value);
            continue;
        }
        if key == "author" {
            query.author = Some(value);
            continue;
        }
        if key == "review-requested" {
            query.scope = Scope::Pr;
            query.review_requested = Some(value);
            continue;
        }
        if key == "reviewed-by" {
            query.scope = Scope::Pr;
            query.reviewed_by = Some(value);
            continue;
        }
        if key == "label" {
            query.labels.push(value);
            continue;
        }
        // `:134` — state value normalized with to_ascii_lowercase (C2).
        let normalized_value = value.to_ascii_lowercase();
        if key == "state"
            && matches!(
                normalized_value.as_str(),
                "open" | "closed" | "merged" | "all"
            )
        {
            query.state = Some(match normalized_value.as_str() {
                "open" => State::Open,
                "closed" => State::Closed,
                "merged" => State::Merged,
                _ => State::All,
            });
            continue;
        }

        // `:146-148` — unknown qualifiers and exact phrases pass through as-is
        // (raw form, so quotes survive).
        free_text_tokens.push(raw);
    }

    // `:151-160` — scope reconciliation.
    if query.draft {
        query.scope = Scope::Pr;
        query.state = Some(State::Open);
    } else if query.state == Some(State::Merged)
        || query.review_requested.is_some()
        || query.reviewed_by.is_some()
    {
        query.scope = Scope::Pr;
    }
    // `:161` — join with spaces then js_trim (C1).
    query.free_text = js_trim(&free_text_tokens.join(" ")).to_string();
    query
}

/// Quote a value only if it contains whitespace (`:165-167`).
///
/// **C3 defect (preserved verbatim):** when quoting, embedded `"` are escaped as
/// `\"`. But [`tokenize_search_query_with_raw`] has no backslash handling, so a
/// re-parse cannot recover the original — the `\"` mangles into a literal `\`
/// followed by a closing quote. Do NOT "fix" this; it is pinned by a regression
/// test so any change is caught. `\s` here is the JS-whitespace set (C1).
fn quote_if_needed(value: &str) -> String {
    if value.chars().any(is_js_whitespace) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Serialize a [`ParsedTaskQuery`] back to a raw search string (`:173-211`).
///
/// Canonical emission order: scope → state → draft → author → assignee →
/// review-requested → reviewed-by → labels → free_text. Round-trips
/// **structurally** (`parse(serialize(parse(x))) == parse(x)`) for understood
/// qualifiers, not as raw-string identity.
///
/// JS truthiness note: the source guards each single-value field with
/// `if (q.author)` etc., where an empty string is falsy — so empty strings are
/// skipped, exactly like `null`. We replicate that with `filter(|s| !s.is_empty())`.
pub fn serialize_task_query(q: &ParsedTaskQuery) -> String {
    let mut parts: Vec<String> = Vec::new();

    match q.scope {
        Scope::Pr => parts.push("is:pr".to_string()),
        Scope::Issue => parts.push("is:issue".to_string()),
        Scope::All => {}
    }
    match q.state {
        Some(State::Open) => parts.push("is:open".to_string()),
        Some(State::Closed) => parts.push("is:closed".to_string()),
        Some(State::Merged) => parts.push("is:merged".to_string()),
        Some(State::All) => parts.push("state:all".to_string()),
        None => {}
    }
    if q.draft {
        parts.push("is:draft".to_string());
    }
    if let Some(author) = q.author.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("author:{}", quote_if_needed(author)));
    }
    if let Some(assignee) = q.assignee.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("assignee:{}", quote_if_needed(assignee)));
    }
    if let Some(rr) = q.review_requested.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("review-requested:{}", quote_if_needed(rr)));
    }
    if let Some(rb) = q.reviewed_by.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("reviewed-by:{}", quote_if_needed(rb)));
    }
    for label in &q.labels {
        parts.push(format!("label:{}", quote_if_needed(label)));
    }
    if !q.free_text.is_empty() {
        parts.push(q.free_text.clone());
    }
    parts.join(" ")
}

/// Apply a single filter change to a raw query and re-serialize (`:227-276`).
///
/// For single-value keys, [`QualifierValue::Null`] (or a [`QualifierValue::List`])
/// clears the field. For `labels`, pass the full next list. Mirrors the source's
/// JS-truthiness on `if (parsed.reviewRequested)` etc. (empty string does not
/// force PR scope).
pub fn with_qualifier(raw_query: &str, key: TaskQueryFilterKey, value: QualifierValue) -> String {
    let mut parsed = parse_task_query(raw_query);
    match key {
        TaskQueryFilterKey::Author => {
            parsed.author = as_string(value);
        }
        TaskQueryFilterKey::Assignee => {
            parsed.assignee = as_string(value);
        }
        TaskQueryFilterKey::ReviewRequested => {
            parsed.review_requested = as_string(value);
            if parsed
                .review_requested
                .as_deref()
                .is_some_and(|s| !s.is_empty())
            {
                parsed.scope = Scope::Pr;
            }
        }
        TaskQueryFilterKey::ReviewedBy => {
            parsed.reviewed_by = as_string(value);
            if parsed.reviewed_by.as_deref().is_some_and(|s| !s.is_empty()) {
                parsed.scope = Scope::Pr;
            }
        }
        TaskQueryFilterKey::Labels => {
            parsed.labels = match value {
                QualifierValue::List(list) => list,
                _ => Vec::new(),
            };
        }
        TaskQueryFilterKey::State => {
            // `:256-259` — exact (non-lowercased) string match.
            parsed.state = match &value {
                QualifierValue::Str(s) => match s.as_str() {
                    "open" => Some(State::Open),
                    "closed" => Some(State::Closed),
                    "merged" => Some(State::Merged),
                    "all" => Some(State::All),
                    _ => None,
                },
                _ => None,
            };
            if parsed.state == Some(State::Merged) {
                parsed.scope = Scope::Pr;
            }
            if parsed.state != Some(State::Open) {
                parsed.draft = false;
            }
        }
        TaskQueryFilterKey::Draft => {
            parsed.draft = matches!(&value, QualifierValue::Str(s) if s == "true");
            if parsed.draft {
                parsed.scope = Scope::Pr;
                parsed.state = Some(State::Open);
            }
        }
    }
    serialize_task_query(&parsed)
}

/// `typeof value === 'string' ? value : null` (`:235` etc.).
fn as_string(value: QualifierValue) -> Option<String> {
    match value {
        QualifierValue::Str(s) => Some(s),
        _ => None,
    }
}

/// True iff `token` matches `/^repo:[^\s]+$/i` (`:290`) — an anchored,
/// case-insensitive `repo:` prefix followed by one-or-more non-whitespace chars.
/// Case-folding is ASCII (C2); `\s` is the JS-whitespace set (C1).
fn is_repo_qualifier(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    match lower.strip_prefix("repo:") {
        Some(rest) => !rest.is_empty() && !rest.chars().any(is_js_whitespace),
        None => false,
    }
}

/// Strip any `repo:owner/name` qualifiers from a raw search string (`:287-305`).
///
/// Tokens containing whitespace are re-quoted so quoted values round-trip. Input
/// is js_trim'd (`:289`, C1) before tokenizing.
pub fn strip_repo_qualifiers(raw_query: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    for token in tokenize_search_query(js_trim(raw_query)) {
        if is_repo_qualifier(&token) {
            continue;
        }
        // `:293` — re-quote whitespace-bearing tokens (JS-whitespace set, C1).
        if token.chars().any(is_js_whitespace) {
            match token.split_once(':') {
                Some((raw_key, rest)) => kept.push(format!("{raw_key}:\"{rest}\"")),
                None => kept.push(format!("\"{token}\"")),
            }
        } else {
            kept.push(token);
        }
    }
    kept.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Oracle: tokenizeSearchQuery (test.ts:10-37) =====

    #[test]
    fn tokenize_splits_on_whitespace() {
        assert_eq!(
            tokenize_search_query("is:open assignee:@me foo"),
            vec!["is:open", "assignee:@me", "foo"]
        );
    }

    #[test]
    fn tokenize_unwraps_standalone_double_quoted() {
        assert_eq!(
            tokenize_search_query("\"needs review\" foo"),
            vec!["needs review", "foo"]
        );
    }

    #[test]
    fn tokenize_unwraps_standalone_single_quoted() {
        assert_eq!(
            tokenize_search_query("'with spaces' bar"),
            vec!["with spaces", "bar"]
        );
    }

    #[test]
    fn tokenize_keeps_quoted_qualifier_values_as_one_token() {
        assert_eq!(
            tokenize_search_query("label:\"needs review\" author:alice"),
            vec!["label:needs review", "author:alice"]
        );
    }

    #[test]
    fn tokenize_empty_string_is_empty_list() {
        assert_eq!(tokenize_search_query(""), Vec::<String>::new());
    }

    // ===== Oracle: parseTaskQuery (test.ts:39-118) =====

    #[test]
    fn parse_defaults_for_empty_query() {
        let parsed = parse_task_query("");
        assert_eq!(parsed.scope, Scope::All);
        assert_eq!(parsed.state, None);
        assert_eq!(parsed.labels, Vec::<String>::new());
        assert_eq!(parsed.free_text, "");
    }

    #[test]
    fn parse_is_issue_and_is_open() {
        let parsed = parse_task_query("is:issue is:open");
        assert_eq!(parsed.scope, Scope::Issue);
        assert_eq!(parsed.state, Some(State::Open));
    }

    #[test]
    fn parse_is_pull_request_alias() {
        let parsed = parse_task_query("is:pull-request is:open");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::Open));
    }

    #[test]
    fn parse_widens_scope_to_all_issue_then_pr() {
        assert_eq!(parse_task_query("is:issue is:pr").scope, Scope::All);
    }

    #[test]
    fn parse_widens_scope_to_all_pr_then_issue() {
        assert_eq!(parse_task_query("is:pr is:issue").scope, Scope::All);
    }

    #[test]
    fn parse_is_draft_forces_pr_open() {
        let parsed = parse_task_query("is:draft");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::Open));
        assert!(parsed.draft);
    }

    #[test]
    fn parse_draft_stays_pr_even_with_later_issue() {
        let parsed = parse_task_query("is:draft is:issue");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::Open));
        assert!(parsed.draft);
    }

    #[test]
    fn parse_is_pr_is_open_does_not_set_draft() {
        let parsed = parse_task_query("is:pr is:open");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::Open));
        assert!(!parsed.draft);
    }

    #[test]
    fn parse_extracts_assignee_author_label_review() {
        let parsed =
            parse_task_query("assignee:@me author:alice review-requested:@me label:bug free text");
        assert_eq!(parsed.assignee.as_deref(), Some("@me"));
        assert_eq!(parsed.author.as_deref(), Some("alice"));
        assert_eq!(parsed.review_requested.as_deref(), Some("@me"));
        assert_eq!(parsed.scope, Scope::Pr); // review-requested forces pr
        assert_eq!(parsed.labels, vec!["bug".to_string()]);
        assert_eq!(parsed.free_text, "free text");
    }

    #[test]
    fn parse_review_stays_pr_even_with_later_issue() {
        let parsed = parse_task_query("review-requested:@me is:issue");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.review_requested.as_deref(), Some("@me"));
    }

    #[test]
    fn parse_unknown_qualifiers_and_bare_words_to_free_text() {
        assert_eq!(
            parse_task_query("custom:value hello").free_text,
            "custom:value hello"
        );
    }

    #[test]
    fn parse_state_all_for_any_filter() {
        let parsed = parse_task_query("is:pr state:all");
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::All));
    }

    // ===== Oracle: stripRepoQualifiers (test.ts:121-147) =====

    #[test]
    fn strip_removes_repo_tokens() {
        assert_eq!(
            strip_repo_qualifiers("is:open repo:foo/bar assignee:@me"),
            "is:open assignee:@me"
        );
    }

    #[test]
    fn strip_is_case_insensitive_on_repo_key() {
        assert_eq!(strip_repo_qualifiers("REPO:Foo/Bar is:open"), "is:open");
    }

    #[test]
    fn strip_keeps_other_qualifiers() {
        assert_eq!(strip_repo_qualifiers("label:bug repo:a/b"), "label:bug");
    }

    #[test]
    fn strip_requotes_standalone_whitespace_token() {
        assert_eq!(
            strip_repo_qualifiers("\"needs review\" repo:x/y"),
            "\"needs review\""
        );
    }

    #[test]
    fn strip_empty_when_only_repo_qualifiers() {
        assert_eq!(strip_repo_qualifiers("repo:foo/bar repo:baz/qux"), "");
    }

    #[test]
    fn strip_preserves_bare_word_without_space() {
        assert_eq!(strip_repo_qualifiers("hello repo:a/b world"), "hello world");
    }

    // ===== Oracle: serializeTaskQuery (test.ts:150-167) =====

    #[test]
    fn serialize_round_trips_qualifiers_and_free_text() {
        // Structural round-trip: parse(serialize(parse(raw))) == parse(raw).
        let raw = "is:pr is:open author:alice label:bug review-requested:bob hello world";
        let reserialized = serialize_task_query(&parse_task_query(raw));
        assert_eq!(parse_task_query(&reserialized), parse_task_query(raw));
    }

    #[test]
    fn serialize_quotes_label_values_with_whitespace() {
        let parsed = parse_task_query("label:\"needs review\"");
        assert_eq!(parsed.labels, vec!["needs review".to_string()]);
        assert_eq!(parsed.free_text, "");
        assert!(serialize_task_query(&parsed).contains("label:\"needs review\""));
    }

    #[test]
    fn serialize_all_state_exact_string() {
        let raw = serialize_task_query(&parse_task_query("is:pr state:all"));
        assert_eq!(raw, "is:pr state:all");
    }

    // ===== Oracle: withQualifier (test.ts:170-215) =====

    #[test]
    fn with_qualifier_sets_and_clears_author() {
        let set = with_qualifier(
            "hello",
            TaskQueryFilterKey::Author,
            QualifierValue::Str("alice".into()),
        );
        assert_eq!(parse_task_query(&set).author.as_deref(), Some("alice"));
        assert_eq!(parse_task_query(&set).free_text, "hello");
        let cleared = with_qualifier(&set, TaskQueryFilterKey::Author, QualifierValue::Null);
        assert_eq!(parse_task_query(&cleared).author, None);
        assert_eq!(parse_task_query(&cleared).free_text, "hello");
    }

    #[test]
    fn with_qualifier_replaces_labels_list() {
        let result = with_qualifier(
            "label:bug label:enh",
            TaskQueryFilterKey::Labels,
            QualifierValue::List(vec!["triage".into()]),
        );
        assert_eq!(parse_task_query(&result).labels, vec!["triage".to_string()]);
    }

    #[test]
    fn with_qualifier_clears_labels_with_empty_array() {
        let result = with_qualifier(
            "label:bug is:pr",
            TaskQueryFilterKey::Labels,
            QualifierValue::List(vec![]),
        );
        assert_eq!(parse_task_query(&result).labels, Vec::<String>::new());
        assert_eq!(parse_task_query(&result).scope, Scope::Pr);
    }

    #[test]
    fn with_qualifier_sets_all_state() {
        let result = with_qualifier(
            "is:pr is:open",
            TaskQueryFilterKey::State,
            QualifierValue::Str("all".into()),
        );
        assert_eq!(parse_task_query(&result).state, Some(State::All));
        assert!(result.contains("state:all"));
    }

    #[test]
    fn with_qualifier_preserves_quoted_free_text() {
        let result = with_qualifier(
            "\"exact phrase\" milestone:\"next release\"",
            TaskQueryFilterKey::Author,
            QualifierValue::Str("alice".into()),
        );
        assert!(result.contains("\"exact phrase\""));
        assert!(result.contains("milestone:\"next release\""));
        assert_eq!(parse_task_query(&result).author.as_deref(), Some("alice"));
    }

    #[test]
    fn with_qualifier_keeps_pr_only_filters_scoped_to_pr() {
        assert_eq!(
            parse_task_query(&with_qualifier(
                "",
                TaskQueryFilterKey::Draft,
                QualifierValue::Str("true".into())
            ))
            .scope,
            Scope::Pr
        );
        assert_eq!(
            parse_task_query(&with_qualifier(
                "",
                TaskQueryFilterKey::State,
                QualifierValue::Str("merged".into())
            ))
            .scope,
            Scope::Pr
        );
        assert_eq!(
            parse_task_query(&with_qualifier(
                "",
                TaskQueryFilterKey::ReviewRequested,
                QualifierValue::Str("@me".into())
            ))
            .scope,
            Scope::Pr
        );
    }

    #[test]
    fn with_qualifier_forces_draft_back_to_open_pr() {
        let parsed = parse_task_query(&with_qualifier(
            "is:pr is:closed",
            TaskQueryFilterKey::Draft,
            QualifierValue::Str("true".into()),
        ));
        assert_eq!(parsed.scope, Scope::Pr);
        assert_eq!(parsed.state, Some(State::Open));
        assert!(parsed.draft);
    }

    // ===== Codex extra pins (oracle-uncovered) =====

    /// C1 pin: the tokenizer splits on the JS-whitespace set, NOT Rust's
    /// `char::is_whitespace`. Reverting `is_js_whitespace` to `char::is_whitespace`
    /// flips BOTH assertions (see the two divergent codepoints).
    #[test]
    fn c1_js_whitespace_divergence() {
        // U+FEFF is JS whitespace (Rust char::is_whitespace = false) -> it SPLITS.
        assert_eq!(tokenize_search_query("a\u{FEFF}b"), vec!["a", "b"]);
        // U+0085/NEL is NOT JS whitespace (Rust char::is_whitespace = true)
        // -> it does NOT split; the token stays intact.
        assert_eq!(
            tokenize_search_query("a\u{0085}b"),
            vec!["a\u{0085}b".to_string()]
        );
    }

    /// C2 pin: recognition uses to_ascii_lowercase — ASCII-cased forms are
    /// recognized, and non-ASCII characters are NOT folded into ASCII literals
    /// (no `to_lowercase` surprise). Every recognition literal is ASCII, so
    /// to_ascii_lowercase and to_lowercase agree on all reachable inputs; this
    /// pin locks in the ASCII behaviour and the no-fold guarantee.
    #[test]
    fn c2_ascii_case_fold() {
        assert_eq!(parse_task_query("IS:PR").scope, Scope::Pr);
        assert_eq!(
            parse_task_query("Author:alice").author.as_deref(),
            Some("alice")
        );
        assert_eq!(parse_task_query("STATE:OPEN").state, Some(State::Open));
        // Fullwidth "IS" (U+FF29 U+FF33) is not folded to ASCII "is" by either
        // to_ascii_lowercase or to_lowercase, so it stays free text (no fold surprise).
        let parsed = parse_task_query("\u{FF29}\u{FF33}:pr");
        assert_eq!(parsed.scope, Scope::All);
        assert_eq!(parsed.free_text, "\u{FF29}\u{FF33}:pr");
    }

    /// C3 pin (a): a label value containing quote+space serializes with a `\"`
    /// escape that the tokenizer cannot re-parse, so a round-trip MANGLES the
    /// quote into a literal backslash. This is a deliberately-preserved Orca
    /// defect — assert the actual buggy output, do NOT "fix" it.
    #[test]
    fn c3_quote_escape_mangle_on_roundtrip() {
        // input label value: `a "b` (a, space, double-quote, b)
        let out = with_qualifier(
            "",
            TaskQueryFilterKey::Labels,
            QualifierValue::List(vec!["a \"b".into()]),
        );
        // serialized with the `\"` escape: `label:"a \"b"`
        assert_eq!(out, "label:\"a \\\"b\"");
        // re-parsing loses the quote and keeps the backslash: label value `a \b`
        // (a, space, backslash, b) — the mangle. NOT `a "b`.
        assert_eq!(parse_task_query(&out).labels, vec!["a \\b".to_string()]);
    }

    /// C3 pin (b): a quote with no surrounding whitespace (`a"b`) is consumed by
    /// the tokenizer as a structural opening quote, collapsing to `ab`.
    /// Deliberately-preserved Orca defect.
    #[test]
    fn c3_quote_without_whitespace_collapses() {
        assert_eq!(tokenize_search_query("a\"b"), vec!["ab".to_string()]);
    }
}
