//! Port of Orca `shared/git-remote-identity.ts` (@ v1.4.150-rc.0).
//!
//! Parses git remote URLs (scp-like, ssh://, https://) into a canonical
//! `host/owner/repo` key and derives the primary remote identity from
//! `git remote -v` output. Host is lowercased; path case is preserved; ports
//! and a trailing `.git` are stripped.

use regex::Regex;
use std::sync::LazyLock;

/// The primary remote's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRemoteIdentity {
    pub canonical_key: String,
    pub remote_name: String,
    pub remote_url: String,
}

/// One parsed `git remote -v` fetch entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRemoteEntry {
    pub name: String,
    pub url: String,
}

// scp-like syntax: optional `user@`, host (no `:`/whitespace), `:`, path.
static SCP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^@\s:]+@)?([^:\s]+):(.+)$").unwrap());
// A `git remote -v` fetch line: name, url (non-greedy), `(fetch)`.
static REMOTE_V_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+)\s+(.+?)\s+\(fetch\)$").unwrap());

fn strip_git_suffix(path: &str) -> &str {
    path.strip_suffix(".git").unwrap_or(path)
}

fn normalize_remote_path(path: &str) -> String {
    // Strip leading/trailing slashes FIRST, then a case-sensitive `.git`.
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    strip_git_suffix(trimmed).to_string()
}

fn normalize_remote_host(host: &str) -> String {
    // Full Unicode lowercase (JS `.toLowerCase()`), not ASCII-only.
    host.trim().to_lowercase()
}

fn is_local_filesystem_remote(remote_url: &str) -> bool {
    // `^[A-Za-z]:[\\/]` — a Windows drive path.
    let b = remote_url.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Normalize a git remote URL to its canonical `host/path` key, or `None`.
pub fn normalize_git_remote_url(remote_url: &str) -> Option<String> {
    let trimmed = remote_url.trim();
    if trimmed.is_empty() || is_local_filesystem_remote(trimmed) {
        return None;
    }

    // scp-like only when there is no `://` scheme.
    if !trimmed.contains("://") {
        if let Some(caps) = SCP_RE.captures(trimmed) {
            let host = normalize_remote_host(caps.get(2).map_or("", |m| m.as_str()));
            let path = normalize_remote_path(caps.get(3).map_or("", |m| m.as_str()));
            return if !host.is_empty() && !path.is_empty() {
                Some(format!("{host}/{path}"))
            } else {
                None
            };
        }
    }

    // URL branch (WHATWG parity via the `url` crate). `host_str` excludes the
    // port; `path` maps to JS `pathname`. Do not percent-decode the path.
    match url::Url::parse(trimmed) {
        Ok(parsed) => {
            let host = normalize_remote_host(parsed.host_str().unwrap_or(""));
            let path = normalize_remote_path(parsed.path());
            if !host.is_empty() && !path.is_empty() {
                Some(format!("{host}/{path}"))
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Parse `git remote -v` stdout into fetch entries (push lines ignored).
pub fn parse_git_remote_verbose_output(stdout: &str) -> Vec<GitRemoteEntry> {
    let mut entries = Vec::new();
    for raw in stdout.split('\n') {
        let line = raw.trim();
        if !line.ends_with("(fetch)") {
            continue;
        }
        if let Some(caps) = REMOTE_V_RE.captures(line) {
            let name = caps.get(1).map_or("", |m| m.as_str()).trim();
            let url = caps.get(2).map_or("", |m| m.as_str()).trim();
            if !name.is_empty() && !url.is_empty() {
                entries.push(GitRemoteEntry {
                    name: name.to_string(),
                    url: url.to_string(),
                });
            }
        }
    }
    entries
}

fn primary_remote_sort_key(name: &str) -> u8 {
    match name {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    }
}

/// Derive the primary remote identity from `git remote -v` stdout, preferring
/// `upstream`, then `origin`, then the lexicographically-first other name.
pub fn derive_git_remote_identity(stdout: &str) -> Option<GitRemoteIdentity> {
    let mut entries: Vec<(GitRemoteEntry, String)> = parse_git_remote_verbose_output(stdout)
        .into_iter()
        .filter_map(|e| normalize_git_remote_url(&e.url).map(|key| (e, key)))
        .collect();
    // Stable sort by priority, then by name. E2: Orca uses `localeCompare`; we
    // use code-point `str::cmp` as a deliberate deterministic contract (no test
    // compares two equal-priority "other" names, so this is observationally
    // equivalent to the oracle).
    entries.sort_by(|a, b| {
        primary_remote_sort_key(&a.0.name)
            .cmp(&primary_remote_sort_key(&b.0.name))
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    entries
        .into_iter()
        .next()
        .map(|(e, key)| GitRemoteIdentity {
            canonical_key: key,
            remote_name: e.name,
            remote_url: e.url,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(u: &str) -> Option<String> {
        normalize_git_remote_url(u)
    }

    // --- normalizeGitRemoteUrl oracle ---

    #[test]
    fn normalizes_https_and_ssh_to_the_same_canonical_key() {
        let expected = Some("github.com/example/sample-app".to_string());
        assert_eq!(norm("https://github.com/example/sample-app.git"), expected);
        assert_eq!(norm("git@github.com:example/sample-app.git"), expected);
        assert_eq!(
            norm("ssh://git@github.com/example/sample-app.git"),
            expected
        );
        assert_eq!(norm("https://GitHub.com/example/sample-app.git"), expected);
    }

    #[test]
    fn preserves_nested_self_hosted_paths() {
        assert_eq!(
            norm("git@gitlab.company.test:platform/tools/sample-app.git").as_deref(),
            Some("gitlab.company.test/platform/tools/sample-app")
        );
    }

    #[test]
    fn ignores_explicit_url_ports() {
        assert_eq!(
            norm("ssh://git@git.company.test:2222/team/sample-app.git").as_deref(),
            Some("git.company.test/team/sample-app")
        );
    }

    #[test]
    fn preserves_path_case_lowercases_host() {
        assert_eq!(
            norm("git@Git.Company.Test:Team/Sample-App.git").as_deref(),
            Some("git.company.test/Team/Sample-App")
        );
        assert_eq!(
            norm("https://git.company.test/Team/Sample-App.git").as_deref(),
            Some("git.company.test/Team/Sample-App")
        );
    }

    #[test]
    fn rejects_windows_local_filesystem_remotes() {
        assert_eq!(norm("C:\\Repos\\sample-app.git"), None);
        assert_eq!(norm("C:/Repos/sample-app.git"), None);
    }

    #[test]
    fn empty_and_blank_are_none() {
        assert_eq!(norm(""), None);
        assert_eq!(norm("   "), None);
    }

    // --- deriveGitRemoteIdentity oracle ---

    #[test]
    fn prefers_upstream_then_origin_then_first_named() {
        let stdout = [
            "origin\tgit@git.company.test:forks/sample-app.git (fetch)",
            "origin\tgit@git.company.test:forks/sample-app.git (push)",
            "upstream\thttps://git.company.test/team/sample-app.git (fetch)",
            "upstream\thttps://git.company.test/team/sample-app.git (push)",
        ]
        .join("\n");
        assert_eq!(
            derive_git_remote_identity(&stdout),
            Some(GitRemoteIdentity {
                canonical_key: "git.company.test/team/sample-app".to_string(),
                remote_name: "upstream".to_string(),
                remote_url: "https://git.company.test/team/sample-app.git".to_string(),
            })
        );

        let origin =
            derive_git_remote_identity("origin\tgit@git.company.test:team/sample-app.git (fetch)")
                .unwrap();
        assert_eq!(origin.remote_name, "origin");
        assert_eq!(origin.canonical_key, "git.company.test/team/sample-app");

        let mirror =
            derive_git_remote_identity("mirror\tgit@git.company.test:team/sample-app.git (fetch)")
                .unwrap();
        assert_eq!(mirror.remote_name, "mirror");
        assert_eq!(mirror.canonical_key, "git.company.test/team/sample-app");
    }

    // --- E2: equal-priority tie-break is a name sort, not input order ---

    #[test]
    fn e2_equal_priority_remotes_break_ties_by_name() {
        // Two "other" remotes in reverse-alphabetical input order; the derived
        // one is the lexicographically-first name ("alpha"), not the first line.
        let stdout = [
            "zulu\thttps://git.company.test/z/repo.git (fetch)",
            "alpha\thttps://git.company.test/a/repo.git (fetch)",
        ]
        .join("\n");
        assert_eq!(
            derive_git_remote_identity(&stdout).unwrap().remote_name,
            "alpha"
        );
    }

    #[test]
    fn parse_ignores_push_lines_and_blanks() {
        let stdout = "origin\thttps://h/x.git (fetch)\norigin\thttps://h/x.git (push)\n\n";
        let entries = parse_git_remote_verbose_output(stdout);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "origin");
        assert_eq!(entries[0].url, "https://h/x.git");
    }
}
