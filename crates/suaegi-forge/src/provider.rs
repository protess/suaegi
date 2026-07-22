use async_trait::async_trait;
use std::path::PathBuf;

/// Orca `ForgeProvider`(`forge-provider.ts:60-72`)의 Rust 번역. gh shell-out(7a-1)과
/// HTTP 내장(7a-2), 이후 GitLab(7c)이 **한 트레잇 뒤**에 들어온다. N=1에서도 의식적으로
/// 도입한다(플랜 §0, Codex #Q1) — 지금 안 두면 7a-2에서 gh impl을 뜯어고쳐야 한다.
///
/// **3-상태 모델의 실제 출처는 `github/client.ts:2908` `getPRForBranchOutcome`**(found/
/// no-pr/upstream-error)다. `ReviewLookup`을 트레잇에서 직접 돌려주는 것이 JS의 throw보다
/// 충실한 번역이다(플랜 §1, Codex N1).
#[async_trait]
pub trait ForgeProvider {
    /// **worktree 경로**로 repo 좌표를 해석한다 — URL 문자열이 아니다[Codex B1].
    /// gh impl은 그 cwd에서 `gh repo view`로 owner/repo·호스트를 자체 해석하고, 미래
    /// HTTP impl은 `git remote get-url origin`을 파싱한다. None이면 GitHub repo가 아니다.
    async fn resolve_repository(
        &self,
        worktree: &std::path::Path,
    ) -> Result<Option<RepoCoords>, ForgeError>;

    /// 브랜치의 PR 상태. **`None`(PR 없음, 확정)과 `Unavailable`(조회 실패)을 구별**한다 —
    /// 일시 오류가 알려진 PR을 지우면 안 된다(§1의 Authoritative/Degraded 규율).
    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup;

    /// PR 번호로 재해석(worktree에 저장된 `linked_github_pr`용).
    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup;

    /// 생성 지원 여부(Bitbucket은 false — 조사 §1). GitHub은 항상 true.
    fn supports_review_creation(&self) -> bool;

    /// PR 생성. `gh pr create`는 `--json`이 없어[Codex B2] 출력된 URL에서 번호를 복구한다.
    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError>;
}

/// found/none/unavailable 3-상태. `Unavailable`은 **분류된** 에러를 든다[Codex S1] —
/// raw stderr는 UI에 안 닿는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewLookup {
    /// PR 있음.
    Found(Review),
    /// PR 없음(확정) — 성공 조회가 아니라 "고정 영어 stderr substring + 비-0 exit"로만 온다.
    None,
    /// 조회 실패 — **None과 구별**. 알려진 PR 상태를 지우면 안 된다.
    Unavailable(ForgeUnavailable),
}

/// 분류된 조회-불가 사유. `Other`만 메시지를 들되 그것도 정제된 것이다(raw stderr 아님).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeUnavailable {
    /// gh 미설치.
    NotInstalled,
    /// gh auth 안 됨 → UI가 "gh auth login" 안내.
    NotAuthenticated,
    /// 레이트 리밋(재시도 가능).
    RateLimited,
    /// 네트워크/연결 오류(재시도 가능).
    Network,
    /// 분류 밖. 정제된 라벨만 담는다 — 원본 stderr를 넣지 않는다.
    Other(String),
}

/// PR 요약. 7a는 CI를 passing/failing/pending 카운트로 의식적 단순화한다(플랜 §1, Codex N2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub number: u64,
    pub state: ReviewState,
    pub title: String,
    pub url: String,
    pub checks: ChecksSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Open,
    Merged,
    Closed,
    Draft,
}

/// CI 체크 요약. 7a는 3단(REST→GraphQL→`gh pr checks`) 폴백을 안 짊어지고
/// `gh pr checks --json bucket`만 쓴다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChecksSummary {
    pub passing: u32,
    pub failing: u32,
    pub pending: u32,
}

/// owner/repo(+host, GHES). gh impl이 `gh repo view`로 채운다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCoords {
    pub owner: String,
    pub repo: String,
    pub host: String,
}

impl RepoCoords {
    /// gh `--repo` 인자. github.com은 `owner/repo`, GHES는 `host/owner/repo`로
    /// 호스트를 붙인다(gh가 엔터프라이즈 호스트를 이렇게 받는다).
    pub fn repo_arg(&self) -> String {
        if self.host == "github.com" {
            format!("{}/{}", self.owner, self.repo)
        } else {
            format!("{}/{}/{}", self.host, self.owner, self.repo)
        }
    }
}

/// Orca `CreateHostedReviewInput`(`shared/hosted-review.ts:60-67`) 모양(Codex N3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateReviewInput {
    pub worktree_path: PathBuf,
    pub base: String,
    /// head 브랜치. None이면 gh가 worktree의 현재 브랜치를 쓴다.
    pub head: Option<String>,
    pub title: String,
    pub body: String,
    /// body가 비었을 때 repo PR 템플릿을 채울지.
    pub use_template: bool,
    pub draft: bool,
}

/// 생성/해석 실패. `Unavailable`은 분류 enum을 재사용한다.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ForgeError {
    #[error("forge unavailable: {0:?}")]
    Unavailable(ForgeUnavailable),
    /// 입력이 잘못됨(base==head 등). 정제된 사용자용 메시지.
    #[error("{0}")]
    Validation(String),
    /// gh 출력에서 기대한 데이터를 못 뽑음(PR 번호 등). 정제된 메시지, raw 출력 아님.
    #[error("{0}")]
    Parse(String),
}
