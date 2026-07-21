mod fixture;

use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::{
    add_worktree, list_worktrees, remove_worktree, BranchDeletion, WorktreeError,
};

#[tokio::test]
async fn creates_worktree_with_new_branch() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "Fix Bug!", "main", ws.path())
        .await
        .unwrap();
    assert_eq!(created.branch, "Fix-Bug");
    assert!(created.path.is_absolute());
    assert!(created.path.join("README.md").exists());
    let list = r
        .run(repo.path(), &["worktree", "list", "--porcelain"])
        .await
        .unwrap();
    assert!(list.stdout.contains("Fix-Bug"));
}

#[tokio::test]
async fn name_collision_gets_numeric_suffix() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let first = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    let second = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
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
    let err = add_worktree(&r, repo.path(), "fix", "no-such-ref", ws.path())
        .await
        .unwrap_err();
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
    let err = add_worktree(&r, repo.path(), "fix", "--force", ws.path())
        .await
        .unwrap_err();
    assert!(matches!(err, WorktreeError::InvalidBaseRef(_)));
}

#[tokio::test]
async fn list_includes_main_and_created_worktrees() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    let list = list_worktrees(&r, repo.path()).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main);
    assert_eq!(list[1].branch.as_deref(), Some("fix"));
    assert_eq!(
        list[1].path.canonicalize().unwrap(),
        created.path.canonicalize().unwrap()
    );
}

#[tokio::test]
async fn remove_worktree_deletes_dir_and_reports_branch_result() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    let outcome = remove_worktree(&r, repo.path(), &created.path, false, Some("fix"))
        .await
        .unwrap();
    assert_eq!(outcome.branch_deletion, BranchDeletion::Deleted);
    assert!(!created.path.exists());
    let list = list_worktrees(&r, repo.path()).await.unwrap();
    assert_eq!(list.len(), 1);
    let br = r
        .run(repo.path(), &["branch", "--list", "fix"])
        .await
        .unwrap();
    assert!(br.stdout.trim().is_empty());
}

#[tokio::test]
async fn removing_already_deleted_branch_counts_as_deleted() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    // 브랜치를 먼저 지워 "이미 없음" 상태를 만든다 (worktree가 잡고 있으므로 강제)
    fixture::run(
        repo.path(),
        &[
            "worktree",
            "remove",
            "--force",
            created.path.to_str().unwrap(),
        ],
    );
    fixture::run(repo.path(), &["branch", "-D", "fix"]);
    let second = add_worktree(&r, repo.path(), "fix2", "main", ws.path())
        .await
        .unwrap();
    let outcome = remove_worktree(&r, repo.path(), &second.path, false, Some("no-such-branch"))
        .await
        .unwrap();
    // 목표 상태(브랜치 없음)는 달성됐으므로 Deleted
    assert_eq!(outcome.branch_deletion, BranchDeletion::Deleted);
}

#[tokio::test]
async fn remove_worktree_keeps_an_unmerged_branch_and_reports_the_failure() {
    // 클린한(uncommitted 변경 없는) worktree라도 그 안에서 커밋한 작업은
    // 아직 다른 브랜치에 병합되지 않았을 수 있다 — 이 앱의 핵심 워크플로우가
    // worktree 안에서 에이전트가 커밋하는 것이므로, 이 경우 브랜치를 강제
    // 삭제하면 그 커밋들이 reflog로만 복구 가능한 상태가 된다. `-d`(안전
    // 삭제)는 git이 알아서 거부해야 하고, 그 거부가 `BranchDeletion::Failed`로
    // 드러나야 한다 — 조용히 성공한 것처럼 보이면 안 된다.
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    // worktree 안에서 커밋 — uncommitted 변경은 없다(worktree remove에
    // force가 필요 없다), 하지만 브랜치는 이제 main보다 앞서 있다(unmerged).
    std::fs::write(created.path.join("work.txt"), "unmerged work").unwrap();
    fixture::run(&created.path, &["add", "work.txt"]);
    fixture::run(&created.path, &["commit", "-m", "unmerged work"]);

    let outcome = remove_worktree(&r, repo.path(), &created.path, false, Some("fix"))
        .await
        .unwrap();

    assert!(
        matches!(outcome.branch_deletion, BranchDeletion::Failed(_)),
        "expected BranchDeletion::Failed, got {:?}",
        outcome.branch_deletion
    );
    assert!(
        !created.path.exists(),
        "the worktree checkout itself should still be removed"
    );
    let br = r
        .run(repo.path(), &["branch", "--list", "fix"])
        .await
        .unwrap();
    assert!(
        br.stdout.contains("fix"),
        "the unmerged branch must survive a refused safe-delete"
    );
}

#[tokio::test]
async fn remove_dirty_worktree_requires_force() {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "fix", "main", ws.path())
        .await
        .unwrap();
    std::fs::write(created.path.join("dirty.txt"), "x").unwrap();
    let err = remove_worktree(&r, repo.path(), &created.path, false, None).await;
    assert!(err.is_err());
    let outcome = remove_worktree(&r, repo.path(), &created.path, true, None)
        .await
        .unwrap();
    assert_eq!(outcome.branch_deletion, BranchDeletion::NotRequested);
    assert!(!created.path.exists());
}
