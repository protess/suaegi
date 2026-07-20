mod fixture;

use suaegi_git::compare::{branch_compare, file_diff, working_tree_dirty, ChangeStatus};
use suaegi_git::runner::GitRunner;
use suaegi_git::worktree::add_worktree;

async fn setup() -> (tempfile::TempDir, tempfile::TempDir, std::path::PathBuf) {
    let repo = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    let created = add_worktree(&r, repo.path(), "feat", "main", ws.path())
        .await
        .unwrap();
    (repo, ws, created.path)
}

#[tokio::test]
async fn compare_reports_committed_changes() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 1);
    let mut paths: Vec<_> = cmp.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    // fixture가 만드는 .test-gitconfig는 untracked로 잡히므로 필터
    let paths: Vec<_> = paths
        .into_iter()
        .filter(|p| !p.starts_with(".test-"))
        .collect();
    assert_eq!(paths, vec!["README.md", "new.txt"]);
    let readme = cmp.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!(readme.status, ChangeStatus::Modified);
    assert_eq!(readme.additions, Some(1));
    assert_eq!(readme.deletions, Some(1));
}

#[tokio::test]
async fn compare_includes_untracked_files() {
    let (_repo, _ws, wt) = setup().await;
    // add도 commit도 하지 않은 새 파일 — 에이전트 작업 중 가장 흔한 상태
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();

    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 0);
    let wip = cmp
        .files
        .iter()
        .find(|f| f.path == "wip.txt")
        .expect("untracked file missing");
    assert_eq!(wip.status, ChangeStatus::Added);
}

#[tokio::test]
async fn file_diff_returns_unified_patch() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("README.md"), "changed\n").unwrap();
    fixture::run(&wt, &["add", "."]);
    fixture::run(&wt, &["commit", "-m", "change"]);

    let r = GitRunner::new();
    let patch = file_diff(&r, &wt, "main", "README.md").await.unwrap();
    assert!(patch.contains("-hello"));
    assert!(patch.contains("+changed"));
}

#[tokio::test]
async fn file_diff_synthesizes_patch_for_untracked() {
    let (_repo, _ws, wt) = setup().await;
    std::fs::write(wt.join("wip.txt"), "wip\n").unwrap();
    let r = GitRunner::new();
    let patch = file_diff(&r, &wt, "main", "wip.txt").await.unwrap();
    assert!(patch.contains("+wip"));
}

#[tokio::test]
async fn no_changes_yields_no_tracked_diffs() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    assert_eq!(cmp.ahead_count, 0);
    // fixture 부산물(.test-gitconfig 등) 외에는 없어야 한다
    assert!(cmp
        .files
        .iter()
        .all(|f| f.path.starts_with(".test-") || f.path.starts_with(".no-hooks")));
}

#[tokio::test]
async fn compare_reports_renamed_files() {
    let (_repo, _ws, wt) = setup().await;
    // 내용 변경 없이 순수 rename만 수행 — 유사도 100%로 R100 감지를 보장한다.
    fixture::run(&wt, &["mv", "README.md", "renamed.md"]);
    fixture::run(&wt, &["add", "-A"]);
    fixture::run(&wt, &["commit", "-m", "rename"]);

    let r = GitRunner::new();
    let cmp = branch_compare(&r, &wt, "main").await.unwrap();
    let renamed = cmp
        .files
        .iter()
        .find(|f| f.path == "renamed.md")
        .expect("renamed file missing");
    assert_eq!(
        renamed.status,
        ChangeStatus::Renamed {
            from: "README.md".into()
        }
    );
    assert!(renamed.additions.is_some());
    assert!(renamed.deletions.is_some());
}

#[tokio::test]
async fn dirty_detection() {
    let (_repo, _ws, wt) = setup().await;
    let r = GitRunner::new();
    // fixture 부산물이 untracked로 존재하므로 이 테스트는 tracked 변경으로 판별
    std::fs::write(wt.join("README.md"), "dirty\n").unwrap();
    assert!(working_tree_dirty(&r, &wt).await.unwrap());
}
