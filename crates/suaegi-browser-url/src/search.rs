//! VERBATIM port of the search-engine / Kagi-session cluster of Orca's
//! `src/shared/browser-url.ts`, milestone M2.
//!
//! **This module handles a bearer token** (the Kagi private-session `token`
//! query parameter) — see C4/C5 below for exactly how it is (and is not)
//! protected.
//!
//! Ported: `O:10` `LOOKS_LIKE_URL_PATTERN` (hand-rolled, see
//! [`matches_looks_like_url_pattern`]), `O:15-19` [`SearchEngine`] /
//! [`SearchUrlOptions`], `O:21-26` [`SEARCH_ENGINE_LABELS`], `O:28-33`
//! `SEARCH_ENGINE_URLS` (module-private, see [`search_engine_url_template`]),
//! `O:35` [`DEFAULT_SEARCH_ENGINE`], `O:142-176`
//! [`normalize_kagi_session_link`] / [`redact_kagi_session_token`],
//! `O:199-213` `buildKagiSessionSearchUrl` (module-private, see
//! [`build_kagi_session_search_url`]), `O:215-227` [`build_search_url`],
//! `O:229-240` [`looks_like_search_query`].
//!
//! Deliberately NOT ported (M3, see the M2 plan's §1): `WINDOWS_*` /
//! `UNIX_ABSOLUTE_PATH_PATTERN`, `absolutePathToFileUrl`,
//! `windowsUncPathToFileUrl`, `normalizeBrowserNavigationUrl`,
//! `normalizeExternalBrowserUrl`, `resolveRemoteFailureExternalUrl`.
//!
//! # Traps (see the plan's §2 for the full rationale)
//! - **C1**: the `url` crate's query-mutation surface pushes a `'?'`
//!   unconditionally and never removes it if the serialization ends up
//!   empty, where WHATWG/JS would produce a `null` query (no `?` at all).
//!   Reachable via `redact_kagi_session_token` when `token` is the SOLE
//!   query parameter. Every query rewrite in this module goes through
//!   [`rewrite_query_pairs`], which explicitly re-checks
//!   `url.query() == Some("")` afterward and nulls it out. All three oracle
//!   redaction cases carry a `q` alongside `token`, so they never exercise
//!   this path — see the `c1_*` pins.
//! - **C2**: JS `searchParams.set(name, v)` replaces the FIRST occurrence of
//!   `name` in place and removes every other same-named entry;
//!   `searchParams.delete(name)` removes ALL occurrences. `?a=1&token=X&b=2`
//!   must come back with `a`/`b` untouched and `token` still in the middle.
//!   Implemented by collecting `query_pairs()` into a `Vec` and editing it
//!   positionally ([`set_first`] / [`delete_all`]) — never `clear()` +
//!   re-`append_pair` (destroys unrelated params) and never
//!   filter-then-append (moves the edited param to the end). Zero oracle
//!   coverage (the only oracle multi-param case has `token` already first)
//!   — see `c2_*`.
//! - **C3**: TWO different encoders meet inside [`build_search_url`]. The
//!   plain template path uses a hand-rolled `encodeURIComponent`
//!   ([`encode_uri_component`]) — space -> `%20`, unreserved set
//!   `A-Za-z0-9 - _ . ! ~ * ' ( )` — and the result is plain string
//!   concatenation, never re-parsed. The Kagi-session path goes through the
//!   `url` crate's own form-urlencoded serialization via
//!   [`rewrite_query_pairs`] — space -> `+`, and `! ' ( ) ~` all get
//!   percent-encoded (the `application/x-www-form-urlencoded` unreserved set
//!   is only `A-Za-z0-9 * - . _`). Not shared with `suaegi-forge`'s
//!   `repo_icon.rs` encoder (different crate, and its header forbids
//!   cross-purpose reuse anyway) — this is an independent, fresh
//!   implementation. See `c3_*`.
//! - **C4**: [`redact_kagi_session_token`] is an INFALLIBLE PASSTHROUGH —
//!   `fn(&str) -> String`, never `Option`/`Result`. On a wrong scheme, wrong
//!   host, wrong path, missing `token`, or a parse failure it returns the
//!   input **verbatim, token and all** (four routes, all oracle-silent) — a
//!   caller's `unwrap_or_default()` over a fallible signature would blank
//!   the URL instead. See `c4_*`.
//! - **C5**: [`redact_kagi_session_token`] never touches the fragment —
//!   `redact_kagi_session_token("https://kagi.com/search?token=A#token=B")`
//!   returns `"https://kagi.com/search#token=B"`: a token surviving a
//!   "successful" redaction, in the fragment. This is an upstream defect in
//!   Orca (`O:178-197` has no `.hash` clearing) **faithfully preserved, NOT
//!   hardened** — hardening it would diverge from the oracle.
//!   [`normalize_kagi_session_link`] DOES clear the fragment (`O:171`,
//!   `parsed.hash = ''`), confirmed by the oracle's `#ignored` disappearing.
//!   See `c5_*`.
//! - **C6**: `port().is_some()`, never `port_or_known_default()` — WHATWG
//!   elides an explicit default port during parsing, so
//!   `https://kagi.com:443/search?token=x` is ACCEPTED (the oracle only
//!   tests `:8443`, so a `port_or_known_default()` regression here would be
//!   invisible to it). `password()` is `Option<&str>`, so "absent or empty"
//!   (`is_none_or`/`is_some_and(!empty)`) is the passing condition to line up
//!   with JS `.password === ''`. See `c6_*`.
//! - **C7**: [`looks_like_search_query`]'s `input.contains(' ')` is
//!   **literal U+0020 only** — never `char::is_whitespace` (a tab does not
//!   count). [`matches_looks_like_url_pattern`] (`/^[^\s]+\.[a-z]{2,}(\/.*)?$/i`)
//!   has three traps: the `/i` flag has no `/u`, so it folds **ASCII only**
//!   (U+017F, U+212A must NOT match `[a-z]`); `\s` is the **ECMAScript**
//!   whitespace set (`suaegi_misc::is_js_whitespace` — includes U+FEFF,
//!   excludes U+0085, the reverse of Rust's `char::is_whitespace`); `.*` has
//!   no dotAll flag, so it cannot cross a line terminator. See `c7_*`.
//! - **C8**: [`js_trim`] on both the raw link and the extracted token, never
//!   `str::trim`. `query_pairs()`'s decoded value matches JS
//!   `searchParams.get()` (`+` -> space, percent-decoded) — so a token whose
//!   raw query value is `+` decodes to a single space, which `js_trim`s down
//!   to empty and is therefore treated as absent. See `c8_*`.
//! - **C9**: the TS `buildKagiSessionSearchUrl` (`O:210`) re-parses
//!   `normalizeKagiSessionLink`'s string output via a bare `new URL(...)`,
//!   which is safe in JS but would need an `.unwrap()` (a panic surface) if
//!   ported literally. [`normalize_kagi_session_url`] is the internal
//!   `&str -> Option<Url>` primitive; the public
//!   [`normalize_kagi_session_link`] is a thin `.map(|u| u.to_string())`
//!   over it, and [`build_kagi_session_search_url`] consumes the `Url`
//!   directly — the re-parse never happens.
//! - **C10**: `!sessionLink` in JS also catches the empty string — modeled
//!   as `Option<&str>` filtered through `.filter(|s| !s.is_empty())`. If the
//!   session link is absent, empty, or fails to normalize,
//!   [`build_search_url`]'s Kagi branch **falls back** to the plain
//!   template — this is not an error path. See `c10_*`.
//! - **C11**: [`SEARCH_ENGINE_LABELS`] has zero oracle coverage (the test
//!   file never imports it) — all four label strings and
//!   [`DEFAULT_SEARCH_ENGINE`] are pinned directly. See `c11_*`.

use suaegi_misc::{is_js_whitespace, js_trim};
use url::Url;

// ---------------------------------------------------------------------------
// O:15 SearchEngine
// ---------------------------------------------------------------------------

/// `O:15`. Declaration order (`Google` = 0 .. `Kagi` = 3) is contractual:
/// [`search_engine_url_template`] indexes `SEARCH_ENGINE_URLS` by
/// `engine as usize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEngine {
    Google,
    DuckDuckGo,
    Bing,
    Kagi,
}

// ---------------------------------------------------------------------------
// O:17-19 SearchUrlOptions
// ---------------------------------------------------------------------------

/// `O:17-19`. TS `kagiSessionLink?: string | null` collapses naturally onto
/// `Option<String>` — both "absent" and "explicitly null" map to `None`.
#[derive(Debug, Clone, Default)]
pub struct SearchUrlOptions {
    pub kagi_session_link: Option<String>,
}

// ---------------------------------------------------------------------------
// O:21-26 SEARCH_ENGINE_LABELS (C11: zero oracle coverage, pin exactly)
// ---------------------------------------------------------------------------

pub const SEARCH_ENGINE_LABELS: [(SearchEngine, &str); 4] = [
    (SearchEngine::Google, "Google"),
    (SearchEngine::DuckDuckGo, "DuckDuckGo"),
    (SearchEngine::Bing, "Bing"),
    (SearchEngine::Kagi, "Kagi"),
];

// ---------------------------------------------------------------------------
// O:28-33 SEARCH_ENGINE_URLS (module-private in the TS source too)
// ---------------------------------------------------------------------------

/// Indexed by [`SearchEngine`]'s declaration order — see
/// [`search_engine_url_template`].
const SEARCH_ENGINE_URLS: [&str; 4] = [
    "https://www.google.com/search?q=",
    "https://duckduckgo.com/?q=",
    "https://www.bing.com/search?q=",
    "https://kagi.com/search?q=",
];

fn search_engine_url_template(engine: SearchEngine) -> &'static str {
    SEARCH_ENGINE_URLS[engine as usize]
}

// ---------------------------------------------------------------------------
// O:35 DEFAULT_SEARCH_ENGINE (C11: pin exactly)
// ---------------------------------------------------------------------------

pub const DEFAULT_SEARCH_ENGINE: SearchEngine = SearchEngine::Google;

// ---------------------------------------------------------------------------
// Shared query-pair rewriting helpers (C1, C2)
// ---------------------------------------------------------------------------

/// Collects `url`'s current query pairs into an owned, ordered `Vec` for
/// positional editing (C2). `query_pairs()` already applies
/// `application/x-www-form-urlencoded` decoding (`+` -> space,
/// percent-decoding), matching JS `URLSearchParams` iteration exactly (C8).
fn collect_query_pairs(url: &Url) -> Vec<(String, String)> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// JS `URLSearchParams.prototype.delete(name)`: removes ALL pairs whose name
/// matches `name`, in place, preserving the relative order of survivors.
fn delete_all(pairs: &mut Vec<(String, String)>, name: &str) {
    pairs.retain(|(k, _)| k != name);
}

/// JS `URLSearchParams.prototype.set(name, value)`: replaces the value of
/// the FIRST pair whose name matches `name` **in place** (preserving its
/// position and its neighbors), and removes every other pair with that name.
/// If no pair matches, appends a new one at the end — not reached by this
/// module's two call sites (both only `set` a key already confirmed
/// present), but implemented for fidelity with the JS method.
fn set_first(pairs: &mut Vec<(String, String)>, name: &str, value: &str) {
    let mut found = false;
    pairs.retain_mut(|(k, v)| {
        if k != name {
            return true;
        }
        if found {
            return false; // remove subsequent duplicates entirely
        }
        *v = value.to_string();
        found = true;
        true
    });
    if !found {
        pairs.push((name.to_string(), value.to_string()));
    }
}

/// Re-serializes `url`'s query string from an explicit, ordered list of
/// `(name, value)` pairs built by [`collect_query_pairs`] +
/// [`delete_all`]/[`set_first`] (C2).
///
/// C1: an empty serialization means a NULL query in WHATWG/JS, not a bare
/// `?`. The `url` crate would happily store `Some("")` and stringify the `?`
/// anyway, so an empty `pairs` list (or a `serialize` call that happens to
/// produce the empty string) must route through `set_query(None)` instead of
/// `set_query(Some(""))`.
fn rewrite_query_pairs(url: &mut Url, pairs: &[(String, String)]) {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    let serialized = serializer.finish();
    // C1: an empty serialization means a NULL query in WHATWG/JS, not a bare
    // `?`. The url crate would happily store Some("") and stringify the `?`.
    if serialized.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&serialized));
    }
}

// ---------------------------------------------------------------------------
// O:142-176 normalizeKagiSessionLink / redactKagiSessionToken
// ---------------------------------------------------------------------------

/// Internal counterpart of [`normalize_kagi_session_link`] that returns a
/// [`Url`] instead of a `String` (C9): the TS `buildKagiSessionSearchUrl`
/// (`O:210`) re-parses `normalizeKagiSessionLink`'s string output via a bare
/// `new URL(...)`, which is safe in JS (the string it just produced is
/// always valid) but would require an `.unwrap()` — and a needless panic
/// surface — if ported literally to Rust. Keeping the already-parsed `Url`
/// around and having [`normalize_kagi_session_link`] be a thin
/// `.map(|u| u.to_string())` over this function eliminates the re-parse
/// entirely; [`build_kagi_session_search_url`] consumes the `Url` directly.
fn normalize_kagi_session_url(raw_link: &str) -> Option<Url> {
    // C8: js_trim, not str::trim.
    let trimmed = js_trim(raw_link);
    // C10: `!trimmed` in JS also catches the empty string.
    if trimmed.is_empty() {
        return None;
    }
    let mut parsed = Url::parse(trimmed).ok()?;
    // O:149: `.toLowerCase()` is full-Unicode (matches M1's B2 precedent).
    let hostname = parsed.host_str().unwrap_or("").to_lowercase();
    let host_ok = hostname == "kagi.com" || hostname == "www.kagi.com";
    let path_ok = parsed.path() == "/search" || parsed.path() == "/search/";
    // C8: `searchParams.get('token')?.trim()` — `query_pairs()` already
    // decodes `+`/percent-escapes, matching `.get()`'s decoded value.
    let token = parsed
        .query_pairs()
        .find(|(name, _)| name == "token")
        .map(|(_, value)| js_trim(&value).to_string());
    let token_ok = token.as_deref().is_some_and(|t| !t.is_empty());
    // C6: `port().is_some()`, NEVER `port_or_known_default()` — an explicit
    // `:443` is accepted because WHATWG elides the default port at parse
    // time (`parsed.port` is `''` for `https://kagi.com:443/...` in JS too).
    // `password()` is `Option<&str>`; JS `.password` is `''` when absent, so
    // "empty-or-absent" is the passing condition on both sides.
    if parsed.scheme() != "https"
        || !host_ok
        || !path_ok
        || !parsed.username().is_empty()
        || parsed.password().is_some_and(|p| !p.is_empty())
        || parsed.port().is_some()
        || !token_ok
    {
        return None;
    }
    let token = token.expect("token_ok guarantees Some");

    let mut pairs = collect_query_pairs(&parsed);
    delete_all(&mut pairs, "q");
    // Why: collapse any duplicate token params so we don't echo two bearer
    // values back to Kagi on every search (O:168-170).
    set_first(&mut pairs, "token", &token);
    rewrite_query_pairs(&mut parsed, &pairs);
    // C5: normalize DOES clear the fragment (`O:171`, `parsed.hash = ''`) —
    // contrast with redact, which never touches it.
    parsed.set_fragment(None);
    Some(parsed)
}

/// `O:142-176`. Public wrapper: `.map(|u| u.to_string())` over
/// [`normalize_kagi_session_url`] — see that function's doc comment for why
/// the TS source's re-parse is eliminated here (C9) rather than ported
/// literally.
pub fn normalize_kagi_session_link(raw_link: &str) -> Option<String> {
    normalize_kagi_session_url(raw_link).map(|u| u.to_string())
}

/// `O:178-197`. C4: INFALLIBLE PASSTHROUGH — `fn(&str) -> String`, never
/// `Option`/`Result`. On any rejection (wrong scheme, wrong host, wrong
/// path, no `token` param, or an unparseable `raw_url`) the input is
/// returned **verbatim, token and all** — mirroring the TS `catch` block's
/// fallthrough to `return rawUrl`. A caller relying on
/// `unwrap_or_default()` over a fallible signature would silently blank a
/// perfectly valid URL instead.
///
/// C5: this function never touches `.hash` (there is no `O:171`-equivalent
/// line in `O:178-197`) — an upstream defect faithfully preserved, NOT
/// hardened here.
/// `redact_kagi_session_token("https://kagi.com/search?token=A#token=B")`
/// therefore returns `"https://kagi.com/search#token=B"`: the query token is
/// stripped but an identically-named fragment token survives untouched.
pub fn redact_kagi_session_token(raw_url: &str) -> String {
    let Ok(mut parsed) = Url::parse(raw_url) else {
        return raw_url.to_string();
    };
    let hostname = parsed.host_str().unwrap_or("").to_lowercase();
    let host_ok = hostname == "kagi.com" || hostname == "www.kagi.com";
    let path_ok = parsed.path() == "/search" || parsed.path() == "/search/";
    let has_token = parsed.query_pairs().any(|(name, _)| name == "token");
    if parsed.scheme() != "https" || !host_ok || !path_ok || !has_token {
        return raw_url.to_string();
    }
    let mut pairs = collect_query_pairs(&parsed);
    delete_all(&mut pairs, "token");
    rewrite_query_pairs(&mut parsed, &pairs);
    parsed.to_string()
}

// ---------------------------------------------------------------------------
// O:199-213 buildKagiSessionSearchUrl (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:199-213`. C10: `!sessionLink` in JS also catches the empty string —
/// modeled as filtering out `Some("")`, so both `None` and an empty session
/// link fall through to `None` here (and from there to
/// [`build_search_url`]'s plain-template fallback). C9: consumes the
/// already-parsed [`Url`] from [`normalize_kagi_session_url`] directly
/// instead of re-parsing its string output.
fn build_kagi_session_search_url(query: &str, session_link: Option<&str>) -> Option<String> {
    let session_link = session_link.filter(|s| !s.is_empty())?;
    let mut parsed = normalize_kagi_session_url(session_link)?;
    // O:211: `searchParams.set('q', query)` — Path B encoder (C3): space ->
    // `+`, `! ' ( ) ~` percent-encoded, via the `url` crate's own
    // form-urlencoded serialization (never the hand-rolled Path A encoder in
    // [`encode_uri_component`]). `q` was already removed by `normalize`, so
    // this always appends a fresh pair at the end.
    let mut pairs = collect_query_pairs(&parsed);
    set_first(&mut pairs, "q", query);
    rewrite_query_pairs(&mut parsed, &pairs);
    Some(parsed.to_string())
}

// ---------------------------------------------------------------------------
// O:215-227 buildSearchUrl
// ---------------------------------------------------------------------------

/// `O:215-227`. C10: the Kagi branch falls back to the plain template (not
/// an error) whenever [`build_kagi_session_search_url`] returns `None` —
/// covers an absent/empty session link and one that fails to normalize.
pub fn build_search_url(query: &str, engine: SearchEngine, options: SearchUrlOptions) -> String {
    if engine == SearchEngine::Kagi {
        if let Some(session_search_url) =
            build_kagi_session_search_url(query, options.kagi_session_link.as_deref())
        {
            return session_search_url;
        }
    }
    // Path A encoder (C3): plain string concatenation, never re-parsed
    // through a `Url`.
    format!(
        "{}{}",
        search_engine_url_template(engine),
        encode_uri_component(query)
    )
}

// ---------------------------------------------------------------------------
// O:10 LOOKS_LIKE_URL_PATTERN (hand-rolled, module-private)
// ---------------------------------------------------------------------------

/// `/^[^\s]+\.[a-z]{2,}(\/.*)?$/i`. Hand-rolled instead of `regex` (the
/// crate adds no new dependency; matches M1's precedent). C7 traps:
/// - the `/i` flag has NO `/u`, so it is an **ASCII-only** case fold: `[a-z]`
///   here means `char::is_ascii_alphabetic()`, nothing more — U+017F (LATIN
///   SMALL LETTER LONG S) and U+212A (KELVIN SIGN) do NOT match it, even
///   though `str::to_lowercase()` would fold them to `s`/`k`.
/// - `\s` is the ECMAScript whitespace set (`suaegi_misc::is_js_whitespace`),
///   which **includes U+FEFF** and **excludes U+0085** — the reverse of
///   Rust's `char::is_whitespace`.
/// - `.*` has no dotAll flag, so it cannot cross a line terminator (`\n`,
///   `\r`, U+2028, U+2029); one anywhere in an otherwise-matching `/...`
///   tail fails the whole pattern (mirrors this crate's M1
///   `tail_matches` for `LOCAL_ADDRESS_PATTERN`).
///
/// Because `[^\s]+` is anchored at the very start (`^`) and forbids
/// whitespace outright (rather than merely stopping at the first one), its
/// only achievable match span is some prefix of the string's leading
/// whitespace-free run; and because every character absorbed by
/// `[a-z]{2,}`'s greedy run is itself an ASCII letter (never `/` or
/// end-of-input), shrinking that run can only ever leave another ASCII
/// letter exactly where `(\/.*)?$` needs `/` or end-of-string — so only the
/// maximal run length is ever worth checking. No general backtracking search
/// is needed: for each candidate `.` position, computing the maximal
/// ASCII-letter run right after it and checking exactly one tail position is
/// equivalent to the full regex engine's exploration.
fn matches_looks_like_url_pattern(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for dot in 1..len {
        if chars[dot] != '.' {
            continue;
        }
        if chars[..dot].iter().copied().any(is_js_whitespace) {
            continue;
        }
        let letters_start = dot + 1;
        let mut letters_end = letters_start;
        while letters_end < len && chars[letters_end].is_ascii_alphabetic() {
            letters_end += 1;
        }
        if letters_end - letters_start < 2 {
            continue;
        }
        if letters_end == len {
            return true;
        }
        if chars[letters_end] == '/' {
            let tail_has_line_terminator = chars[letters_end + 1..]
                .iter()
                .any(|&c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'));
            if !tail_has_line_terminator {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// O:229-240 looksLikeSearchQuery
// ---------------------------------------------------------------------------

/// `O:229-240`. C7: `input.includes(' ')` is **literal U+0020 only** — a tab
/// or any other whitespace character does not trigger this branch (never
/// `input.contains(char::is_whitespace)`, which would diverge — see the
/// `c7_*` pins). Exported but with zero oracle coverage (the test file never
/// imports it) — the `:` branch in particular has no direct or indirect
/// coverage in the oracle at all.
pub fn looks_like_search_query(input: &str) -> bool {
    if input.contains(' ') {
        return true;
    }
    if matches_looks_like_url_pattern(input) {
        return false;
    }
    if input.contains('.') || input.contains(':') {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// encodeURIComponent (Path A encoder, C3) — module-private, hand-rolled
// ---------------------------------------------------------------------------

/// Hand-rolled `encodeURIComponent` (Path A of C3): unreserved set
/// `A-Za-z0-9 - _ . ! ~ * ' ( )`, space -> `%20`. Used only by the plain
/// template path in [`build_search_url`] — plain string concatenation, the
/// result is never re-parsed through a `Url`. NOT shared with
/// `suaegi-forge`'s `repo_icon.rs` encoder (different crate, unreachable
/// from here anyway, and its own header forbids cross-purpose reuse) — this
/// is a fresh, independent implementation for this exact JS semantics.
pub(crate) fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if is_encode_uri_component_unreserved(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

fn is_encode_uri_component_unreserved(ch: char) -> bool {
    matches!(
        ch,
        'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '!' | '~' | '*' | '\'' | '(' | ')'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Oracle block `T:215-223` — buildSearchUrl, plain template (%20).
    // -----------------------------------------------------------------

    #[test]
    fn oracle_builds_search_urls_with_percent20_space_encoding() {
        // `bing` is not covered by this oracle block.
        assert_eq!(
            build_search_url(
                "hello world",
                SearchEngine::Google,
                SearchUrlOptions::default()
            ),
            "https://www.google.com/search?q=hello%20world"
        );
        assert_eq!(
            build_search_url(
                "hello world",
                SearchEngine::DuckDuckGo,
                SearchUrlOptions::default()
            ),
            "https://duckduckgo.com/?q=hello%20world"
        );
        assert_eq!(
            build_search_url(
                "hello world",
                SearchEngine::Kagi,
                SearchUrlOptions::default()
            ),
            "https://kagi.com/search?q=hello%20world"
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:225-238` — Kagi private session link (+ encoding).
    // -----------------------------------------------------------------

    #[test]
    fn oracle_uses_kagi_private_session_link_when_configured() {
        // The third assertion in the source block (`T:234-237`) calls
        // `normalizeBrowserNavigationUrl`, which is M3 scope — skipped here,
        // it lands when that function is ported.
        let session_link = "https://kagi.com/search?token=secret&q=%s#ignored";
        assert_eq!(
            normalize_kagi_session_link(session_link),
            Some("https://kagi.com/search?token=secret".to_string())
        );
        assert_eq!(
            build_search_url(
                "hello world",
                SearchEngine::Kagi,
                SearchUrlOptions {
                    kagi_session_link: Some(session_link.to_string())
                }
            ),
            "https://kagi.com/search?token=secret&q=hello+world"
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:240-246` — rejects invalid Kagi session links.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_rejects_invalid_kagi_session_links() {
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search?q=%s"),
            None
        );
        assert_eq!(
            normalize_kagi_session_link("http://kagi.com/search?token=secret"),
            None
        );
        assert_eq!(
            normalize_kagi_session_link("https://example.com/search?token=secret"),
            None
        );
        assert_eq!(
            normalize_kagi_session_link("https://user:pass@kagi.com/search?token=secret"),
            None
        );
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com:8443/search?token=secret"),
            None
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:248-252` — /search/ trailing slash accepted.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_accepts_kagi_search_trailing_slash() {
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search/?token=secret"),
            Some("https://kagi.com/search/?token=secret".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:254-258` — duplicate token params collapse.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_collapses_duplicate_token_params() {
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search?token=A&token=B"),
            Some("https://kagi.com/search?token=A".to_string())
        );
    }

    // -----------------------------------------------------------------
    // Oracle block `T:260-270` — redaction.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_redacts_kagi_session_tokens() {
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?token=secret&q=hello+world"),
            "https://kagi.com/search?q=hello+world"
        );
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?q=hello+world"),
            "https://kagi.com/search?q=hello+world"
        );
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search/?token=secret&q=hi"),
            "https://kagi.com/search/?q=hi"
        );
    }

    // -----------------------------------------------------------------
    // C1 — no bare `?` left behind when the query empties.
    // -----------------------------------------------------------------

    #[test]
    fn c1_redact_leaves_no_trailing_question_mark_when_token_is_sole_param() {
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?token=secret"),
            "https://kagi.com/search"
        );
    }

    #[test]
    fn c1_rewrite_query_pairs_clears_bare_question_mark_when_empty() {
        // Direct pin on the shared helper both public functions route
        // through, so the guarantee is proven at the mechanism, not just at
        // one caller.
        let mut url = Url::parse("https://kagi.com/search?token=secret").unwrap();
        rewrite_query_pairs(&mut url, &[]);
        assert_eq!(url.as_str(), "https://kagi.com/search");
        assert_eq!(url.query(), None);
    }

    // -----------------------------------------------------------------
    // C2 — parameter position and neighbors survive.
    // -----------------------------------------------------------------

    #[test]
    fn c2_normalize_preserves_parameter_position_and_neighbors() {
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search?a=1&token=X&b=2"),
            Some("https://kagi.com/search?a=1&token=X&b=2".to_string())
        );
    }

    // -----------------------------------------------------------------
    // C3 — two encoders, provably different on the same input.
    // -----------------------------------------------------------------

    #[test]
    fn c3_two_encoders_diverge_on_reserved_characters() {
        let query = "hello world!'()~";
        // Path A: plain template, hand-rolled encodeURIComponent — space ->
        // `%20`, `! ' ( ) ~` left unescaped.
        assert_eq!(
            build_search_url(query, SearchEngine::Google, SearchUrlOptions::default()),
            "https://www.google.com/search?q=hello%20world!'()~"
        );
        // Path B: Kagi session link, `url` crate's form-urlencoded
        // serialization — space -> `+`, `! ' ( ) ~` all percent-encoded.
        let options = SearchUrlOptions {
            kagi_session_link: Some("https://kagi.com/search?token=secret".to_string()),
        };
        assert_eq!(
            build_search_url(query, SearchEngine::Kagi, options),
            "https://kagi.com/search?token=secret&q=hello+world%21%27%28%29%7E"
        );
    }

    // -----------------------------------------------------------------
    // C4 — infallible passthrough, all four routes.
    // -----------------------------------------------------------------

    // -----------------------------------------------------------------
    // C4 — duplicate tokens: a partial delete would leak a live bearer
    // token, so both occurrences must be removed by redaction.
    // -----------------------------------------------------------------

    #[test]
    fn c4_duplicate_tokens_are_all_removed() {
        // Security-relevant: if `delete_all` only dropped the FIRST matching
        // pair (mirroring `set_first`'s single-slot semantics instead of
        // `URLSearchParams.prototype.delete`'s "remove every occurrence"),
        // the second `token=B` would survive redaction and leak a live
        // bearer token to any consumer of the "redacted" URL.
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?token=A&token=B&q=x"),
            "https://kagi.com/search?q=x"
        );
        // Also re-covers C1: once both tokens are gone the query is empty,
        // so no trailing bare `?` may remain.
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?token=A&token=B"),
            "https://kagi.com/search"
        );
    }

    #[test]
    fn c4_all_four_passthrough_routes_preserve_token_verbatim() {
        assert_eq!(
            redact_kagi_session_token("http://kagi.com/search?token=secret"),
            "http://kagi.com/search?token=secret"
        );
        assert_eq!(
            redact_kagi_session_token("https://example.com/search?token=secret"),
            "https://example.com/search?token=secret"
        );
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/other?token=secret"),
            "https://kagi.com/other?token=secret"
        );
        assert_eq!(redact_kagi_session_token("not a url"), "not a url");
    }

    // -----------------------------------------------------------------
    // C5 — fragment asymmetry: redact leaks, normalize clears.
    // -----------------------------------------------------------------

    #[test]
    fn c5_redact_leaves_fragment_token_but_normalize_clears_fragment() {
        // Documented upstream defect, faithfully preserved — NOT hardened.
        assert_eq!(
            redact_kagi_session_token("https://kagi.com/search?token=A#token=B"),
            "https://kagi.com/search#token=B"
        );
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search?token=A#frag"),
            Some("https://kagi.com/search?token=A".to_string())
        );
    }

    // -----------------------------------------------------------------
    // C6 — explicit default port accepted; host casing; explicit port
    // rejected.
    // -----------------------------------------------------------------

    #[test]
    fn c6_default_port_accepted_explicit_port_rejected_host_casing_accepted() {
        // WHATWG elides an explicit default port at parse time, so this is
        // ACCEPTED — `port_or_known_default()` would wrongly reject it.
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com:443/search?token=x"),
            Some("https://kagi.com/search?token=x".to_string())
        );
        assert!(normalize_kagi_session_link("https://www.kagi.com/search?token=x").is_some());
        assert!(normalize_kagi_session_link("https://KAGI.COM/search?token=x").is_some());
        // A genuinely non-default port is rejected.
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com:8443/search?token=x"),
            None
        );
    }

    // -----------------------------------------------------------------
    // C7 — looks_like_search_query / LOOKS_LIKE_URL_PATTERN traps.
    // -----------------------------------------------------------------

    #[test]
    fn c7_tab_does_not_trigger_space_branch_but_pattern_still_matches() {
        // Literal tab, no literal space: `input.contains(' ')` (step 1)
        // does NOT fire, so execution reaches the pattern check, which DOES
        // match (a tab is not a line terminator, so it's allowed inside
        // `(\/.*)?`'s tail).
        assert!(matches_looks_like_url_pattern("foo.com/a\tb"));
        assert!(!looks_like_search_query("foo.com/a\tb"));
    }

    #[test]
    fn c7_ascii_only_case_fold_rejects_kelvin_and_long_s() {
        assert!(!matches_looks_like_url_pattern("abc.a\u{017F}")); // ſ
        assert!(!matches_looks_like_url_pattern("abc.k\u{212A}")); // KELVIN SIGN
                                                                   // Control: plain ASCII letters of the same shape DO match.
        assert!(matches_looks_like_url_pattern("abc.ab"));
    }

    #[test]
    fn c7_feff_does_not_match_non_whitespace_class() {
        assert!(!matches_looks_like_url_pattern("\u{FEFF}foo.com"));
    }

    #[test]
    fn c7_newline_in_tail_is_rejected() {
        assert!(!matches_looks_like_url_pattern("foo.com/bar\nbaz"));
    }

    #[test]
    fn c7_colon_branch_without_dot() {
        assert!(!looks_like_search_query("localhost:3000"));
    }

    #[test]
    fn c7_dot_branch_without_colon_pins_the_dot_half_of_the_guard() {
        // All four inputs below contain a `.` but do NOT match
        // `LOOKS_LIKE_URL_PATTERN`, so — unlike every dotted input in the
        // other oracle/pin cases — the `.` half of
        // `input.contains('.') || input.contains(':')` is the check that
        // actually decides the outcome here.
        assert!(!looks_like_search_query("foo.")); // trailing dot: `[a-z]{2,}` needs >=2 letters after it
        assert!(!looks_like_search_query("foo.a")); // single-letter TLD fails `{2,}`
        assert!(!looks_like_search_query(".com")); // no `[^\s]+` before the dot
                                                   // Contrasting true case: no `.`, no `:`, no space.
        assert!(looks_like_search_query("react"));
    }

    // -----------------------------------------------------------------
    // C8 — js_trim on the token; `+` decodes to space before trimming.
    // -----------------------------------------------------------------

    #[test]
    fn c8_feff_padded_token_is_trimmed_nel_padded_is_not() {
        let feff_link = "https://kagi.com/search?token=\u{FEFF}secret\u{FEFF}";
        assert_eq!(
            normalize_kagi_session_link(feff_link),
            Some("https://kagi.com/search?token=secret".to_string())
        );
        let nel_link = "https://kagi.com/search?token=\u{0085}secret\u{0085}";
        let normalized =
            normalize_kagi_session_link(nel_link).expect("NEL is not ECMAScript whitespace");
        assert_ne!(normalized, "https://kagi.com/search?token=secret");
    }

    #[test]
    fn c8_raw_link_side_feff_stripped_nel_kept_pins_js_trim_on_the_link() {
        // The token-side `js_trim` is pinned by `c8_feff_padded_token_...`
        // above; this pins the RAW-LINK side, which `str::trim` would also
        // satisfy for ordinary whitespace, but diverges from on ECMAScript's
        // exact set.
        //
        // U+FEFF (BOM) IS ECMAScript whitespace: `js_trim` strips it from
        // the raw link and the URL parses normally.
        assert_eq!(
            normalize_kagi_session_link("\u{FEFF}https://kagi.com/search?token=secret"),
            Some("https://kagi.com/search?token=secret".to_string())
        );
        // U+0085 (NEL) is NOT ECMAScript whitespace: `js_trim` leaves it in
        // place, so it sits before the scheme and the URL fails to parse.
        // Under `str::trim` (which DOES strip NEL) this would wrongly
        // succeed — this pins the divergence in that direction.
        assert_eq!(
            normalize_kagi_session_link("\u{0085}https://kagi.com/search?token=secret"),
            None
        );
    }

    #[test]
    fn c8_plus_only_token_decodes_to_blank_space_and_is_rejected() {
        // `+` decodes to a single space (`query_pairs()` applies
        // `application/x-www-form-urlencoded` decoding, matching JS
        // `searchParams.get()`); `js_trim(" ")` is empty, so the token is
        // treated as absent. If `+` were NOT decoded, the raw `"+"` would
        // survive `js_trim` untouched and this link would be ACCEPTED.
        assert_eq!(
            normalize_kagi_session_link("https://kagi.com/search?token=+"),
            None
        );
    }

    // -----------------------------------------------------------------
    // C10 — empty / non-normalizing session link falls back, not errors.
    // -----------------------------------------------------------------

    #[test]
    fn c10_empty_and_non_normalizing_session_link_fall_back_to_plain_template() {
        let empty_options = SearchUrlOptions {
            kagi_session_link: Some(String::new()),
        };
        assert_eq!(
            build_search_url("hello world", SearchEngine::Kagi, empty_options),
            "https://kagi.com/search?q=hello%20world"
        );
        let bad_options = SearchUrlOptions {
            kagi_session_link: Some("https://example.com/not-kagi".to_string()),
        };
        assert_eq!(
            build_search_url("hello world", SearchEngine::Kagi, bad_options),
            "https://kagi.com/search?q=hello%20world"
        );
    }

    // -----------------------------------------------------------------
    // C11 — labels and default engine, pinned exactly.
    // -----------------------------------------------------------------

    #[test]
    fn c11_labels_and_default_engine_pinned_exactly() {
        assert_eq!(
            SEARCH_ENGINE_LABELS,
            [
                (SearchEngine::Google, "Google"),
                (SearchEngine::DuckDuckGo, "DuckDuckGo"),
                (SearchEngine::Bing, "Bing"),
                (SearchEngine::Kagi, "Kagi"),
            ]
        );
        assert_eq!(DEFAULT_SEARCH_ENGINE, SearchEngine::Google);
    }
}
