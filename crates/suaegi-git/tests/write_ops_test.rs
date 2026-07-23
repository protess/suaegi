//! M1 스테이징 write-ops의 real-git 통합 테스트. 격리된 tempdir repo(`fixture::init_repo`)
//! 위에서 실제 git으로 왕복시키고, `:(literal)`이 glob 파일명을 보호하는지, bulk의
//! per-path 결과 벡터가 청크 원자성/청크 경계를 지키는지 실측으로 못 박는다.

mod fixture;

use std::collections::HashSet;
use std::path::Path;
use suaegi_git::runner::GitRunner;
use suaegi_git::write_ops::{bulk_stage, bulk_unstage, stage, unstage};

/// 격리된 repo tempdir + `GitRunner`.
fn setup() -> (tempfile::TempDir, GitRunner) {
    let repo = tempfile::tempdir().unwrap();
    fixture::init_repo(repo.path());
    (repo, GitRunner::new())
}

/// 현재 스테이징된 경로 집합 — `git diff --cached --name-only`. init 커밋 이후라
/// HEAD 대비 인덱스 차이가 곧 "스테이징된 것"이다. 파일명에 개행이 없으므로 줄 분할로 충분.
async fn staged(r: &GitRunner, wt: &Path) -> HashSet<String> {
    let out = r
        .run(wt, &["diff", "--cached", "--name-only"])
        .await
        .unwrap();
    out.stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

// --- crux: stage/unstage real-git 왕복 (mutant: add ↔ restore --staged 뒤바꿈) ---
// 미추적 파일을 stage → --cached에 뜬다; unstage → 사라진다. stage가 `restore --staged`로
// 바뀌면 미추적 파일은 인덱스/HEAD에 없어 아무 일도 안 일어나 --cached가 비어 FAIL.
#[tokio::test]
async fn stage_then_unstage_round_trip() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("f.txt"), "hi\n").unwrap();

    assert!(
        !staged(&r, wt).await.contains("f.txt"),
        "사전 조건: 아직 스테이징 안 됨"
    );

    stage(&r, wt, "f.txt").await.unwrap();
    assert!(
        staged(&r, wt).await.contains("f.txt"),
        "stage 후 --cached에 f.txt가 있어야 한다"
    );

    unstage(&r, wt, "f.txt").await.unwrap();
    assert!(
        !staged(&r, wt).await.contains("f.txt"),
        "unstage 후 f.txt가 사라져야 한다"
    );
}

// --- crux: :(literal)이 glob 파일명을 보호한다 (mutant: 접두 없는 bare path) ---
// 리터럴 `a*.txt` 파일 하나와 그 glob에 걸릴 a1.txt/astar.txt를 두고 stage("a*.txt").
// :(literal)이면 파일 하나만; 접두를 떼면 git이 glob으로 봐 a1.txt/astar.txt까지 스테이징.
#[tokio::test]
async fn literal_pathspec_protects_glob_filename() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("a1.txt"), "1\n").unwrap();
    std::fs::write(wt.join("astar.txt"), "2\n").unwrap();
    std::fs::write(wt.join("a*.txt"), "star\n").unwrap(); // 리터럴 별표 파일명

    stage(&r, wt, "a*.txt").await.unwrap();

    let s = staged(&r, wt).await;
    assert!(
        s.contains("a*.txt"),
        "리터럴 파일 a*.txt는 스테이징되어야 한다"
    );
    assert!(
        !s.contains("a1.txt"),
        "glob 오해로 a1.txt가 스테이징되면 안 된다 (:(literal) 보호 실패)"
    );
    assert!(
        !s.contains("astar.txt"),
        "glob 오해로 astar.txt가 스테이징되면 안 된다"
    );
    assert_eq!(s.len(), 1, "정확히 리터럴 파일 하나만 스테이징");
}

// 선행 대시 파일명도 리터럴로 보호된다 — `-n`을 stage하면 그 파일만.
#[tokio::test]
async fn literal_pathspec_protects_dash_filename() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("-n"), "dash\n").unwrap();

    stage(&r, wt, "-n").await.unwrap();

    assert!(
        staged(&r, wt).await.contains("-n"),
        "파일 '-n'이 스테이징되어야 한다"
    );
}

// --- crux: bulk per-path 결과 벡터 + 청크 경계 (mutant: 청크 크기 확대 / per-path 매핑 제거) ---
// 유효 파일 100개 + 존재하지 않는 경로 1개 = 101 입력. 청크=100이면 첫 청크(유효 100)는
// 성공해 전부 스테이징+Ok, 둘째 청크(존재X 1)만 원자적 실패로 Err. 청크 크기를 1000으로
// 바꾸면 101개가 한 청크가 되어 원자적으로 전부 실패 → results[0]가 Err가 되어 FAIL.
#[tokio::test]
async fn bulk_stage_per_path_outcome_respects_chunk_boundary() {
    let (repo, r) = setup();
    let wt = repo.path();

    let names: Vec<String> = (0..100).map(|i| format!("v{i}.txt")).collect();
    for n in &names {
        std::fs::write(wt.join(n), "x\n").unwrap();
    }
    let mut paths: Vec<&str> = names.iter().map(String::as_str).collect();
    paths.push("does-not-exist.txt"); // 101번째 = 둘째 청크

    let results = bulk_stage(&r, wt, &paths).await;

    // 입력마다 정확히 한 결과, 같은 순서.
    assert_eq!(results.len(), 101, "결과 벡터는 입력 경로 수와 같아야 한다");
    for (i, (path, res)) in results.iter().enumerate().take(100) {
        assert_eq!(path, &paths[i]);
        assert!(
            res.is_ok(),
            "첫 청크의 유효 경로 {path}는 Ok여야 한다 (청크 경계 붕괴?)"
        );
    }
    assert_eq!(results[100].0, "does-not-exist.txt");
    assert!(results[100].1.is_err(), "존재하지 않는 경로는 Err여야 한다");

    // 첫 청크의 유효 100개는 실제로 스테이징되었다(둘째 청크 실패가 오염시키지 않는다).
    let s = staged(&r, wt).await;
    assert_eq!(s.len(), 100, "유효 파일 100개가 스테이징되어야 한다");
    for n in &names {
        assert!(s.contains(n), "{n}이 스테이징되지 않았다");
    }
}

// 청크 원자성 문서화: 한 청크 안에 잘못된 경로가 있으면 그 청크의 **모든** 경로가 Err이고
// 아무것도 스테이징되지 않는다(git add는 pathspec 하나라도 매칭 실패 시 청크 전체 실패).
#[tokio::test]
async fn bulk_stage_failed_chunk_marks_all_its_paths() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("ok1.txt"), "a\n").unwrap();
    std::fs::write(wt.join("ok2.txt"), "b\n").unwrap();

    // 세 경로가 한 청크(≤100). 가운데가 존재하지 않아 청크 전체가 원자적으로 실패.
    let results = bulk_stage(&r, wt, &["ok1.txt", "nope.txt", "ok2.txt"]).await;

    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|(_, res)| res.is_err()),
        "실패 청크의 모든 경로가 Err"
    );
    assert!(
        staged(&r, wt).await.is_empty(),
        "원자적 실패 → 아무것도 스테이징되지 않음"
    );
}

// bulk 왕복: 유효 경로들을 bulk_stage → 전부 Ok+스테이징, bulk_unstage → 전부 사라짐.
#[tokio::test]
async fn bulk_stage_then_unstage_round_trip() {
    let (repo, r) = setup();
    let wt = repo.path();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(wt.join(n), "x\n").unwrap();
    }

    let staged_res = bulk_stage(&r, wt, &["a.txt", "b.txt", "c.txt"]).await;
    assert!(staged_res.iter().all(|(_, r)| r.is_ok()));
    assert_eq!(staged(&r, wt).await.len(), 3, "셋 다 스테이징");

    let unstaged_res = bulk_unstage(&r, wt, &["a.txt", "b.txt", "c.txt"]).await;
    assert!(unstaged_res.iter().all(|(_, r)| r.is_ok()));
    assert!(staged(&r, wt).await.is_empty(), "셋 다 언스테이징");
}

// 빈 입력은 git을 부르지 않고 빈 벡터.
#[tokio::test]
async fn bulk_stage_empty_is_empty_vec() {
    let (repo, r) = setup();
    let results = bulk_stage(&r, repo.path(), &[]).await;
    assert!(results.is_empty());
}
