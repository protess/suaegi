//! Provider 라우팅: worktree의 `origin` 원격을 보고 GitHub·GitLab을, 그리고 GitHub 안에서
//! gh CLI(`GhForge`)와 HTTP(`HttpGhForge`)를 고른다. 셋 다 `ForgeProvider`+`PrActions`를
//! 구현하므로 [`AnyForge`]가 그 위에 얇은 dispatch enum으로 앉아, 앱(`forge_tasks`)이
//! provider 종류를 몰라도 되게 한다.
//!
//! **GitHub 백엔드 선택**(7a-2b): gh가 준비되면 gh(주력), gh가 없거나 미인증이지만 토큰이
//! 저장돼 있으면 HTTP로 폴백, 둘 다 아니면 gh(기존 NotInstalled/NotAuthenticated 표면).
//! 결정 규율은 순수 함수 [`choose_github_backend`]에 있어 테스트 가능하다 — 여기선 실제
//! probe(preflight·시크릿 로드)만 엮는다. 임의 self-hosted 호스트 인식·다중-provider UI는 후속.

use crate::github_http::{
    choose_github_backend, GithubBackend, HttpGhForge, KEYCHAIN_ACCOUNT, KEYCHAIN_SERVICE,
};
use crate::gitlab::{parse::parse_gitlab_remote, GlabForge};
use crate::pr_actions::{
    CommentLookup, MergeMethod, MergeOptions, MergeOutcome, MergeabilityState, PrActions,
    ReviewThreadLookup,
};
use crate::provider::{
    CreateReviewInput, ForgeError, ForgeProvider, RepoCoords, Review, ReviewLookup,
};
use crate::runner::GhRunner;
use crate::{preflight, GhForge, Preflight};
use async_trait::async_trait;
use std::path::Path;
use suaegi_git::runner::GitRunner;
use suaegi_secrets::SecretRequest;

/// 선택된 forge provider. 어느 쪽이든 같은 두 트레잇을 만족한다.
#[derive(Debug, Clone)]
pub enum AnyForge {
    /// gh CLI GitHub 백엔드.
    Github(GhForge),
    /// HTTP(토큰) GitHub 백엔드.
    GithubHttp(HttpGhForge),
    /// glab CLI GitLab 백엔드.
    Gitlab(GlabForge),
}

impl AnyForge {
    /// worktree의 `origin` 원격을 읽어 provider를 고른다. GitLab 호스트면 GitLab, 그 밖은
    /// GitHub — GitHub 안에서 gh vs HTTP를 [`choose_github_backend`]로 가른다.
    pub async fn select(worktree: &Path) -> AnyForge {
        let git = GitRunner::new();
        if let Ok(out) = git.run(worktree, &["remote", "get-url", "origin"]).await {
            if parse_gitlab_remote(out.stdout.trim()).is_some() {
                return AnyForge::Gitlab(GlabForge::new());
            }
        }
        Self::select_github().await
    }

    /// GitHub 백엔드 선택. gh preflight로 gh 준비 여부를, 그다음(필요할 때만) 시크릿에서
    /// 토큰 존재 여부를 얻어 순수 결정 함수에 넘긴다. gh가 준비된 흔한 경우엔 키체인을 안 친다.
    async fn select_github() -> AnyForge {
        let gh_ready = matches!(preflight(&GhRunner::new()).await, Preflight::Ready);
        // gh가 준비됐으면 토큰을 볼 필요가 없다(키체인 I/O 절약). 아니면 시크릿을 로드한다.
        let token = if gh_ready {
            None
        } else {
            suaegi_secrets::load(&SecretRequest::github(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)).secret
        };
        match choose_github_backend(gh_ready, token.is_some()) {
            GithubBackend::Gh => AnyForge::Github(GhForge::new()),
            GithubBackend::Http => AnyForge::GithubHttp(HttpGhForge::new(token)),
        }
    }

    /// 이 provider가 GitLab인지(앱이 eligibility 경로를 provider별로 가를 때 쓴다).
    pub fn is_gitlab(&self) -> bool {
        matches!(self, AnyForge::Gitlab(_))
    }
}

#[async_trait]
impl ForgeProvider for AnyForge {
    async fn resolve_repository(
        &self,
        worktree: &Path,
    ) -> Result<Option<RepoCoords>, ForgeError> {
        match self {
            AnyForge::Github(f) => f.resolve_repository(worktree).await,
            AnyForge::GithubHttp(f) => f.resolve_repository(worktree).await,
            AnyForge::Gitlab(f) => f.resolve_repository(worktree).await,
        }
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        match self {
            AnyForge::Github(f) => f.review_for_branch(repo, branch).await,
            AnyForge::GithubHttp(f) => f.review_for_branch(repo, branch).await,
            AnyForge::Gitlab(f) => f.review_for_branch(repo, branch).await,
        }
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        match self {
            AnyForge::Github(f) => f.review_by_number(repo, number).await,
            AnyForge::GithubHttp(f) => f.review_by_number(repo, number).await,
            AnyForge::Gitlab(f) => f.review_by_number(repo, number).await,
        }
    }

    fn supports_review_creation(&self) -> bool {
        match self {
            AnyForge::Github(f) => f.supports_review_creation(),
            AnyForge::GithubHttp(f) => f.supports_review_creation(),
            AnyForge::Gitlab(f) => f.supports_review_creation(),
        }
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
        match self {
            AnyForge::Github(f) => f.create_review(input).await,
            AnyForge::GithubHttp(f) => f.create_review(input).await,
            AnyForge::Gitlab(f) => f.create_review(input).await,
        }
    }
}

#[async_trait]
impl PrActions for AnyForge {
    async fn merge_pr(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
        options: MergeOptions,
    ) -> Result<MergeOutcome, ForgeError> {
        match self {
            AnyForge::Github(f) => f.merge_pr(repo, number, method, options).await,
            AnyForge::GithubHttp(f) => f.merge_pr(repo, number, method, options).await,
            AnyForge::Gitlab(f) => f.merge_pr(repo, number, method, options).await,
        }
    }

    async fn set_auto_merge(
        &self,
        repo: &RepoCoords,
        number: u64,
        method: MergeMethod,
    ) -> Result<(), ForgeError> {
        match self {
            AnyForge::Github(f) => f.set_auto_merge(repo, number, method).await,
            AnyForge::GithubHttp(f) => f.set_auto_merge(repo, number, method).await,
            AnyForge::Gitlab(f) => f.set_auto_merge(repo, number, method).await,
        }
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        match self {
            AnyForge::Github(f) => f.pr_reviews(repo, number).await,
            AnyForge::GithubHttp(f) => f.pr_reviews(repo, number).await,
            AnyForge::Gitlab(f) => f.pr_reviews(repo, number).await,
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        match self {
            AnyForge::Github(f) => f.pr_comments(repo, number).await,
            AnyForge::GithubHttp(f) => f.pr_comments(repo, number).await,
            AnyForge::Gitlab(f) => f.pr_comments(repo, number).await,
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        match self {
            AnyForge::Github(f) => f.mergeability_state(repo, number).await,
            AnyForge::GithubHttp(f) => f.mergeability_state(repo, number).await,
            AnyForge::Gitlab(f) => f.mergeability_state(repo, number).await,
        }
    }
}
