mod fixture;

use suaegi_git::repo_probe::probe_repo;
use suaegi_git::runner::GitRunner;

#[tokio::test]
async fn detects_git_repo_and_head_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(probe.is_git_repo);
    assert_eq!(probe.head_branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn non_repo_reports_false() {
    let dir = tempfile::tempdir().unwrap();
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(!probe.is_git_repo);
    assert_eq!(probe.head_branch, None);
}

#[tokio::test]
async fn detached_head_reports_none_branch() {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    fixture::run(dir.path(), &["checkout", "--detach"]);
    let probe = probe_repo(&GitRunner::new(), dir.path()).await.unwrap();
    assert!(probe.is_git_repo);
    assert_eq!(probe.head_branch, None);
}
