use crate::pr_actions::MergeabilityState;
use crate::provider::{ChecksSummary, RepoCoords, ReviewState};
use serde::Deserialize;

/// `glab mr create`의 출력에서 MR 번호(iid)+URL을 복구한다. **`glab mr create`는 신뢰할
/// 만한 `--output json`이 없어**(Orca `parseMergeRequestPayload`도 JSON을 먼저 시도한 뒤
/// 텍스트 URL 파싱으로 폴백한다), 출력된 URL을 파싱하는 수밖에 없다. gh `pr create`와 같은
/// "사람 텍스트 안 긁는다" 규칙에 대한 **의도된 예외**다.
///
/// Orca `merge-request-creation-lookup.ts`를 미러: 먼저 JSON(`iid`/`web_url`)을 시도하고,
/// 실패하면 텍스트에서 `https?://<host>/<path>/-/merge_requests/<n>` 를 스캔한다. GitLab MR
/// URL은 GitHub의 `/pull/<n>`과 달리 `/-/merge_requests/<n>` 세그먼트를 쓴다.
pub fn parse_created_mr(stdout: &str) -> Option<(u64, String)> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1. JSON 먼저(`glab mr create`가 언젠가 JSON을 내면 그대로 잡는다).
    if let Ok(v) = serde_json::from_str::<GlabCreatedMr>(trimmed) {
        if let Some(hit) = v.into_number_url() {
            return Some(hit);
        }
    }
    // 2. 텍스트 URL 스캔(glab의 일반 출력).
    for line in trimmed.lines() {
        for token in line.split_whitespace() {
            if let Some(hit) = parse_mr_url(token) {
                return Some(hit);
            }
        }
    }
    None
}

/// `glab mr create`가 JSON을 낼 경우의 형태(iid/number + web_url/url).
#[derive(Debug, Deserialize)]
struct GlabCreatedMr {
    #[serde(default)]
    iid: Option<u64>,
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    web_url: Option<String>,
    #[serde(rename = "webUrl", default)]
    web_url_camel: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

impl GlabCreatedMr {
    fn into_number_url(self) -> Option<(u64, String)> {
        let number = self.iid.or(self.number)?;
        if number == 0 {
            return None;
        }
        let url = self
            .web_url
            .or(self.web_url_camel)
            .or(self.url)
            .filter(|u| !u.trim().is_empty())?;
        Some((number, url.trim().to_string()))
    }
}

/// 단일 토큰이 `https?://host/<path>/-/merge_requests/<digits>`면 (번호, 정규화된 URL) 반환.
/// GitLab의 MR URL 형태다(nested group이라 path 세그먼트 개수는 가변 — `/-/`를 앵커로 쓴다).
fn parse_mr_url(token: &str) -> Option<(u64, String)> {
    let scheme_len = if token.starts_with("https://") {
        "https://".len()
    } else if token.starts_with("http://") {
        "http://".len()
    } else {
        return None;
    };
    // `/-/merge_requests/` 앵커를 찾는다.
    let anchor = "/-/merge_requests/";
    let idx = token.find(anchor)?;
    let after = &token[idx + anchor.len()..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let number: u64 = digits.parse().ok()?;
    if number == 0 {
        return None;
    }
    // scheme과 host/path가 비어 있지 않은지 최소 검증.
    let host_and_path = &token[scheme_len..idx];
    if host_and_path.is_empty() || !host_and_path.contains('/') {
        return None;
    }
    // 정규화된 URL(쿼리/후행 세그먼트 제거).
    let url = format!("{}{}", &token[..idx + anchor.len()], number);
    Some((number, url))
}

/// "glab version 1.36.0 (2024-01-01)"의 첫 줄에서 (major, minor)를 뽑는다(gh 미러).
pub fn parse_glab_version(stdout: &str) -> Option<(u32, u32)> {
    let first = stdout.lines().next()?;
    let token = first
        .split_whitespace()
        .skip_while(|t| *t != "version")
        .nth(1)?;
    // "v1.36.0" 같은 선행 v 제거.
    let token = token.trim_start_matches('v');
    let mut parts = token.split('.');
    let maj: u32 = parts.next()?.parse().ok()?;
    let min: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some((maj, min))
}

/// `glab mr view <sel> --output json` 출력의 필요한 필드. 나머지는 무시한다.
#[derive(Debug, Deserialize)]
pub struct GlabMrView {
    pub iid: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub web_url: String,
    /// GitLab은 draft를 `draft`(신) 또는 `work_in_progress`(구)로 낸다 — 둘 다 본다.
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub work_in_progress: bool,
    #[serde(default)]
    pub has_conflicts: bool,
    /// 신 GitLab: 세분화된 머지 상태(mergeable/conflict/not_approved/...).
    #[serde(default)]
    pub detailed_merge_status: String,
    /// 구 GitLab: can_be_merged / cannot_be_merged / unchecked / checking.
    #[serde(default)]
    pub merge_status: String,
    /// 신: head_pipeline, 구: pipeline. 둘 다 시도한다(Orca `derivePipelineStatus`).
    #[serde(default)]
    pub head_pipeline: Option<GlabPipeline>,
    #[serde(default)]
    pub pipeline: Option<GlabPipeline>,
}

#[derive(Debug, Deserialize)]
pub struct GlabPipeline {
    #[serde(default)]
    pub status: String,
}

impl GlabMrView {
    pub fn is_draft(&self) -> bool {
        self.draft || self.work_in_progress
    }

    /// GitLab state("opened"/"closed"/"merged"/"locked") + draft → `ReviewState`.
    pub fn review_state(&self) -> ReviewState {
        match self.state.to_ascii_lowercase().as_str() {
            "merged" => ReviewState::Merged,
            "closed" | "locked" => ReviewState::Closed,
            "opened" if self.is_draft() => ReviewState::Draft,
            _ => ReviewState::Open,
        }
    }

    /// 파이프라인 상태를 passing/failing/pending 카운트로 요약한다. gh가 개별 체크를 세는
    /// 것과 달리 GitLab MR은 head pipeline 하나로 롤업된다(Orca `derivePipelineStatus`
    /// 정신) — 이는 의식적 단순화다. 파이프라인이 없으면 빈 요약(모름).
    pub fn checks_summary(&self) -> ChecksSummary {
        let status = self
            .head_pipeline
            .as_ref()
            .or(self.pipeline.as_ref())
            .map(|p| p.status.to_ascii_lowercase())
            .unwrap_or_default();
        let mut s = ChecksSummary::default();
        match status.as_str() {
            "success" => s.passing = 1,
            "failed" | "canceled" | "cancelled" => s.failing = 1,
            "running" | "pending" | "created" | "preparing" | "waiting_for_resource"
            | "scheduled" => s.pending = 1,
            // "manual"/"skipped"/""/미지 값은 세지 않는다(보수적).
            _ => {}
        }
        s
    }

    /// 머지가능성 4-상태. gh `mergeability_from_fields`의 우선순위를 미러하되 GitLab 필드로:
    /// 승인 필요/변경 요청 → 충돌 → 그 밖 차단 → mergeable → Unknown. 어느 것도 안 맞으면
    /// **`Mergeable`이 아니라 `Unknown`**으로 떨어진다(안전 흡수 상태).
    pub fn mergeability(&self) -> MergeabilityState {
        let detailed = self.detailed_merge_status.to_ascii_lowercase();
        let merge_status = self.merge_status.to_ascii_lowercase();

        // 1. 승인 필요/변경 요청 — 차단.
        if detailed == "not_approved" || detailed == "requested_changes" {
            return MergeabilityState::Blocked;
        }
        // 2. 충돌.
        if self.has_conflicts || detailed == "conflict" {
            return MergeabilityState::Conflicting;
        }
        // 3. 그 밖 차단(파이프라인·토론·draft·rebase·닫힘 등).
        if matches!(
            detailed.as_str(),
            "blocked_status"
                | "ci_must_pass"
                | "ci_still_running"
                | "discussions_not_resolved"
                | "draft_status"
                | "need_rebase"
                | "not_open"
                | "external_status_checks"
                | "requires_rebase"
        ) {
            return MergeabilityState::Blocked;
        }
        // 4. 머지 가능. detailed가 우선, 없으면 구 merge_status.
        if detailed == "mergeable" || (detailed.is_empty() && merge_status == "can_be_merged") {
            return MergeabilityState::Mergeable;
        }
        // 5. 그 밖(unchecked/checking/cannot_be_merged/빈 값·미지) — 안전한 Unknown.
        //    **절대 Mergeable로 넘기지 않는다.**
        MergeabilityState::Unknown
    }
}

/// `glab api projects/:id/merge_requests/:iid/notes` 원소(이슈-레벨/일반 코멘트).
#[derive(Debug, Deserialize)]
pub struct GlabNote {
    #[serde(default)]
    pub author: Option<GlabUser>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    /// system note(자동 생성: "changed the description" 등)는 대화 코멘트가 아니라 제외한다.
    #[serde(default)]
    pub system: bool,
}

/// `glab api projects/:id/merge_requests/:iid/approvals` 응답의 승인자.
#[derive(Debug, Deserialize)]
pub struct GlabApprovals {
    #[serde(default)]
    pub approved_by: Vec<GlabApprovedBy>,
}

#[derive(Debug, Deserialize)]
pub struct GlabApprovedBy {
    #[serde(default)]
    pub user: Option<GlabUser>,
}

#[derive(Debug, Deserialize)]
pub struct GlabUser {
    #[serde(default)]
    pub username: String,
}

/// glab의 project 좌표를 git 원격 URL에서 파싱한다(Orca `parseGitLabProjectRef` 정신).
/// gh impl은 `gh repo view`로 좌표를 얻지만, GitLab의 project path는 nested group을 담을 수
/// 있어(`group/sub/project`) 원격 URL을 직접 파싱하는 것이 더 충실하다 — 그리고 이 파싱이
/// **provider 라우팅**(이 remote가 GitLab인가)의 근거가 된다.
///
/// 지원 형태: `https://host[:port]/group/.../project[.git]`,
/// `git@host:group/.../project[.git]`(scp), `ssh://git@host[:port]/group/.../project[.git]`.
/// GitLab 호스트가 아니면 None.
pub fn parse_gitlab_remote(url: &str) -> Option<RepoCoords> {
    let (host, path) = split_remote(url)?;
    if !is_gitlab_host(&host) {
        return None;
    }
    let path = strip_git_suffix(path.trim_matches('/'));
    // 최소 group/project 두 세그먼트.
    let last_slash = path.rfind('/')?;
    let owner = &path[..last_slash];
    let repo = &path[last_slash + 1..];
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(RepoCoords {
        owner: owner.to_string(),
        repo: repo.to_string(),
        host,
    })
}

/// 원격 URL을 (host_authority, path)로 가른다. host_authority는 포트를 포함할 수 있다.
fn split_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    // scp 형태: git@host:group/project(.git) — 스킴이 없고 host:path.
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
    // userinfo@ 제거.
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

fn strip_git_suffix(path: &str) -> String {
    path.strip_suffix(".git").unwrap_or(path).to_string()
}

/// host가 GitLab인지. `gitlab.com`(+서브도메인)과 host 이름에 "gitlab"이 든 self-hosted를
/// 인식한다. **휴리스틱이다** — Orca는 명시적 known-hosts 설정(config)까지 보지만, 7c는
/// 최소 라우팅으로 이 휴리스틱을 쓴다(임의 self-hosted 호스트명 인식은 후속). 포트는 무시.
pub fn is_gitlab_host(host: &str) -> bool {
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    host == "gitlab.com" || host.ends_with(".gitlab.com") || host.contains("gitlab")
}

/// glab `-R`/`--repo` 인자 = project path(`owner/repo`, nested group이면 `group/sub/repo`).
/// **호스트는 여기 붙이지 않는다** — Orca가 `-R <path>` + `--hostname <host>`로 분리하는 것을
/// 미러한다([`hostname_flag`]). glab의 `-R host/owner/repo` 파싱은 버전 간 불확실하지만
/// `--hostname`은 확실히 지원되므로, 명시적으로 나눈다.
pub fn glab_repo_arg(repo: &RepoCoords) -> String {
    format!("{}/{}", repo.owner, repo.repo)
}

/// self-hosted 호스트를 `--hostname`으로 못 박아야 하면 그 호스트를 돌려준다. gitlab.com은
/// glab의 기본 호스트라 None(플래그 생략). 이렇게 해야 neutral cwd에서 부른 glab이 올바른
/// GitLab 인스턴스를 친다(Orca `glabHostnameArgs` 정신).
pub fn hostname_flag(repo: &RepoCoords) -> Option<&str> {
    let host_only = repo.host.split(':').next().unwrap_or(&repo.host);
    if host_only.eq_ignore_ascii_case("gitlab.com") {
        None
    } else {
        Some(&repo.host)
    }
}

/// URL-인코딩된 project path(`glab api projects/<enc>/...`용). 슬래시를 `%2F`로 이스케이프
/// 한다(Orca `encodedProject` = `encodeURIComponent`). nested group을 REST가 한 세그먼트로
/// 받게 하려는 것이다.
pub fn encoded_project(repo: &RepoCoords) -> String {
    let full = format!("{}/{}", repo.owner, repo.repo);
    let mut out = String::with_capacity(full.len());
    for b in full.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_gitlab_com_mr_url() {
        let out = "https://gitlab.com/acme/widget/-/merge_requests/123\n";
        assert_eq!(
            parse_created_mr(out),
            Some((
                123,
                "https://gitlab.com/acme/widget/-/merge_requests/123".to_string()
            ))
        );
    }

    /// **MR 번호 파싱 회귀 방어**: nested group + 안내 텍스트 + self-hosted 호스트도 잡아야
    /// 한다. 이 파싱이 깨지면 생성한 MR을 worktree에 연결할 수 없다.
    #[test]
    fn parses_nested_group_and_ignores_surrounding_text() {
        let out = "\nCreating merge request for feat into main\n\
                   https://gitlab.example.com/group/sub/widget/-/merge_requests/7 (draft)\n";
        assert_eq!(
            parse_created_mr(out),
            Some((
                7,
                "https://gitlab.example.com/group/sub/widget/-/merge_requests/7".to_string()
            ))
        );
    }

    #[test]
    fn parses_json_payload_when_present() {
        let out = r#"{"iid":42,"web_url":"https://gitlab.com/acme/widget/-/merge_requests/42"}"#;
        assert_eq!(
            parse_created_mr(out),
            Some((
                42,
                "https://gitlab.com/acme/widget/-/merge_requests/42".to_string()
            ))
        );
    }

    #[test]
    fn non_mr_output_yields_none() {
        assert_eq!(parse_created_mr("https://gitlab.com/acme/widget/-/issues/9"), None);
        assert_eq!(parse_created_mr("some warning without a url"), None);
        assert_eq!(
            parse_created_mr("https://gitlab.com/acme/widget/-/merge_requests/0"),
            None
        );
    }

    #[test]
    fn version_parsing() {
        assert_eq!(
            parse_glab_version("glab version 1.36.0 (2024-05-01)"),
            Some((1, 36))
        );
        assert_eq!(parse_glab_version("glab version v1.40.1"), Some((1, 40)));
    }

    #[test]
    fn remote_parsing_https_and_scp_and_ssh() {
        assert_eq!(
            parse_gitlab_remote("https://gitlab.com/acme/widget.git"),
            Some(RepoCoords {
                owner: "acme".into(),
                repo: "widget".into(),
                host: "gitlab.com".into()
            })
        );
        assert_eq!(
            parse_gitlab_remote("git@gitlab.com:group/sub/widget.git"),
            Some(RepoCoords {
                owner: "group/sub".into(),
                repo: "widget".into(),
                host: "gitlab.com".into()
            })
        );
        assert_eq!(
            parse_gitlab_remote("ssh://git@gitlab.example.com:2222/acme/widget"),
            Some(RepoCoords {
                owner: "acme".into(),
                repo: "widget".into(),
                host: "gitlab.example.com:2222".into()
            })
        );
    }

    #[test]
    fn non_gitlab_remote_is_none() {
        assert_eq!(parse_gitlab_remote("https://github.com/acme/widget.git"), None);
        assert_eq!(parse_gitlab_remote("git@bitbucket.org:acme/widget.git"), None);
    }

    #[test]
    fn repo_arg_is_path_only_and_hostname_qualifies_self_hosted() {
        let com = RepoCoords {
            owner: "acme".into(),
            repo: "widget".into(),
            host: "gitlab.com".into(),
        };
        assert_eq!(glab_repo_arg(&com), "acme/widget");
        // gitlab.com은 기본 호스트 → --hostname 생략.
        assert_eq!(hostname_flag(&com), None);
        let sh = RepoCoords {
            owner: "group/sub".into(),
            repo: "widget".into(),
            host: "gitlab.example.com".into(),
        };
        assert_eq!(glab_repo_arg(&sh), "group/sub/widget");
        assert_eq!(hostname_flag(&sh), Some("gitlab.example.com"));
    }

    #[test]
    fn encoded_project_escapes_slashes() {
        let repo = RepoCoords {
            owner: "group/sub".into(),
            repo: "widget".into(),
            host: "gitlab.com".into(),
        };
        assert_eq!(encoded_project(&repo), "group%2Fsub%2Fwidget");
    }

    #[test]
    fn draft_and_state_mapping() {
        let mk = |state: &str, draft: bool| GlabMrView {
            iid: 1,
            title: "t".into(),
            state: state.into(),
            web_url: "u".into(),
            draft,
            work_in_progress: false,
            has_conflicts: false,
            detailed_merge_status: String::new(),
            merge_status: String::new(),
            head_pipeline: None,
            pipeline: None,
        };
        assert_eq!(mk("opened", false).review_state(), ReviewState::Open);
        assert_eq!(mk("opened", true).review_state(), ReviewState::Draft);
        assert_eq!(mk("merged", false).review_state(), ReviewState::Merged);
        assert_eq!(mk("closed", false).review_state(), ReviewState::Closed);
        assert_eq!(mk("locked", false).review_state(), ReviewState::Closed);
        // merged인데 draft여도 merged가 이긴다.
        assert_eq!(mk("merged", true).review_state(), ReviewState::Merged);
    }

    fn mr_with(
        detailed: &str,
        merge_status: &str,
        has_conflicts: bool,
        pipeline: Option<&str>,
    ) -> GlabMrView {
        GlabMrView {
            iid: 1,
            title: "t".into(),
            state: "opened".into(),
            web_url: "u".into(),
            draft: false,
            work_in_progress: false,
            has_conflicts,
            detailed_merge_status: detailed.into(),
            merge_status: merge_status.into(),
            head_pipeline: pipeline.map(|s| GlabPipeline { status: s.into() }),
            pipeline: None,
        }
    }

    #[test]
    fn checks_summary_from_pipeline() {
        assert_eq!(mr_with("", "", false, Some("success")).checks_summary().passing, 1);
        assert_eq!(mr_with("", "", false, Some("failed")).checks_summary().failing, 1);
        assert_eq!(mr_with("", "", false, Some("canceled")).checks_summary().failing, 1);
        assert_eq!(mr_with("", "", false, Some("running")).checks_summary().pending, 1);
        // 파이프라인 없음/skipped → 빈 요약.
        assert_eq!(mr_with("", "", false, None).checks_summary(), ChecksSummary::default());
        assert_eq!(
            mr_with("", "", false, Some("skipped")).checks_summary(),
            ChecksSummary::default()
        );
    }

    #[test]
    fn mergeability_conflict_blocked_mergeable() {
        assert_eq!(
            mr_with("conflict", "cannot_be_merged", true, None).mergeability(),
            MergeabilityState::Conflicting
        );
        assert_eq!(
            mr_with("not_approved", "", false, None).mergeability(),
            MergeabilityState::Blocked
        );
        assert_eq!(
            mr_with("ci_still_running", "", false, None).mergeability(),
            MergeabilityState::Blocked
        );
        assert_eq!(
            mr_with("mergeable", "can_be_merged", false, None).mergeability(),
            MergeabilityState::Mergeable
        );
        // 구 GitLab: detailed 없이 merge_status만.
        assert_eq!(
            mr_with("", "can_be_merged", false, None).mergeability(),
            MergeabilityState::Mergeable
        );
    }

    /// **핵심 회귀 방어 (b)**: 불완전/알 수 없는 메타데이터는 `Unknown`이지 절대
    /// `Mergeable`이 아니다.
    #[test]
    fn unknown_metadata_is_unknown_never_mergeable() {
        assert_eq!(
            mr_with("", "", false, None).mergeability(),
            MergeabilityState::Unknown
        );
        assert_eq!(
            mr_with("checking", "unchecked", false, None).mergeability(),
            MergeabilityState::Unknown
        );
        // cannot_be_merged인데 충돌 플래그 없음 → 안전하게 Unknown(Mergeable 아님).
        assert_eq!(
            mr_with("", "cannot_be_merged", false, None).mergeability(),
            MergeabilityState::Unknown
        );
    }
}
