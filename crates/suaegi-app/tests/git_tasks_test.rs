mod fixture;

use suaegi_app::git_tasks::{
    build_repo_now, create_worktree_now, list_worktrees_now, probe_repo_now, remove_worktree_now,
};

#[tokio::test]
async fn probe_rejects_a_non_repo_with_a_readable_error() {
    let dir = tempfile::tempdir().unwrap();
    let repo = build_repo_now(dir.path().to_path_buf()).unwrap();
    let err = probe_repo_now(repo).await.unwrap_err();
    assert!(!err.is_empty());
}

#[tokio::test]
async fn probe_keeps_the_detected_head_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    let repo = build_repo_now(dir.path().to_path_buf()).unwrap();
    let (repo, head) = probe_repo_now(repo).await.unwrap();
    assert_eq!(head.as_deref(), Some("main"), "the head branch must not be dropped");
    assert!(repo.path.is_absolute());
}

#[tokio::test]
async fn create_then_list_contains_exactly_the_new_worktree() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();

    let created = create_worktree_now(
        repo.clone(), "feature one".into(), "main".into(), ws.path().to_path_buf(),
    ).await.unwrap();
    assert_eq!(created.branch, "feature-one");

    // len()만 보면 main worktree가 두 번 나와도 통과한다 — 경로와 브랜치로 확인한다
    let list = list_worktrees_now(repo).await.unwrap();
    let matched: Vec<_> = list.iter().filter(|e| {
        e.branch.as_deref() == Some(created.branch.as_str())
            && e.path.canonicalize().ok() == created.path.canonicalize().ok()
    }).collect();
    assert_eq!(matched.len(), 1, "exactly one entry must be the worktree we created");
}

#[tokio::test]
async fn remove_takes_the_worktree_out_of_the_listing() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();

    let created = create_worktree_now(
        repo.clone(), "doomed".into(), "main".into(), ws.path().to_path_buf(),
    ).await.unwrap();
    remove_worktree_now(repo.clone(), created.path.clone(), false, Some(created.branch.clone()))
        .await
        .unwrap();

    let list = list_worktrees_now(repo).await.unwrap();
    assert!(
        !list.iter().any(|e| e.branch.as_deref() == Some(created.branch.as_str())),
        "the removed worktree must be gone from the listing"
    );
    assert!(!created.path.exists());
}

#[tokio::test]
async fn a_bad_base_ref_surfaces_as_an_error_not_a_panic() {
    let repo_dir = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo_dir.path());
    let repo = build_repo_now(repo_dir.path().to_path_buf()).unwrap();
    let (repo, _) = probe_repo_now(repo).await.unwrap();
    let err = create_worktree_now(repo, "x".into(), "no-such-ref".into(), ws.path().to_path_buf())
        .await.unwrap_err();
    assert!(!err.is_empty());
}
