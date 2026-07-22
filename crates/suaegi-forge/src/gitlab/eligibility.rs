use super::forge::{glab_preflight, GlabForge, GlabPreflight};
use super::runner::GlabRunner;
use crate::eligibility::{CreationBlockedReason, CreationEligibility};
use crate::provider::{ForgeError, ForgeProvider, ForgeUnavailable, ReviewLookup};
use std::path::Path;
use suaegi_git::runner::GitRunner;

/// GitLab판 Create-MR 자격 게이팅. `eligibility.rs::creation_eligibility`(gh)의
/// near-mechanical 미러다 — glab 설치·인증 + GitLab repo + **upstream 존재** + 기존 MR 없음.
/// gh 버전과 같은 `CreationEligibility`/`CreationBlockedReason`을 재사용한다(변형 없음).
///
/// **upstream 체크가 load-bearing이다**(gh와 동일): 빼면 push 안 된 브랜치에 "Create MR"이
/// 뜨고 `glab mr create`가 tty 없이 불투명하게 실패한다.
pub async fn glab_creation_eligibility(
    provider: &GlabForge,
    git_runner: &GitRunner,
    glab_runner: &GlabRunner,
    worktree: &Path,
    branch: &str,
) -> CreationEligibility {
    use CreationBlockedReason as R;

    // 1. glab 설치/버전/인증.
    match glab_preflight(glab_runner).await {
        GlabPreflight::NotInstalled => return CreationEligibility::Blocked(R::NotInstalled),
        GlabPreflight::NotAuthenticated => {
            return CreationEligibility::Blocked(R::NotAuthenticated)
        }
        GlabPreflight::OutdatedVersion { found, min } => {
            return CreationEligibility::Blocked(R::OutdatedGh { found, min })
        }
        GlabPreflight::Ready => {}
    }

    // 2. GitLab repo인가.
    let repo = match provider.resolve_repository(worktree).await {
        Ok(Some(repo)) => repo,
        Ok(None) => return CreationEligibility::Blocked(R::NotGitHubRepo),
        Err(ForgeError::Unavailable(u)) => return CreationEligibility::Blocked(R::Unavailable(u)),
        Err(_) => {
            return CreationEligibility::Blocked(R::Unavailable(ForgeUnavailable::Other(
                "GitLab is unavailable".to_string(),
            )))
        }
    };

    // 3. upstream 추적 ref 존재(= 브랜치가 push됨).
    if !has_upstream(git_runner, worktree).await {
        return CreationEligibility::Blocked(R::NoUpstream);
    }

    // 4. 기존 MR 없음. Unavailable을 AlreadyExists로 뭉개지 않는다.
    match provider.review_for_branch(&repo, branch).await {
        ReviewLookup::None => CreationEligibility::Eligible,
        ReviewLookup::Found(_) => CreationEligibility::Blocked(R::AlreadyExists),
        ReviewLookup::Unavailable(u) => CreationEligibility::Blocked(R::Unavailable(u)),
    }
}

/// `git rev-parse --abbrev-ref @{u}`가 성공하면 upstream 추적 ref가 있다(gh eligibility 미러).
async fn has_upstream(git_runner: &GitRunner, worktree: &Path) -> bool {
    match git_runner
        .run_expecting(worktree, &["rev-parse", "--abbrev-ref", "@{u}"], &[128])
        .await
    {
        Ok(out) => out.code == 0,
        Err(_) => false,
    }
}
