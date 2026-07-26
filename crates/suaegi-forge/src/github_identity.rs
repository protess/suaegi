//! GitHub repository identity — verbatim port of Orca's
//! `src/shared/github-repository-identity-key.ts` (@ v1.4.150-rc.0).
//!
//! Why: the github.com-vs-GHES boundary is the core invariant of Enterprise
//! support — cache identity, quota scoping, and exec-host routing must all
//! agree on it, so the predicate lives here once.
//!
//! Two contract points that are easy to get subtly wrong:
//!
//! - Trimming uses [`suaegi_misc::js_trim`] (the ECMAScript whitespace set),
//!   **not** Rust's `str::trim` (Unicode `White_Space`) — they diverge at
//!   U+FEFF (JS strips it, Rust's `trim` does not).
//! - Case-folding uses `str::to_lowercase` (full Unicode case folding, e.g.
//!   `É` -> `é`), **not** `to_ascii_lowercase`, which would leave non-ASCII
//!   host characters (IDN hosts) unfolded.
//! - `owner`/`repo` are lowercased but **deliberately not trimmed** — this
//!   asymmetry with `host` (which is both trimmed and lowercased) is
//!   preserved verbatim from the source.

/// `isDefaultGitHubHost`: `!host?.trim() || host.trim().toLowerCase() ==
/// 'github.com'`. `None` (JS `undefined`) is true; `Some("")` / `Some("   ")`
/// are true too since a js-trimmed empty string is JS-falsy; otherwise the
/// js-trimmed, fully-Unicode-lowercased host is compared to `"github.com"`.
pub fn is_default_github_host(host: Option<&str>) -> bool {
    match host.map(suaegi_misc::js_trim) {
        None => true,
        Some("") => true,
        Some(trimmed) => trimmed.to_lowercase() == "github.com",
    }
}

/// `githubRepoIdentityKey`: cache keys and equality checks for GitHub repos
/// must include the host, or a GHES repo and a same-named github.com repo
/// would collide. `github.com` is omitted so pre-Enterprise host-less keys
/// stay stable.
///
/// slug = `owner.to_lowercase()/repo.to_lowercase()` (owner/repo are
/// lowercased but NOT trimmed — deliberate asymmetry with `host`). host =
/// js-trimmed + lowercased. Returns `{host}/{slug}` only when the host is
/// non-empty (JS `host &&` falsy guard) AND is not the default host;
/// otherwise the bare slug.
pub fn github_repo_identity_key(owner: &str, repo: &str, host: Option<&str>) -> String {
    let slug = format!("{}/{}", owner.to_lowercase(), repo.to_lowercase());
    let host = host.map(|h| suaegi_misc::js_trim(h).to_lowercase());
    match host {
        Some(host) if !host.is_empty() && !is_default_github_host(Some(&host)) => {
            format!("{host}/{slug}")
        }
        _ => slug,
    }
}

#[cfg(test)]
mod tests {
    use super::{github_repo_identity_key, is_default_github_host};

    // Oracle: github-repository-identity-key.test.ts

    #[test]
    fn normalizes_case_and_harmless_whitespace_without_merging_ghes_hosts() {
        assert!(is_default_github_host(Some(" GitHub.com ")));
        assert_eq!(
            github_repo_identity_key("Acme", "Widgets", Some(" GitHub.com ")),
            "acme/widgets"
        );
        assert_eq!(
            github_repo_identity_key("Acme", "Widgets", Some(" GHE.EXAMPLE:8443 ")),
            "ghe.example:8443/acme/widgets"
        );
    }

    // Mandatory extra pins (oracle-silent):

    /// `None` host is the default host, and yields the bare slug.
    #[test]
    fn pin_none_host_is_default() {
        assert!(is_default_github_host(None));
        assert_eq!(
            github_repo_identity_key("acme", "widgets", None),
            "acme/widgets"
        );
    }

    /// `Some("")` is JS-falsy after trim, so it counts as the default host.
    #[test]
    fn pin_empty_host_is_default() {
        assert!(is_default_github_host(Some("")));
        assert_eq!(
            github_repo_identity_key("acme", "widgets", Some("")),
            "acme/widgets"
        );
    }

    /// `Some("   ")` js-trims to empty, which is JS-falsy -> default host.
    #[test]
    fn pin_whitespace_only_host_is_default() {
        assert!(is_default_github_host(Some("   ")));
        assert_eq!(
            github_repo_identity_key("acme", "widgets", Some("   ")),
            "acme/widgets"
        );
    }

    /// U+FEFF-padded host proves `js_trim` is used, not Rust's `str::trim`
    /// (which does not strip U+FEFF and would leave the BOM embedded in the
    /// host, producing a wrong non-default-host key).
    #[test]
    fn pin_feff_padded_host_uses_js_trim() {
        let host = "\u{FEFF}github.com\u{FEFF}";
        // Sanity: Rust's own `str::trim` does NOT strip U+FEFF.
        assert_eq!(host.trim(), host);
        assert!(is_default_github_host(Some(host)));
        assert_eq!(
            github_repo_identity_key("acme", "widgets", Some(host)),
            "acme/widgets"
        );
    }

    /// Non-ASCII case fold: `to_ascii_lowercase` would leave `É` unfolded, so
    /// this proves full-Unicode `to_lowercase` is used for the host.
    #[test]
    fn pin_non_ascii_host_case_fold_is_full_unicode() {
        let host = "GHE.\u{c9}XAMPLE"; // "GHE.ÉXAMPLE"
        assert_eq!(
            github_repo_identity_key("acme", "widgets", Some(host)),
            "ghe.\u{e9}xample/acme/widgets" // "ghe.éxample/acme/widgets"
        );
    }

    /// owner/repo are lowercased but NOT trimmed — deliberate asymmetry with
    /// host preserved verbatim from the source.
    #[test]
    fn pin_owner_and_repo_are_not_trimmed() {
        assert_eq!(
            github_repo_identity_key(" Acme ", " Widgets ", None),
            " acme / widgets "
        );
    }
}
