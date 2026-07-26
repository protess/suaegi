//! VERBATIM port of the local-dev-address / certificate-host cluster of
//! Orca's `src/shared/browser-url.ts`, milestone M1.
//!
//! Ported: `O:3-4` `LOCAL_ADDRESS_PATTERN` (hand-rolled, see
//! [`matches_local_address_pattern`]), `O:37-47`
//! [`classify_scheme_less_local_dev_address`], `O:49-53`
//! `normalizeCertificateHostname`, `O:55-65` `isValidDnsName`, `O:67-77`
//! `isIpv4Loopback`, `O:79-88` [`is_eligible_local_certificate_host`],
//! `O:90-93` `isWildcardBindHost`, `O:95-106`
//! [`to_https_recovery_url`], `O:108-125` [`to_secure_certificate_endpoint`].
//!
//! `normalizeCertificateHostname` / `isValidDnsName` / `isIpv4Loopback` /
//! `isWildcardBindHost` are NOT exported by the TS module (no `export`
//! keyword) — only reachable through the four exported functions above and
//! through this crate's own tests, so they stay module-private here too.
//!
//! Deliberately NOT ported (see the M1 plan, `docs/superpowers/plans/
//! 2026-07-26-browser-url-m1.md`): everything at `browser-url.ts:130` and
//! below — `resolveRemoteFailureExternalUrl`, the search-engine constants,
//! the Kagi session/redaction functions, and the navigation/file-URL
//! decision tree. Those land in later PRs (M2/M3).
//!
//! # Traps (see the plan's §2 for the full rationale)
//! - **B1**: `.host`/`.origin`/`.href` appear nowhere in this module, so a
//!   `port_or_known_default()`-style helper (reviving WHATWG's elided
//!   default port) has zero legitimate use here. The one `.port` read in
//!   scope (`O:121`, `parsed.port || '443'`) is `port().map(|p|
//!   p.to_string()).unwrap_or_else(|| "443".into())` — nothing more.
//! - **B2**: TWO case-folding mechanisms coexist and must never be unified.
//!   `O:50`'s `.toLowerCase()` is full-Unicode -> Rust `str::to_lowercase()`
//!   (U+212A KELVIN SIGN folds to `k`). `O:4`'s regex `/i` (no `/u`) is
//!   ASCII-only -> [`matches_local_address_pattern`] uses
//!   `eq_ignore_ascii_case`/byte-level ASCII checks exclusively, so U+212A
//!   and U+017F (ſ) do NOT fold there.
//! - **B3**: JS `\d` is ASCII-only; Rust's Unicode `\d` (`Nd`) is not used
//!   anywhere here — every digit check is `u8::is_ascii_digit`.
//! - **B4**: every `.trim()` in scope (`O:38`, `O:50`) is
//!   `suaegi_misc::js_trim`, never `str::trim` (they diverge at U+FEFF and
//!   U+0085).
//! - **B5**: `normalize_certificate_hostname`'s four steps are ordered and
//!   each strip happens exactly once: js_trim -> to_lowercase -> strip one
//!   leading `[` and one trailing `]` (only if BOTH present) -> strip one
//!   trailing `.`. Bracket-stripping precedes dot-stripping, so `"[::1]."`
//!   does NOT lose its brackets. `strip_prefix`/`strip_suffix` only — never
//!   `&s[1..s.len()-1]` (panics on non-ASCII trailing bytes).
//! - **B6**: `is_ipv4_loopback` rejects leading zeros via re-serialization
//!   (`part == value.to_string()`), requires exactly 4 `split('.')` parts
//!   (never `split_terminator`, which would swallow a trailing empty part
//!   and wrongly accept `"127.0.0.1."`), and accepts the whole
//!   `127.0.0.0/8` block, not just `127.0.0.1`.
//! - **B7**: `is_valid_dns_name`'s length caps are UTF-16 code units
//!   (`encode_utf16().count()`); the label pattern has NO case-insensitive
//!   flag (it relies on the prior `to_lowercase()`).
//! - **B8**: `*.local` is handled nowhere and must be rejected; `*.localhost`
//!   is accepted at any depth; `"0.0.0.0"` is loopback-ineligible but IS a
//!   wildcard bind host.
//! - **B9**: the loose regex and the strict predicate deliberately do NOT
//!   share validation. `LOCAL_ADDRESS_PATTERN`'s `\d{1,3}` admits
//!   `"127.0.0.01"`; real validation is delegated to `Url::parse`, which
//!   applies WHATWG's IPv4 rules and normalizes it to `127.0.0.1`.
//!   `is_eligible_local_certificate_host("127.00.0.1")` takes the raw-string
//!   path and never sees the URL parser, so it stays `false`.
//! - **B10**: `Url::set_scheme` returns a `Result` and can silently no-op;
//!   its result is always checked, returning `None` on failure rather than
//!   `let _ = ...`.
//! - **B11**: `LOCAL_ADDRESS_PATTERN` has no `s` flag, so its trailing `.*`
//!   does not cross a newline (or `\r`, U+2028, U+2029) — a tail containing
//!   one of those fails the pattern outright.

use suaegi_misc::js_trim;
use url::Url;

mod search;
pub use search::{
    build_search_url, looks_like_search_query, normalize_kagi_session_link,
    redact_kagi_session_token, SearchEngine, SearchUrlOptions, DEFAULT_SEARCH_ENGINE,
    SEARCH_ENGINE_LABELS,
};

// ---------------------------------------------------------------------------
// O:3-4 LOCAL_ADDRESS_PATTERN
// ---------------------------------------------------------------------------
//
// `/^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[[0-9a-f:]+\])(?::\d+)?(?:[/?#].*)?$/i`
//
// Hand-rolled instead of `regex` (see the crate's Cargo.toml "Why"). The
// grammar below is fully deterministic with NO backtracking search needed:
// every `\d{1,3}` / `\d+` run is immediately followed by a literal ('.', or
// nothing but end-of-input/optional-group-start), so greedily consuming the
// maximal run is always the only choice that can possibly lead to an overall
// match — see the doc comments on each helper for the per-case argument.

/// Returns the byte offset right after a successfully matched host
/// alternative, or `None`. The returned offset is always on a `char`
/// boundary because every byte consumed on the way there is ASCII.
fn match_local_host(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();

    // `localhost` — ASCII-only case fold (B2): NOT `str::to_lowercase`.
    if bytes.len() >= 9 && bytes[..9].eq_ignore_ascii_case(b"localhost") {
        return Some(9);
    }

    // `127(?:\.\d{1,3}){3}` — B3: `\d` is ASCII digits only.
    if bytes.starts_with(b"127") {
        let mut pos = 3;
        for _ in 0..3 {
            if bytes.get(pos) != Some(&b'.') {
                return None;
            }
            pos += 1;
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() && pos - start < 3 {
                pos += 1;
            }
            if pos == start {
                return None; // zero digits: `\d{1,3}` needs at least one
            }
            if bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                // A 4th+ consecutive digit can never be absorbed elsewhere
                // (the next required token is always non-digit), so this
                // octet position can never match — no point backtracking.
                return None;
            }
        }
        return Some(pos);
    }

    // `0\.0\.0\.0` — a literal, not an octet pattern (deliberately does NOT
    // accept e.g. "0.0.0.1"; that's a different function's concern).
    if bytes.starts_with(b"0.0.0.0") {
        return Some(7);
    }

    // `\[[0-9a-f:]+\]` — ASCII-only hex/colon class, case-insensitive (B2).
    if bytes.first() == Some(&b'[') {
        let mut pos = 1;
        while pos < bytes.len() && is_bracket_body_byte(bytes[pos]) {
            pos += 1;
        }
        if pos == 1 {
            return None; // `+` needs at least one allowed char
        }
        if bytes.get(pos) == Some(&b']') {
            return Some(pos + 1);
        }
        return None;
    }

    None
}

fn is_bracket_body_byte(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'a'..=b'f' | b'A'..=b'F') || b == b':'
}

/// `(?::\d+)?` — optional port. B3: ASCII digits only. Consuming the
/// maximal digit run is the only viable choice (a leftover digit can never
/// be absorbed by the optional tail group, whose first char must be one of
/// `/?#`, nor by end-of-input).
fn match_optional_port(bytes: &[u8], pos: usize) -> usize {
    if bytes.get(pos) != Some(&b':') {
        return pos;
    }
    let mut p = pos + 1;
    let start = p;
    while p < bytes.len() && bytes[p].is_ascii_digit() {
        p += 1;
    }
    if p > start {
        p
    } else {
        pos // `\d+` needs >=1 digit; group doesn't apply, ':' stays unconsumed
    }
}

/// `(?:[/?#].*)?$` — optional tail. B11: no `s` flag, so `.` excludes LF,
/// CR, U+2028 and U+2029; since there is nothing after this group but `$`,
/// a line terminator anywhere in the remainder makes the whole pattern fail
/// (greedy `.*` cannot cross it, and no amount of backtracking recovers
/// `$`).
fn tail_matches(s: &str, pos: usize) -> bool {
    let rest = &s[pos..];
    if rest.is_empty() {
        return true;
    }
    match rest.chars().next() {
        Some('/') | Some('?') | Some('#') => !rest
            .chars()
            .any(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')),
        _ => false,
    }
}

fn matches_local_address_pattern(s: &str) -> bool {
    match match_local_host(s) {
        Some(host_end) => {
            let port_end = match_optional_port(s.as_bytes(), host_end);
            tail_matches(s, port_end)
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// O:37-47 classifySchemeLessLocalDevAddress
// ---------------------------------------------------------------------------

/// `O:37-47`. B4: `rawInput.trim()` is `js_trim`, not `str::trim`. B9: the
/// pattern is deliberately loose (`127.0.0.01` passes it); all real
/// validation — octal-ish IPv4 canonicalization, port range, etc. — is
/// delegated to [`Url::parse`], matching JS's `new URL(...)`.
pub fn classify_scheme_less_local_dev_address(raw_input: &str) -> Option<Url> {
    let trimmed = js_trim(raw_input);
    if !matches_local_address_pattern(trimmed) {
        return None;
    }
    Url::parse(&format!("http://{trimmed}")).ok()
}

// ---------------------------------------------------------------------------
// O:49-53 normalizeCertificateHostname (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:49-53`. B5: order is contractual and each strip happens exactly once —
/// js_trim -> `to_lowercase()` (full Unicode, B2) -> strip one leading `[`
/// and one trailing `]` (only if BOTH are present, via
/// `strip_prefix('[').and_then(strip_suffix(']'))`, falling back to the
/// unmodified string when either delimiter is missing) -> strip one trailing
/// `.`. Bracket-stripping precedes dot-stripping, so `"[::1]."` keeps its
/// brackets (its `to_lowercase()` result doesn't end with `]`, it ends with
/// `.`) while `"[::1]"` loses them.
fn normalize_certificate_hostname(hostname: &str) -> String {
    let trimmed = js_trim(hostname);
    let lower = trimmed.to_lowercase();
    let unbracketed: &str = match lower.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        Some(inner) => inner,
        None => lower.as_str(),
    };
    match unbracketed.strip_suffix('.') {
        Some(stripped) => stripped.to_string(),
        None => unbracketed.to_string(),
    }
}

// ---------------------------------------------------------------------------
// O:55-65 isValidDnsName (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:55-65`. B7: length caps are counted in UTF-16 code units
/// (`encode_utf16().count()`), matching JS `String.prototype.length`. The
/// label pattern has NO case-insensitive flag (relies on the caller's prior
/// `to_lowercase()`).
fn is_valid_dns_name(name: &str) -> bool {
    let total_len = name.encode_utf16().count();
    if total_len == 0 || total_len > 253 {
        return false;
    }
    name.split('.').all(|label| {
        let label_len = label.encode_utf16().count();
        label_len > 0 && label_len <= 63 && matches_dns_label_pattern(label)
    })
}

/// `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$` — no `/i` flag (B7): only lowercase
/// ASCII letters, ASCII digits and `-`; no leading/trailing hyphen; no empty
/// label (guarded by the caller's `label_len > 0`, but this also handles an
/// empty slice defensively).
fn matches_dns_label_pattern(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || !bytes.is_ascii() {
        return false;
    }
    let is_lower_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if bytes.len() == 1 {
        return is_lower_alnum(bytes[0]);
    }
    if !is_lower_alnum(bytes[0]) || !is_lower_alnum(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes[1..bytes.len() - 1]
        .iter()
        .all(|&b| is_lower_alnum(b) || b == b'-')
}

// ---------------------------------------------------------------------------
// O:67-77 isIpv4Loopback (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:67-77`. B6: `split('.')` (never `split_terminator`, which would drop a
/// trailing empty part and wrongly accept `"127.0.0.1."`) must yield exactly
/// 4 parts; each part is `1..=3` ASCII digits (B3); every value is
/// `0..=255`; the first value is `127` (so all of `127.0.0.0/8` is
/// eligible, not just `127.0.0.1`); and — the leading-zero guard — each part
/// re-serializes back to itself (`part == value.to_string()`), which is what
/// rejects `"127.00.0.1"`.
fn is_ipv4_loopback(hostname: &str) -> bool {
    let parts: Vec<&str> = hostname.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let mut values = [0u32; 4];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let value: u32 = match part.parse() {
            Ok(value) => value,
            Err(_) => return false,
        };
        if value > 255 || *part != value.to_string() {
            return false;
        }
        values[index] = value;
    }
    values[0] == 127
}

// ---------------------------------------------------------------------------
// O:79-88 isEligibleLocalCertificateHost
// ---------------------------------------------------------------------------

/// `O:79-88`.
pub fn is_eligible_local_certificate_host(hostname: &str) -> bool {
    let normalized = normalize_certificate_hostname(hostname);
    if normalized == "::1" || is_ipv4_loopback(&normalized) {
        return true;
    }
    if !is_valid_dns_name(&normalized) {
        return false;
    }
    // B8: `*.local` is handled nowhere in the TS source and must stay
    // rejected; `*.localhost` is accepted at any depth via `endsWith`.
    normalized == "localhost" || normalized.ends_with(".localhost")
}

// ---------------------------------------------------------------------------
// O:90-93 isWildcardBindHost (module-private in the TS source)
// ---------------------------------------------------------------------------

/// `O:90-93`. B8: `"0.0.0.0"` is loopback-ineligible but IS a wildcard bind
/// host; `"::"` is the IPv6 counterpart. Its only TS caller,
/// `resolveRemoteFailureExternalUrl` (`O:130-140`), is out of scope for M1
/// (deferred to M3), so this has no production caller yet — only the `b8`
/// pin below exercises it. Ported now anyway per the M1 plan's explicit
/// item list, so its behavior is locked in ahead of the M3 wiring.
#[cfg_attr(not(test), allow(dead_code))]
fn is_wildcard_bind_host(hostname: &str) -> bool {
    let normalized = normalize_certificate_hostname(hostname);
    normalized == "0.0.0.0" || normalized == "::"
}

// ---------------------------------------------------------------------------
// O:95-106 toHttpsRecoveryUrl
// ---------------------------------------------------------------------------

/// `O:95-106`. B10: `Url::set_scheme`'s `Result` is checked — a silent
/// `let _ = ...` would turn a failed scheme swap into a falsely-successful
/// recovery URL. The `url` crate re-runs `set_port` after a scheme change,
/// which is what makes `http://localhost:80/path` become
/// `https://localhost/path` (the default port is re-elided) rather than
/// `https://localhost:80/path`.
pub fn to_https_recovery_url(raw_url: &str) -> Option<String> {
    let mut parsed = Url::parse(raw_url).ok()?;
    let hostname = parsed.host_str().unwrap_or("").to_string();
    if parsed.scheme() != "http" || !is_eligible_local_certificate_host(&hostname) {
        return None;
    }
    if parsed.set_scheme("https").is_err() {
        return None;
    }
    Some(parsed.to_string())
}

// ---------------------------------------------------------------------------
// O:108-125 toSecureCertificateEndpoint
// ---------------------------------------------------------------------------

/// `O:108-125`. B1: the only `.port` read in scope is `parsed.port ||
/// '443'`, ported as `port().map(...).unwrap_or_else(|| "443".into())` —
/// never `port_or_known_default()`, which would revive a default port that
/// WHATWG deliberately elides in other contexts this module never touches.
pub fn to_secure_certificate_endpoint(raw_url: &str) -> Option<String> {
    let parsed = Url::parse(raw_url).ok()?;
    if parsed.scheme() != "https" && parsed.scheme() != "wss" {
        return None;
    }
    let hostname = parsed.host_str().unwrap_or("");
    let normalized_hostname = normalize_certificate_hostname(hostname);
    if normalized_hostname.is_empty() {
        return None;
    }
    let endpoint_host = if normalized_hostname.contains(':') {
        format!("[{normalized_hostname}]")
    } else {
        normalized_hostname
    };
    let port = parsed
        .port()
        .map(|p| p.to_string())
        .unwrap_or_else(|| "443".to_string());
    Some(format!("https://{endpoint_host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Oracle block `T:28-41` — classify vs. cert-eligibility breadth.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_local_dev_classifier_broader_than_cert_eligibility() {
        for input in [
            "localhost:3000/path",
            "127.0.0.1:5173",
            "0.0.0.0:8080",
            "[::1]:3000",
            "[2001:db8::1]:3000",
        ] {
            assert!(
                classify_scheme_less_local_dev_address(input).is_some(),
                "expected Some for {input:?}"
            );
        }
        assert_eq!(
            classify_scheme_less_local_dev_address("app.localhost:3000"),
            None
        );
        assert!(!is_eligible_local_certificate_host("0.0.0.0"));
        assert!(!is_eligible_local_certificate_host("[2001:db8::1]"));
    }

    // -----------------------------------------------------------------
    // Oracle block `T:43-71` — canonical loopback certificate hosts.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_recognizes_only_canonical_loopback_certificate_hosts() {
        for hostname in [
            "localhost",
            "LOCALHOST.",
            "app.localhost",
            "deep.app.localhost.",
            "127.0.0.1",
            "127.255.255.255",
            "::1",
            "[::1]",
        ] {
            assert!(
                is_eligible_local_certificate_host(hostname),
                "expected true for {hostname:?}"
            );
        }
        for hostname in [
            "0.0.0.0",
            "::",
            "[2001:db8::1]",
            "192.168.1.1",
            "localhost.example.com",
            "notlocalhost",
            ".localhost",
            "-bad.localhost",
            "bad-.localhost",
            "127.0.0.999",
            "127.00.0.1",
        ] {
            assert!(
                !is_eligible_local_certificate_host(hostname),
                "expected false for {hostname:?}"
            );
        }
    }

    // -----------------------------------------------------------------
    // Oracle block `T:73-85` — HTTPS recovery URLs.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_https_recovery_urls_preserve_everything_but_scheme() {
        assert_eq!(
            to_https_recovery_url("http://localhost:3000/path?q=1#preview"),
            Some("https://localhost:3000/path?q=1#preview".to_string())
        );
        assert_eq!(
            to_https_recovery_url("http://user:pass@127.0.0.2:8080/"),
            Some("https://user:pass@127.0.0.2:8080/".to_string())
        );
        assert_eq!(
            to_https_recovery_url("http://localhost:80/path"),
            Some("https://localhost/path".to_string())
        );
        assert_eq!(to_https_recovery_url("https://localhost:3000/"), None);
        assert_eq!(to_https_recovery_url("http://0.0.0.0:3000/"), None);
        assert_eq!(to_https_recovery_url("http://example.com/"), None);
        assert_eq!(to_https_recovery_url("not a url"), None);
    }

    // -----------------------------------------------------------------
    // Oracle block `T:87-97` — secure certificate endpoints.
    // -----------------------------------------------------------------

    #[test]
    fn oracle_secure_certificate_endpoints_strip_path_and_credentials() {
        assert_eq!(
            to_secure_certificate_endpoint("https://User:secret@LOCALHOST.:443/path?q=1"),
            Some("https://localhost:443".to_string())
        );
        assert_eq!(
            to_secure_certificate_endpoint("wss://localhost:3000/socket"),
            Some("https://localhost:3000".to_string())
        );
        assert_eq!(
            to_secure_certificate_endpoint("https://[::1]/"),
            Some("https://[::1]:443".to_string())
        );
        assert_eq!(
            to_secure_certificate_endpoint("http://localhost:3000/"),
            None
        );
        assert_eq!(to_secure_certificate_endpoint("not a url"), None);
    }

    // -----------------------------------------------------------------
    // B2 — two case-folding mechanisms, both directions.
    // -----------------------------------------------------------------

    #[test]
    fn b2_kelvin_sign_folds_under_full_unicode_lowercase_path() {
        // U+212A KELVIN SIGN lowercases to 'k' under `str::to_lowercase()`,
        // which `normalize_certificate_hostname` uses (O:50).
        let normalized = normalize_certificate_hostname("\u{212A}.localhost");
        assert_eq!(normalized, "k.localhost");
        assert!(is_eligible_local_certificate_host("\u{212A}.localhost"));
    }

    #[test]
    fn b2_kelvin_and_long_s_do_not_fold_under_ascii_only_pattern_path() {
        // Same two characters must NOT be recognized as their ASCII
        // look-alikes by the hand-rolled LOCAL_ADDRESS_PATTERN matcher,
        // which is ASCII-only (O:4's `/i` without `/u`).
        assert_eq!(
            classify_scheme_less_local_dev_address("\u{212A}ocalhost:3000"), // 'K'-sign + "ocalhost"
            None
        );
        assert_eq!(
            classify_scheme_less_local_dev_address("localho\u{017F}t:3000"), // ſ instead of 's'
            None
        );
    }

    // -----------------------------------------------------------------
    // B3 — ASCII-only digits.
    // -----------------------------------------------------------------

    #[test]
    fn b3_arabic_indic_digits_are_rejected_everywhere() {
        // Arabic-Indic digits (U+0660..U+0669) are not ASCII `[0-9]`.
        let arabic_127 = "\u{0661}\u{0662}\u{0667}.0.0.1"; // "١٢٧.0.0.1"
        assert!(!is_ipv4_loopback(arabic_127));
        assert_eq!(classify_scheme_less_local_dev_address(arabic_127), None);
    }

    // -----------------------------------------------------------------
    // B4 — js_trim, not str::trim.
    // -----------------------------------------------------------------

    #[test]
    fn b4_feff_is_trimmed_but_nel_is_not() {
        // U+FEFF (BOM): ECMAScript whitespace, stripped by js_trim.
        assert!(classify_scheme_less_local_dev_address("\u{FEFF}localhost:3000").is_some());
        // U+0085 (NEL): NOT ECMAScript whitespace, so it survives the trim
        // and breaks the pattern match (it isn't part of any alternative).
        assert_eq!(
            classify_scheme_less_local_dev_address("\u{0085}localhost:3000"),
            None
        );
        // Kills `js_trim(hostname) -> hostname.trim()` on the
        // certificate-host path: `str::trim` does NOT strip U+FEFF (so the
        // BOM would survive into the DNS-label check and fail it) but DOES
        // strip U+0085 NEL (so it would wrongly vanish) — both directions of
        // the divergence pinned directly on `normalize_certificate_hostname`
        // so a failure here points at the helper, not the classifier.
        assert_eq!(
            normalize_certificate_hostname("\u{FEFF}localhost"),
            "localhost"
        );
        assert_eq!(
            normalize_certificate_hostname("\u{0085}localhost"),
            "\u{0085}localhost"
        );
        assert!(is_eligible_local_certificate_host("\u{FEFF}localhost"));
        assert!(!is_eligible_local_certificate_host("\u{0085}localhost"));
    }

    // -----------------------------------------------------------------
    // B5 — normalize_certificate_hostname ordering / single-strip.
    // -----------------------------------------------------------------

    #[test]
    fn b5_bracket_strip_precedes_dot_strip_and_each_runs_once() {
        assert_eq!(normalize_certificate_hostname("[::1]."), "[::1]");
        assert!(!is_eligible_local_certificate_host("[::1]."));
        assert_eq!(normalize_certificate_hostname("[::1]"), "::1");
        assert!(is_eligible_local_certificate_host("[::1]"));
        // Only one trailing dot is stripped, not a run of them.
        assert_eq!(normalize_certificate_hostname("localhost.."), "localhost.");
        // Kills "strip '[' / strip ']' independently if present": an
        // unbalanced bracket must leave the hostname untouched entirely —
        // Orca's `browser-url.ts:51` only strips the pair when BOTH ends
        // are present.
        assert_eq!(normalize_certificate_hostname("[::1"), "[::1");
        assert!(!is_eligible_local_certificate_host("[::1"));
        assert_eq!(normalize_certificate_hostname("::1]"), "::1]");
        assert!(!is_eligible_local_certificate_host("::1]"));
    }

    // -----------------------------------------------------------------
    // B6 — leading-zero / trailing-dot / range boundaries.
    // -----------------------------------------------------------------

    #[test]
    fn b6_ipv4_loopback_boundaries() {
        // Tests `is_ipv4_loopback` directly (not through the certificate-host
        // predicate, whose `normalize_certificate_hostname` would strip the
        // trailing dot before this function ever sees it — see O:67-77's own
        // `split('.')` behavior on the raw string).
        assert!(!is_ipv4_loopback("127.0.0.1.")); // split('.') -> 5 parts
        assert!(is_ipv4_loopback("127.255.255.255")); // whole /8 is eligible
        assert!(is_eligible_local_certificate_host("127.255.255.255"));
        assert!(!is_ipv4_loopback("127.00.0.1")); // "00" != "0"
        assert!(!is_eligible_local_certificate_host("127.00.0.1"));
        // Kills `values[0] == 127 || values[0] == 10`: only a first octet of
        // exactly 127 is loopback — RFC1918 private space and the immediate
        // neighbors of the 127.0.0.0/8 boundary must all be rejected.
        assert!(!is_ipv4_loopback("10.0.0.1")); // RFC1918 private, not loopback
        assert!(!is_ipv4_loopback("128.0.0.1")); // one above 127
        assert!(!is_ipv4_loopback("126.255.255.255")); // one below 127
        assert!(is_ipv4_loopback("127.0.0.0")); // low end of 127.0.0.0/8
        assert!(!is_eligible_local_certificate_host("10.0.0.1"));
    }

    // -----------------------------------------------------------------
    // B7 — UTF-16 length caps + empty label.
    // -----------------------------------------------------------------

    #[test]
    fn b7_dns_name_length_and_label_boundaries() {
        // 63-char label, exactly at the cap: accepted.
        let label_63 = "a".repeat(63);
        assert!(is_valid_dns_name(&label_63));
        // 64-char label: one over the cap, rejected.
        let label_64 = "a".repeat(64);
        assert!(!is_valid_dns_name(&label_64));
        // Total length exactly 253 (using 63+1+63+1+63+1+61 = 253 via
        // dot-joined labels each <= 63): accepted.
        let name_253 = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "a".repeat(63),
            "a".repeat(63),
            "a".repeat(61)
        );
        assert_eq!(name_253.encode_utf16().count(), 253);
        assert!(is_valid_dns_name(&name_253));
        // One character longer (254 total): rejected.
        let name_254 = format!("{name_253}a");
        assert_eq!(name_254.encode_utf16().count(), 254);
        assert!(!is_valid_dns_name(&name_254));
        // Empty label (consecutive dots) is rejected.
        assert!(!is_valid_dns_name("foo..bar"));
    }

    // -----------------------------------------------------------------
    // B8 — `.local` unhandled, `.localhost` at any depth, 0.0.0.0 split.
    // -----------------------------------------------------------------

    #[test]
    fn b8_local_suffix_unhandled_and_wildcard_vs_loopback_split() {
        assert!(!is_eligible_local_certificate_host("foo.local"));
        assert!(is_eligible_local_certificate_host("a.b.c.localhost"));
        assert!(!is_ipv4_loopback("0.0.0.0"));
        assert!(!is_eligible_local_certificate_host("0.0.0.0"));
        assert!(is_wildcard_bind_host("0.0.0.0"));
    }

    // -----------------------------------------------------------------
    // B9 — loose regex + parser delegation vs. raw-string strictness.
    // -----------------------------------------------------------------

    #[test]
    fn b9_loose_pattern_and_strict_cert_predicate_coexist() {
        // The pattern's \d{1,3} admits "127.0.0.01"; Url::parse normalizes
        // the octal-looking octet down to "127.0.0.1".
        let classified = classify_scheme_less_local_dev_address("127.0.0.01");
        assert_eq!(
            classified.as_ref().map(Url::as_str),
            Some("http://127.0.0.1/")
        );
        // The certificate-host predicate never reaches the URL parser, so
        // the same leading-zero octet is rejected outright.
        assert!(!is_eligible_local_certificate_host("127.0.0.01"));
        assert!(!is_eligible_local_certificate_host("127.00.0.1"));
    }

    // -----------------------------------------------------------------
    // B10 — set_scheme result checked; default port re-elided.
    // -----------------------------------------------------------------

    #[test]
    fn b10_https_recovery_elides_the_default_port() {
        assert_eq!(
            to_https_recovery_url("http://localhost:80/path"),
            Some("https://localhost/path".to_string())
        );
    }

    // -----------------------------------------------------------------
    // B11 — no `s` flag: a newline in the tail is rejected.
    // -----------------------------------------------------------------

    #[test]
    fn b11_newline_in_tail_is_rejected() {
        assert_eq!(
            classify_scheme_less_local_dev_address("localhost:3000/\nfoo"),
            None
        );
    }
}
