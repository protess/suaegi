use crate::refname::validate_user_ref;
use crate::runner::{GitError, GitRunner};
use crate::worktree_name::{candidate_names, sanitize_worktree_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// OneDrive placeholder 등으로 checkout이 스톨할 수 있어 넉넉히 (Orca 차용).
const WORKTREE_ADD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct CreatedWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub display_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("no available worktree name after 100 attempts")]
    NoAvailableName,
    #[error("invalid base ref: {0}")]
    InvalidBaseRef(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// rev-parse exit 1만 "브랜치 없음". 타임아웃/스폰 실패/기타 에러를 "없음"으로
/// 오독하면 잘못된 전제로 생성이 진행되므로 전파한다.
async fn branch_exists(
    runner: &GitRunner,
    repo: &Path,
    branch: &str,
) -> Result<bool, GitError> {
    let refname = format!("refs/heads/{branch}");
    match runner
        .run(repo, &["rev-parse", "--verify", "--quiet", &refname])
        .await
    {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(e) => Err(e),
    }
}

pub async fn add_worktree(
    runner: &GitRunner,
    repo_path: &Path,
    requested_name: &str,
    base_ref: &str,
    workspace_root: &Path,
) -> Result<CreatedWorktree, WorktreeError> {
    validate_user_ref(base_ref)
        .map_err(|_| WorktreeError::InvalidBaseRef(base_ref.to_string()))?;

    let sanitized = sanitize_worktree_name(requested_name);
    let repo_dir_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = workspace_root.join(&repo_dir_name);
    std::fs::create_dir_all(&parent)?;
    // WorktreeId의 근간이 되는 경로이므로 항상 canonical 절대 경로로
    let parent = parent.canonicalize()?;

    let mut chosen: Option<(String, PathBuf)> = None;
    for name in candidate_names(&sanitized) {
        let path = parent.join(&name);
        if path.exists() || branch_exists(runner, repo_path, &name).await? {
            continue;
        }
        chosen = Some((name, path));
        break;
    }
    let (branch, path) = chosen.ok_or(WorktreeError::NoAvailableName)?;

    // --no-track: base가 remote ref일 때 미푸시 브랜치가 "behind"로 오보되는 것 방지 (Orca 차용)
    let path_str = path.to_string_lossy().into_owned();
    let result = runner
        .run_with_timeout(
            repo_path,
            &["worktree", "add", "--no-track", "-b", &branch, &path_str, base_ref],
            WORKTREE_ADD_TIMEOUT,
        )
        .await;

    if let Err(e) = result {
        // 롤백. 브랜치/경로 부재는 위에서 확인했으므로(단일 인스턴스 가정 하에)
        // 여기 있는 생성물은 이번 호출의 부산물이다. 롤백 자체의 실패는 원인
        // 에러를 가리지 않기 위해 무시한다 — 잔여물은 다음 `worktree prune`이 정리.
        let _ = runner
            .run(repo_path, &["worktree", "remove", "--force", &path_str])
            .await;
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        let _ = runner.run(repo_path, &["worktree", "prune"]).await;
        let _ = runner.run(repo_path, &["branch", "-D", &branch]).await;
        return Err(e.into());
    }

    Ok(CreatedWorktree { path, branch: branch.clone(), display_name: branch })
}
