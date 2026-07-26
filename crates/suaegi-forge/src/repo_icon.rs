//! Repo icon sanitization — verbatim port of Orca's `src/shared/repo-icon.ts`
//! (134L, @ v1.4.150-rc.0). Oracle: `repo-icon.test.ts` (130L, 16 `expect()`
//! assertions across 4 `it` blocks).
//!
//! # Placement (U1)
//! This lives in `suaegi-forge`, not the dependency-free `suaegi-misc`,
//! because the `favicon`/`github` image-source branches and
//! `normalizeGitHubAvatarHost` all need `url::Url` parsing — `suaegi-forge`
//! already depends on both `url` (hosted-review M4) and `suaegi-misc`
//! (hosted-review M3, for [`suaegi_misc::js_trim`]). No new dependency is
//! added to any `Cargo.toml`.
//!
//! # U2 — three-state return, not `Option<RepoIcon>`
//! TS `sanitizeRepoIcon` returns `RepoIcon | null | undefined`: `undefined`
//! means "reject / no change", `null` means "explicit reset". Collapsing
//! both into a single `Option` would lose that distinction, so
//! [`SanitizedRepoIcon`] is a dedicated 3-variant enum (`Icon`, `Reset` ←
//! `null`, `Rejected` ← `undefined`).
//!
//! # U3 — two unrelated case-folding mechanisms in one file
//! `normalizeGitHubAvatarHost` (`:45`) does `rawHost?.trim().toLowerCase()`
//! — JS `String.prototype.toLowerCase()` is **full-Unicode** case folding
//! (ported as Rust [`str::to_lowercase`]): U+212A KELVIN SIGN folds to `k`.
//! `isSupportedImageSrc`'s regexes (`:66`, `:81`) use a non-`u` `/i` flag —
//! ECMAScript `Canonicalize` for non-`u` patterns is **ASCII-only**: U+212A
//! does **not** fold to `k`. This module hand-rolls both regexes
//! ([`matches_data_url_png_base64_pattern`],
//! [`matches_png_avatar_path_pattern`]) using `char::eq_ignore_ascii_case`
//! and plain ASCII checks, never `to_lowercase`, to preserve that asymmetry.
//! Both directions are pinned in the tests below.
//!
//! # U4 — `js_trim`, not `str::trim`; `source` is deliberately untrimmed
//! Six trim sites (`:16, :45, :100, :108, :116, :124`) all use
//! [`suaegi_misc::js_trim`] (the ECMAScript whitespace set), which diverges
//! from Rust's `str::trim` at U+FEFF (JS strips it, Rust doesn't) and
//! U+0085 (Rust strips it, JS doesn't). `source` (`:117`) is **not** trimmed
//! at all — `" github "` (with surrounding spaces) fails
//! `isRepoIconImageSource` and is rejected; pinned below.
//!
//! # U5 — UTF-16 code-unit caps, and snap-down slicing for the label
//! All three size caps (lucide name > 40, emoji > 16, image src > 409600)
//! count **UTF-16 code units** (`encode_utf16().count()`), matching JS
//! `.length`. `label.slice(0, 80)` is also a UTF-16-unit slice; when the
//! 80th unit would split an astral character's surrogate pair, JS produces
//! a lone surrogate (unrepresentable in a Rust `&str`), so
//! [`utf16_slice_prefix`] **snaps down**, dropping the whole straddling
//! character (79 units survive, not 80) — the same technique as
//! `suaegi_misc::codex_auth_errors`'s private `utf16_slice_prefix`,
//! duplicated here rather than shared (this module has no other reason to
//! depend on that one). Order is trim → slice, with **no re-trim**
//! afterward (trailing whitespace can survive the cut).
//!
//! # U6 — `.host` (port included) vs `.hostname` (port excluded)
//! `normalizeGitHubAvatarHost` (`:53, :57`) compares/returns WHATWG
//! `url.host`, which includes a non-default port; this is reassembled from
//! Rust's `host_str()` + `port()` (**never** `port_or_known_default()`,
//! which would wrongly inject the scheme-default port — see
//! `hosted_review_gitlab.rs`'s K1 for the same footgun, empirically
//! verified here too against `url` 2.5.8). `isSupportedImageSrc`
//! (`:26, :84`) instead compares `url.hostname`, which excludes the port —
//! ported as `host_str()` alone.
//!
//! # U7 — a purpose-built `encodeURIComponent`
//! [`encode_uri_component`]'s unreserved set is exactly
//! `A-Za-z0-9 - _ . ! ~ * ' ( )`. The two RFC3986-unreserved encoders
//! elsewhere in this crate (`gitlab::parse::encoded_project`,
//! `github_http::forge::encode_component`) only exempt `-_.~` and would
//! over-escape `!'()*` — do not reuse them here, and do not widen them with
//! this module's set either; they serve different call sites.
//!
//! # U8 — do not reuse `github_identity::is_default_github_host`
//! It is a *predicate* (trim + lowercase, then `== "github.com"`).
//! [`github_avatar_icon`] needs a *normalizer*: `|| 'github.com'`, then a
//! `new URL(...)` parse and a six-condition validation, returning a host
//! *string*. Reusing the predicate would silently drop that validation.
//!
//! # U9 — exactly 4 image-source variants; unknown → reject
//! [`RepoIconImageSource`] has exactly the 4 TS variants. An unrecognized
//! `source` string is conservatively [`SanitizedRepoIcon::Rejected`].
//!
//! # U10 — an empty label means the field is absent
//! `label ? { label } : {}` (`:129`) — an empty (post-trim-and-slice) label
//! is `None`, never `Some(String::new())`.
//!
//! # U11 — the six-condition host validation, `:443` escape hatch included
//! `!username && !password && (host === candidate || `${host}:443` ===
//! candidate) && pathname === '/' && !search && !hash`. The third condition
//! is a two-way string equality that amounts to "the host must round-trip
//! through URL parsing unchanged" — an IDN host gets punycode-encoded and
//! therefore never matches, falling back to `github.com`.
//!
//! # U12/U13 — two upstream inconsistencies, preserved verbatim
//! [`github_avatar_icon`]'s `label` (`:40`) is untrimmed and uncapped, so
//! feeding its own output back through [`sanitize_repo_icon`] can change
//! it — an upstream self-inconsistency, kept as-is. The `favicon` branch of
//! `isSupportedImageSrc` (`:84`) checks only `hostname === 'www.google.com'
//! && pathname === '/s2/favicons'`; it validates neither port nor query.
//! Do not tighten either — both are ported for fidelity, not by oversight.
//!
//! # U14 — `faviconUrlFromWebsite` has zero oracle coverage
//! All of its behavior is pinned by hand in the tests below, including the
//! `:22` `trimmed.includes('://')` check, which is a **positional-agnostic
//! substring test, not a scheme check** — `"a/b://c"` is passed through
//! as-is to `new URL(...)`, which fails to parse it (invalid scheme token),
//! yielding `None`.

use suaegi_misc::{is_js_whitespace, js_trim};
use url::Url;

/// Max upload size in bytes for an uploaded repo-icon image
/// (`repo-icon.ts:8`). **Zero consumers in Orca** (grepped across the full
/// upstream tree at port time) — a dead export upstream, ported verbatim
/// anyway for 1:1 fidelity.
pub const MAX_REPO_ICON_UPLOAD_BYTES: usize = 256 * 1024;

/// Max UTF-16-code-unit length (`repo-icon.ts:9`) of a `type: 'image'`
/// icon's `src` field, enforced in [`sanitize_repo_icon`] via
/// `encode_utf16().count()`, never byte length (U5).
pub const MAX_REPO_ICON_DATA_URL_LENGTH: usize = 400 * 1024;

/// `RepoIconImageSource` (`repo-icon.ts:1`) — exactly 4 variants (U9); an
/// unrecognized source string is rejected by [`sanitize_repo_icon`], never
/// coerced into one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoIconImageSource {
    Upload,
    File,
    Favicon,
    Github,
}

impl RepoIconImageSource {
    /// `isRepoIconImageSource` (`:12-13`): recognizes exactly the 4 literal
    /// strings; anything else is `None` (U9).
    fn parse(value: &str) -> Option<Self> {
        match value {
            "upload" => Some(Self::Upload),
            "file" => Some(Self::File),
            "favicon" => Some(Self::Favicon),
            "github" => Some(Self::Github),
            _ => None,
        }
    }
}

/// `RepoIcon` (`repo-icon.ts:3-6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoIcon {
    Lucide {
        name: String,
    },
    Emoji {
        emoji: String,
    },
    Image {
        src: String,
        source: RepoIconImageSource,
        /// `None` when the field is absent — an empty label is never
        /// `Some(String::new())` (U10).
        label: Option<String>,
    },
}

/// Three-state result of [`sanitize_repo_icon`] (U2). TS's
/// `RepoIcon | null | undefined` collapsed the wrong way (into a single
/// `Option`) would lose the reject-vs-reset distinction the oracle pins at
/// `repo-icon.test.ts:75-77`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanitizedRepoIcon {
    /// A validated icon (TS: a non-null, non-undefined return).
    Icon(RepoIcon),
    /// Explicit reset (TS `null`).
    Reset,
    /// Rejected, or no change (TS `undefined`).
    Rejected,
}

/// UTF-16 code-unit length of `s` (JS `.length` semantics) — used for all
/// three size caps in [`sanitize_repo_icon`] (U5).
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Largest prefix of `s` whose UTF-16-code-unit count is `<= max_units`,
/// snapping down to a whole-character boundary rather than splitting an
/// astral character's surrogate pair (U5, `repo-icon.ts:124`'s
/// `label.slice(0, 80)`). Duplicated from the identical technique in
/// `suaegi_misc::codex_auth_errors`'s private `utf16_slice_prefix` rather
/// than shared — this module has no other reason to depend on that one.
fn utf16_slice_prefix(s: &str, max_units: usize) -> &str {
    let mut units = 0usize;
    for (byte_offset, ch) in s.char_indices() {
        let next_units = units + ch.len_utf16();
        if next_units > max_units {
            return &s[..byte_offset];
        }
        units = next_units;
    }
    s
}

/// `LUCIDE_ICON_NAME_PATTERN` (`repo-icon.ts:11`): `/^[A-Za-z][A-Za-z0-9]*$/`,
/// hand-rolled (a simple anchored ASCII pattern, no lookaround) rather than
/// pulled in via the `regex` crate.
fn matches_lucide_icon_name_pattern(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

/// `isSupportedImageSrc`'s `upload`/`file` pattern (`repo-icon.ts:66`):
/// `/^data:image\/png;base64,[A-Za-z0-9+/=\s]+$/i`. The `/i` flag is
/// non-`u`, so it is ASCII-only case folding (U3) — implemented here via
/// `char::eq_ignore_ascii_case` for the literal prefix, never
/// `to_lowercase`. `\s` matches exactly [`is_js_whitespace`]'s set (JS's
/// non-`u` `\s` character class), not Rust's Unicode `char::is_whitespace`.
fn matches_data_url_png_base64_pattern(src: &str) -> bool {
    const PREFIX: &str = "data:image/png;base64,";
    let mut chars = src.chars();
    for prefix_ch in PREFIX.chars() {
        match chars.next() {
            Some(c) if c.eq_ignore_ascii_case(&prefix_ch) => {}
            _ => return false,
        }
    }
    let mut saw_payload_char = false;
    for c in chars {
        saw_payload_char = true;
        let allowed =
            c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || is_js_whitespace(c);
        if !allowed {
            return false;
        }
    }
    saw_payload_char
}

/// `isSupportedImageSrc`'s `github` pathname pattern (`repo-icon.ts:81`):
/// `/^\/[^/?#]+\.png$/i`. Same ASCII-only `/i` semantics as
/// [`matches_data_url_png_base64_pattern`] (U3) — only the `.png` suffix
/// needs case folding, done via `eq_ignore_ascii_case` on a whole `&str`
/// slice built from a char `Vec` so no byte index ever risks landing on a
/// non-char-boundary.
fn matches_png_avatar_path_pattern(pathname: &str) -> bool {
    let Some(rest) = pathname.strip_prefix('/') else {
        return false;
    };
    if rest.contains(['/', '?', '#']) {
        return false;
    }
    let mut chars: Vec<char> = rest.chars().collect();
    // `[^/?#]+\.png`: at least 1 disallowed-char-free char, then literal
    // `.png` — 5 chars minimum total, so `/.png` (4 chars after the leading
    // `/`) is rejected.
    if chars.len() < 5 {
        return false;
    }
    let ext: String = chars.split_off(chars.len() - 4).into_iter().collect();
    ext.eq_ignore_ascii_case(".png")
}

/// Hand-rolled `encodeURIComponent` (U7). The exact unreserved set is
/// `A-Za-z0-9 - _ . ! ~ * ' ( )` — do NOT reuse this crate's other two
/// percent-encoders (`gitlab::parse::encoded_project`,
/// `github_http::forge::encode_component`), which only exempt RFC3986's
/// `-_.~` and would over-escape `!'()*`. Operates byte-by-byte over the
/// UTF-8 encoding, so non-ASCII input is percent-encoded exactly the way
/// `encodeURIComponent` encodes a UTF-8 byte sequence.
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

/// `normalizeGitHubAvatarHost` (`repo-icon.ts:44-62`) — NOT a reuse target
/// for `github_identity::is_default_github_host` (U8): this is a
/// normalizer, not a predicate.
fn normalize_github_avatar_host(raw_host: Option<&str>) -> String {
    let trimmed_lower = raw_host.map(|h| js_trim(h).to_lowercase());
    let candidate = match trimmed_lower {
        Some(s) if !s.is_empty() => s,
        _ => "github.com".to_string(),
    };

    let Ok(url) = Url::parse(&format!("https://{candidate}")) else {
        return "github.com".to_string();
    };
    let Some(host) = url.host_str() else {
        return "github.com".to_string();
    };

    // U6: `.host` (port included) — reassemble from `host_str()` + `port()`.
    // NEVER `port_or_known_default()`, which would inject the default port
    // even when the URL normalized it away, breaking the `:443` escape
    // hatch below.
    let host_with_port = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };

    // U11: six conditions, `:443` escape hatch included verbatim.
    let matches_candidate =
        host_with_port == candidate || format!("{host_with_port}:443") == candidate;
    let valid = url.username().is_empty()
        && url.password().unwrap_or_default().is_empty()
        && matches_candidate
        && url.path() == "/"
        && url.query().unwrap_or_default().is_empty()
        && url.fragment().unwrap_or_default().is_empty();

    if valid {
        host_with_port
    } else {
        "github.com".to_string()
    }
}

/// `faviconUrlFromWebsite` (`repo-icon.ts:15-30`) — zero oracle coverage
/// (U14); all pins for this function are hand-written in the tests below.
pub fn favicon_url_from_website(raw_url: &str) -> Option<String> {
    let trimmed = js_trim(raw_url);
    if trimmed.is_empty() {
        return None;
    }

    // `:22`: `trimmed.includes('://')` is a positional-agnostic substring
    // test, not a scheme check — e.g. `"a/b://c"` is passed through as-is
    // and fails to parse (U14).
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    let url = Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let hostname = url.host_str()?;
    if hostname.is_empty() {
        return None;
    }
    Some(format!(
        "https://www.google.com/s2/favicons?domain={}&sz=64",
        encode_uri_component(hostname)
    ))
}

/// `githubAvatarIcon`'s `slug` parameter (`repo-icon.ts:33`).
#[derive(Debug, Clone)]
pub struct GithubAvatarSlug {
    pub owner: String,
    pub repo: String,
    pub host: Option<String>,
}

/// `githubAvatarIcon` (`repo-icon.ts:32-42`) — why: shared default icon
/// URL/label for main auto-detect and the renderer picker. GHES uses the
/// same `/<login>.png` avatar path as github.com.
///
/// U12: `label` is untrimmed and uncapped, unlike the trimmed+80-capped
/// label path in [`sanitize_repo_icon`] — an upstream self-inconsistency
/// (this function's own output is not sanitize-stable), preserved verbatim.
pub fn github_avatar_icon(slug: &GithubAvatarSlug) -> RepoIcon {
    let host = normalize_github_avatar_host(slug.host.as_deref());
    RepoIcon::Image {
        src: format!(
            "https://{host}/{}.png?size=64",
            encode_uri_component(&slug.owner)
        ),
        source: RepoIconImageSource::Github,
        label: Some(format!("{}/{}", slug.owner, slug.repo)),
    }
}

/// `isSupportedImageSrc` (`repo-icon.ts:64-85`).
fn is_supported_image_src(src: &str, source: RepoIconImageSource) -> bool {
    if matches!(
        source,
        RepoIconImageSource::Upload | RepoIconImageSource::File
    ) {
        return matches_data_url_png_base64_pattern(src);
    }

    let Ok(url) = Url::parse(src) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }

    if source == RepoIconImageSource::Github {
        // Why: only owner-avatar paths; no credentials (GHES hosts may be
        // internal). U13-adjacent: neither port nor query is validated here
        // either — the pathname regex is the only check.
        return url.username().is_empty()
            && url.password().unwrap_or_default().is_empty()
            && matches_png_avatar_path_pattern(url.path());
    }

    // source == Favicon. U13: verbatim — neither port nor query is
    // validated, only `.hostname` (port-excluded, U6) and the exact path.
    url.host_str() == Some("www.google.com") && url.path() == "/s2/favicons"
}

/// `sanitizeRepoIcon` (`repo-icon.ts:87-134`). `value: None` models JS
/// `undefined` (field absent); `value: Some(&serde_json::Value::Null)`
/// models JS `null` (explicit reset); anything else is validated as the
/// untrusted, arbitrarily-shaped JSON value TS's `unknown` represents.
pub fn sanitize_repo_icon(value: Option<&serde_json::Value>) -> SanitizedRepoIcon {
    let Some(value) = value else {
        return SanitizedRepoIcon::Rejected;
    };
    if value.is_null() {
        return SanitizedRepoIcon::Reset;
    }
    let Some(candidate) = value.as_object() else {
        // TS: `!value || typeof value !== 'object'`. Non-null JSON scalars
        // (string/number/bool) hit this guard directly; a JSON array is
        // `typeof === 'object'` in JS and falls through to the `.type`
        // lookup instead (which is `undefined` for an array, landing on the
        // same final `return undefined`) — same end result, different path.
        return SanitizedRepoIcon::Rejected;
    };

    match candidate.get("type").and_then(serde_json::Value::as_str) {
        Some("lucide") => {
            let name = candidate
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(js_trim)
                .unwrap_or("");
            if !matches_lucide_icon_name_pattern(name) || utf16_len(name) > 40 {
                return SanitizedRepoIcon::Rejected;
            }
            SanitizedRepoIcon::Icon(RepoIcon::Lucide {
                name: name.to_string(),
            })
        }
        Some("emoji") => {
            let emoji = candidate
                .get("emoji")
                .and_then(serde_json::Value::as_str)
                .map(js_trim)
                .unwrap_or("");
            if emoji.is_empty() || utf16_len(emoji) > 16 {
                return SanitizedRepoIcon::Rejected;
            }
            SanitizedRepoIcon::Icon(RepoIcon::Emoji {
                emoji: emoji.to_string(),
            })
        }
        Some("image") => {
            let src = candidate
                .get("src")
                .and_then(serde_json::Value::as_str)
                .map(js_trim)
                .unwrap_or("");
            // U4: `source` is deliberately NOT trimmed (`:117`) — `" github "`
            // fails `RepoIconImageSource::parse` and is rejected.
            let source_str = candidate
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let Some(source) = RepoIconImageSource::parse(source_str) else {
                return SanitizedRepoIcon::Rejected;
            };
            if utf16_len(src) > MAX_REPO_ICON_DATA_URL_LENGTH {
                return SanitizedRepoIcon::Rejected;
            }
            if !is_supported_image_src(src, source) {
                return SanitizedRepoIcon::Rejected;
            }
            let label_raw = candidate
                .get("label")
                .and_then(serde_json::Value::as_str)
                .map(js_trim)
                .unwrap_or("");
            // U5: trim -> slice, no re-trim afterward.
            let label_sliced = utf16_slice_prefix(label_raw, 80);
            let label = if label_sliced.is_empty() {
                None
            } else {
                Some(label_sliced.to_string())
            };
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: src.to_string(),
                source,
                label,
            })
        }
        _ => SanitizedRepoIcon::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- Oracle: repo-icon.test.ts (16 `expect()` assertions) ----

    #[test]
    fn accepts_lucide_emoji_and_supported_image_icons() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "lucide", "name": "Folder" }))),
            SanitizedRepoIcon::Icon(RepoIcon::Lucide {
                name: "Folder".to_string()
            })
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "emoji", "emoji": "🚀" }))),
            SanitizedRepoIcon::Icon(RepoIcon::Emoji {
                emoji: "🚀".to_string()
            })
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://github.com/stablyai.png?size=64",
                "source": "github",
                "label": "stablyai/orca"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://github.com/stablyai.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("stablyai/orca".to_string()),
            })
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://github.acme.test/stablyai.png?size=64",
                "source": "github",
                "label": "stablyai/orca"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://github.acme.test/stablyai.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("stablyai/orca".to_string()),
            })
        );
        // Favicon icon with no label key at all -> U10 no-label field.
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://www.google.com/s2/favicons?domain=example.com&sz=64",
                "source": "favicon"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string(),
                source: RepoIconImageSource::Favicon,
                label: None,
            })
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "data:image/png;base64,aGVsbG8=",
                "source": "upload"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "data:image/png;base64,aGVsbG8=".to_string(),
                source: RepoIconImageSource::Upload,
                label: None,
            })
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "data:image/png;base64,aGVsbG8=",
                "source": "file"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "data:image/png;base64,aGVsbG8=".to_string(),
                source: RepoIconImageSource::File,
                label: None,
            })
        );
    }

    #[test]
    fn keeps_null_as_an_explicit_reset() {
        assert_eq!(
            sanitize_repo_icon(Some(&serde_json::Value::Null)),
            SanitizedRepoIcon::Reset
        );
    }

    #[test]
    fn rejects_unsupported_image_urls_and_oversized_payloads() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "javascript:alert(1)",
                "source": "favicon"
            }))),
            SanitizedRepoIcon::Rejected
        );
        let oversized = format!("data:image/png;base64,{}", "a".repeat(401 * 1024));
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": oversized,
                "source": "upload"
            }))),
            SanitizedRepoIcon::Rejected
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=",
                "source": "upload"
            }))),
            SanitizedRepoIcon::Rejected
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://example.com/nested/icon.png",
                "source": "github"
            }))),
            SanitizedRepoIcon::Rejected
        );
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://user@example.com/icon.png",
                "source": "github"
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    #[test]
    fn builds_hosted_avatar_urls_only_from_a_valid_host_value() {
        // Non-default port survives (U6: `.host` includes port).
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("GitHub.Acme.Test:8443".to_string()),
            }),
            RepoIcon::Image {
                src: "https://github.acme.test:8443/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
        // Explicit default port 443 is canonical for HTTPS: accepted (and
        // serialized without the port) rather than falling back (U11
        // escape hatch).
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("ghe.example:443".to_string()),
            }),
            RepoIcon::Image {
                src: "https://ghe.example/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
        // Credential-confusion fallback (U11): `@` splits into
        // userinfo/host, so this is NOT the github.com host.
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("github.com@evil.example".to_string()),
            }),
            RepoIcon::Image {
                src: "https://github.com/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    // ---- U14: favicon_url_from_website has zero oracle coverage ----

    #[test]
    fn pin_favicon_empty_input_is_none() {
        assert_eq!(favicon_url_from_website(""), None);
    }

    #[test]
    fn pin_favicon_whitespace_only_input_is_none() {
        assert_eq!(favicon_url_from_website("   \t\n  "), None);
    }

    #[test]
    fn pin_favicon_bare_domain_gets_https_prefixed() {
        assert_eq!(
            favicon_url_from_website("example.com"),
            Some("https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string())
        );
    }

    #[test]
    fn pin_favicon_accepts_explicit_http_and_https_urls() {
        assert_eq!(
            favicon_url_from_website("http://example.com/path"),
            Some("https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string())
        );
        assert_eq!(
            favicon_url_from_website("https://example.com/path?x=1"),
            Some("https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string())
        );
    }

    #[test]
    fn pin_favicon_rejects_a_non_http_scheme() {
        assert_eq!(favicon_url_from_website("ftp://example.com"), None);
    }

    /// `:22`'s `includes('://')` is a positional-agnostic substring test,
    /// not a scheme check — `"a/b://c"` is passed through as-is and fails
    /// to parse (invalid scheme token `a/b`), yielding `None`.
    #[test]
    fn pin_favicon_positional_agnostic_substring_check_rejects_a_slash_b_colon_slash_slash_c() {
        assert_eq!(favicon_url_from_website("a/b://c"), None);
    }

    #[test]
    fn pin_favicon_percent_encodes_the_hostname() {
        // IPv6 host serializes with brackets (`[::1]`), which are outside
        // `encodeURIComponent`'s unreserved set and must be escaped.
        assert_eq!(
            favicon_url_from_website("https://[::1]/foo"),
            Some("https://www.google.com/s2/favicons?domain=%5B%3A%3A1%5D&sz=64".to_string())
        );
    }

    // ---- U3: both case-folding directions, pinned with U+212A KELVIN SIGN ----

    #[test]
    fn pin_kelvin_sign_folds_in_the_host_normalizer_full_unicode() {
        // Sanity: Rust's full-Unicode `to_lowercase` DOES fold U+212A to 'k'.
        assert_eq!('\u{212A}'.to_lowercase().collect::<String>(), "k");
        let icon = github_avatar_icon(&GithubAvatarSlug {
            owner: "acme".to_string(),
            repo: "widgets".to_string(),
            host: Some("gh\u{212A}.example".to_string()),
        });
        assert_eq!(
            icon,
            RepoIcon::Image {
                src: "https://ghk.example/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    #[test]
    fn pin_kelvin_sign_does_not_fold_in_the_data_url_regex_ascii_only() {
        // A base64 payload containing U+212A: ASCII-only `/i` folding does
        // NOT map it into the `[A-Za-z0-9+/=\s]` class, so it must reject
        // (contrast the host normalizer, which folds it above).
        let src = "data:image/png;base64,aGVsbG8=\u{212A}".to_string();
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": src,
                "source": "upload"
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    // ---- U4: source is untrimmed; U+FEFF-padded fields ARE trimmed ----

    #[test]
    fn pin_source_with_surrounding_spaces_is_rejected() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://www.google.com/s2/favicons?domain=example.com&sz=64",
                "source": " favicon "
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    #[test]
    fn pin_feff_padded_lucide_name_is_js_trimmed() {
        let padded = "\u{FEFF}Folder\u{FEFF}";
        // Sanity: Rust's own `str::trim` does NOT strip U+FEFF.
        assert_eq!(padded.trim(), padded);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "lucide", "name": padded }))),
            SanitizedRepoIcon::Icon(RepoIcon::Lucide {
                name: "Folder".to_string()
            })
        );
    }

    /// The host-normalizer trim site (`js_trim(h).to_lowercase()`) is
    /// distinct from the lucide-name trim site pinned by
    /// `pin_feff_padded_lucide_name_is_js_trimmed` above (U4). Why a
    /// padded `github.com` would NOT discriminate: with `str::trim` the
    /// BOM survives into the candidate, the six-condition identity check
    /// in `normalize_github_avatar_host` fails either way, and the
    /// fallback host is `github.com` under BOTH `js_trim` and `str::trim`
    /// — same answer, no signal. A padded *non-default* host does
    /// discriminate: `js_trim` strips the BOM so the identity check
    /// passes and the host survives; `str::trim` leaves the BOM in place,
    /// the identity check fails, and it falls back to `github.com`.
    #[test]
    fn pin_feff_padded_non_default_host_is_js_trimmed() {
        // Sanity: Rust's own `str::trim` does NOT strip U+FEFF.
        let padded = "\u{FEFF}ghe.example\u{FEFF}";
        assert_eq!(padded.trim(), padded);

        let icon = github_avatar_icon(&GithubAvatarSlug {
            owner: "acme".to_string(),
            repo: "widgets".to_string(),
            host: Some(padded.to_string()),
        });
        assert_eq!(
            icon,
            RepoIcon::Image {
                src: "https://ghe.example/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    // ---- U5: UTF-16 caps and the label surrogate-straddle snap-down ----

    #[test]
    fn pin_emoji_of_16_units_passes_and_18_units_is_rejected() {
        let emoji_16 = "\u{1F680}".repeat(8); // 8 astral chars * 2 units = 16
        assert_eq!(utf16_len(&emoji_16), 16);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "emoji", "emoji": emoji_16.clone() }))),
            SanitizedRepoIcon::Icon(RepoIcon::Emoji { emoji: emoji_16 })
        );

        let emoji_18 = "\u{1F680}".repeat(9); // 18 units > 16
        assert_eq!(utf16_len(&emoji_18), 18);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "emoji", "emoji": emoji_18 }))),
            SanitizedRepoIcon::Rejected
        );
    }

    #[test]
    fn pin_src_cap_boundary_at_exactly_409600_units_passes_409601_rejects() {
        const PREFIX: &str = "data:image/png;base64,";
        let payload_len_at_cap = MAX_REPO_ICON_DATA_URL_LENGTH - PREFIX.len();
        let src_at_cap = format!("{PREFIX}{}", "a".repeat(payload_len_at_cap));
        assert_eq!(utf16_len(&src_at_cap), MAX_REPO_ICON_DATA_URL_LENGTH);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": src_at_cap.clone(),
                "source": "upload"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: src_at_cap,
                source: RepoIconImageSource::Upload,
                label: None,
            })
        );

        let src_over_cap = format!("{PREFIX}{}", "a".repeat(payload_len_at_cap + 1));
        assert_eq!(utf16_len(&src_over_cap), MAX_REPO_ICON_DATA_URL_LENGTH + 1);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": src_over_cap,
                "source": "upload"
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    /// A label whose 80th UTF-16 unit would split an astral character's
    /// surrogate pair snaps down to 79 units instead (the astral character
    /// is dropped wholesale, never split into a lone surrogate).
    #[test]
    fn pin_label_surrogate_straddle_snaps_down_to_79_units() {
        let label = format!("{}\u{1F680}", "a".repeat(79)); // 79 + 2 = 81 units
        assert_eq!(utf16_len(&label), 81);
        let expected = "a".repeat(79);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://www.google.com/s2/favicons?domain=example.com&sz=64",
                "source": "favicon",
                "label": label
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string(),
                source: RepoIconImageSource::Favicon,
                label: Some(expected.clone()),
            })
        );
        assert_eq!(utf16_len(&expected), 79);
    }

    /// Why: the lucide name cap is `utf16_len(name) > 40` — exclusive, so
    /// exactly 40 units must pass and 41 must reject. Only this exact
    /// boundary distinguishes `>` from a mutated `>=`.
    #[test]
    fn pin_lucide_name_cap_boundary_at_exactly_40_units_passes_41_rejects() {
        let name_40 = format!("A{}", "a".repeat(39)); // 40 chars total
        assert_eq!(utf16_len(&name_40), 40);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "lucide", "name": name_40.clone() }))),
            SanitizedRepoIcon::Icon(RepoIcon::Lucide { name: name_40 })
        );

        let name_41 = format!("A{}", "a".repeat(40)); // 41 chars total
        assert_eq!(utf16_len(&name_41), 41);
        assert_eq!(
            sanitize_repo_icon(Some(&json!({ "type": "lucide", "name": name_41 }))),
            SanitizedRepoIcon::Rejected
        );
    }

    // ---- U6: `.host` (port included) vs `.hostname` (port excluded) ----

    #[test]
    fn pin_favicon_hostname_comparison_ignores_a_non_default_port() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://www.google.com:8443/s2/favicons?domain=example.com",
                "source": "favicon"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://www.google.com:8443/s2/favicons?domain=example.com".to_string(),
                source: RepoIconImageSource::Favicon,
                label: None,
            })
        );
    }

    // ---- U7: `!'()*` are NOT escaped by encode_uri_component ----

    #[test]
    fn pin_unreserved_punctuation_is_not_escaped() {
        assert_eq!(
            encode_uri_component("a!b'c(d)e*f~g-h_i.j"),
            "a!b'c(d)e*f~g-h_i.j"
        );
        assert_eq!(encode_uri_component("a b/c"), "a%20b%2Fc");
    }

    // ---- U9: unknown source string is rejected ----

    #[test]
    fn pin_unknown_source_is_rejected() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://github.com/acme.png",
                "source": "dropbox"
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    // ---- U10: empty label yields no label field ----

    #[test]
    fn pin_blank_label_field_yields_none_not_empty_string() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://www.google.com/s2/favicons?domain=example.com&sz=64",
                "source": "favicon",
                "label": "   "
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "https://www.google.com/s2/favicons?domain=example.com&sz=64".to_string(),
                source: RepoIconImageSource::Favicon,
                label: None,
            })
        );
    }

    // ---- U11: IDN fallback, non-'/' pathname, query, fragment ----

    #[test]
    fn pin_idn_host_falls_back_to_github_com() {
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("m\u{fc}nchen.example".to_string()), // "münchen.example"
            }),
            RepoIcon::Image {
                src: "https://github.com/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    #[test]
    fn pin_non_root_pathname_falls_back_to_github_com() {
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("ghe.example/extra".to_string()),
            }),
            RepoIcon::Image {
                src: "https://github.com/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    #[test]
    fn pin_query_component_falls_back_to_github_com() {
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("ghe.example?x=1".to_string()),
            }),
            RepoIcon::Image {
                src: "https://github.com/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    #[test]
    fn pin_fragment_component_falls_back_to_github_com() {
        assert_eq!(
            github_avatar_icon(&GithubAvatarSlug {
                owner: "acme".to_string(),
                repo: "widgets".to_string(),
                host: Some("ghe.example#frag".to_string()),
            }),
            RepoIcon::Image {
                src: "https://github.com/acme.png?size=64".to_string(),
                source: RepoIconImageSource::Github,
                label: Some("acme/widgets".to_string()),
            }
        );
    }

    // ---- extra pins: /.png rejected; uppercase data-URL passes ----

    #[test]
    fn pin_dot_png_with_nothing_before_it_is_rejected() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "https://github.com/.png",
                "source": "github"
            }))),
            SanitizedRepoIcon::Rejected
        );
    }

    #[test]
    fn pin_uppercase_data_url_prefix_passes() {
        assert_eq!(
            sanitize_repo_icon(Some(&json!({
                "type": "image",
                "src": "DATA:IMAGE/PNG;BASE64,aGVsbG8=",
                "source": "upload"
            }))),
            SanitizedRepoIcon::Icon(RepoIcon::Image {
                src: "DATA:IMAGE/PNG;BASE64,aGVsbG8=".to_string(),
                source: RepoIconImageSource::Upload,
                label: None,
            })
        );
    }

    // ---- U2 sanity: JS `undefined` (field absent) also rejects ----

    #[test]
    fn pin_absent_value_is_rejected_not_reset() {
        assert_eq!(sanitize_repo_icon(None), SanitizedRepoIcon::Rejected);
    }
}
