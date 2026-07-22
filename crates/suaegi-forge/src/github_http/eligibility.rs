//! HTTP 백엔드용 "Create PR" 자격 게이팅. gh 경로의 [`crate::eligibility::creation_eligibility`]
//! 미러이되, **gh preflight 대신 토큰 존재**를 준비 신호로 본다(HTTP 백엔드는 gh가 없을 때
//! 선택되므로 gh preflight를 물을 수 없다). 나머지 단계(GitHub repo·upstream·기존 PR 없음)는
//! 동일하다.

use crate::eligibility::{has_upstream, CreationBlockedReason, CreationEligibility};
use crate::github_http::HttpGhForge;
use crate::provider::{ForgeError, ForgeProvider, ForgeUnavailable, ReviewLookup};
use std::path::Path;
use suaegi_git::runner::GitRunner;

/// v1 최소 게이팅(HTTP): 토큰 존재 + GitHub repo + upstream 존재 + 기존 PR 없음.
///
/// gh 버전의 4단계를 그대로 따르되 1단계만 다르다: gh 설치/버전/인증 preflight 대신 저장된
/// 토큰이 있는지 본다. 토큰이 없으면(선택은 됐지만 이후 사라진 경우 등) `NotAuthenticated`.
pub async fn http_creation_eligibility(
    provider: &HttpGhForge,
    git_runner: &GitRunner,
    worktree: &Path,
    branch: &str,
) -> CreationEligibility {
    use CreationBlockedReason as R;

    // 1. 토큰(HTTP "preflight"). 없으면 gh 경로의 NotAuthenticated 표면과 동일.
    if !provider.is_authenticated() {
        return CreationEligibility::Blocked(R::NotAuthenticated);
    }

    // 2. GitHub repo인가.
    let repo = match provider.resolve_repository(worktree).await {
        Ok(Some(repo)) => repo,
        Ok(None) => return CreationEligibility::Blocked(R::NotGitHubRepo),
        Err(ForgeError::Unavailable(u)) => return CreationEligibility::Blocked(R::Unavailable(u)),
        Err(_) => {
            return CreationEligibility::Blocked(R::Unavailable(ForgeUnavailable::Other(
                "GitHub is unavailable".to_string(),
            )))
        }
    };

    // 3. upstream 추적 ref(= 브랜치가 push됨). gh 경로와 공유하는 판정.
    if !has_upstream(git_runner, worktree).await {
        return CreationEligibility::Blocked(R::NoUpstream);
    }

    // 4. 기존 PR 없음. Unavailable을 AlreadyExists로 뭉개지 않는다(일시 실패 규율).
    match provider.review_for_branch(&repo, branch).await {
        ReviewLookup::None => CreationEligibility::Eligible,
        ReviewLookup::Found(_) => CreationEligibility::Blocked(R::AlreadyExists),
        ReviewLookup::Unavailable(u) => CreationEligibility::Blocked(R::Unavailable(u)),
    }
}
