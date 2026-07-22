use crate::github::{preflight, Preflight};
use crate::provider::{ForgeError, ForgeProvider, ForgeUnavailable, ReviewLookup};
use crate::runner::GhRunner;
use std::path::Path;
use suaegi_git::runner::GitRunner;

/// "Create PR"을 **제안조차** 할지 판별하는 층(플랜 §3.4, Orca
/// `hosted-review-creation-eligibility`). 막혔으면 UI가 대응할 수 있게 사유를 돌려준다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationEligibility {
    Eligible,
    Blocked(CreationBlockedReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationBlockedReason {
    /// gh 미설치.
    NotInstalled,
    /// gh 미인증 → "gh auth login".
    NotAuthenticated,
    /// gh가 하한보다 낮음.
    OutdatedGh { found: String, min: String },
    /// GitHub 원격이 아님.
    NotGitHubRepo,
    /// upstream 추적 ref 없음(브랜치 push 안 됨) → UI는 "publish" 유도[Codex B3].
    NoUpstream,
    /// 이미 이 브랜치에 PR이 있음.
    AlreadyExists,
    /// 조회 실패로 자격을 확정할 수 없음(재시도 가능) — **AlreadyExists로 뭉개지 않는다**.
    Unavailable(ForgeUnavailable),
}

/// v1 최소 게이팅: gh 설치·인증 + GitHub repo + **upstream 존재** + 기존 PR 없음.
///
/// **upstream 체크가 load-bearing이다**[Codex B3]: 빼면 push 안 된 브랜치에 "Create PR"이
/// 뜨고 `gh pr create`가 tty 없이 불투명하게 실패한다. Orca도 `hasUpstream === false`를
/// `no_upstream`/`publish`로 막는다.
pub async fn creation_eligibility<P: ForgeProvider + Sync>(
    provider: &P,
    git_runner: &GitRunner,
    gh_runner: &GhRunner,
    worktree: &Path,
    branch: &str,
) -> CreationEligibility {
    use CreationBlockedReason as R;

    // 1. gh 설치/버전/인증.
    match preflight(gh_runner).await {
        Preflight::NotInstalled => return CreationEligibility::Blocked(R::NotInstalled),
        Preflight::NotAuthenticated => {
            return CreationEligibility::Blocked(R::NotAuthenticated)
        }
        Preflight::OutdatedVersion { found, min } => {
            return CreationEligibility::Blocked(R::OutdatedGh { found, min })
        }
        Preflight::Ready => {}
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

    // 3. upstream 추적 ref 존재(= 브랜치가 push됨).
    if !has_upstream(git_runner, worktree).await {
        return CreationEligibility::Blocked(R::NoUpstream);
    }

    // 4. 기존 PR 없음. Unavailable을 AlreadyExists로 뭉개지 않는다.
    match provider.review_for_branch(&repo, branch).await {
        ReviewLookup::None => CreationEligibility::Eligible,
        ReviewLookup::Found(_) => CreationEligibility::Blocked(R::AlreadyExists),
        ReviewLookup::Unavailable(u) => CreationEligibility::Blocked(R::Unavailable(u)),
    }
}

/// `git rev-parse --abbrev-ref @{u}`가 성공하면 upstream 추적 ref가 있다. upstream이
/// 없으면 git은 exit 128("no upstream configured")을 낸다. 확정 못 하면(=git 오류)
/// 보수적으로 false — push를 auth-check 없이 제안하지 않는다.
pub(crate) async fn has_upstream(git_runner: &GitRunner, worktree: &Path) -> bool {
    match git_runner
        .run_expecting(worktree, &["rev-parse", "--abbrev-ref", "@{u}"], &[128])
        .await
    {
        Ok(out) => out.code == 0,
        Err(_) => false,
    }
}
