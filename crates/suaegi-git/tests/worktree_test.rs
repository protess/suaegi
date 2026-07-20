mod fixture;

use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::{add_worktree, WorktreeError};

#[tokio::test]
async fn creates_worktree_with_new_branch() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "Fix Bug!", "main", ws.path()).await.unwrap();
    assert_eq!(created.branch, "Fix-Bug");
    assert!(created.path.is_absolute());
    assert!(created.path.join("README.md").exists());
    let list = r.run(repo.path(), &["worktree", "list", "--porcelain"]).await.unwrap();
    assert!(list.stdout.contains("Fix-Bug"));
}

#[tokio::test]
async fn name_collision_gets_numeric_suffix() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let first = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    let second = add_worktree(&r, repo.path(), "fix", "main", ws.path()).await.unwrap();
    assert_eq!(first.branch, "fix");
    assert_eq!(second.branch, "fix-2");
    assert_ne!(first.path, second.path);
}

#[tokio::test]
async fn bad_base_ref_fails_without_leftover_directory() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let err = add_worktree(&r, repo.path(), "fix", "no-such-ref", ws.path()).await.unwrap_err();
    assert!(matches!(err, WorktreeError::Git(_)));
    // 롤백: workspace_root 아래에 잔여 디렉토리가 없어야 한다
    let repo_dir = ws.path().join(repo.path().file_name().unwrap());
    let leftover = std::fs::read_dir(&repo_dir).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0);
}

#[tokio::test]
async fn option_like_base_ref_is_rejected() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let err = add_worktree(&r, repo.path(), "fix", "--force", ws.path()).await.unwrap_err();
    assert!(matches!(err, WorktreeError::InvalidBaseRef(_)));
}
