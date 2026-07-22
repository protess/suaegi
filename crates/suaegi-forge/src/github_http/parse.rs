//! GitHub REST v3 JSON 모양 → 공유 도메인 타입, + `origin` 원격 URL 파싱. gh 백엔드는
//! `gh repo view`로 좌표를 얻지만 HTTP 백엔드는 `gh`가 없을 수도 있어(그게 이 백엔드를 고른
//! 이유다) `git remote get-url origin`을 직접 파싱한다(gitlab `parse_gitlab_remote` 미러).

use crate::pr_actions::MergeabilityState;
use crate::provider::{RepoCoords, ReviewState};
use serde::Deserialize;

/// `git remote get-url origin` URL에서 GitHub 좌표를 파싱한다. GitHub 원격이 아니면 None
/// (→ resolve_repository가 "GitHub 아님"으로 접는다). 지원 형태(gitlab 미러):
/// `https://host[:port]/owner/repo[.git]`, `git@host:owner/repo[.git]`(scp),
/// `ssh://git@host[:port]/owner/repo[.git]`.
///
/// GitHub은 project path가 nested group을 안 갖는다(정확히 `owner/repo` 2세그먼트) — GitLab과
/// 다른 지점. 첫 두 세그먼트만 취하고 나머지는 무시한다.
pub fn parse_github_remote(url: &str) -> Option<RepoCoords> {
    let (host, path) = split_remote(url)?;
    if !is_github_host(&host) {
        return None;
    }
    let path = path.trim_matches('/');
    let mut segs = path.split('/');
    let owner = segs.next().filter(|s| !s.is_empty())?;
    let repo_seg = segs.next().filter(|s| !s.is_empty())?;
    let repo = repo_seg.strip_suffix(".git").unwrap_or(repo_seg);
    if repo.is_empty() {
        return None;
    }
    Some(RepoCoords {
        owner: owner.to_string(),
        repo: repo.to_string(),
        host,
    })
}

/// 원격 URL을 (host_authority, path)로 가른다(gitlab `split_remote` 미러). host_authority는
/// 포트를 포함할 수 있다.
fn split_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    // scp 형태: git@host:owner/repo(.git).
    if !url.contains("://") {
        if let Some(at) = url.find('@') {
            let rest = &url[at + 1..];
            if let Some(colon) = rest.find(':') {
                let host = &rest[..colon];
                let path = &rest[colon + 1..];
                if !host.is_empty() && !path.is_empty() {
                    return Some((host.to_string(), path.to_string()));
                }
            }
        }
        return None;
    }
    // 스킴 형태: https:// | http:// | ssh://
    let after_scheme = url.split_once("://")?.1;
    let after_userinfo = match after_scheme.find('@') {
        Some(at) => &after_scheme[at + 1..],
        None => after_scheme,
    };
    let slash = after_userinfo.find('/')?;
    let host = &after_userinfo[..slash];
    let path = &after_userinfo[slash + 1..];
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

/// host가 GitHub인지. `github.com`과 GHES(`github.<corp>`/host에 "github" 포함)를 인식한다.
/// **휴리스틱이다**(gitlab `is_gitlab_host` 미러) — 임의 self-hosted 호스트명 인식은 후속.
/// 포트는 무시.
pub fn is_github_host(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    host == "github.com" || host.ends_with(".github.com") || host.contains("github")
}

/// GitHub REST **API 베이스 URL**. github.com은 `api.github.com`, GHES는
/// `https://<host>/api/v3`(엔터프라이즈 관례). host에 포트가 있으면 유지.
pub fn api_base(host: &str) -> String {
    let h = host.trim();
    if h.eq_ignore_ascii_case("github.com") || h.eq_ignore_ascii_case("www.github.com") {
        "https://api.github.com".to_string()
    } else {
        format!("https://{h}/api/v3")
    }
}

/// `GET /repos/{o}/{r}/pulls/{n}` 또는 목록 원소. 필요한 필드만.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpPr {
    pub number: u64,
    #[serde(default)]
    pub title: String,
    /// "open" | "closed".
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    /// 단일 PR GET에 있는 bool. 목록에는 없을 수 있어 Option.
    #[serde(default)]
    pub merged: Option<bool>,
    /// 목록/단일 공통. merged면 timestamp, 아니면 null.
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub html_url: String,
    /// null이면 GitHub이 아직 계산 중 → Unknown.
    #[serde(default)]
    pub mergeable: Option<bool>,
    /// clean|dirty|blocked|behind|unstable|draft|unknown|has_hooks.
    #[serde(default)]
    pub mergeable_state: String,
    /// head 커밋(check-runs 조회용 sha). 목록/단일 공통.
    #[serde(default)]
    pub head: Option<HttpPrHead>,
}

/// PR head 참조. check-runs를 sha로 조회하는 데만 쓴다.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpPrHead {
    #[serde(default)]
    pub sha: String,
}

impl HttpPr {
    fn is_merged(&self) -> bool {
        self.merged == Some(true) || self.merged_at.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    /// REST state("open"/"closed") + merged + draft → `ReviewState`(gh `review_state` 미러:
    /// merged가 draft/closed를 이긴다).
    pub fn review_state(&self) -> ReviewState {
        if self.is_merged() {
            return ReviewState::Merged;
        }
        match self.state.to_ascii_lowercase().as_str() {
            "closed" => ReviewState::Closed,
            "open" if self.draft => ReviewState::Draft,
            _ => ReviewState::Open,
        }
    }

    /// `mergeable`(bool|null) + `mergeable_state`를 4-상태로. **우선순위가 load-bearing**
    /// (gh `mergeability_from_fields` 미러): 충돌 → blocked/behind/draft → mergeable →
    /// Unknown. null/unknown/미지 값은 **절대 Mergeable이 아니라 Unknown**으로 떨어진다.
    pub fn mergeability(&self) -> MergeabilityState {
        let s = self.mergeable_state.to_ascii_lowercase();
        // 1. 충돌: dirty 상태 또는 mergeable=false.
        if s == "dirty" || self.mergeable == Some(false) {
            return MergeabilityState::Conflicting;
        }
        // 2. 차단: blocked(필수 리뷰/체크)·behind·draft.
        if matches!(s.as_str(), "blocked" | "behind" | "draft") {
            return MergeabilityState::Blocked;
        }
        // 3. 머지 가능: mergeable=true이고 상태가 clean류. unstable=비필수 체크 실패(머지는 가능).
        if self.mergeable == Some(true) && matches!(s.as_str(), "clean" | "has_hooks" | "unstable") {
            return MergeabilityState::Mergeable;
        }
        // 4. 그 밖(unknown·빈 값·null mergeable) — 안전한 Unknown. **절대 Mergeable 아님.**
        MergeabilityState::Unknown
    }
}

/// `GET /repos/{o}/{r}/pulls/{n}/reviews` 원소. state는 gh와 같은 대문자 토큰.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpReview {
    #[serde(default)]
    pub user: Option<HttpUser>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub submitted_at: String,
}

/// `GET /repos/{o}/{r}/issues/{n}/comments` 원소(이슈-레벨 대화 코멘트, gh `comments` 대응).
#[derive(Debug, Clone, Deserialize)]
pub struct HttpComment {
    #[serde(default)]
    pub user: Option<HttpUser>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HttpUser {
    #[serde(default)]
    pub login: String,
}

/// null/빈 user → "ghost"(gh·gitlab과 동일).
pub fn user_login(u: Option<HttpUser>) -> String {
    match u {
        Some(u) if !u.login.is_empty() => u.login,
        _ => "ghost".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_scp_ssh_github_remotes() {
        assert_eq!(
            parse_github_remote("https://github.com/acme/widget.git"),
            Some(RepoCoords {
                owner: "acme".into(),
                repo: "widget".into(),
                host: "github.com".into()
            })
        );
        assert_eq!(
            parse_github_remote("git@github.com:acme/widget.git"),
            Some(RepoCoords {
                owner: "acme".into(),
                repo: "widget".into(),
                host: "github.com".into()
            })
        );
        assert_eq!(
            parse_github_remote("ssh://git@github.example.com:2222/acme/widget"),
            Some(RepoCoords {
                owner: "acme".into(),
                repo: "widget".into(),
                host: "github.example.com:2222".into()
            })
        );
    }

    #[test]
    fn non_github_remote_is_none() {
        assert_eq!(parse_github_remote("https://gitlab.com/acme/widget.git"), None);
        assert_eq!(parse_github_remote("git@bitbucket.org:acme/widget.git"), None);
    }

    #[test]
    fn api_base_github_com_and_ghes() {
        assert_eq!(api_base("github.com"), "https://api.github.com");
        assert_eq!(api_base("ghe.corp.example"), "https://ghe.corp.example/api/v3");
    }

    fn pr(state: &str, draft: bool, merged: Option<bool>, merged_at: Option<&str>) -> HttpPr {
        HttpPr {
            number: 1,
            title: "t".into(),
            state: state.into(),
            draft,
            merged,
            merged_at: merged_at.map(|s| s.to_string()),
            html_url: "u".into(),
            mergeable: None,
            mergeable_state: String::new(),
            head: None,
        }
    }

    #[test]
    fn review_state_mapping_merged_wins() {
        assert_eq!(pr("open", false, None, None).review_state(), ReviewState::Open);
        assert_eq!(pr("open", true, None, None).review_state(), ReviewState::Draft);
        assert_eq!(pr("closed", false, None, None).review_state(), ReviewState::Closed);
        assert_eq!(
            pr("closed", false, Some(true), None).review_state(),
            ReviewState::Merged
        );
        // merged_at set even if merged bool absent (list endpoint).
        assert_eq!(
            pr("closed", true, None, Some("2020-01-01T00:00:00Z")).review_state(),
            ReviewState::Merged
        );
    }

    fn pr_merge(mergeable: Option<bool>, state: &str) -> HttpPr {
        HttpPr {
            number: 1,
            title: String::new(),
            state: "open".into(),
            draft: false,
            merged: None,
            merged_at: None,
            html_url: String::new(),
            mergeable,
            mergeable_state: state.into(),
            head: None,
        }
    }

    #[test]
    fn mergeability_conflict_blocked_mergeable() {
        assert_eq!(
            pr_merge(Some(false), "dirty").mergeability(),
            MergeabilityState::Conflicting
        );
        assert_eq!(
            pr_merge(Some(true), "blocked").mergeability(),
            MergeabilityState::Blocked
        );
        assert_eq!(
            pr_merge(Some(true), "behind").mergeability(),
            MergeabilityState::Blocked
        );
        assert_eq!(
            pr_merge(Some(true), "clean").mergeability(),
            MergeabilityState::Mergeable
        );
        // unstable = 비필수 체크 실패지만 머지 가능.
        assert_eq!(
            pr_merge(Some(true), "unstable").mergeability(),
            MergeabilityState::Mergeable
        );
    }

    /// **핵심 회귀 방어 (b)**: null mergeable / unknown 상태는 `Unknown`이지 절대
    /// `Mergeable`이 아니다. 우선순위/기본값을 mutate해 Mergeable로 접으면 깨진다.
    #[test]
    fn unknown_metadata_is_unknown_never_mergeable() {
        assert_eq!(
            pr_merge(None, "unknown").mergeability(),
            MergeabilityState::Unknown
        );
        assert_eq!(pr_merge(None, "").mergeability(), MergeabilityState::Unknown);
        // mergeable=null이지만 상태가 clean이어도(계산 중) Mergeable로 넘기지 않는다.
        assert_eq!(
            pr_merge(None, "clean").mergeability(),
            MergeabilityState::Unknown
        );
        // draft는 차단.
        assert_eq!(
            pr_merge(Some(true), "draft").mergeability(),
            MergeabilityState::Blocked
        );
    }

    #[test]
    fn null_user_becomes_ghost() {
        assert_eq!(user_login(None), "ghost");
        assert_eq!(user_login(Some(HttpUser { login: String::new() })), "ghost");
        assert_eq!(
            user_login(Some(HttpUser {
                login: "octocat".into()
            })),
            "octocat"
        );
    }
}
