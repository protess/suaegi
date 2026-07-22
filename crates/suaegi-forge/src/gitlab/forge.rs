use super::classify::{
    classify_glab_merge_failure, classify_glab_unavailable, is_no_merge_request,
};
use super::parse::{
    encoded_project, glab_repo_arg, hostname_flag, parse_created_mr, parse_glab_version,
    parse_gitlab_remote, GlabApprovals, GlabMrView, GlabNote,
};
use super::runner::{GlabError, GlabRunner, CREATE_TIMEOUT};
use crate::pr_actions::{
    CommentLookup, MergeFailure, MergeMethod, MergeOptions, MergeOutcome, MergeabilityState,
    PrActions, PrComment, PrReview, PrReviewState, ReviewThreadLookup,
};
use crate::provider::{
    ChecksSummary, CreateReviewInput, ForgeError, ForgeProvider, ForgeUnavailable, RepoCoords,
    Review, ReviewLookup, ReviewState,
};
use async_trait::async_trait;
use std::path::Path;
use suaegi_git::runner::GitRunner;

/// preflight에서 고정하는 최소 glab 버전. `--output json`(mr view/list)은 glab 1.22 이후
/// 안정적이므로, gh impl이 2.0을 하한으로 못 박는 것과 같은 정신으로 여기서 하한을 둔다.
pub const MIN_GLAB_VERSION: (u32, u32) = (1, 22);

/// glab CLI를 통한 GitLab `ForgeProvider`/`PrActions` 구현. `GhForge`의 near-mechanical
/// 미러다 — 같은 트레잇 뒤에서 같은 found/none/unavailable·확정거부/일시실패 규율을 GitLab
/// MR에 적용한다.
#[derive(Debug, Clone, Default)]
pub struct GlabForge {
    runner: GlabRunner,
}

impl GlabForge {
    pub fn new() -> Self {
        Self {
            runner: GlabRunner::new(),
        }
    }

    /// `-R`/`--hostname`이 명시된 호출용 중립 cwd. glab은 -R/--hostname이 주어지면 cwd의
    /// 원격을 볼 필요가 없다(gh `neutral_cwd`와 동일).
    fn neutral_cwd() -> &'static Path {
        Path::new(".")
    }
}

/// preflight 결과. glab 미설치와 미인증을 **구분**한다(gh `Preflight` 미러, Orca
/// `diagnoseAuth`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlabPreflight {
    Ready,
    /// glab 없음.
    NotInstalled,
    /// glab 있지만 `glab auth status` 실패 → "glab auth login" 안내.
    NotAuthenticated,
    /// glab이 하한보다 낮음.
    OutdatedVersion { found: String, min: String },
}

/// glab 설치·버전·인증을 검사한다. 실패를 불투명하게 던지지 않고 분류해 돌려준다(gh
/// `preflight` 미러).
pub async fn glab_preflight(runner: &GlabRunner) -> GlabPreflight {
    let cwd = Path::new(".");
    match runner.run(cwd, &["--version"]).await {
        Err(e) if e.is_glab_not_found() => return GlabPreflight::NotInstalled,
        Err(_) => return GlabPreflight::NotInstalled,
        Ok(out) => {
            if let Some((maj, min)) = parse_glab_version(&out.stdout) {
                if (maj, min) < MIN_GLAB_VERSION {
                    return GlabPreflight::OutdatedVersion {
                        found: format!("{maj}.{min}"),
                        min: format!("{}.{}", MIN_GLAB_VERSION.0, MIN_GLAB_VERSION.1),
                    };
                }
            }
        }
    }
    // `glab auth status`는 인증돼 있으면 exit 0, 아니면 비-0(주로 1).
    match runner.run_expecting(cwd, &["auth", "status"], &[1]).await {
        Ok(out) if out.code == 0 => GlabPreflight::Ready,
        Ok(_) => GlabPreflight::NotAuthenticated,
        Err(e) if e.is_glab_not_found() => GlabPreflight::NotInstalled,
        Err(_) => GlabPreflight::NotAuthenticated,
    }
}

/// Io/Timeout/TooLarge 계열 GlabError를 분류된 조회-불가로. (Failed는 호출부가 stderr로
/// 별도 처리한다.) gh `unavailable_from_gh_error` 미러.
fn unavailable_from_glab_error(e: &GlabError) -> ForgeUnavailable {
    if e.is_glab_not_found() {
        return ForgeUnavailable::NotInstalled;
    }
    match e {
        GlabError::Timeout { .. } => ForgeUnavailable::Network,
        _ => ForgeUnavailable::Other("GitLab is unavailable".to_string()),
    }
}

impl GlabForge {
    /// project 좌표를 host-qualify하는 공용 인자를 만든다: `-R <path> [--hostname <host>]`.
    /// 반환한 String을 호출부가 소유하고 &str 슬라이스로 args에 얹는다.
    fn repo_args(repo: &RepoCoords) -> (String, Option<&str>) {
        (glab_repo_arg(repo), hostname_flag(repo))
    }

    /// `glab mr view <sel> -R <arg> [--hostname] --output json` 한 조합 → ReviewLookup.
    /// selector는 브랜치명 또는 MR IID 문자열(gh `pr view <selector>`와 같은 uniform path).
    async fn review_by_selector(&self, repo: &RepoCoords, selector: &str) -> ReviewLookup {
        let (repo_arg, host) = Self::repo_args(repo);
        let mut args: Vec<&str> = vec!["mr", "view", selector, "-R", &repo_arg];
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        args.push("--output");
        args.push("json");

        match self.runner.run(Self::neutral_cwd(), &args).await {
            Ok(out) => match serde_json::from_str::<GlabMrView>(&out.stdout) {
                Ok(mr) => ReviewLookup::Found(Review {
                    number: mr.iid,
                    state: mr.review_state(),
                    title: mr.title.clone(),
                    url: mr.web_url.clone(),
                    checks: mr.checks_summary(),
                }),
                // 성공 exit인데 JSON이 안 풀리면 **None이 아니다** — 예상 밖 출력은
                // Unavailable이다(§5 mutation이 잡는 붕괴).
                Err(_) => ReviewLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected glab output".to_string(),
                )),
            },
            Err(GlabError::Failed { stderr, .. }) => {
                // 여기서만 None이 나온다: 비-0 exit + 고정 영어 stderr substring.
                if is_no_merge_request(&stderr) {
                    ReviewLookup::None
                } else {
                    ReviewLookup::Unavailable(classify_glab_unavailable(&stderr))
                }
            }
            Err(e) => ReviewLookup::Unavailable(unavailable_from_glab_error(&e)),
        }
    }
}

#[async_trait]
impl ForgeProvider for GlabForge {
    /// worktree의 `origin` 원격 URL을 파싱해 project 좌표를 얻는다. gh impl은 `gh repo view`를
    /// 쓰지만, GitLab의 project path는 nested group을 담을 수 있고 이 파싱이 provider
    /// 라우팅(이 remote가 GitLab인가)의 근거이므로 Orca `parseGitLabProjectRef`처럼 원격을
    /// 직접 파싱한다. GitLab 원격이 아니면 None.
    async fn resolve_repository(
        &self,
        worktree: &Path,
    ) -> Result<Option<RepoCoords>, ForgeError> {
        let git = GitRunner::new();
        // "no origin" / "not a git repository"는 exit 128 → Failed. 이를 "GitLab 아님"(None)
        // 으로 접는다. git 실행 자체가 안 되는(timeout 등) 경우만 Unavailable.
        match git
            .run(worktree, &["remote", "get-url", "origin"])
            .await
        {
            Ok(out) => Ok(parse_gitlab_remote(out.stdout.trim())),
            Err(suaegi_git::runner::GitError::Failed { .. }) => Ok(None),
            Err(_) => Err(ForgeError::Unavailable(ForgeUnavailable::Other(
                "could not read git remote".to_string(),
            ))),
        }
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        self.review_by_selector(repo, branch).await
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        self.review_by_selector(repo, &number.to_string()).await
    }

    fn supports_review_creation(&self) -> bool {
        true
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
        let repo = match self.resolve_repository(&input.worktree_path).await? {
            Some(r) => r,
            None => {
                return Err(ForgeError::Validation(
                    "Creating merge requests requires a GitLab remote.".to_string(),
                ))
            }
        };

        let base = input.base.trim();
        let title = input.title.trim();
        if base.is_empty() || title.is_empty() {
            return Err(ForgeError::Validation(
                "Create MR failed: base branch and title are required.".to_string(),
            ));
        }
        if let Some(head) = input.head.as_deref() {
            if head.eq_ignore_ascii_case(base) {
                return Err(ForgeError::Validation(
                    "Create MR failed: choose a different base branch before creating a merge request."
                        .to_string(),
                ));
            }
        }

        // body 결정: use_template && body 비었으면 repo 템플릿을 채운다.
        let body = if input.use_template && input.body.trim().is_empty() {
            read_mr_template(&input.worktree_path).unwrap_or_default()
        } else {
            input.body.clone()
        };

        let (repo_arg, host) = Self::repo_args(&repo);
        // `--yes`로 비대화형 확인. `--description`으로 body 전달(Orca `merge-request-creation`).
        let mut args: Vec<&str> = vec![
            "mr",
            "create",
            "-R",
            &repo_arg,
            "--target-branch",
            base,
            "--title",
            title,
            "--description",
            &body,
            "--yes",
        ];
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        if let Some(head) = input.head.as_deref() {
            args.push("--source-branch");
            args.push(head);
        }
        if input.draft {
            args.push("--draft");
        }

        let res = self
            .runner
            .run_with_timeout(&input.worktree_path, &args, CREATE_TIMEOUT)
            .await;

        match res {
            Ok(out) => match parse_created_mr(&out.stdout) {
                Some((number, url)) => Ok(Review {
                    number,
                    state: if input.draft {
                        ReviewState::Draft
                    } else {
                        ReviewState::Open
                    },
                    title: title.to_string(),
                    url,
                    checks: ChecksSummary::default(),
                }),
                None => Err(ForgeError::Parse(
                    "could not determine the created MR number from glab output".to_string(),
                )),
            },
            Err(GlabError::Failed { stderr, .. }) => {
                let lower = stderr.to_lowercase();
                if lower.contains("already exists")
                    || lower.contains("merge request already exists")
                {
                    Err(ForgeError::Validation(
                        "A merge request already exists for this branch.".to_string(),
                    ))
                } else {
                    Err(ForgeError::Unavailable(classify_glab_unavailable(&stderr)))
                }
            }
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_glab_error(&e))),
        }
    }
}

/// worktree에서 GitLab MR 템플릿을 찾아 읽는다(Orca `readMergeRequestTemplate` 미러). 첫 매치만.
fn read_mr_template(worktree: &Path) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        ".gitlab/merge_request_templates/Default.md",
        ".gitlab/merge_request_templates/default.md",
        ".gitlab/merge_request_template.md",
        ".gitlab/MERGE_REQUEST_TEMPLATE.md",
    ];
    for rel in CANDIDATES {
        let p = worktree.join(rel);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(text);
        }
    }
    None
}

/// merge write op은 네트워크 왕복이라 create와 같은 넉넉한 타임아웃(60초)을 쓴다.
const MERGE_TIMEOUT: std::time::Duration = CREATE_TIMEOUT;

/// `MergeMethod` → glab flag. GitLab은 기본이 merge commit이므로 Merge는 **플래그 없음**,
/// Squash/Rebase만 플래그를 붙인다(Orca `mergeMR`의 `method === 'squash' ? ['--squash'] :
/// method === 'rebase' ? ['--rebase'] : []`). gh가 `--merge`를 명시하는 것과의 차이다.
fn glab_merge_method_flag(method: MergeMethod) -> Option<&'static str> {
    match method {
        MergeMethod::Merge => None,
        MergeMethod::Squash => Some("--squash"),
        MergeMethod::Rebase => Some("--rebase"),
    }
}

#[async_trait]
impl PrActions for GlabForge {
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError> {
        // **파괴적**. 이 백엔드는 auto-confirm을 하지 않는다 — UI가 먼저 확인한 뒤 부른다.
        // `--yes`는 glab의 비대화형 확인일 뿐(자동 승인 아님, gh와 동일 규율).
        let (repo_arg, host) = Self::repo_args(repo);
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec!["mr", "merge", &number_str, "-R", &repo_arg, "--yes"];
        if let Some(flag) = glab_merge_method_flag(method) {
            args.push(flag);
        }
        if options.delete_branch {
            args.push("--remove-source-branch");
        }
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }

        let res = self
            .runner
            .run_with_timeout(Self::neutral_cwd(), &args, MERGE_TIMEOUT)
            .await;
        match res {
            Ok(_) => Ok(MergeOutcome::Merged),
            Err(GlabError::Failed { stderr, .. }) => match classify_glab_merge_failure(&stderr) {
                // 확정적 거부는 데이터(Ok), 일시 실패는 에러(Err) — None vs Unavailable 규율.
                MergeFailure::Rejected(reason) => Ok(MergeOutcome::Rejected(reason)),
                MergeFailure::Transient(u) => Err(ForgeError::Unavailable(u)),
            },
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_glab_error(&e))),
        }
    }

    async fn set_auto_merge(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
    ) -> Result<(), ForgeError> {
        let (repo_arg, host) = Self::repo_args(repo);
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec![
            "mr",
            "merge",
            &number_str,
            "-R",
            &repo_arg,
            "--yes",
            "--auto-merge",
        ];
        if let Some(flag) = glab_merge_method_flag(method) {
            args.push(flag);
        }
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        let res = self
            .runner
            .run_with_timeout(Self::neutral_cwd(), &args, MERGE_TIMEOUT)
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(GlabError::Failed { stderr, .. }) => {
                // 이미 머지 가능/파이프라인 없음 등으로 auto-merge를 쓸 수 없으면 확정 거부에
                // 준하는 Validation으로(raw stderr 대신 실행 가능한 안내). 그 밖은 분류된
                // Unavailable(일시 실패를 확정으로 못박지 않는다).
                match classify_glab_merge_failure(&stderr) {
                    MergeFailure::Rejected(_) => Err(ForgeError::Validation(
                        "Auto-merge is not available for this merge request. Use Merge instead."
                            .to_string(),
                    )),
                    MergeFailure::Transient(u) => Err(ForgeError::Unavailable(u)),
                }
            }
            Err(e) => Err(ForgeError::Unavailable(unavailable_from_glab_error(&e))),
        }
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        // GitLab의 "리뷰"는 승인(approvals)이다. `glab api projects/<enc>/merge_requests/<iid>
        // /approvals`로 승인자를 읽는다(Orca가 approvals endpoint를 쓰는 것과 같은 축). GitLab
        // approvals에는 변경-요청 상태가 없어 승인만 `Approved`로 표면화한다(의식적 단순화).
        let path = format!(
            "projects/{}/merge_requests/{}/approvals",
            encoded_project(repo),
            number
        );
        let (_repo_arg, host) = Self::repo_args(repo);
        let mut args: Vec<&str> = vec!["api", &path];
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        match self.runner.run(Self::neutral_cwd(), &args).await {
            Ok(out) => match serde_json::from_str::<GlabApprovals>(&out.stdout) {
                Ok(env) => {
                    let reviews = env
                        .approved_by
                        .into_iter()
                        .map(|a| PrReview {
                            author: a
                                .user
                                .map(|u| u.username)
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "ghost".to_string()),
                            state: PrReviewState::Approved,
                            body: String::new(),
                            submitted_at: String::new(),
                        })
                        .collect();
                    ReviewThreadLookup::Found(reviews)
                }
                // 성공 exit인데 JSON이 안 풀리면 **빈 Found가 아니다** — Unavailable이다.
                Err(_) => ReviewThreadLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected glab output".to_string(),
                )),
            },
            // 일시 실패는 "리뷰 없음"(빈 Found)이 아니라 분류된 Unavailable(캐시-오염 방지).
            Err(GlabError::Failed { stderr, .. }) => {
                ReviewThreadLookup::Unavailable(classify_glab_unavailable(&stderr))
            }
            Err(e) => ReviewThreadLookup::Unavailable(unavailable_from_glab_error(&e)),
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        // `glab api projects/<enc>/merge_requests/<iid>/notes` — MR의 일반 코멘트(note). system
        // note(자동 생성)는 대화 코멘트가 아니라 제외한다.
        let path = format!(
            "projects/{}/merge_requests/{}/notes",
            encoded_project(repo),
            number
        );
        let (_repo_arg, host) = Self::repo_args(repo);
        let mut args: Vec<&str> = vec!["api", &path];
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        match self.runner.run(Self::neutral_cwd(), &args).await {
            Ok(out) => match serde_json::from_str::<Vec<GlabNote>>(&out.stdout) {
                Ok(notes) => {
                    let comments = notes
                        .into_iter()
                        .filter(|n| !n.system)
                        .map(|n| PrComment {
                            author: n
                                .author
                                .map(|u| u.username)
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| "ghost".to_string()),
                            body: n.body,
                            created_at: n.created_at,
                            url: String::new(),
                        })
                        .collect();
                    CommentLookup::Found(comments)
                }
                Err(_) => CommentLookup::Unavailable(ForgeUnavailable::Other(
                    "unexpected glab output".to_string(),
                )),
            },
            Err(GlabError::Failed { stderr, .. }) => {
                CommentLookup::Unavailable(classify_glab_unavailable(&stderr))
            }
            Err(e) => CommentLookup::Unavailable(unavailable_from_glab_error(&e)),
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        // `glab mr view <iid> --output json`의 has_conflicts/detailed_merge_status/merge_status를
        // 4-상태로. 일시 실패·파싱 실패는 `Unknown`(안전)이지 **절대 Mergeable이 아니다**.
        let (repo_arg, host) = Self::repo_args(repo);
        let number_str = number.to_string();
        let mut args: Vec<&str> = vec!["mr", "view", &number_str, "-R", &repo_arg];
        if let Some(host) = host {
            args.push("--hostname");
            args.push(host);
        }
        args.push("--output");
        args.push("json");
        match self.runner.run(Self::neutral_cwd(), &args).await {
            Ok(out) => match serde_json::from_str::<GlabMrView>(&out.stdout) {
                Ok(mr) => mr.mergeability(),
                Err(_) => MergeabilityState::Unknown,
            },
            Err(_) => MergeabilityState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **회귀 방어**: 방식→플래그 매핑이 어긋나면 사용자가 고른 것과 다르게 머지된다.
    /// GitLab은 기본이 merge commit이라 Merge는 플래그 없음(gh가 `--merge`를 명시하는 것과
    /// 다른 지점)이므로 이 매핑을 못 박는다.
    #[test]
    fn merge_method_maps_to_the_right_flag() {
        assert_eq!(glab_merge_method_flag(MergeMethod::Merge), None);
        assert_eq!(glab_merge_method_flag(MergeMethod::Squash), Some("--squash"));
        assert_eq!(glab_merge_method_flag(MergeMethod::Rebase), Some("--rebase"));
    }
}
