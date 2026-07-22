//! Provider 라우팅: worktree의 `origin` 원격을 보고 GitHub(`GhForge`)와 GitLab(`GlabForge`)
//! 중 하나를 고른다. 둘 다 `ForgeProvider`+`PrActions`를 구현하므로 [`AnyForge`]가 그 위에
//! 얇은 dispatch enum으로 앉아, 앱(`forge_tasks`)이 provider 종류를 몰라도 되게 한다.
//!
//! 7c의 **최소 배선**이다 — remote가 GitLab 호스트면 GlabForge, 아니면(GitHub 포함) GhForge.
//! 임의 self-hosted GitLab 호스트명 인식이나 다중-provider UI 선택은 후속이다.

use crate::gitlab::{parse::parse_gitlab_remote, GlabForge};
use crate::pr_actions::{
    CommentLookup, MergeMethod, MergeOptions, MergeOutcome, MergeabilityState, PrActions,
    ReviewThreadLookup,
};
use crate::provider::{
    CreateReviewInput, ForgeError, ForgeProvider, RepoCoords, Review, ReviewLookup,
};
use crate::GhForge;
use async_trait::async_trait;
use std::path::Path;
use suaegi_git::runner::GitRunner;

/// 선택된 forge provider. 어느 쪽이든 같은 두 트레잇을 만족한다.
#[derive(Debug, Clone)]
pub enum AnyForge {
    Github(GhForge),
    Gitlab(GlabForge),
}

impl AnyForge {
    /// worktree의 `origin` 원격을 읽어 provider를 고른다. 원격이 GitLab 호스트면 GitLab,
    /// 그 밖(GitHub·해석 실패·원격 없음)은 GitHub으로 폴백한다 — GhForge가 자체적으로
    /// "GitHub 아님"을 None으로 처리하므로 기본값으로 안전하다.
    pub async fn select(worktree: &Path) -> AnyForge {
        let git = GitRunner::new();
        if let Ok(out) = git.run(worktree, &["remote", "get-url", "origin"]).await {
            if parse_gitlab_remote(out.stdout.trim()).is_some() {
                return AnyForge::Gitlab(GlabForge::new());
            }
        }
        AnyForge::Github(GhForge::new())
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
            AnyForge::Gitlab(f) => f.resolve_repository(worktree).await,
        }
    }

    async fn review_for_branch(&self, repo: &RepoCoords, branch: &str) -> ReviewLookup {
        match self {
            AnyForge::Github(f) => f.review_for_branch(repo, branch).await,
            AnyForge::Gitlab(f) => f.review_for_branch(repo, branch).await,
        }
    }

    async fn review_by_number(&self, repo: &RepoCoords, number: u64) -> ReviewLookup {
        match self {
            AnyForge::Github(f) => f.review_by_number(repo, number).await,
            AnyForge::Gitlab(f) => f.review_by_number(repo, number).await,
        }
    }

    fn supports_review_creation(&self) -> bool {
        match self {
            AnyForge::Github(f) => f.supports_review_creation(),
            AnyForge::Gitlab(f) => f.supports_review_creation(),
        }
    }

    async fn create_review(&self, input: CreateReviewInput) -> Result<Review, ForgeError> {
        match self {
            AnyForge::Github(f) => f.create_review(input).await,
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
            AnyForge::Gitlab(f) => f.set_auto_merge(repo, number, method).await,
        }
    }

    async fn pr_reviews(&self, repo: &RepoCoords, number: u64) -> ReviewThreadLookup {
        match self {
            AnyForge::Github(f) => f.pr_reviews(repo, number).await,
            AnyForge::Gitlab(f) => f.pr_reviews(repo, number).await,
        }
    }

    async fn pr_comments(&self, repo: &RepoCoords, number: u64) -> CommentLookup {
        match self {
            AnyForge::Github(f) => f.pr_comments(repo, number).await,
            AnyForge::Gitlab(f) => f.pr_comments(repo, number).await,
        }
    }

    async fn mergeability_state(&self, repo: &RepoCoords, number: u64) -> MergeabilityState {
        match self {
            AnyForge::Github(f) => f.mergeability_state(repo, number).await,
            AnyForge::Gitlab(f) => f.mergeability_state(repo, number).await,
        }
    }
}
