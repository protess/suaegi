//! Work-item reference parser — a verbatim port of Orca's
//! `src/shared/work-item-reference.ts` (@ v1.4.150-rc.0, 190 lines).
//!
//! A single, host-aware parser for the review target named in a prompt (a PR,
//! MR, issue, or ticket). URLs are validated by *path structure*
//! (`owner/repo/pull/N`, GitLab's `/-/` marker) rather than hostname, which keeps
//! GitHub Enterprise / self-hosted GitLab working while rejecting stray URLs that
//! merely contain `/pull/<n>` (CDN assets, docs pages).
//!
//! # Public surface (4 exports)
//! - [`WorkIdentifier`] — the parsed label + lowercased tokens.
//! - [`extract_work_identifier`] — the 6-stage precedence chain.
//! - [`format_identifier_first`] — identifier-first label composition.
//! - [`strip_work_identifier_echo`] — remove the identifier's own echo.
//!
//! # Documented divergences from Orca (NOT bugs — plan Codex decisions C1-C5)
//! - **C1 — ASCII `\d`/`\b` lock.** Rust `regex` defaults `\d` to Unicode `Nd`
//!   (Arabic-Indic `٠١٢`, full-width `０１２`) and `\b`/`\w` to Unicode word
//!   semantics; JS uses ASCII. So every `\d` is written `[0-9]` and every `\b` as
//!   `(?-u:\b)`. Without this, `JIRA-١٢٣` / `#٤٥` would wrongly match. Captured
//!   digits stay `String` (no `parseInt` — leading zeros preserved, no overflow).
//! - **C2 — WHATWG URL via the `url` crate.** `new URL()` → `url::Url::parse`; an
//!   invalid URL is skipped (JS `try/catch`), not an error/panic. Path matching
//!   uses `url.path()` (JS `url.pathname`, excludes query/fragment). The `url`
//!   crate lowercases the scheme and preserves a trailing-slash pathname, matching
//!   JS — pinned in the E-url tests.
//! - **C3 — JS whitespace + ASCII case-fold.** `\s`/`.trim()` use the ECMAScript
//!   whitespace set (see [`js_ws`], includes U+FEFF, excludes U+0085), not Rust's
//!   Unicode `White_Space`. `.toLowerCase()` on the always-ASCII tokens maps to
//!   `to_ascii_lowercase`/`eq_ignore_ascii_case`. The `.slice(0, 4096)` scan cap
//!   is JS UTF-16 code units; we truncate on the 4096th Unicode scalar instead
//!   (char-boundary-safe, never panics — a documented divergence, since it is only
//!   an input length bound, not a semantic boundary).
//! - **C4 — `regex::escape()` in [`strip_work_identifier_echo`].** Orca builds the
//!   strip regex from raw tokens; the exported signature takes arbitrary tokens,
//!   so a metachar token is a regex-injection / ReDoS footgun. We escape each
//!   token before building the pattern — behaviorally identical for the
//!   extractor's metachar-free tokens, and a deliberate security hardening for
//!   arbitrary ones. Pinned in the E-strip test.
//! - **C5 — the 24-entry ticket-prefix denylist** ([`NON_TICKET_PREFIXES`]),
//!   ported verbatim from source lines 24-49.

mod js_ws;

use regex::Regex;
use std::sync::OnceLock;
use url::Url;

use js_ws::{js_trim, WS_CLASS};

/// A parsed work-item reference.
///
/// Mirrors Orca's `WorkIdentifier` type (`work-item-reference.ts:8-14`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkIdentifier {
    /// Human label, identifier-first, e.g. `PR 1033`, `MR 42`, `ENG-456`, `#321`.
    pub label: String,
    /// Lowercased identifier tokens, so consumers can drop them from a slug or
    /// description rather than echoing `Pr`, a bare number, or the ticket twice.
    pub tokens: Vec<String>,
}

/// Scan cap (`:18`). JS `.slice(0, 4096)` counts UTF-16 code units; we truncate on
/// the 4096th Unicode scalar (char-boundary-safe — see [`truncate_scan`]).
const IDENTIFIER_SCAN_LIMIT: usize = 4096;

/// Uppercase prefixes that look like Jira/Linear keys but are standards, ciphers,
/// or encodings — kept off the ticket path so `SHA-256` / `UTF-8` / `ISO-8601`
/// don't become work identifiers. Ported verbatim from `:24-49` — **24 entries**
/// (Codex-corrected count). Single-letter prefixes (`P-256`) can't match the
/// two-letter-minimum pattern, so they need no entry here.
const NON_TICKET_PREFIXES: [&str; 24] = [
    "UTF", "SHA", "MD", "ISO", "RFC", "AES", "RSA", "EC", "ES", "RS", "HS", "PS", "GPT", "MPEG",
    "UTC", "GMT", "IPV", "IEEE", "ANSI", "ASCII", "TLS", "SSL", "HTTP", "HTTPS",
];

/// The `NON_TICKET_PREFIXES.has(prefix)` check (`:156`). The ticket regex forces
/// `[A-Z]` uppercase, so an exact (case-sensitive) membership test is correct — a
/// lowercase `sha-256` never reaches here.
fn is_non_ticket_prefix(prefix: &str) -> bool {
    NON_TICKET_PREFIXES.contains(&prefix)
}

/// The 14 compile-once static regexes, ASCII-locked per C1.
struct Patterns {
    url_in_text: Regex,
    gitlab: Regex,
    github: Regex,
    bitbucket_cloud: Regex,
    bitbucket_server: Regex,
    azure_devops: Regex,
    trailing_punct: Regex,
    merge: Regex,
    pull_request: Regex,
    pr: Regex,
    issue: Regex,
    ticket: Regex,
    bare_hash: Regex,
    collapse_ws: Regex,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        // `ws` is the JS `\s` set as a regex class body (C3). `\d`→`[0-9]` and
        // `\b`→`(?-u:\b)` everywhere (C1). Unicode stays enabled globally (so
        // `\x{FEFF}` in `ws` is valid); only `\b` is locally ASCII.
        let ws = format!("[{WS_CLASS}]");
        // Negated URL class `[^\s<>()[\]"']` (R1). `r##""##` so `"` is literal.
        let url_neg = format!(r##"[^{WS_CLASS}<>()\[\]"']"##);
        Patterns {
            url_in_text: Regex::new(&format!("(?i)https?://{url_neg}+")).unwrap(),
            gitlab: Regex::new(r"(?i)/-/(issues|work_items|merge_requests)/([0-9]+)(?:[/?#]|$)")
                .unwrap(),
            github: Regex::new(r"(?i)^/[^/]+/[^/]+/(issues|pull)/([0-9]+)(?:[/?#]|$)").unwrap(),
            bitbucket_cloud: Regex::new(r"(?i)^/[^/]+/[^/]+/pull-requests/([0-9]+)(?:[/?#]|$)")
                .unwrap(),
            bitbucket_server: Regex::new(
                r"(?i)/(?:projects|users)/[^/]+/repos/[^/]+/pull-requests/([0-9]+)(?:[/?#]|$)",
            )
            .unwrap(),
            azure_devops: Regex::new(r"(?i)/_git/[^/]+/pullrequests?/([0-9]+)(?:[/?#]|$)").unwrap(),
            trailing_punct: Regex::new(r"[.,;:!?*_~]+$").unwrap(),
            merge: Regex::new(&format!(
                r"(?i)(?-u:\b)merge{ws}+request{ws}*[#!]?{ws}*([0-9]+)"
            ))
            .unwrap(),
            pull_request: Regex::new(&format!(
                r"(?i)(?-u:\b)pull{ws}+request{ws}*#?{ws}*([0-9]+)"
            ))
            .unwrap(),
            pr: Regex::new(&format!(r"(?i)(?-u:\b)pr{ws}*#?{ws}*([0-9]+)")).unwrap(),
            issue: Regex::new(&format!(r"(?i)(?-u:\b)issue{ws}*#?{ws}*([0-9]+)")).unwrap(),
            // R12: NO `(?i)` — uppercase-only so `gpt-4` never matches. `\d{1,7}`
            // → `[0-9]{1,7}`; the trailing `(?-u:\b)` rejects an 8th digit.
            ticket: Regex::new(r"(?-u:\b)([A-Z]{2,10})-([0-9]{1,7})(?-u:\b)").unwrap(),
            bare_hash: Regex::new(&format!(r"(?:^|{ws})#([0-9]+)(?-u:\b)")).unwrap(),
            collapse_ws: Regex::new(&format!(r"{ws}+")).unwrap(),
        }
    })
}

/// `taggedIdentifier` (`:66-68`). `type` ∈ `PR|MR|Issue`; `num`/`type` flow into
/// the label verbatim, and tokens are `[type.to_ascii_lowercase(), num]`.
fn tagged_identifier(kind: &str, num: &str) -> WorkIdentifier {
    WorkIdentifier {
        label: format!("{kind} {num}"),
        tokens: vec![kind.to_ascii_lowercase(), num.to_string()],
    }
}

/// `urlToIdentifier` (`:70-106`). WHATWG parse (invalid → `None`, JS `try/catch`),
/// http/https-only gate, then match provider path regexes in order against
/// `url.path()` (JS `url.pathname`).
fn url_to_identifier(raw: &str) -> Option<WorkIdentifier> {
    let url = Url::parse(raw).ok()?; // invalid URL → skip (C2)
    let scheme = url.scheme(); // already lowercased by the url crate (C2)
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let path = url.path(); // JS `url.pathname` (excludes query/fragment)
    let p = patterns();

    if let Some(c) = p.gitlab.captures(path) {
        return Some(if c[1].eq_ignore_ascii_case("merge_requests") {
            tagged_identifier("MR", &c[2])
        } else {
            tagged_identifier("Issue", &c[2])
        });
    }
    if let Some(c) = p.github.captures(path) {
        return Some(if c[1].eq_ignore_ascii_case("pull") {
            tagged_identifier("PR", &c[2])
        } else {
            tagged_identifier("Issue", &c[2])
        });
    }
    if let Some(c) = p.bitbucket_cloud.captures(path) {
        return Some(tagged_identifier("PR", &c[1]));
    }
    if let Some(c) = p.bitbucket_server.captures(path) {
        return Some(tagged_identifier("PR", &c[1]));
    }
    if let Some(c) = p.azure_devops.captures(path) {
        return Some(tagged_identifier("PR", &c[1]));
    }
    None
}

/// `findUrlIdentifier` (`:108-123`). Scan every URL in appearance order; for each,
/// strip trailing sentence punctuation / markdown emphasis (`R7`, keeps interior
/// `_` such as `merge_requests`), then try `url_to_identifier`. First success wins.
fn find_url_identifier(text: &str) -> Option<WorkIdentifier> {
    let p = patterns();
    for m in p.url_in_text.find_iter(text) {
        let raw = p.trailing_punct.replace(m.as_str(), "");
        if let Some(id) = url_to_identifier(&raw) {
            return Some(id);
        }
    }
    None
}

/// Truncate `text` to the first [`IDENTIFIER_SCAN_LIMIT`] Unicode scalars.
///
/// JS `.slice(0, 4096)` cuts on the 4096th UTF-16 code unit; we cut on the 4096th
/// `char` instead (C3). This is a documented divergence (the cut point differs for
/// text with astral characters), but it is only an input length bound — never a
/// semantic boundary — and it is char-boundary-safe, so it can never panic.
fn truncate_scan(text: &str) -> &str {
    match text.char_indices().nth(IDENTIFIER_SCAN_LIMIT) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

/// Pull the review-target identifier out of raw prompt text (`:130-168`).
///
/// Precedence runs most-reliable → least, verbatim (C1): provider URL →
/// `merge request` → `pull request`/`pr` → `issue` → namespaced ticket
/// (denylist-filtered) → bare `#123` → `None`. A higher stage wins and lower
/// stages are not even attempted.
pub fn extract_work_identifier(text: &str) -> Option<WorkIdentifier> {
    let scanned = truncate_scan(text);
    let p = patterns();

    // 1. Provider URL.
    if let Some(id) = find_url_identifier(scanned) {
        return Some(id);
    }

    // 2. merge request ("merge request !9").
    if let Some(c) = p.merge.captures(scanned) {
        return Some(tagged_identifier("MR", &c[1]));
    }

    // 3. pull request / pr ("pull request 500", "PR #1094", "pr123").
    if let Some(c) = p
        .pull_request
        .captures(scanned)
        .or_else(|| p.pr.captures(scanned))
    {
        return Some(tagged_identifier("PR", &c[1]));
    }

    // 4. issue ("issue 88").
    if let Some(c) = p.issue.captures(scanned) {
        return Some(tagged_identifier("Issue", &c[1]));
    }

    // 5. Namespaced ticket (Jira/Linear), uppercase-only; skip denylisted
    //    prefixes but keep scanning so a real key after one still resolves
    //    (`SHA-256 … ENG-456` → ENG-456).
    for c in p.ticket.captures_iter(scanned) {
        let prefix = &c[1];
        if !is_non_ticket_prefix(prefix) {
            return Some(WorkIdentifier {
                label: format!("{}-{}", &c[1], &c[2]),
                tokens: vec![c[1].to_ascii_lowercase(), c[2].to_string()],
            });
        }
    }

    // 6. Bare `#123` as a last resort.
    if let Some(c) = p.bare_hash.captures(scanned) {
        return Some(WorkIdentifier {
            label: format!("#{}", &c[1]),
            tokens: vec![c[1].to_string()],
        });
    }

    None
}

/// Compose an identifier-first label — `PR 1033 - Review`, or just `PR 1033` when
/// there is no trailing detail (`:175-177`).
///
/// Mirrors JS `detail ? \`${label} - ${detail}\` : label`: only the empty string
/// is falsy, so a whitespace-only `detail` (`"  "`) still takes the joined branch.
/// Hence `detail.is_empty()`, NOT `detail.trim().is_empty()`.
pub fn format_identifier_first(label: &str, detail: &str) -> String {
    if detail.is_empty() {
        label.to_string()
    } else {
        format!("{label} - {detail}")
    }
}

/// Remove the identifier's own tokens from a description (`:184-190`) so a caller
/// can prepend the label without echoing it.
///
/// **C4 hardening:** each token is `regex::escape()`d before building the
/// `\b<token>\b` strip pattern. Orca inserts tokens raw; because the exported
/// signature accepts arbitrary tokens, a metachar token would otherwise be
/// interpreted as a regex (injection / ReDoS / compile-throw). Escaping is
/// behaviorally identical for the extractor's metachar-free tokens
/// (`[a-z]+` / `[0-9]+`) and treats an arbitrary token literally — a deliberate,
/// documented divergence from Orca. `\b` is ASCII (`(?-u:\b)`, C1); the collapse
/// uses the JS `\s` set and the final trim uses JS `.trim()` semantics (C3).
pub fn strip_work_identifier_echo(text: &str, tokens: &[&str]) -> String {
    let mut stripped = text.to_string();
    for token in tokens {
        // `regex::escape` always yields a valid pattern, so the compile is
        // infallible (unlike Orca's raw-token `new RegExp`).
        let pattern = format!(r"(?i)(?-u:\b){}(?-u:\b)", regex::escape(token));
        let re = Regex::new(&pattern).expect("escaped token is always a valid regex");
        stripped = re.replace_all(&stripped, " ").into_owned();
    }
    let collapsed = patterns().collapse_ws.replace_all(&stripped, " ");
    js_trim(&collapsed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper mirroring `?.label` in the TS oracle.
    fn label(text: &str) -> Option<String> {
        extract_work_identifier(text).map(|id| id.label)
    }

    // ---- Oracle: extractWorkIdentifier (15 cases, ported verbatim) ----

    #[test]
    fn reads_a_github_pull_request_url() {
        assert_eq!(
            extract_work_identifier("Review https://github.com/EveryInc/plugin/pull/1033"),
            Some(WorkIdentifier {
                label: "PR 1033".into(),
                tokens: vec!["pr".into(), "1033".into()],
            })
        );
    }

    #[test]
    fn reads_a_bitbucket_cloud_pull_requests_url() {
        assert_eq!(
            label("Look at https://bitbucket.org/team/repo/pull-requests/77").as_deref(),
            Some("PR 77")
        );
    }

    #[test]
    fn reads_a_bitbucket_server_pull_requests_url() {
        assert_eq!(
            extract_work_identifier(
                "Review https://bitbucket.example.com/projects/ENG/repos/orca/pull-requests/1288"
            ),
            Some(WorkIdentifier {
                label: "PR 1288".into(),
                tokens: vec!["pr".into(), "1288".into()],
            })
        );
        // Personal (fork) repos live under /users instead of /projects.
        assert_eq!(
            label("see https://bitbucket.example.com/users/jane/repos/orca/pull-requests/9/overview")
                .as_deref(),
            Some("PR 9")
        );
    }

    #[test]
    fn reads_azure_devops_pull_request_urls() {
        assert_eq!(
            extract_work_identifier(
                "Look at https://dev.azure.com/contoso/Orca/_git/orca/pullrequest/4521"
            ),
            Some(WorkIdentifier {
                label: "PR 4521".into(),
                tokens: vec!["pr".into(), "4521".into()],
            })
        );
        // Query string is excluded from the pathname → the `$` branch matches.
        assert_eq!(
            label("https://contoso.visualstudio.com/Orca/_git/orca/pullrequest/4521?_a=files")
                .as_deref(),
            Some("PR 4521")
        );
    }

    #[test]
    fn reads_gitlab_merge_request_and_work_items_urls() {
        assert_eq!(
            extract_work_identifier("Check https://gitlab.com/group/app/-/merge_requests/42"),
            Some(WorkIdentifier {
                label: "MR 42".into(),
                tokens: vec!["mr".into(), "42".into()],
            })
        );
        assert_eq!(
            label("https://gitlab.example.com/g/p/-/work_items/9").as_deref(),
            Some("Issue 9")
        );
    }

    #[test]
    fn reads_an_issue_url() {
        assert_eq!(
            label("Fix https://github.com/o/r/issues/88").as_deref(),
            Some("Issue 88")
        );
    }

    #[test]
    fn ignores_urls_that_only_resemble_a_work_item() {
        // A CDN asset path contains `/pull/2023` but is not a pull request.
        assert_eq!(
            extract_work_identifier("Load https://cdn.vendor.com/assets/pull/2023/data.json"),
            None
        );
        // No trailing number after the item segment.
        assert_eq!(
            extract_work_identifier("see https://github.com/o/r/pull/notanumber"),
            None
        );
    }

    #[test]
    fn tolerates_trailing_punctuation_around_a_url() {
        assert_eq!(
            label("(see https://github.com/o/r/pull/5).").as_deref(),
            Some("PR 5")
        );
    }

    #[test]
    fn reads_a_url_wrapped_in_markdown_emphasis() {
        assert_eq!(
            label("Review _https://github.com/o/r/pull/5_ now").as_deref(),
            Some("PR 5")
        );
        assert_eq!(
            label("Review **https://github.com/o/r/pull/1094**").as_deref(),
            Some("PR 1094")
        );
    }

    #[test]
    fn reads_textual_references() {
        assert_eq!(label("please review PR #1094").as_deref(), Some("PR 1094"));
        assert_eq!(label("triage pull request 500").as_deref(), Some("PR 500"));
        assert_eq!(label("reproduce issue 12").as_deref(), Some("Issue 12"));
        assert_eq!(label("handle merge request !9").as_deref(), Some("MR 9"));
    }

    #[test]
    fn reads_a_namespaced_ticket_id_bare() {
        assert_eq!(
            extract_work_identifier("implement ENG-456 login flow"),
            Some(WorkIdentifier {
                label: "ENG-456".into(),
                tokens: vec!["eng".into(), "456".into()],
            })
        );
    }

    #[test]
    fn does_not_treat_standards_ciphers_or_encodings_as_tickets() {
        assert_eq!(extract_work_identifier("implement SHA-256 hashing"), None);
        assert_eq!(extract_work_identifier("parse UTF-8 input"), None);
        assert_eq!(extract_work_identifier("handle ISO-8601 dates"), None);
    }

    #[test]
    fn skips_a_denylisted_prefix_but_finds_a_real_key_after_it() {
        assert_eq!(
            label("encrypt with AES-256 for ticket ENG-99").as_deref(),
            Some("ENG-99")
        );
    }

    #[test]
    fn prefers_a_provider_url_over_an_incidental_ticket_shaped_token() {
        assert_eq!(
            label("per RFC-2616 notes, review https://github.com/o/r/pull/7").as_deref(),
            Some("PR 7")
        );
    }

    #[test]
    fn falls_back_to_a_bare_number_then_to_null() {
        assert_eq!(label("look at #321 when free").as_deref(), Some("#321"));
        assert_eq!(
            extract_work_identifier("add a dark mode toggle to settings"),
            None
        );
    }

    // ---- Oracle: stripWorkIdentifierEcho (2 cases) ----

    #[test]
    fn strip_removes_the_identifier_tokens_from_a_description() {
        assert_eq!(
            strip_work_identifier_echo("Review this community PR", &["pr", "1094"]),
            "Review this community"
        );
    }

    #[test]
    fn strip_removes_a_ticket_key_echoed_in_the_description() {
        assert_eq!(
            strip_work_identifier_echo("Fix ENG 456 crash", &["eng", "456"]),
            "Fix crash"
        );
    }

    // ---- Codex pins (E-fmt, E-digit, E-url, E-8digit, E-strip, E-slice) ----

    /// E-fmt: `format_identifier_first` has ZERO oracle coverage. Both branches +
    /// the JS falsy rule (empty is falsy, whitespace-only is truthy).
    #[test]
    fn e_fmt_both_branches() {
        assert_eq!(format_identifier_first("PR 5", "Review"), "PR 5 - Review");
        assert_eq!(format_identifier_first("PR 5", ""), "PR 5");
        // "  " is truthy in JS → joined branch (NOT trimmed to the label).
        assert_eq!(format_identifier_first("PR 5", "  "), "PR 5 -   ");
    }

    /// E-digit (the load-bearing C1 `\d` lock): non-ASCII digits must NOT match.
    /// A Unicode-`\d` port would wrongly treat these as tickets / bare numbers.
    #[test]
    fn e_digit_non_ascii_numerals_do_not_match() {
        // Arabic-Indic digits after a ticket-shaped prefix.
        assert_eq!(extract_work_identifier("JIRA-\u{0661}\u{0662}\u{0663}"), None);
        // Arabic-Indic digits after a bare `#`.
        assert_eq!(extract_work_identifier("#\u{0664}\u{0665}"), None);
        // Full-width digits likewise stay out of `issue` matches.
        assert_eq!(extract_work_identifier("issue \u{FF11}\u{FF12}"), None);
        // `pr` and the URL path regexes have NO trailing `(?-u:\b)` guard, so their
        // `[0-9]` ASCII lock is the SOLE defense against a non-ASCII digit (review
        // N1). A `[0-9]->\d` regression at either site would leak here.
        assert_eq!(extract_work_identifier("pr \u{FF11}\u{FF12}"), None);
        assert_eq!(
            extract_work_identifier("https://github.com/o/r/pull/\u{0661}\u{0662}\u{0663}"),
            None
        );
    }

    /// E-url: multi-URL fallback loop, `http://`, trailing-slash pathname, and the
    /// protocol-casing behavior (the `url` crate lowercases the scheme, matching
    /// JS). All assert the ACTUAL Rust behavior.
    #[test]
    fn e_url_fallback_loop_http_and_casing() {
        // First URL (CDN) fails path validation; the loop tries the second.
        assert_eq!(
            label(
                "Load https://cdn.vendor.com/assets/pull/2023/data.json \
                 then https://github.com/o/r/pull/7"
            )
            .as_deref(),
            Some("PR 7")
        );
        // Non-TLS http:// is accepted (`:77` allows http:).
        assert_eq!(
            label("see http://github.com/o/r/pull/5").as_deref(),
            Some("PR 5")
        );
        // Trailing-slash pathname: `/o/r/pull/5/` still resolves via the `[/?#]`
        // branch (url crate preserves the trailing slash, like JS).
        assert_eq!(
            label("https://github.com/o/r/pull/5/").as_deref(),
            Some("PR 5")
        );
        // Uppercase scheme: url crate lowercases it → still recognized as https.
        assert_eq!(
            label("HTTPS://github.com/o/r/pull/9").as_deref(),
            Some("PR 9")
        );
    }

    /// E-url (invalid-URL skip, C2): an unparseable URL candidate is skipped like
    /// JS `try/catch`, not an error/panic — the loop falls through to the next,
    /// valid URL. `https://%/x` has an invalid percent-encoded host.
    #[test]
    fn e_url_invalid_url_is_skipped() {
        assert_eq!(
            label("bad https://%/x then https://github.com/o/r/pull/8").as_deref(),
            Some("PR 8")
        );
    }

    /// E-8digit: `[0-9]{1,7}` + trailing `\b` boundary. A 7-digit ticket resolves;
    /// an 8-digit number is rejected (the trailing `\b` fails between two digits).
    #[test]
    fn e_8digit_boundary() {
        assert_eq!(
            label("ticket ENG-1234567 please").as_deref(),
            Some("ENG-1234567")
        );
        assert_eq!(extract_work_identifier("ticket ENG-12345678 please"), None);
    }

    /// E-strip (C4): a metachar token is treated LITERALLY (escaped), so `a.b`
    /// does NOT strip `axb`, but does strip a literal `a.b`.
    #[test]
    fn e_strip_metachar_token_is_literal() {
        // `.` is escaped → matches only a literal `a.b`, never `axb`.
        assert_eq!(strip_work_identifier_echo("axb here", &["a.b"]), "axb here");
        assert_eq!(strip_work_identifier_echo("a.b here", &["a.b"]), "here");
    }

    /// E-slice: a >4096-char input with non-ASCII around the cut must never panic
    /// (char-boundary-safe), and content beyond the scan cap is not seen.
    #[test]
    fn e_slice_char_boundary_safe() {
        // 5000 Korean chars (3 bytes each) then a ticket well past char 4096.
        let mut s = "\u{AC00}".repeat(5000);
        s.push_str("ENG-123");
        // Must not panic; the ticket is beyond the 4096-scalar scan cap → None.
        assert_eq!(extract_work_identifier(&s), None);

        // And an identifier inside the cap is still found (cut lands in non-ASCII).
        let mut s2 = "ENG-7 ".to_string();
        s2.push_str(&"\u{AC00}".repeat(5000));
        assert_eq!(label(&s2).as_deref(), Some("ENG-7"));
    }

    /// pr123 (no separator) still matches via `\bpr\s*#?\s*(\d+)` (research E8).
    #[test]
    fn pr_without_separator_matches() {
        assert_eq!(label("landed pr123 yesterday").as_deref(), Some("PR 123"));
    }
}
