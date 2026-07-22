//! Plan 7a-1: forge(gh) shell-out을 **UI 스레드 밖에서** 돌리고 결과를 `Message`로
//! 태워 돌려주는 얇은 배선. `git_tasks.rs`와 **같은 패턴**이다(spawn → Task::perform
//! → Message). gh 호출은 async/blocking이라 절대 `update` 루프에서 직접 부르지 않는다.
//!
//! 위젯/리듀서가 검사하는 순수 결정은 `forge_ui.rs`에 있고, 여기 있는 것은 실제로
//! gh를 때리는 (그래서 헤드리스로 단언 불가능한) 접착제뿐이라 최대한 얇게 둔다.

use std::path::{Path, PathBuf};

use iced::Task;
use suaegi_core::domain::WorktreeId;
use suaegi_forge::{
    creation_eligibility, CreateReviewInput, CreationBlockedReason, CreationEligibility, ForgeError,
    ForgeProvider, ForgeUnavailable, GhForge, GhRunner, Review, ReviewLookup,
};
use suaegi_git::runner::GitRunner;

use crate::forge_ui::{create_error_text, GithubFetch};
use crate::state::{Message, OpId};

// ---- `*_now`: 실제 gh 작업. iced::Task 없이 직접 테스트 가능(하지만 gh를 때리므로
//      단위 테스트로는 안 돌린다 — 검사 대상 로직은 `forge_ui`에 있다). ----

/// worktree의 PR 상태 + Create-PR 자격을 **한 번의 활성화**에 함께 조회한다.
/// 백그라운드 폴링이 아니라 on-activate 1회 + 수동 새로고침으로만 불린다.
pub async fn fetch_status_now(
    worktree_path: PathBuf,
    branch: Option<String>,
    linked_pr: Option<u64>,
) -> (GithubFetch, CreationEligibility) {
    let provider = GhForge::new();

    // 1. PR 상태.
    let fetch = fetch_only(&provider, &worktree_path, branch.as_deref(), linked_pr).await;

    // 2. Create-PR 자격. NotGitHub/detached는 추가 gh 호출 없이 단락시킨다.
    let eligibility = match (&fetch, branch.as_deref()) {
        (GithubFetch::NotGitHub, _) => {
            CreationEligibility::Blocked(CreationBlockedReason::NotGitHubRepo)
        }
        // 브랜치가 없으면(detached HEAD) upstream을 논할 수 없다 — push 유도로 막는다.
        (_, None) => CreationEligibility::Blocked(CreationBlockedReason::NoUpstream),
        (_, Some(branch)) => {
            let git_runner = GitRunner::new();
            let gh_runner = GhRunner::new();
            creation_eligibility(&provider, &git_runner, &gh_runner, &worktree_path, branch).await
        }
    };

    (fetch, eligibility)
}

/// resolve_repository → review 조회. `linked_pr`가 있으면 번호로(상태가 안정적),
/// 없으면 브랜치로 조회한다. 번호로 조회했는데 PR이 사라졌으면 브랜치로 폴백한다.
async fn fetch_only(
    provider: &GhForge,
    worktree_path: &Path,
    branch: Option<&str>,
    linked_pr: Option<u64>,
) -> GithubFetch {
    let coords = match provider.resolve_repository(worktree_path).await {
        Ok(Some(coords)) => coords,
        Ok(None) => return GithubFetch::NotGitHub,
        Err(ForgeError::Unavailable(u)) => return GithubFetch::Unavailable(u),
        Err(_) => {
            return GithubFetch::Unavailable(ForgeUnavailable::Other(
                "GitHub is unavailable".to_string(),
            ))
        }
    };

    let lookup = match linked_pr {
        Some(number) => match provider.review_by_number(&coords, number).await {
            // 저장된 PR이 사라졌고 브랜치가 있으면 브랜치로 다시 본다.
            ReviewLookup::None => match branch {
                Some(branch) => provider.review_for_branch(&coords, branch).await,
                None => ReviewLookup::None,
            },
            other => other,
        },
        None => match branch {
            Some(branch) => provider.review_for_branch(&coords, branch).await,
            None => {
                return GithubFetch::Unavailable(ForgeUnavailable::Other(
                    "worktree has no branch (detached HEAD)".to_string(),
                ))
            }
        },
    };

    GithubFetch::Resolved(lookup)
}

/// PR 생성. 에러는 여기서 **분류된 문구**로 접는다(raw stderr는 UI에 안 닿는다).
pub async fn create_pr_now(input: CreateReviewInput) -> Result<Review, String> {
    let provider = GhForge::new();
    provider.create_review(input).await.map_err(create_error_text)
}

// ---- 얇은 Task<Message> 래퍼: 검사 불가능한 접착제, 최대한 작게. ----

/// worktree 활성화(또는 수동 새로고침) 시 PR 상태+자격 조회를 발급한다.
pub fn fetch_status(
    op: OpId,
    worktree: WorktreeId,
    worktree_path: PathBuf,
    branch: Option<String>,
    linked_pr: Option<u64>,
) -> Task<Message> {
    Task::perform(
        fetch_status_now(worktree_path, branch, linked_pr),
        move |(fetch, eligibility)| Message::GithubStatusFetched {
            worktree: worktree.clone(),
            op,
            fetch,
            eligibility,
        },
    )
}

/// Create-PR 다이얼로그 제출.
pub fn create_pr(op: OpId, worktree: WorktreeId, input: CreateReviewInput) -> Task<Message> {
    Task::perform(create_pr_now(input), move |result| {
        Message::CreatePrCreated {
            worktree: worktree.clone(),
            op,
            result,
        }
    })
}
