mod fixture;

use suaegi_git::commit_show::commit_show;
use suaegi_git::compare::ChangeStatus;
use suaegi_git::runner::GitRunner;

/// 해당 리비전의 full oid를 판다(테스트 헬퍼).
fn oid(dir: &std::path::Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", dir.join(".test-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

// ─── argv-injection 가드 (crux) ───────────────────────────────────────────

/// 40/64 hex가 아닌 입력은 git을 부르기 전에 즉시 거부한다. `HEAD~1`은 특히
/// **가드가 없으면 git이 실제로 풀어버려 Ok가 나오는** 입력이라(레포에 커밋이
/// 둘 이상), 가드-off mutant를 죽인다.
#[tokio::test]
async fn argv_guard_rejects_non_full_object_ids() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    // 두 번째 커밋 — 이래야 HEAD~1이 실재하고, 가드가 없으면 git이 성공한다.
    std::fs::write(repo.path().join("README.md"), "second\n").unwrap();
    fixture::run(repo.path(), &["commit", "-am", "second"]);

    let r = GitRunner::new();
    for bad in [
        "HEAD~1",
        "-foo",
        "abc",
        "main",
        "HEAD",
        &"a".repeat(39),
        &"a".repeat(41),
        &"a".repeat(50),
        &"a".repeat(63),
        &"a".repeat(65),
        &"A".repeat(40), // 대문자
    ] {
        let res = commit_show(&r, repo.path(), bad).await;
        assert!(res.is_err(), "guard must reject {bad:?}, got {res:?}");
    }
}

/// 유효한 40-hex(SHA-1)는 가드를 통과해 실제 diff를 낸다.
#[tokio::test]
async fn valid_40_hex_is_accepted() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let head = oid(repo.path(), "HEAD");
    assert_eq!(head.len(), 40);

    let r = GitRunner::new();
    let diff = commit_show(&r, repo.path(), &head).await.unwrap();
    assert_eq!(diff.commit, head);
}

/// 유효한 64-hex(SHA-256)도 가드를 통과한다. 실제 SHA-256 레포를 만들어
/// end-to-end로 확인한다 — 가드가 40으로 굳으면 여기서 거부돼 실패한다.
#[tokio::test]
async fn valid_64_hex_sha256_is_accepted() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".test-gitconfig"), "").unwrap();
    fixture::run(
        repo.path(),
        &["init", "-b", "main", "--object-format=sha256"],
    );
    fixture::run(repo.path(), &["config", "user.email", "t@example.com"]);
    fixture::run(repo.path(), &["config", "user.name", "test"]);
    fixture::run(repo.path(), &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.path().join("f.txt"), "a\n").unwrap();
    fixture::run(repo.path(), &["add", "f.txt"]);
    fixture::run(repo.path(), &["commit", "-m", "c1"]);
    let head = oid(repo.path(), "HEAD");
    assert_eq!(head.len(), 64, "sha256 oid must be 64 hex");

    let r = GitRunner::new();
    let diff = commit_show(&r, repo.path(), &head).await.unwrap();
    // root 커밋이므로 f.txt가 Added로 나온다.
    assert!(
        diff.files.iter().any(|f| f.path == "f.txt"),
        "sha256 64-hex commit was not diffed: {:?}",
        diff.files
    );
}

// ─── root commit (crux) ────────────────────────────────────────────────────

/// 부모가 없는 **첫 커밋**은 하드코딩 empty-tree 해시가 아니라 `diff-tree --root`로
/// 전체 트리를 Added로 낸다. parent는 `None`이어야 한다. root 경로를 깨면(가짜
/// 부모 사용 등) git이 실패하거나 빈 결과가 돼 이 단언이 무너진다.
#[tokio::test]
async fn root_commit_returns_full_tree_as_added() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let root = oid(repo.path(), "HEAD"); // init 커밋 = 첫 커밋(부모 없음)

    let r = GitRunner::new();
    let diff = commit_show(&r, repo.path(), &root).await.unwrap();

    assert_eq!(diff.parent, None, "첫 커밋은 부모가 없어야 한다");
    assert!(!diff.files.is_empty(), "root 커밋은 빈 목록이 아니다");
    // init 커밋의 트리 = README.md + .gitignore, 둘 다 Added.
    let readme = diff
        .files
        .iter()
        .find(|f| f.path == "README.md")
        .expect("README.md missing from root tree");
    assert_eq!(readme.status, ChangeStatus::Added);
    assert!(
        diff.files
            .iter()
            .all(|f| matches!(f.status, ChangeStatus::Added)),
        "root 커밋의 모든 파일은 Added여야 한다: {:?}",
        diff.files
    );
}

// ─── 일반 커밋: modify + add + rename (first-parent 추출 crux) ─────────────

/// 부모 대비 수정 + 추가 + rename이 각각 올바른 상태·카운트로 나온다. first-parent
/// 추출이 틀린 필드를 집으면(예: 커밋 자신[0] → 빈 diff, 2번째 부모[2] → root 분기)
/// 이 목록이 통째로 어긋난다.
#[tokio::test]
async fn normal_commit_reports_modify_add_and_rename() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    // 부모 커밋: rename 대상이 될 파일 하나 추가.
    std::fs::write(repo.path().join("orig.txt"), "a\nb\nc\n").unwrap();
    fixture::run(repo.path(), &["add", "orig.txt"]);
    fixture::run(repo.path(), &["commit", "-m", "parent"]);

    // 대상 커밋: README 수정 + added.txt 추가 + orig.txt -> renamed.txt.
    std::fs::write(repo.path().join("README.md"), "changed\nplus\n").unwrap();
    std::fs::write(repo.path().join("added.txt"), "new\n").unwrap();
    fixture::run(repo.path(), &["mv", "orig.txt", "renamed.txt"]);
    std::fs::write(repo.path().join("renamed.txt"), "a\nb\nc\nd\n").unwrap();
    fixture::run(repo.path(), &["add", "-A"]);
    fixture::run(repo.path(), &["commit", "-m", "target"]);

    let parent = oid(repo.path(), "HEAD~1");
    let commit = oid(repo.path(), "HEAD");

    let r = GitRunner::new();
    let diff = commit_show(&r, repo.path(), &commit).await.unwrap();

    assert_eq!(
        diff.parent.as_deref(),
        Some(parent.as_str()),
        "first-parent 추출이 틀렸다"
    );

    let readme = diff.files.iter().find(|f| f.path == "README.md").unwrap();
    assert_eq!(readme.status, ChangeStatus::Modified);
    // 원본 "hello\n" → "changed\nplus\n": +2 -1 (비대칭 — 파서가 두 칸 맞바꿔도 잡힌다).
    assert_eq!(readme.additions, Some(2));
    assert_eq!(readme.deletions, Some(1));

    let added = diff.files.iter().find(|f| f.path == "added.txt").unwrap();
    assert_eq!(added.status, ChangeStatus::Added);
    assert_eq!(added.additions, Some(1));
    assert_eq!(added.deletions, Some(0));

    let renamed = diff
        .files
        .iter()
        .find(|f| f.path == "renamed.txt")
        .expect("rename missing");
    assert_eq!(
        renamed.status,
        ChangeStatus::Renamed {
            from: "orig.txt".into()
        }
    );
    // 원본 3줄 → 4줄: +1 -0. 카운트가 to 경로로 조인됐는지 고정.
    assert_eq!(renamed.additions, Some(1));
    assert_eq!(renamed.deletions, Some(0));

    // orig.txt는 rename의 source로 흡수돼 별도 항목으로 남지 않는다.
    assert!(
        !diff.files.iter().any(|f| f.path == "orig.txt"),
        "rename source가 별도 파일로 샜다: {:?}",
        diff.files
    );
}

// ─── transient 규율 ───────────────────────────────────────────────────────

/// 존재하지 않는 커밋(형태는 유효한 40-hex)은 진짜 실패다 → `Err`. 빈 결과로
/// 뭉개면 안 된다.
#[tokio::test]
async fn nonexistent_commit_is_err() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    let r = GitRunner::new();
    // 유효한 40-hex 모양이지만 이 레포에 없는 oid.
    let ghost = "0".repeat(40);
    let res = commit_show(&r, repo.path(), &ghost).await;
    assert!(
        res.is_err(),
        "nonexistent commit must surface as Err: {res:?}"
    );
}

/// 부모와 차이가 없는 **빈 커밋**은 오류가 아니라 빈 목록이다.
#[tokio::test]
async fn empty_commit_is_empty_not_error() {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    fixture::run(repo.path(), &["commit", "--allow-empty", "-m", "empty"]);
    let commit = oid(repo.path(), "HEAD");

    let r = GitRunner::new();
    let diff = commit_show(&r, repo.path(), &commit).await.unwrap();
    assert!(diff.parent.is_some(), "빈 커밋도 부모는 있다");
    assert!(
        diff.files.is_empty(),
        "빈 커밋은 빈 목록이어야 한다: {:?}",
        diff.files
    );
}
