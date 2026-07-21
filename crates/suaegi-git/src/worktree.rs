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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchDeletion {
    NotRequested,
    /// 삭제 성공 또는 이미 없음 (목표 상태 달성)
    Deleted,
    /// worktree는 제거됐지만 브랜치 삭제 실패 (예: 다른 worktree가 체크아웃 중)
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub branch_deletion: BranchDeletion,
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
async fn branch_exists(runner: &GitRunner, repo: &Path, branch: &str) -> Result<bool, GitError> {
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
    validate_user_ref(base_ref).map_err(|_| WorktreeError::InvalidBaseRef(base_ref.to_string()))?;

    let sanitized = sanitize_worktree_name(requested_name);
    let repo_dir_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let parent = workspace_root.join(&repo_dir_name);
    tokio::fs::create_dir_all(&parent).await?;
    // WorktreeId의 근간이 되는 경로이므로 항상 canonical 절대 경로로
    let parent = tokio::fs::canonicalize(&parent).await?;

    let mut chosen: Option<(String, PathBuf)> = None;
    for name in candidate_names(&sanitized) {
        let path = parent.join(&name);
        // Path::exists()와 동일하게 권한 에러 등은 "존재하지 않음"으로 취급
        // (unwrap_or(false)) — try_exists()의 Err를 그대로 전파하면 기존
        // exists() 동작(모든 에러를 false로 뭉갬)과 달라진다.
        if tokio::fs::try_exists(&path).await.unwrap_or(false)
            || branch_exists(runner, repo_path, &name).await?
        {
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
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                &branch,
                &path_str,
                base_ref,
            ],
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
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
        let _ = runner.run(repo_path, &["worktree", "prune"]).await;
        let _ = runner.run(repo_path, &["branch", "-D", &branch]).await;
        return Err(e.into());
    }

    Ok(CreatedWorktree {
        path,
        branch: branch.clone(),
        display_name: branch,
    })
}

/// `git worktree list --porcelain -z` 파싱. -z 모드는 각 속성 라인이 NUL로
/// 끝나고 엔트리 사이에 빈 NUL 레코드가 온다. 경로에 개행이 있어도 안전.
/// git 문서 보장에 따라 첫 엔트리가 main worktree다.
pub async fn list_worktrees(
    runner: &GitRunner,
    repo_path: &Path,
) -> Result<Vec<WorktreeEntry>, GitError> {
    let out = runner
        .run(repo_path, &["worktree", "list", "--porcelain", "-z"])
        .await?;
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for record in out.stdout.split('\0') {
        if record.is_empty() {
            // 엔트리 구분자
            if let Some(e) = current.take() {
                entries.push(e);
            }
            continue;
        }
        if let Some(rest) = record.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(rest),
                branch: None,
                head: None,
                is_main: entries.is_empty(),
            });
        } else if let Some(rest) = record.strip_prefix("HEAD ") {
            if let Some(e) = current.as_mut() {
                e.head = Some(rest.to_string());
            }
        } else if let Some(rest) = record.strip_prefix("branch ") {
            if let Some(e) = current.as_mut() {
                e.branch = Some(rest.trim_start_matches("refs/heads/").to_string());
            }
        }
        // detached / locked / prunable 속성은 MVP에서 미사용 — Plan 3+ (삭제 UI)에서 확장
    }
    if let Some(e) = current.take() {
        entries.push(e);
    }
    Ok(entries)
}

pub async fn remove_worktree(
    runner: &GitRunner,
    repo_path: &Path,
    worktree_path: &Path,
    force: bool,
    delete_branch: Option<&str>,
) -> Result<RemoveOutcome, WorktreeError> {
    let path_str = worktree_path.to_string_lossy().into_owned();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path_str);
    runner.run(repo_path, &args).await?;
    let branch_deletion = match delete_branch {
        None => BranchDeletion::NotRequested,
        // `-d`(안전 삭제): 커밋됐지만 아직 병합 안 된 작업이 있으면 git이
        // 거부한다. worktree가 클린해도(uncommitted 변경 없음) 그 안에서
        // 커밋한 작업은 살아 있을 수 있다 — 이 앱의 핵심 워크플로우가 바로
        // worktree 안에서 에이전트가 커밋하는 것이므로, `-D`(강제)는 그 커밋을
        // reflog로만 복구 가능한 상태로 만들 수 있다. 여기엔 강제 삭제 경로를
        // 두지 않는다 — 필요해지면 별도 파라미터로 명시적으로 받는다.
        Some(branch) => match runner.run(repo_path, &["branch", "-d", branch]).await {
            Ok(_) => BranchDeletion::Deleted,
            // "이미 없음"은 목표 상태 달성 — 실패로 보고하면 UI가 헛경고를 띄운다
            Err(GitError::Failed { ref stderr, .. }) if stderr.contains("not found") => {
                BranchDeletion::Deleted
            }
            Err(e) => BranchDeletion::Failed(e.to_string()),
        },
    };
    Ok(RemoveOutcome { branch_deletion })
}
