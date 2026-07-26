//! GitHub issue/PR link parsing — a verbatim port of Orca's
//! `src/shared/github-links.ts` (@ v1.4.146-rc.0, 102 lines).
//!
//! This is a **path-shape matcher, not a GitHub matcher** (P3): the host is
//! never checked, so `https://git.corp.com/MyOrg/my_repo/pull/395` is
//! accepted exactly like a `github.com` URL — only [`build_github_repo_url`]
//! hardcodes `github.com` (P12). Do not "harden" the parser with a host
//! check; that would be a behavior change, not a bug fix.
//!
//! # Scope (plan M1 of 2)
//! Only `github-links.ts` (102L) is ported here. The companion detector
//! (`terminal-github-pr-link-detector.ts`, 174L) lands in a separate M2 PR —
//! its only runtime import from this module is [`parse_github_issue_or_pr_link`].
//!
//! # Public surface (5 items)
//! - [`RepoSlug`] — an owner/repo pair, case preserved.
//! - [`GitHubItemKind`] — `Issue` | `Pr` (JS `'issue' | 'pr'`; renamed from the
//!   reserved word `type`).
//! - [`GitHubIssueOrPRLink`] — a parsed slug + number + kind.
//! - [`build_github_repo_url`] — `buildGitHubRepoUrl` (`:17-22`).
//! - [`parse_github_issue_or_pr_number`] — `parseGitHubIssueOrPRNumber` (`:37-65`).
//! - [`parse_github_issue_or_pr_link`] — `parseGitHubIssueOrPRLink` (`:71-102`).
//!
//! **Not ported**: `normalizeGitHubLinkQuery` (renderer-only wrapper, tested at
//! `github-links.test.ts:132-183`) is out of scope for this shared module —
//! see the plan's M1/M2 split.
//!
//! # Documented divergences from Orca (plan decisions P1-P13)
//! - **P1 — ASCII-only case fold, hand-rolled (no `regex`), chosen for
//!   faithfulness rather than because a divergence is reachable here.**
//!   `GH_ITEM_PATH_RE` carries JS `/i` **without** `/u`, so ECMAScript's
//!   non-Unicode `Canonicalize` folds ASCII only. A case-sensitive compare
//!   would be a real bug — `PULL`/`Issues` **must** match `pull`/`issues`,
//!   and that direction of the fold is genuinely load-bearing and pinned by
//!   `p1_route_case_variants_accepted_and_unicode_fold_rejected`. What is
//!   NOT reachable is the ASCII-vs-Unicode *distinction* itself, for two
//!   independent reasons:
//!   1. WHATWG `URL` percent-encodes any code point > U+007E in the path
//!      (confirmed against Node: `new URL(...).pathname` turns `iſſueſ`
//!      into `i%C5%BF%C5%BFue%C5%BF`, in BOTH JS and the `url` crate), so a
//!      non-ASCII route segment can never reach the comparison as literal
//!      letters through the public URL-based API — see
//!      `p1_unicode_case_fold_rejected_at_path_matcher_level`, which calls
//!      the matcher directly (bypassing `Url`) to even construct the input.
//!   2. **Even bypassing the URL parser entirely, no input can distinguish
//!      ASCII folding from Unicode folding here**, because the only two
//!      words ever compared are `issues` and `pull`, and no character's
//!      Unicode lowercase produces a letter outside `{i,s,u,e,p,l}` from one
//!      outside that alphabet: `ſ` (U+017F) lowercases to itself, not `s`;
//!      `K` (U+212A, KELVIN SIGN) lowercases to `k`, which appears in
//!      neither word. Verified by mutation: swapping `eq_ignore_ascii_case`
//!      for a `to_lowercase()` comparison kills no test, and cannot, since
//!      the two are extensionally equal on this alphabet. `eq_ignore_ascii_case`
//!      is still the right choice — it's the literal ASCII-only fold JS
//!      performs, and it's defensive against a future third route word that
//!      could make the distinction observable — but it is not fixing an
//!      observable bug in the two current words. (This is also why we
//!      didn't reach for `regex`'s `(?i)` here even though it's available in
//!      the workspace: `(?i)` is Unicode-simple-case-folding, and reaching
//!      for it would trade an explicit, auditable ASCII fold for an
//!      implicit Unicode one with no compensating benefit.)
//! - **P2 — ASCII-only digits.** JS `\d` is always `[0-9]`; Rust `regex`'s
//!   `\d` defaults to Unicode `Nd` (Arabic-Indic `٤٢`, full-width `４２`).
//!   The hand-rolled matcher checks `is_ascii_digit` directly. The bare-number
//!   fast path (P4) is a raw string with no URL involved, so it genuinely
//!   exercises this; the URL-path digit check has the same percent-encoding
//!   caveat as P1 above (a non-ASCII digit in a URL is invisible to the
//!   matcher for an unrelated reason) — see the two P2 tests below.
//! - **P3 — host is never checked.** See the module doc above.
//! - **P4/P5 — the two functions differ ONLY by a bare-number fast path.**
//!   [`parse_github_issue_or_pr_number`] accepts `"42"` / `"#42"`;
//!   [`parse_github_issue_or_pr_link`] does not (`"42"` -> `None`). Only ONE
//!   leading `#` is stripped, so `"##42"` fails the all-digit test and falls
//!   through to URL parsing (no scheme -> `None`). `owner/repo#123` is not a
//!   supported form (no such path shape exists in the matcher).
//! - **P6 — `parseInt` never fails; we saturate instead of erroring.** JS
//!   `Number.parseInt("99999999999999999999", 10)` yields `1e20` (passes the
//!   `> 0` gate); a 309+ digit string yields `Infinity` (also `> 0`). Rust
//!   `u64::from_str` would `Err` on either, which diverges (the link would be
//!   silently dropped instead of parsed with a huge number). **Zero oracle
//!   coverage** on this branch. We pick a **saturating `u64`** policy
//!   (pragmatic, keeps the `number` field an actual integer instead of an
//!   `f64` that is exact only up to 2^53): every digit is folded in with
//!   `saturating_mul(10).saturating_add(digit)`, so an overflowing digit
//!   string clamps to `u64::MAX` rather than erroring. Pinned in
//!   `p6_overflow_saturates_to_u64_max`.
//! - **P7 — the numeric gate is `> 0`.** `0`, `#0`, `/pull/0` all -> `None`.
//!   Radix is explicitly 10 (no octal): `"007"` -> `7`, even though the
//!   original URL string keeps the leading zeros.
//! - **P8 — ALL trailing slashes are stripped** before matching
//!   (`/\/+$/`, anchored), so `/923///` -> `923`. A pathname of exactly `/`
//!   becomes empty and fails to match (no leading `/` left for the anchor).
//! - **P9 — matches against `url.path()`** (JS `url.pathname`), not the raw
//!   input, so query/fragment are excluded by construction and percent
//!   encoding is already applied by the WHATWG parser.
//! - **P10 — owner/repo are returned VERBATIM, case preserved.** The
//!   lowercase comparison decides `pull` vs `issues` ONLY; `MyOrg`/`my_repo`
//!   come back unchanged.
//! - **P11 — both `.trim()` call sites are ECMAScript whitespace semantics**
//!   -> [`suaegi_misc::js_trim`], never `str::trim` (U+FEFF/U+0085 disagree
//!   in opposite directions).
//! - **P12 — `build_github_repo_url` hardcodes `https://github.com/`**,
//!   ignoring GHE entirely, and percent-encodes both segments with a locally
//!   copied `encode_uri_component` (copied per charter from
//!   `suaegi-forge/src/repo_icon.rs`, not reused cross-crate — it is a
//!   private helper there). The guard is a **falsy** check: `None` slug,
//!   empty owner, or empty repo all yield `None`.
//! - **P13 — `url` crate mapping.** `new URL(x)` -> `Url::parse(x).ok()?`;
//!   `url.protocol !== 'https:' && !== 'http:'` -> `scheme() != "https" &&
//!   != "http"` (the crate already lowercases the scheme); `url.pathname` ->
//!   `url.path()`.
//!
//! No `regex`, no `serde` (plan §1/crate charter).

use suaegi_misc::js_trim;
use url::Url;

/// An owner/repo slug. Both fields are case-preserved verbatim (P10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSlug {
    pub owner: String,
    pub repo: String,
}

/// Mirrors JS `'issue' | 'pr'` (renamed from the reserved word `type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubItemKind {
    Issue,
    Pr,
}

/// Mirrors Orca's `GitHubIssueOrPRLink` (`:11-15`). The `type` field is named
/// `kind` here since `type` is a Rust keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubIssueOrPRLink {
    pub slug: RepoSlug,
    pub number: u64,
    pub kind: GitHubItemKind,
}

/// `encodeURIComponent`'s unreserved set: `A-Za-z0-9 - _ . ! ~ * ' ( )`.
/// Copied locally (not reused from `suaegi-forge::repo_icon`, which is
/// private there) per crate charter — P12.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `buildGitHubRepoUrl` (`:17-22`). Hardcodes `https://github.com/`,
/// ignoring GHE (P12). Guard is falsy: `None` slug, empty owner, or empty
/// repo all yield `None`.
pub fn build_github_repo_url(slug: Option<&RepoSlug>) -> Option<String> {
    let slug = slug?;
    if slug.owner.is_empty() || slug.repo.is_empty() {
        return None;
    }
    Some(format!(
        "https://github.com/{}/{}",
        encode_uri_component(&slug.owner),
        encode_uri_component(&slug.repo)
    ))
}

/// `true` iff `s` is non-empty and every byte is an ASCII digit — the JS
/// `/^\d+$/.test(value)` check (P2: ASCII-only, never Unicode `Nd`).
fn is_all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Fold a decimal digit string into a `u64`, saturating on overflow instead
/// of failing (P6). Every byte of `digits` must be an ASCII digit (callers
/// only ever pass matcher-verified digit runs).
fn parse_decimal_digits_saturating(digits: &str) -> u64 {
    let mut acc: u64 = 0;
    for b in digits.bytes() {
        let digit = u64::from(b - b'0');
        acc = acc.saturating_mul(10).saturating_add(digit);
    }
    acc
}

/// `parseGitHubItemNumber` (`:28-31`). Radix is fixed at 10 (no octal, so
/// `"007"` -> `7`), and the gate is `> 0` (P7), so `"0"` -> `None`.
fn parse_github_item_number(value: &str) -> Option<u64> {
    let parsed = parse_decimal_digits_saturating(value);
    if parsed > 0 {
        Some(parsed)
    } else {
        None
    }
}

/// `GH_ITEM_PATH_RE` (`:4`), hand-rolled as a 4-token `/`-split instead of a
/// regex (P1/P2). `path` is the parsed URL's pathname (P9); trailing slashes
/// are stripped first (P8). Returns `(owner, repo, route, digits)` with
/// `route` one of `"issues"`/`"pull"` case-INSENSITIVELY (compared with
/// `eq_ignore_ascii_case`, never `(?i)`), and `digits` the verbatim leading
/// digit run of the 4th segment (leading zeros intact, P7 handled by the
/// caller).
fn match_github_item_path(path: &str) -> Option<(&str, &str, &str, &str)> {
    // `/\/+$/` — strip ALL trailing slashes (P8). A bare "/" strips to "",
    // which then fails the `strip_prefix('/')` below, matching JS (the
    // anchored `^/` has nothing left to match).
    let stripped = path.trim_end_matches('/');
    let rest = stripped.strip_prefix('/')?;

    // `([^/]+)/([^/]+)/(issues|pull)/(\d+)(?:/.*)?` — the 4th piece from
    // `splitn` keeps everything after the 3rd '/' unsplit, which is exactly
    // the `(\d+)(?:/.*)?` tail we need to re-scan below.
    let mut parts = rest.splitn(4, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let route = parts.next()?;
    let tail = parts.next()?;

    // `[^/]+` requires at least one non-slash char; `splitn` can still yield
    // an empty piece (e.g. a doubled slash), which the regex would reject.
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    if !route.eq_ignore_ascii_case("issues") && !route.eq_ignore_ascii_case("pull") {
        return None;
    }

    // `(\d+)` — the longest all-ASCII-digit prefix of `tail`.
    let digit_end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    let digits = &tail[..digit_end];
    if digits.is_empty() {
        return None;
    }
    // `(?:/.*)?$` — after the digit run, either end-of-string or a '/'.
    let after = &tail[digit_end..];
    if !after.is_empty() && !after.starts_with('/') {
        return None;
    }

    Some((owner, repo, route, digits))
}

/// Parses a GitHub issue/PR reference from plain input. Supports issue/PR
/// numbers (e.g. `"42"`), `"#42"`, and full GitHub-shaped URLs.
///
/// `parseGitHubIssueOrPRNumber` (`:37-65`).
pub fn parse_github_issue_or_pr_number(input: &str) -> Option<u64> {
    let trimmed = js_trim(input); // P11
    if trimmed.is_empty() {
        return None;
    }

    // Only ONE leading '#' is stripped (P4/P5): "##42" keeps a '#' in
    // `numeric`, fails the all-digit test, and falls through to URL parsing
    // (no scheme -> None).
    let numeric = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if is_all_ascii_digits(numeric) {
        return parse_github_item_number(numeric);
    }

    let url = Url::parse(trimmed).ok()?; // P13: try/catch -> Option
    let scheme = url.scheme(); // already lowercased by the `url` crate
    if scheme != "https" && scheme != "http" {
        return None;
    }

    let (_, _, _, digits) = match_github_item_path(url.path())?;
    parse_github_item_number(digits)
}

/// Parses an owner/repo slug plus issue/PR number from a GitHub-shaped URL.
/// Returns `None` for anything that isn't a recognizable issue/pull path
/// (P3: host is never checked). Unlike
/// [`parse_github_issue_or_pr_number`], a bare number is NOT accepted (P4).
///
/// `parseGitHubIssueOrPRLink` (`:71-102`).
pub fn parse_github_issue_or_pr_link(input: &str) -> Option<GitHubIssueOrPRLink> {
    let trimmed = js_trim(input); // P11
    if trimmed.is_empty() {
        return None;
    }

    let url = Url::parse(trimmed).ok()?; // P13
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        return None;
    }

    let (owner, repo, route, digits) = match_github_item_path(url.path())?;
    let number = parse_github_item_number(digits)?;
    // `match[3].toLowerCase() === 'pull'` decides the kind ONLY; owner/repo
    // stay verbatim (P10).
    let kind = if route.eq_ignore_ascii_case("pull") {
        GitHubItemKind::Pr
    } else {
        GitHubItemKind::Issue
    };

    Some(GitHubIssueOrPRLink {
        slug: RepoSlug {
            owner: owner.to_string(),
            repo: repo.to_string(),
        },
        number,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(owner: &str, repo: &str) -> RepoSlug {
        RepoSlug {
            owner: owner.to_string(),
            repo: repo.to_string(),
        }
    }

    // ==== Oracle: github-links.test.ts:10-130, ported verbatim ====
    // (:132-183 tests `normalizeGitHubLinkQuery`, a renderer-only wrapper —
    // out of scope for this shared module per the plan's M1/M2 split.)

    #[test]
    fn oracle_builds_a_github_repository_url_from_an_owner_repo_slug() {
        assert_eq!(
            build_github_repo_url(Some(&slug("stablyai", "orca"))),
            Some("https://github.com/stablyai/orca".to_string())
        );
    }

    #[test]
    fn oracle_encodes_path_segments() {
        assert_eq!(
            build_github_repo_url(Some(&slug("stably ai", "orca/tools"))),
            Some("https://github.com/stably%20ai/orca%2Ftools".to_string())
        );
    }

    #[test]
    fn oracle_parses_plain_issue_numbers_and_github_pull_request_urls() {
        assert_eq!(parse_github_issue_or_pr_number("42"), Some(42));
        assert_eq!(parse_github_issue_or_pr_number("#42"), Some(42));
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/stablyai/orca/pull/123"),
            Some(123)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/stablyai/orca/issues/923"),
            Some(923)
        );
        assert_eq!(
            parse_github_issue_or_pr_number(
                "https://github.my-company.net/MyOrg/my_repo/pull/395"
            ),
            Some(395)
        );
    }

    #[test]
    fn oracle_parses_github_item_urls_with_trailing_page_segments() {
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/1965/changes"),
            Some(1965)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/1965/files"),
            Some(1965)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/1965/commits"),
            Some(1965)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/issues/923/comments"),
            Some(923)
        );
    }

    #[test]
    fn oracle_parses_trailing_segments_with_query_fragment_and_repeated_slashes() {
        assert_eq!(
            parse_github_issue_or_pr_number(
                "https://github.com/o/r/pull/1965/changes?diff=split"
            ),
            Some(1965)
        );
        assert_eq!(
            parse_github_issue_or_pr_number(
                "https://github.com/o/r/issues/923/comments#issuecomment-1"
            ),
            Some(923)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/1965//changes///"),
            Some(1965)
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/issues/923///"),
            Some(923)
        );
    }

    #[test]
    fn oracle_rejects_invalid_github_item_urls_for_number_parsing() {
        assert_eq!(parse_github_issue_or_pr_number("0"), None);
        assert_eq!(parse_github_issue_or_pr_number("#0"), None);
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/0"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/not-a-number/changes"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/issues/123abc"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/owner/repo/pulls/123"),
            None
        );
    }

    #[test]
    fn oracle_parses_slug_number_and_type_for_direct_item_urls() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/stablyai/orca/pull/123"),
            Some(GitHubIssueOrPRLink {
                slug: slug("stablyai", "orca"),
                number: 123,
                kind: GitHubItemKind::Pr,
            })
        );

        assert_eq!(
            parse_github_issue_or_pr_link(
                "https://github.my-company.net/MyOrg/my_repo/pull/395"
            ),
            Some(GitHubIssueOrPRLink {
                slug: slug("MyOrg", "my_repo"),
                number: 395,
                kind: GitHubItemKind::Pr,
            })
        );

        // P3: host is never checked — a non-github.com host with the same
        // path shape is accepted.
        assert_eq!(
            parse_github_issue_or_pr_link("https://git.corp.com/MyOrg/my_repo/pull/395"),
            Some(GitHubIssueOrPRLink {
                slug: slug("MyOrg", "my_repo"),
                number: 395,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/stablyai/orca/issues/923"),
            Some(GitHubIssueOrPRLink {
                slug: slug("stablyai", "orca"),
                number: 923,
                kind: GitHubItemKind::Issue,
            })
        );
    }

    #[test]
    fn oracle_derives_item_type_from_the_route_segment_when_trailing_segments_are_present() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/1965/changes"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 1965,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/issues/923/comments"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 923,
                kind: GitHubItemKind::Issue,
            })
        );
    }

    #[test]
    fn oracle_accepts_query_fragment_and_repeated_trailing_slashes() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/1965/files?plain=1#diff"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 1965,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/issues/923/comments///"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 923,
                kind: GitHubItemKind::Issue,
            })
        );
    }

    #[test]
    fn oracle_rejects_non_github_and_malformed_item_urls() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/0"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/issues/0"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/not-a-number/changes"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/issues/123abc"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/owner/repo/pulls/123"),
            None
        );
    }

    // ==== Additional pins (oracle-silent branches, plan §3) ====

    /// P1: all four route-case variants are accepted (case-insensitive route
    /// match), but a route containing `ſ` (U+017F, LATIN SMALL LETTER LONG S)
    /// is rejected — proving the fold is ASCII-only, not Unicode-simple.
    #[test]
    fn p1_route_case_variants_accepted_and_unicode_fold_rejected() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/PULL/42"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 42,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/Pull/42"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 42,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/ISSUES/42"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 42,
                kind: GitHubItemKind::Issue,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/Issues/42"),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 42,
                kind: GitHubItemKind::Issue,
            })
        );
    }

    /// P1 crux pin — 'ſ' (U+017F) must NOT ASCII-fold to 's'. This calls the
    /// path matcher DIRECTLY on a raw string, not through a `Url`, because
    /// WHATWG `URL` percent-encodes every code point > U+007E in the path
    /// (verified empirically against Node: `new URL('https://github.com/o/r/
    /// iſſueſ/42').pathname` is `"/o/r/i%C5%BF%C5%BFue%C5%BF/42"`, in BOTH
    /// JS and the `url` crate). That means a URL-routed test of this input
    /// rejects for an unrelated reason (percent-encoded bytes never equal
    /// "issues" under ANY fold) and can never distinguish an ASCII-only fold
    /// from a Unicode-simple fold — a mutation swapping
    /// `eq_ignore_ascii_case` for a Unicode-aware compare would be
    /// UNDETECTABLE by any URL-based test. Exercising the matcher directly is
    /// the only way to pin the actual regex-equivalent semantics.
    #[test]
    fn p1_unicode_case_fold_rejected_at_path_matcher_level() {
        assert_eq!(
            match_github_item_path("/o/r/i\u{017F}\u{017F}ue\u{017F}/42"),
            None
        );
        assert_eq!(match_github_item_path("/o/r/ISSUE\u{017F}/42"), None);
        // Sanity: the ASCII-only counterparts still match at this same level.
        assert_eq!(
            match_github_item_path("/o/r/ISSUES/42"),
            Some(("o", "r", "ISSUES", "42"))
        );
    }

    /// P2: Arabic-Indic and full-width digits are NOT ASCII digits and must
    /// be rejected in the bare-number fast path (a raw string, so this
    /// genuinely exercises `is_all_ascii_digits` with no URL layer involved).
    #[test]
    fn p2_non_ascii_digits_rejected_in_bare_number_path() {
        // Crux pin: Arabic-Indic '٤٢' (U+0664 U+0662).
        assert_eq!(parse_github_issue_or_pr_number("\u{0664}\u{0662}"), None);
        // Full-width digits '４２' (U+FF14 U+FF12).
        assert_eq!(parse_github_issue_or_pr_number("\u{FF14}\u{FF12}"), None);
    }

    /// P2 crux pin — same rationale as the P1 pin above: WHATWG `URL`
    /// percent-encodes Arabic-Indic/full-width digits (code points > U+007E)
    /// before the path matcher ever runs (verified against Node: `new
    /// URL('https://github.com/o/r/pull/٤٢').pathname` is
    /// `"/o/r/pull/%D9%A4%D9%A2"`), so a URL-routed test of non-ASCII digits
    /// rejects for the unrelated reason that '%' isn't a digit at all — it
    /// can never distinguish `is_ascii_digit` from a Unicode `Nd` check. Call
    /// the matcher directly on a raw path to pin the real semantics.
    #[test]
    fn p2_non_ascii_digits_rejected_at_path_matcher_level() {
        assert_eq!(
            match_github_item_path("/o/r/pull/\u{0664}\u{0662}"),
            None
        );
        assert_eq!(
            match_github_item_path("/o/r/pull/\u{FF14}\u{FF12}"),
            None
        );
        // Sanity: ASCII digits still match at this same level.
        assert_eq!(
            match_github_item_path("/o/r/pull/42"),
            Some(("o", "r", "pull", "42"))
        );
    }

    /// Real-world consistency check (NOT a P1/P2 crux pin — see the two
    /// tests above for why): feeding the same non-ASCII text through the
    /// PUBLIC, URL-based API also rejects, just for the unrelated reason of
    /// WHATWG percent-encoding rather than case-fold or digit-class
    /// semantics. Pinned so a future contributor doesn't "fix" the encoding
    /// away and accidentally paper over the real P1/P2 hazard.
    #[test]
    fn zero_coverage_non_ascii_in_url_rejected_via_percent_encoding() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/i\u{017F}\u{017F}ue\u{017F}/42"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/\u{0664}\u{0662}"),
            None
        );
    }

    /// P6: zero oracle coverage on integer overflow. We saturate to
    /// `u64::MAX` rather than failing to parse (documented policy above).
    #[test]
    fn p6_overflow_saturates_to_u64_max() {
        // Crux pin: 20-digit number (JS: 1e20).
        let twenty_nines = "9".repeat(20);
        assert_eq!(
            parse_github_issue_or_pr_number(&twenty_nines),
            Some(u64::MAX)
        );
        assert_eq!(
            parse_github_issue_or_pr_link(&format!(
                "https://github.com/o/r/pull/{twenty_nines}"
            )),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: u64::MAX,
                kind: GitHubItemKind::Pr,
            })
        );

        // 309-digit number (JS: Infinity).
        let three_hundred_nine_nines = "9".repeat(309);
        assert_eq!(
            parse_github_issue_or_pr_number(&three_hundred_nine_nines),
            Some(u64::MAX)
        );
    }

    /// P7: radix is fixed at 10 (no octal) so "007" -> 7, while the URL
    /// string itself keeps the leading zeros; and the `> 0` gate rejects
    /// "0", "#0", and "/pull/0".
    #[test]
    fn p7_leading_zeros_parse_as_decimal_and_zero_is_rejected() {
        assert_eq!(parse_github_issue_or_pr_number("007"), Some(7));
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/007"),
            Some(7)
        );
        assert_eq!(parse_github_issue_or_pr_number("0"), None);
        assert_eq!(parse_github_issue_or_pr_number("#0"), None);
        assert_eq!(
            parse_github_issue_or_pr_number("https://github.com/o/r/pull/0"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/o/r/pull/0"),
            None
        );
    }

    /// Unparseable URLs (JS `new URL()` throw -> `try/catch` -> `null`):
    /// no scheme at all, and a string that isn't a URL by any grammar.
    #[test]
    fn zero_coverage_unparseable_urls_rejected() {
        assert_eq!(parse_github_issue_or_pr_number("not a url at all"), None);
        assert_eq!(parse_github_issue_or_pr_link("not a url at all"), None);
        assert_eq!(parse_github_issue_or_pr_number("::::"), None);
        assert_eq!(parse_github_issue_or_pr_link("::::"), None);
    }

    /// Non-http(s) schemes are rejected by the protocol gate.
    #[test]
    fn zero_coverage_non_http_schemes_rejected() {
        assert_eq!(
            parse_github_issue_or_pr_link("ftp://github.com/o/r/pull/1"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("file:///o/r/pull/1"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_link("javascript:alert(1)"),
            None
        );
        assert_eq!(
            parse_github_issue_or_pr_number("ftp://github.com/o/r/pull/1"),
            None
        );
    }

    /// Empty-after-trim input is rejected before any URL parsing is attempted.
    #[test]
    fn zero_coverage_empty_after_trim_rejected() {
        assert_eq!(parse_github_issue_or_pr_number(""), None);
        assert_eq!(parse_github_issue_or_pr_number("   "), None);
        assert_eq!(parse_github_issue_or_pr_link(""), None);
        assert_eq!(parse_github_issue_or_pr_link("   "), None);
    }

    /// `build_github_repo_url`: `None` slug, empty owner, and empty repo all
    /// yield `None` (the falsy guard, P12).
    #[test]
    fn zero_coverage_build_repo_url_falsy_guard() {
        assert_eq!(build_github_repo_url(None), None);
        // Crux pin: empty owner.
        assert_eq!(build_github_repo_url(Some(&slug("", "orca"))), None);
        assert_eq!(build_github_repo_url(Some(&slug("stablyai", ""))), None);
        assert_eq!(build_github_repo_url(Some(&slug("", ""))), None);
    }

    /// P11: U+FEFF (BOM) is ECMAScript whitespace and must be trimmed; U+0085
    /// (NEL) is NOT and must be preserved (in both directions this diverges
    /// from Rust's `str::trim`, which would do the opposite for each).
    #[test]
    fn p11_ecmascript_trim_feff_and_nel() {
        assert_eq!(
            parse_github_issue_or_pr_number("\u{FEFF}42\u{FEFF}"),
            Some(42)
        );
        // U+0085 is preserved by js_trim, so the leading '#42' is not a
        // pure digit string in the fast path, and prepending it to a URL
        // breaks the scheme -- both routes must reject it.
        assert_eq!(parse_github_issue_or_pr_number("\u{0085}42\u{0085}"), None);
        assert_eq!(
            parse_github_issue_or_pr_link(
                "\u{FEFF}https://github.com/o/r/pull/5\u{FEFF}"
            ),
            Some(GitHubIssueOrPRLink {
                slug: slug("o", "r"),
                number: 5,
                kind: GitHubItemKind::Pr,
            })
        );
        assert_eq!(
            parse_github_issue_or_pr_link(
                "\u{0085}https://github.com/o/r/pull/5\u{0085}"
            ),
            None
        );
    }

    /// P12: encoding pin, mirroring the two oracle cases directly (space and
    /// slash both need percent-encoding, distinctly from each other).
    #[test]
    fn p12_encodes_space_and_slash_distinctly() {
        assert_eq!(
            build_github_repo_url(Some(&slug("stably ai", "orca/tools"))),
            Some("https://github.com/stably%20ai/orca%2Ftools".to_string())
        );
    }

    /// P4/P5: the link parser rejects a bare number that the number parser
    /// accepts; `owner/repo#123` is not a supported form; and only one
    /// leading '#' is stripped.
    #[test]
    fn p4_p5_link_parser_rejects_bare_number_and_double_hash() {
        assert_eq!(parse_github_issue_or_pr_link("42"), None);
        assert_eq!(parse_github_issue_or_pr_number("42"), Some(42));

        assert_eq!(parse_github_issue_or_pr_number("acme/orca#42"), None);
        assert_eq!(parse_github_issue_or_pr_link("acme/orca#42"), None);

        assert_eq!(parse_github_issue_or_pr_number("##42"), None);
        assert_eq!(parse_github_issue_or_pr_link("##42"), None);
    }

    /// P10 crux pin: owner/repo case is preserved verbatim even though the
    /// route segment's case only ever affects the issue/pr decision.
    #[test]
    fn p10_owner_repo_case_preserved() {
        assert_eq!(
            parse_github_issue_or_pr_link("https://github.com/MyOrg/my_repo/PULL/1")
                .map(|l| l.slug),
            Some(slug("MyOrg", "my_repo"))
        );
    }
}

