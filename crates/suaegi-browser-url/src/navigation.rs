//! VERBATIM port of the navigation-decision-tree / path-to-file-URL /
//! external-gate cluster of Orca's `src/shared/browser-url.ts`, milestone M3.
//! **This module is a security boundary**: what falls through to "allowed"
//! here is the whole point.
//!
//! Ported: `O:11-13` `WINDOWS_ABSOLUTE_PATH_PATTERN` /
//! `WINDOWS_UNC_PATH_PATTERN` / `UNIX_ABSOLUTE_PATH_PATTERN` (hand-rolled,
//! see [`matches_windows_absolute_path_pattern`] /
//! [`matches_windows_unc_path_pattern`] /
//! [`matches_unix_absolute_path_pattern`]), `O:126-140`
//! [`resolve_remote_failure_external_url`] (deferred from M1's plan),
//! `O:242-253` `absolutePathToFileUrl` (private, see
//! [`absolute_path_to_file_url`]), `O:255-259` `windowsUncPathToFileUrl`
//! (private, see [`windows_unc_path_to_file_url`]), `O:261-318`
//! [`normalize_browser_navigation_url`], `O:320-333`
//! [`normalize_external_browser_url`]. `ORCA_BROWSER_BLANK_URL`
//! (`constants.ts:64`, `"data:text/html,"`) is re-exposed here since this is
//! its first production consumer.
//!
//! This completes the port of `browser-url.ts` (M1 = local-dev-address /
//! certificate-host cluster in `lib.rs`, M2 = search-engine / Kagi-session
//! cluster in `search.rs`, M3 = this file).
//!
//! # Traps (see the plan's §2 for the full rationale)
//! - **D1**: `searchEngine` is THREE-STATE in the TS source —
//!   `searchEngine !== undefined` (`O:303`) decides whether search fallback
//!   is enabled at all, and `searchEngine ?? DEFAULT_SEARCH_ENGINE` (`O:316`)
//!   only then fills an explicit `null` with the default engine. Collapsing
//!   this onto `Option<SearchEngine>` is a security defect: it would make
//!   `normalize_browser_navigation_url("not a url")` return a Google search
//!   URL instead of `None`, turning a navigation *validator* into an open
//!   redirect to a search engine. [`SearchFallback`] models the three states
//!   explicitly: `Disabled` (omitted — the main-process URL-validation path),
//!   `DefaultEngine` (explicit `null` — the address bar), `Engine(_)` (an
//!   explicit engine). Oracle `T:176-178` (omitted) vs `T:184-194` (`null`)
//!   pins exactly this distinction.
//! - **D2**: `javascript:` and `data:` PARSE SUCCESSFULLY under `Url::parse`
//!   and are rejected by the ALLOW-LIST (`O:293-297`), not by a parse
//!   failure. The allowed set is exactly `http:`/`https:`/`file:`; anything
//!   else returns `None` from the `Ok` arm. The `match Url::parse(trimmed)`
//!   below is deliberately NOT restructured as
//!   `.map(...).unwrap_or_else(|_| fallback())` — that shape would route a
//!   non-web scheme into the `https://{trimmed}` promotion or the search
//!   fallback instead of rejecting it outright. This structure guarantees a
//!   non-web scheme can never reach the `Err` arm's fallback logic.
//! - **D3**: [`absolute_path_to_file_url`] / [`windows_unc_path_to_file_url`]
//!   NEVER go through a URL parser — they build the result by string
//!   concatenation, so a UNC host is neither lowercased nor
//!   percent-encoded: `\\SERVER\share\x` -> `file://SERVER/share/x`.
//!   `Url::from_file_path` is FORBIDDEN here (it normalizes, rejects
//!   relative paths, and is platform-dependent) — both functions return
//!   `String`, never `Url`.
//! - **D4**: [`encode_uri_component`] is the SAME function used by M2's
//!   plain search-template path (promoted `pub(crate)` in `search.rs` — the
//!   only change made to that file); not duplicated here. Oracle
//!   `T:156-166` pins that `!` survives unescaped while `^` becomes `%5E`.
//! - **D5**: [`normalize_external_browser_url`] tests the string PREFIX
//!   `"file:"` (`O:329`), not `Url::parse(...).scheme()`. Re-parsing would
//!   lowercase a D3 UNC host that was deliberately never parsed. Gate order:
//!   `None` or the blank sentinel -> `None`; `file:` prefix -> `None`;
//!   otherwise pass through unchanged.
//! - **D6**: the blank sentinel (`O:267-269`) is matched by an exact,
//!   case-sensitive string comparison BEFORE any parsing: empty /
//!   `"about:blank"` / [`ORCA_BROWSER_BLANK_URL`] (`"data:text/html,"`) ->
//!   the sentinel. `"ABOUT:BLANK"` does NOT hit it (case-sensitive). This
//!   pre-parse check is the only reason `"data:text/html,"` is reachable at
//!   all, since D2's allow-list rejects `data:` generally.
//! - **D7**: the `https://{trimmed}` promotion (`O:305`) is the WIDEST part
//!   of this boundary. After `Url::parse(trimmed)` fails, if
//!   `https://{trimmed}` parses AND (search is disabled OR the input
//!   doesn't look like a search query), that URL is returned verbatim. With
//!   search disabled, [`looks_like_search_query`] is never even called (see
//!   the `match` on [`SearchFallback`] below), so e.g.
//!   `normalize_browser_navigation_url("singleword", Disabled, _)` ->
//!   `Some("https://singleword/")` — and so does `"user@evil.com:8080"` ->
//!   `Some("https://user@evil.com:8080/")` (userinfo smuggled straight
//!   through; see the `d7_*` pins). NOTE: the plan text names
//!   `"user:pass@evil.com"` / `"evil.com:8080"` for this trap, but both were
//!   verified (against this crate's `url` parser AND real Node
//!   `new URL(...)`) to parse SUCCESSFULLY as opaque URLs with scheme
//!   `"user"` / `"evil.com"` respectively (a scheme is any
//!   `ALPHA *(ALPHA/DIGIT/"+"/"-"/".")` run before the first `:`) — they
//!   never reach this promotion at all, and are instead rejected by D2's
//!   allow-list. See `d7_plan_examples_are_actually_rejected_not_promoted`.
//! - **D8**: check ORDER is contractual: (1) js_trim + sentinel (2)
//!   [`crate::classify_scheme_less_local_dev_address`] (3) UNC pattern (4)
//!   UNIX-absolute OR Windows-drive-absolute (5) `Url::parse` + allow-list
//!   (6) the catch/fallback. Steps 3-4 run BEFORE the parse, so `C:\...`,
//!   `\\srv\sh\..` and `/a/b` never reach `Url::parse` at all. If step 5's
//!   parse succeeds, the fallback never runs (see D2).
//! - **D9**: [`resolve_remote_failure_external_url`] (`O:130-140`) passes
//!   the RAW input string to [`normalize_external_browser_url`], not the
//!   already-`Url::parse`d value — and that call sits OUTSIDE the
//!   `match`/`try`. Parse failure -> `None`; a wildcard-bind or
//!   loopback-eligible hostname -> `None`; otherwise
//!   `normalize_external_browser_url(raw_url)`. `file:///etc/passwd` has an
//!   empty hostname, passes both predicates, reaches the last line, and is
//!   then killed by D5's `file:` rule (oracle `T:116` pins this exact path).
//! - **D10**: the three patterns are hand-rolled (see M1/M2 precedent, no
//!   `regex` dependency). The UNC pattern's `[^\s\\/]` host class uses the
//!   ECMAScript whitespace set (`O:12`) -> [`is_js_whitespace`], not Rust's
//!   `char::is_whitespace`. None of the three patterns has a dotAll/`s`
//!   flag, so every `.*`/`.` in them does NOT cross a line terminator (`\n`,
//!   `\r`, U+2028, U+2029) — mirrors M1's `tail_matches` /
//!   M2's `matches_looks_like_url_pattern` precedent exactly.
//! - **D11**: every `.trim()` in scope (`O:266`) is [`js_trim`], never
//!   `str::trim` — the blank-sentinel comparison is downstream of this trim,
//!   so a U+FEFF-padded `"about:blank"` IS the sentinel (`d11_*` pin).

use suaegi_misc::{is_js_whitespace, js_trim};
use url::Url;

use crate::search::encode_uri_component;
use crate::{
    build_search_url, classify_scheme_less_local_dev_address, is_eligible_local_certificate_host,
    is_wildcard_bind_host, looks_like_search_query, SearchEngine, SearchUrlOptions,
    DEFAULT_SEARCH_ENGINE,
};

// ---------------------------------------------------------------------------
// constants.ts:64 ORCA_BROWSER_BLANK_URL
// ---------------------------------------------------------------------------

/// `constants.ts:64`. The blank-tab sentinel URL — a `data:` URI, which is
/// otherwise rejected wholesale by D2's allow-list; it is only reachable via
/// D6's pre-parse, exact-string sentinel check.
pub const ORCA_BROWSER_BLANK_URL: &str = "data:text/html,";

// ---------------------------------------------------------------------------
// D1 — SearchFallback (three-state, see the module doc's D1 entry)
// ---------------------------------------------------------------------------

/// Models the TS `searchEngine?: SearchEngine | null` parameter's three
/// distinct states (D1). NEVER collapse this to `Option<SearchEngine>` — see
/// the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFallback {
    /// `searchEngine === undefined` (`O:303`'s `searchEnabled` is `false`):
    /// the main-process URL-validation path. Non-URL input is rejected
    /// outright, never converted to a search query.
    Disabled,
    /// `searchEngine === null`: the address bar's default state — search is
    /// enabled, falling back to [`DEFAULT_SEARCH_ENGINE`] (`O:316`'s `??`).
    DefaultEngine,
    /// `searchEngine` is an explicit engine: search is enabled with that
    /// engine.
    Engine(SearchEngine),
}

// ---------------------------------------------------------------------------
// O:11 WINDOWS_ABSOLUTE_PATH_PATTERN
// ---------------------------------------------------------------------------
//
// `/^[A-Za-z]:[\\/].*$/` — no `s` flag (D10): the trailing `.*` does not
// cross a line terminator (`\n`, `\r`, U+2028, U+2029).

fn matches_windows_absolute_path_pattern(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    if chars.next() != Some(':') {
        return false;
    }
    match chars.next() {
        Some('\\') | Some('/') => {}
        _ => return false,
    }
    // Remainder is `.*$` — anything except a line terminator (D10); empty is
    // fine since `*` allows zero.
    !chars.any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

// ---------------------------------------------------------------------------
// O:12 WINDOWS_UNC_PATH_PATTERN
// ---------------------------------------------------------------------------
//
// `/^\\\\[^\s\\/]+[\\/][^\\/]+(?:[\\/].*)?$/` — D10: `[^\s...]` uses the
// ECMAScript whitespace set (`is_js_whitespace`), and the trailing optional
// group's `.*` has no `s` flag either.
//
// Deterministic, no backtracking search needed: the greedy host run
// `[^\s\\/]+` can only stop at whitespace, `\`, `/`, or end-of-input; if it
// stopped at whitespace, no amount of giving back characters (which are
// themselves non-separator by definition) can produce the required `[\\/]`
// token there, so the maximal run is the only candidate worth trying. Same
// argument applies to the share segment's `[^\\/]+` run, which can only stop
// at `\`, `/`, or end.

fn matches_windows_unc_path_pattern(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    // Literal `\\\\` = two backslash characters.
    if len < 2 || chars[0] != '\\' || chars[1] != '\\' {
        return false;
    }
    let mut pos = 2;

    // `[^\s\\/]+` — host, at least one char, no ECMAScript whitespace, no
    // separator.
    let host_start = pos;
    while pos < len && !is_js_whitespace(chars[pos]) && chars[pos] != '\\' && chars[pos] != '/' {
        pos += 1;
    }
    if pos == host_start {
        return false;
    }

    // `[\\/]` — exactly one separator.
    if pos >= len || !matches!(chars[pos], '\\' | '/') {
        return false;
    }
    pos += 1;

    // `[^\\/]+` — share, at least one char, no separator (whitespace is
    // allowed here, unlike the host).
    let share_start = pos;
    while pos < len && chars[pos] != '\\' && chars[pos] != '/' {
        pos += 1;
    }
    if pos == share_start {
        return false;
    }

    // `(?:[\\/].*)?$` — optional tail.
    if pos == len {
        return true;
    }
    if !matches!(chars[pos], '\\' | '/') {
        return false; // leftover chars that aren't a separator: `$` can't match
    }
    pos += 1;
    // D10: no `s` flag, so a line terminator anywhere in the remainder fails
    // the whole pattern (greedy `.*` cannot cross it, and `$` never matches
    // early).
    !chars[pos..]
        .iter()
        .any(|&c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

// ---------------------------------------------------------------------------
// O:13 UNIX_ABSOLUTE_PATH_PATTERN
// ---------------------------------------------------------------------------
//
// `/^\/.*$/` — D10: no `s` flag.

fn matches_unix_absolute_path_pattern(s: &str) -> bool {
    let mut chars = s.chars();
    if chars.next() != Some('/') {
        return false;
    }
    !chars.any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

// ---------------------------------------------------------------------------
// O:242-253 absolutePathToFileUrl (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `^[A-Za-z]:$` — exactly one ASCII letter then a colon, nothing else.
fn matches_drive_letter_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(letter), Some(':'), None) => letter.is_ascii_alphabetic(),
        _ => false,
    }
}

/// `O:242-253`. D3: string concatenation only, never a URL parser — the
/// first path segment is kept verbatim when it looks like a Windows drive
/// letter (`C:`), everything else is [`encode_uri_component`]'d. Splitting
/// on `/` after replacing `\` with `/` means a leading `/` (Unix absolute
/// paths) produces an empty first segment, whose `encode_uri_component`
/// output is also empty — that's what makes `file:///a/b` (three slashes)
/// come out of the `file://` + join branch for `/a/b`.
fn absolute_path_to_file_url(file_path: &str) -> String {
    let normalized_path = file_path.replace('\\', "/");
    let segments: Vec<String> = normalized_path
        .split('/')
        .enumerate()
        .map(|(index, segment)| {
            if index == 0 && matches_drive_letter_segment(segment) {
                segment.to_string()
            } else {
                encode_uri_component(segment)
            }
        })
        .collect();
    let joined = segments.join("/");
    if normalized_path.starts_with('/') {
        format!("file://{joined}")
    } else {
        format!("file:///{joined}")
    }
}

// ---------------------------------------------------------------------------
// O:255-259 windowsUncPathToFileUrl (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:255-259`. D3: the host (first segment after stripping ALL leading
/// slashes) is kept verbatim — NOT lowercased, NOT percent-encoded — only
/// the remaining path segments are [`encode_uri_component`]'d. Never routes
/// through `Url::parse`/`Url::from_file_path` (D3), so a host like
/// `SERVER` (uppercase) survives unchanged into the output.
fn windows_unc_path_to_file_url(file_path: &str) -> String {
    let normalized_path = file_path.replace('\\', "/");
    let stripped = normalized_path.trim_start_matches('/');
    let mut parts = stripped.split('/');
    let host = parts.next().unwrap_or("");
    let path_segments: Vec<String> = parts.map(encode_uri_component).collect();
    format!("file://{host}/{}", path_segments.join("/"))
}

// ---------------------------------------------------------------------------
// O:261-318 normalizeBrowserNavigationUrl
// ---------------------------------------------------------------------------

/// `O:261-318`. See the module doc's D1/D2/D6/D7/D8 entries for the traps in
/// this function specifically; the check order below is load-bearing (D8).
pub fn normalize_browser_navigation_url(
    raw_url: &str,
    search_fallback: SearchFallback,
    options: SearchUrlOptions,
) -> Option<String> {
    // D11/D6: js_trim, then an exact case-sensitive pre-parse sentinel
    // check.
    let trimmed = js_trim(raw_url);
    if trimmed.is_empty() || trimmed == "about:blank" || trimmed == ORCA_BROWSER_BLANK_URL {
        return Some(ORCA_BROWSER_BLANK_URL.to_string());
    }

    // D8 step 2.
    if let Some(local_dev_address) = classify_scheme_less_local_dev_address(trimmed) {
        return Some(local_dev_address.to_string());
    }

    // D8 step 3 — before the parser, so a UNC path never reaches `Url::parse`
    // (D3: the host stays unlowercased/unencoded).
    if matches_windows_unc_path_pattern(trimmed) {
        return Some(windows_unc_path_to_file_url(trimmed));
    }

    // D8 step 4 — also before the parser.
    if matches_unix_absolute_path_pattern(trimmed) || matches_windows_absolute_path_pattern(trimmed)
    {
        return Some(absolute_path_to_file_url(trimmed));
    }

    // D8 step 5 / D2: non-web schemes are rejected HERE, on the success
    // path — never routed into the `Err` arm's fallback below.
    match Url::parse(trimmed) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            if scheme == "http" || scheme == "https" || scheme == "file" {
                Some(parsed.to_string())
            } else {
                None
            }
        }
        Err(_) => {
            // D8 step 6 / D7: the widest part of the boundary.
            if let Ok(with_scheme) = Url::parse(&format!("https://{trimmed}")) {
                // D1/D7: with search disabled, `looks_like_search_query` is
                // never even called.
                let treat_as_search_query = match search_fallback {
                    SearchFallback::Disabled => false,
                    SearchFallback::DefaultEngine | SearchFallback::Engine(_) => {
                        looks_like_search_query(trimmed)
                    }
                };
                if !treat_as_search_query {
                    return Some(with_scheme.to_string());
                }
            }

            // D1: the three-state dispatch.
            match search_fallback {
                SearchFallback::Disabled => None,
                SearchFallback::DefaultEngine => {
                    Some(build_search_url(trimmed, DEFAULT_SEARCH_ENGINE, options))
                }
                SearchFallback::Engine(engine) => Some(build_search_url(trimmed, engine, options)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// O:320-333 normalizeExternalBrowserUrl
// ---------------------------------------------------------------------------

/// `O:320-333`. D5: gates on the string PREFIX `"file:"`, never
/// `Url::parse(...).scheme()` (which would re-parse and lowercase a D3 UNC
/// string). Calls [`normalize_browser_navigation_url`] with
/// [`SearchFallback::Disabled`] — external-link opening never enables the
/// search fallback (`O:321`'s single-argument call).
pub fn normalize_external_browser_url(raw_url: &str) -> Option<String> {
    let normalized = normalize_browser_navigation_url(
        raw_url,
        SearchFallback::Disabled,
        SearchUrlOptions::default(),
    )?;
    if normalized == ORCA_BROWSER_BLANK_URL {
        return None;
    }
    if normalized.starts_with("file:") {
        return None;
    }
    Some(normalized)
}

// ---------------------------------------------------------------------------
// O:130-140 resolveRemoteFailureExternalUrl
// ---------------------------------------------------------------------------

/// `O:130-140`. D9: passes the RAW `raw_url` to
/// [`normalize_external_browser_url`] (never the already-parsed `Url`), and
/// that call is OUTSIDE the parse/predicate check below.
pub fn resolve_remote_failure_external_url(raw_url: &str) -> Option<String> {
    match Url::parse(raw_url) {
        Ok(parsed) => {
            let hostname = parsed.host_str().unwrap_or("");
            if is_wildcard_bind_host(hostname) || is_eligible_local_certificate_host(hostname) {
                return None;
            }
        }
        Err(_) => return None,
    }
    normalize_external_browser_url(raw_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nav(raw: &str, fallback: SearchFallback) -> Option<String> {
        normalize_browser_navigation_url(raw, fallback, SearchUrlOptions::default())
    }

    // -----------------------------------------------------------------
    // Oracle block `T:17-26` — manual local-dev inputs, query/hash kept.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_normalizes_manual_local_dev_inputs_to_http() {
        assert_eq!(
            nav("localhost:3000", SearchFallback::Disabled),
            Some("http://localhost:3000/".to_string())
        );
        assert_eq!(
            nav("127.0.0.1:5173", SearchFallback::Disabled),
            Some("http://127.0.0.1:5173/".to_string())
        );
        assert_eq!(
            nav("localhost:3000?debug=1", SearchFallback::Disabled),
            Some("http://localhost:3000/?debug=1".to_string())
        );
        assert_eq!(
            nav("localhost:3000#preview", SearchFallback::Disabled),
            Some("http://localhost:3000/#preview".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:99-118` — resolveRemoteFailureExternalUrl.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_resolve_remote_failure_external_url_loopback_and_wildcard_null() {
        for input in [
            "https://localhost:3000/",
            "https://127.0.0.1:3000/",
            "https://127.0.0.9:3000/",
            "https://[::1]:3000/",
            "https://app.localhost:3000/",
            "http://0.0.0.0:3000/",
            "http://[::]:3000/",
        ] {
            assert_eq!(
                resolve_remote_failure_external_url(input),
                None,
                "expected None for {input:?}"
            );
        }
    }

    #[test]
    fn oracle_resolve_remote_failure_external_url_public_hosts_pass_through() {
        assert_eq!(
            resolve_remote_failure_external_url("https://example.com/app"),
            Some("https://example.com/app".to_string())
        );
        assert_eq!(
            resolve_remote_failure_external_url("http://example.com:8080/x"),
            Some("http://example.com:8080/x".to_string())
        );
    }

    #[test]
    fn oracle_resolve_remote_failure_external_url_file_and_garbage_null() {
        assert_eq!(
            resolve_remote_failure_external_url("file:///etc/passwd"),
            None
        );
        assert_eq!(resolve_remote_failure_external_url("not a url"), None);
    }

    // -----------------------------------------------------------------
    // Oracle block `T:120-124` — web URLs and blank tabs.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_keeps_normal_web_urls_and_blank_tabs_in_allowed_set() {
        assert_eq!(
            nav("https://example.com", SearchFallback::Disabled),
            Some("https://example.com/".to_string())
        );
        assert_eq!(
            nav("", SearchFallback::Disabled),
            Some(ORCA_BROWSER_BLANK_URL.to_string())
        );
        assert_eq!(
            nav("about:blank", SearchFallback::Disabled),
            Some(ORCA_BROWSER_BLANK_URL.to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:126-129` — non-web schemes rejected.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_rejects_non_web_schemes_for_in_app_navigation() {
        assert_eq!(nav("javascript:alert(1)", SearchFallback::Disabled), None);
        assert_eq!(normalize_external_browser_url("about:blank"), None);
    }

    // -----------------------------------------------------------------
    // Oracle block `T:135-139` — file:// allowed in-app.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_allows_file_urls_for_in_app_preview() {
        assert_eq!(
            nav("file:///Users/me/site/index.html", SearchFallback::Disabled),
            Some("file:///Users/me/site/index.html".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:141-154` — absolute local paths to file URLs.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_normalizes_pasted_absolute_local_paths_to_file_urls() {
        assert_eq!(
            nav(
                "/Users/me/Downloads/Example.ipynb",
                SearchFallback::Disabled
            ),
            Some("file:///Users/me/Downloads/Example.ipynb".to_string())
        );
        assert_eq!(
            nav(
                "C:\\Users\\me\\Downloads\\Example.ipynb",
                SearchFallback::Disabled
            ),
            Some("file:///C:/Users/me/Downloads/Example.ipynb".to_string())
        );
        assert_eq!(
            nav("\\\\server\\share\\Example.ipynb", SearchFallback::Disabled),
            Some("file://server/share/Example.ipynb".to_string())
        );
        assert_eq!(
            nav(
                "\\\\wsl.localhost\\Ubuntu\\home\\me\\Example.ipynb",
                SearchFallback::Disabled
            ),
            Some("file://wsl.localhost/Ubuntu/home/me/Example.ipynb".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:156-166` — spaces and reserved characters encoded.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_normalizes_absolute_local_paths_with_spaces_and_reserved_chars() {
        assert_eq!(
            nav("/Users/me/My Site/index #1.html", SearchFallback::Disabled),
            Some("file:///Users/me/My%20Site/index%20%231.html".to_string())
        );
        assert_eq!(
            nav(
                "C:\\Users\\me\\My Site\\index #1.html",
                SearchFallback::Disabled
            ),
            Some("file:///C:/Users/me/My%20Site/index%20%231.html".to_string())
        );
        assert_eq!(
            nav(
                "C:\\tmp\\orca & 100% ! ^\\index.html",
                SearchFallback::Disabled
            ),
            Some("file:///C:/tmp/orca%20%26%20100%25%20!%20%5E/index.html".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:171-174` — external rejects file:/UNC.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_rejects_file_and_unc_for_external_opens() {
        assert_eq!(normalize_external_browser_url("file:///etc/passwd"), None);
        assert_eq!(
            normalize_external_browser_url("\\\\server\\share\\Example.ipynb"),
            None
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:176-178` — no search opt-in -> None.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_returns_none_for_non_url_input_without_search_opt_in() {
        assert_eq!(nav("not a url", SearchFallback::Disabled), None);
    }

    // -----------------------------------------------------------------
    // Oracle block `T:180-182` — bare word promoted to https://.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_attempts_https_prefix_for_bare_words_without_search_opt_in() {
        assert_eq!(
            nav("singleword", SearchFallback::Disabled),
            Some("https://singleword/".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:184-194` — search enabled (default engine via null).
    // -----------------------------------------------------------------

    #[test]
    fn oracle_treats_bare_and_multi_word_input_as_search_queries_when_enabled() {
        assert_eq!(
            nav("react hooks", SearchFallback::DefaultEngine),
            Some("https://www.google.com/search?q=react%20hooks".to_string())
        );
        assert_eq!(
            nav("what is typescript", SearchFallback::DefaultEngine),
            Some("https://www.google.com/search?q=what%20is%20typescript".to_string())
        );
        assert_eq!(
            nav("singleword", SearchFallback::DefaultEngine),
            Some("https://www.google.com/search?q=singleword".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:196-206` — respects the search engine parameter.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_respects_the_search_engine_parameter() {
        assert_eq!(
            nav(
                "react hooks",
                SearchFallback::Engine(SearchEngine::DuckDuckGo)
            ),
            Some("https://duckduckgo.com/?q=react%20hooks".to_string())
        );
        assert_eq!(
            nav("react hooks", SearchFallback::Engine(SearchEngine::Bing)),
            Some("https://www.bing.com/search?q=react%20hooks".to_string())
        );
        assert_eq!(
            nav("react hooks", SearchFallback::Engine(SearchEngine::Kagi)),
            Some("https://kagi.com/search?q=react%20hooks".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:208-213` — domain-like inputs treated as URLs.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_treats_domain_like_inputs_as_urls_not_searches() {
        assert_eq!(
            nav("example.com", SearchFallback::DefaultEngine),
            Some("https://example.com/".to_string())
        );
        assert_eq!(
            nav("github.com/org/repo", SearchFallback::DefaultEngine),
            Some("https://github.com/org/repo".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle `T:234-237` — Kagi session link carried through navigation
    // (the M2-deferred assertion).
    // -----------------------------------------------------------------

    #[test]
    fn oracle_kagi_session_link_via_navigation() {
        let session_link = "https://kagi.com/search?token=secret&q=%s#ignored";
        assert_eq!(
            normalize_browser_navigation_url(
                "hello world",
                SearchFallback::Engine(SearchEngine::Kagi),
                SearchUrlOptions {
                    kagi_session_link: Some(session_link.to_string())
                }
            ),
            Some("https://kagi.com/search?token=secret&q=hello+world".to_string())
        );
    }

    // -----------------------------------------------------------------
    // D1 — three-state search fallback, same input, three different
    // outcomes. THE crux pin: collapsing to `Option<SearchEngine>` would
    // turn this validator into an open redirect.
    // -----------------------------------------------------------------

    #[test]
    fn d1_three_state_search_fallback_same_input_three_outcomes() {
        assert_eq!(nav("not a url", SearchFallback::Disabled), None);
        assert_eq!(
            nav("not a url", SearchFallback::DefaultEngine),
            Some("https://www.google.com/search?q=not%20a%20url".to_string())
        );
        assert_eq!(
            nav("not a url", SearchFallback::Engine(SearchEngine::Bing)),
            Some("https://www.bing.com/search?q=not%20a%20url".to_string())
        );
    }

    // -----------------------------------------------------------------
    // D2 — non-web schemes rejected on the parse SUCCESS path, never
    // reaching the fallback, even with search enabled.
    // -----------------------------------------------------------------

    #[test]
    fn d2_non_web_schemes_rejected_never_reach_search_fallback() {
        assert_eq!(nav("data:text/plain,x", SearchFallback::Disabled), None);
        assert_eq!(nav("mailto:a@b.c", SearchFallback::Disabled), None);
        assert_eq!(nav("ftp://h/f", SearchFallback::Disabled), None);
        // The crux: even with search fully enabled, javascript: must not
        // fall through to the https:// promotion or a search URL.
        assert_eq!(
            nav("javascript:alert(1)", SearchFallback::DefaultEngine),
            None
        );
        assert_eq!(
            nav(
                "javascript:alert(1)",
                SearchFallback::Engine(SearchEngine::Bing)
            ),
            None
        );
    }

    // -----------------------------------------------------------------
    // D3 — UNC host case preserved and never percent-encoded; `C:/a`
    // (forward slash) still a drive path.
    // -----------------------------------------------------------------

    #[test]
    fn d3_unc_host_case_preserved_and_unencoded() {
        assert_eq!(
            nav("\\\\SERVER\\share\\x", SearchFallback::Disabled),
            Some("file://SERVER/share/x".to_string())
        );
        // A host containing characters that WOULD be percent-encoded by
        // encode_uri_component (if it were mistakenly applied) proves the
        // host truly skips the encoder.
        assert_eq!(
            windows_unc_path_to_file_url("\\\\SERVER NAME\\share\\x"),
            "file://SERVER NAME/share/x".to_string()
        );
    }

    #[test]
    fn d3_forward_slash_drive_path_still_treated_as_drive_absolute() {
        assert_eq!(
            nav("C:/a", SearchFallback::Disabled),
            Some("file:///C:/a".to_string())
        );
    }

    // -----------------------------------------------------------------
    // D5 — experimentally-determined behavior for an uppercase `FILE:`
    // input (the plan asked us to determine this by experiment and pin the
    // real observed value).
    //
    // FINDING: `normalize_external_browser_url("FILE:///etc/passwd")`
    // returns `None` — correctly rejected, NOT a bypass. The reason is that
    // this input never reaches D5's string-prefix check with its original
    // casing intact: it fails all three hand-rolled path patterns (a
    // multi-letter run before `:` is not `^[A-Za-z]:$`/`^[A-Za-z]:[\\/]`),
    // so it falls through to `Url::parse` at D8 step 5, which — per the
    // WHATWG URL Standard, independent of this port — lowercases the
    // scheme during parsing. By the time `normalize_browser_navigation_url`
    // returns, the string already reads `file:///etc/passwd` (lowercase),
    // so D5's `starts_with("file:")` catches it regardless. The
    // uppercase-scheme risk the plan raises would only be live if some
    // producer emitted an un-lowercased `"FILE://"` string bypassing
    // `Url::parse` entirely — but the only two functions that skip the
    // parser ([`absolute_path_to_file_url`], [`windows_unc_path_to_file_url`])
    // both hard-code a lowercase `file://` literal in their `format!`
    // calls, never derived from input casing. So no path through this
    // module's current code can actually produce an uppercase-prefixed
    // `"FILE:"` string for D5 to miss.
    // -----------------------------------------------------------------

    #[test]
    fn d5_uppercase_file_scheme_input_still_rejected_because_url_parse_lowercases_it() {
        assert_eq!(
            nav("FILE:///etc/passwd", SearchFallback::Disabled),
            Some("file:///etc/passwd".to_string()),
            "Url::parse lowercases the scheme before normalize_browser_navigation_url returns"
        );
        assert_eq!(
            normalize_external_browser_url("FILE:///etc/passwd"),
            None,
            "D5's string-prefix check still catches it because the scheme was already lowercased"
        );
    }

    // -----------------------------------------------------------------
    // D6 — sentinel is exact-match, case-sensitive, pre-parse.
    // -----------------------------------------------------------------

    #[test]
    fn d6_uppercase_about_blank_is_not_the_sentinel() {
        assert_ne!(
            nav("ABOUT:BLANK", SearchFallback::Disabled),
            Some(ORCA_BROWSER_BLANK_URL.to_string())
        );
    }

    #[test]
    fn d6_only_exact_data_text_html_comma_is_the_sentinel() {
        assert_eq!(
            nav("data:text/html,", SearchFallback::Disabled),
            Some(ORCA_BROWSER_BLANK_URL.to_string())
        );
        // A near-miss data: URI is NOT the sentinel, and falls to D2's
        // allow-list rejection instead.
        assert_eq!(nav("data:text/html,x", SearchFallback::Disabled), None);
    }

    // -----------------------------------------------------------------
    // D7 — the promotion is the widest part of the boundary: userinfo and
    // a digit-led host:port both get promoted to https:// with search off.
    //
    // NOTE ON THE PLAN'S LITERAL EXAMPLES: the plan text names
    // `user:pass@evil.com` and `evil.com:8080` for this pin. Both were
    // verified NOT to reach this promotion at all -- confirmed against
    // both this crate's `url` parser AND real `node -e 'new URL(...)'`
    // (same WHATWG algorithm, same result). The text before the first `:`
    // in each (`user`, `evil.com`) is itself a syntactically valid URL
    // scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`), so
    // `Url::parse` SUCCEEDS on step 5 with scheme `"user"` / `"evil.com"`
    // and an opaque path -- landing in the `Ok` arm, not the `Err`/fallback
    // arm this pin is meant to exercise, and returning `None` via D2's
    // allow-list instead of a promoted URL:
    //   nav("user:pass@evil.com", Disabled) == None
    //   nav("evil.com:8080", Disabled)       == None
    // (see the d7_plan_examples_are_actually_rejected_not_promoted test
    // below). The examples here are genuine replacements that exhibit the
    // SAME boundary-width property the plan was describing -- an input
    // whose pre-colon text is NOT a valid scheme (contains `@`, or starts
    // with a digit) fails step 5's parse and falls through to the `https://`
    // promotion, carrying userinfo/host:port straight into the result.
    // -----------------------------------------------------------------

    #[test]
    fn d7_userinfo_and_digit_led_host_port_promoted_to_https_with_search_off() {
        // `user@evil.com` (no second colon) is not a valid scheme (`@` is
        // not a scheme character), so step 5's parse fails and the
        // userinfo survives into the promoted URL verbatim.
        assert_eq!(
            nav("user@evil.com:8080", SearchFallback::Disabled),
            Some("https://user@evil.com:8080/".to_string())
        );
        // A host starting with a digit cannot be a valid scheme (schemes
        // must start with an ASCII letter), so this also falls through to
        // the promotion.
        assert_eq!(
            nav("1evil.com:8080", SearchFallback::Disabled),
            Some("https://1evil.com:8080/".to_string())
        );
    }

    #[test]
    fn d7_plan_examples_are_actually_rejected_not_promoted() {
        // Documents the finding above: these two plan-cited strings parse
        // successfully as opaque, non-web-scheme URLs (`"user:"` /
        // `"evil.com:"`) and are rejected by D2's allow-list -- they never
        // reach the `https://` promotion at all.
        assert_eq!(nav("user:pass@evil.com", SearchFallback::Disabled), None);
        assert_eq!(nav("evil.com:8080", SearchFallback::Disabled), None);
    }

    // -----------------------------------------------------------------
    // D8 — check order: path patterns short-circuit before the parser.
    // -----------------------------------------------------------------

    #[test]
    fn d8_drive_path_never_reaches_the_parser() {
        assert_eq!(
            nav("C:\\x", SearchFallback::Disabled),
            Some("file:///C:/x".to_string())
        );
    }

    #[test]
    fn d8_unix_absolute_path_takes_the_unix_branch() {
        assert_eq!(
            nav("/a/b", SearchFallback::Disabled),
            Some("file:///a/b".to_string())
        );
    }

    // -----------------------------------------------------------------
    // D10 — UNC pattern rejects U+FEFF in the host and a newline in the
    // tail.
    // -----------------------------------------------------------------

    #[test]
    fn d10_unc_pattern_rejects_feff_in_host() {
        assert!(!matches_windows_unc_path_pattern(
            "\\\\\u{FEFF}server\\share\\x"
        ));
    }

    #[test]
    fn d10_unc_pattern_rejects_newline_in_tail() {
        assert!(!matches_windows_unc_path_pattern("\\\\server\\share\\a\nb"));
    }

    // -----------------------------------------------------------------
    // D11 — U+FEFF-padded about:blank IS the sentinel (js_trim, not
    // str::trim).
    // -----------------------------------------------------------------

    #[test]
    fn d11_feff_padded_about_blank_is_the_sentinel() {
        assert_eq!(
            nav("\u{FEFF}about:blank\u{FEFF}", SearchFallback::Disabled),
            Some(ORCA_BROWSER_BLANK_URL.to_string())
        );
    }
}
