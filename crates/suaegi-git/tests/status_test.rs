//! M3 통합 테스트: 실제 git을 `tempdir`에서 돌려 `check_ignored`/`working_tree_status`를
//! 고정한다(모킹 금지 — `compare_test.rs`와 같은 규율). fixture는 개발자 전역 설정
//! (gpg 서명·훅 템플릿·전역 ignore)이 테스트를 오염시키지 않게 격리한다.

mod fixture;

use suaegi_git::runner::GitRunner;
use suaegi_git::status::{check_ignored, working_tree_status, FileStatus};

/// 격리된 실제 repo. `init_repo`가 `core.excludesFile=/dev/null`을 로컬 설정에 박아
/// **개발자 기계의 전역 ignore가 새어들지 않게** 한다(fixture 주석의 공허-테스트 함정).
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fixture::init_repo(dir.path());
    dir
}

// --- check_ignored: 무시된 것만 골라낸다 ---
#[tokio::test]
async fn check_ignored_returns_only_ignored_paths() {
    let dir = repo();
    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("a.log"), "x").unwrap();
    std::fs::write(dir.path().join("b.rs"), "x").unwrap();

    let r = GitRunner::new();
    let ignored = check_ignored(&r, dir.path(), &["a.log", "b.rs"])
        .await
        .unwrap();
    assert!(
        ignored.contains("a.log"),
        "a.log가 무시로 안 잡혔다: {ignored:?}"
    );
    assert!(!ignored.contains("b.rs"), "b.rs가 잘못 무시로 잡혔다");
    assert_eq!(ignored.len(), 1);
}

// --- crux(Mutant A): exit 1("무시된 것 없음")은 오류가 아니라 빈 집합 ---
// `run_with_stdin`에 넘기는 `&[1]`을 `&[]`로 바꾸면(=exit 1을 오류로) 이 테스트가
// Err를 받아 FAIL한다. 실측(git 2.50.1): 아무것도 안 걸리면 check-ignore는 exit 1.
#[tokio::test]
async fn check_ignored_nothing_matches_is_empty_not_error() {
    let dir = repo();
    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "x").unwrap();
    std::fs::write(dir.path().join("c.rs"), "x").unwrap();

    let r = GitRunner::new();
    // 둘 다 `*.log`에 안 걸린다 → git exit 1 → Ok(빈 집합)이어야 한다.
    let ignored = check_ignored(&r, dir.path(), &["b.rs", "c.rs"])
        .await
        .expect("exit 1은 오류가 아니라 '무시된 것 없음'이어야 한다");
    assert!(ignored.is_empty(), "무시된 게 없어야 하는데: {ignored:?}");
}

// --- crux(Mutant B): 진짜 오류(exit 128)는 "무시된 것 없음"으로 뭉개지 않는다 ---
// `&[1]`을 `&[1, 128]`(=128도 성공-빈결과로 수용)로 넓히면 이 테스트가 Ok(빈 집합)을
// 받아 FAIL한다. transient/fatal은 반드시 표면화해야 한다는 규율.
#[tokio::test]
async fn check_ignored_real_error_surfaces_not_empty() {
    // git repo가 **아닌** 그냥 디렉터리 → check-ignore는 "not a git repository" exit 128.
    let not_repo = tempfile::tempdir().unwrap();
    let r = GitRunner::new();
    let result = check_ignored(&r, not_repo.path(), &["a.log"]).await;
    assert!(
        result.is_err(),
        "non-repo에서의 fatal이 오류로 표면화되지 않고 {result:?}로 뭉개졌다"
    );
}

// --- check_ignored: 빈 입력은 git을 부르지 않고 빈 집합 ---
#[tokio::test]
async fn check_ignored_empty_input_is_empty() {
    let dir = repo();
    let r = GitRunner::new();
    let ignored = check_ignored(&r, dir.path(), &[]).await.unwrap();
    assert!(ignored.is_empty());
}

// --- working_tree_status: modified + untracked + renamed 를 한 맵에 ---
#[tokio::test]
async fn status_reports_modified_untracked_and_rename() {
    let dir = repo();
    // 커밋된 파일 둘: 하나는 rename, 하나는 modify.
    std::fs::write(dir.path().join("orig.txt"), "content\n").unwrap();
    std::fs::write(dir.path().join("keep.txt"), "keep\n").unwrap();
    fixture::run(dir.path(), &["add", "orig.txt", "keep.txt"]);
    fixture::run(dir.path(), &["commit", "-m", "seed"]);

    // rename(staged) → R, keep 수정(unstaged) → " M", 새 파일 → "??".
    fixture::run(dir.path(), &["mv", "orig.txt", "renamed.txt"]);
    std::fs::write(dir.path().join("keep.txt"), "keep\nmore\n").unwrap();
    std::fs::write(dir.path().join("brand.new"), "new\n").unwrap();

    let r = GitRunner::new();
    let map = working_tree_status(&r, dir.path()).await.unwrap();

    // rename: 목적지 키 + 원본 from.
    assert_eq!(
        map.get("renamed.txt"),
        Some(&FileStatus::Renamed {
            from: "orig.txt".to_string()
        }),
        "rename이 목적지 키/원본 from으로 안 잡혔다: {map:?}"
    );
    // 원본 경로는 소비되어 키가 아니어야 한다.
    assert!(
        !map.contains_key("orig.txt"),
        "rename 원본이 별도 키로 샜다: {map:?}"
    );
    assert_eq!(map.get("keep.txt"), Some(&FileStatus::Modified));
    assert_eq!(map.get("brand.new"), Some(&FileStatus::Untracked));
}

// --- working_tree_status: untracked 분류 (실 git으로 재확인) ---
// (unit test가 파서를 고정하지만 실제 git이 "??"를 내는 것도 여기서 고정한다)
#[tokio::test]
async fn status_untracked_file_is_untracked() {
    let dir = repo();
    std::fs::write(dir.path().join("fresh.txt"), "hi\n").unwrap();
    let r = GitRunner::new();
    let map = working_tree_status(&r, dir.path()).await.unwrap();
    assert_eq!(map.get("fresh.txt"), Some(&FileStatus::Untracked));
}

// --- working_tree_status: 깨끗한 트리는 빈 맵 ---
#[tokio::test]
async fn status_clean_tree_is_empty() {
    let dir = repo();
    let r = GitRunner::new();
    let map = working_tree_status(&r, dir.path()).await.unwrap();
    assert!(map.is_empty(), "깨끗한 트리인데 상태가 있다: {map:?}");
}
