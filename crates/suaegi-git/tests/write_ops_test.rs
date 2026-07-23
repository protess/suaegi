//! M1 스테이징 write-ops의 real-git 통합 테스트. 격리된 tempdir repo(`fixture::init_repo`)
//! 위에서 실제 git으로 왕복시키고, `:(literal)`이 glob 파일명을 보호하는지, bulk의
//! per-path 결과 벡터가 청크 원자성/청크 경계를 지키는지 실측으로 못 박는다.

mod fixture;

use std::collections::HashSet;
use std::path::Path;
use suaegi_git::runner::GitRunner;
use suaegi_git::write_ops::{
    bulk_stage, bulk_unstage, commit_changes, stage, unstage, CommitOutcome,
};

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

// --- M2: commit_changes real-git 통합 ---

/// 현재 HEAD 커밋 SHA.
async fn head(r: &GitRunner, wt: &Path) -> String {
    r.run(wt, &["rev-parse", "HEAD"])
        .await
        .unwrap()
        .stdout
        .trim()
        .to_string()
}

/// HEAD까지의 커밋 개수 — `git rev-list --count HEAD`.
async fn commit_count(r: &GitRunner, wt: &Path) -> usize {
    r.run(wt, &["rev-list", "--count", "HEAD"])
        .await
        .unwrap()
        .stdout
        .trim()
        .parse()
        .unwrap()
}

// --- crux: happy commit (mutant: -m/메시지 미전달 → 메시지 불일치로 FAIL) ---
// 파일을 stage → commit → Committed; HEAD가 정확히 1개 전진하고 워크트리가 깨끗하다.
#[tokio::test]
async fn commit_happy_path_advances_head_and_cleans_worktree() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("f.txt"), "hi\n").unwrap();
    stage(&r, wt, "f.txt").await.unwrap();

    let before = commit_count(&r, wt).await;
    let outcome = commit_changes(&r, wt, "add f.txt").await.unwrap();

    assert_eq!(outcome, CommitOutcome::Committed);
    assert_eq!(
        commit_count(&r, wt).await,
        before + 1,
        "HEAD가 커밋 1개만큼 전진해야 한다"
    );
    // 워크트리 clean: porcelain 출력이 비어 있다.
    let status = r.run(wt, &["status", "--porcelain"]).await.unwrap().stdout;
    assert!(status.trim().is_empty(), "커밋 후 워크트리가 깨끗해야 한다");
    // 커밋 메시지가 그대로 기록되었다(-m/메시지 전달 확인).
    let msg = r
        .run(wt, &["log", "-1", "--format=%B"])
        .await
        .unwrap()
        .stdout;
    assert_eq!(msg.trim(), "add f.txt");
}

// --- crux: empty-index 게이트 (mutant: non-zero→Committed → HEAD 전진 단언으로 FAIL) ---
// 아무것도 stage 안 한 채 commit → Failed("nothing to commit"); HEAD가 전진하지 않는다.
#[tokio::test]
async fn commit_empty_index_is_failed_and_head_unchanged() {
    let (repo, r) = setup();
    let wt = repo.path();

    let before = head(&r, wt).await;
    let outcome = commit_changes(&r, wt, "nothing here").await.unwrap();

    match outcome {
        CommitOutcome::Failed { message } => assert!(
            message.contains("nothing to commit"),
            "empty index 메시지는 stdout의 'nothing to commit'이어야 한다: {message:?}"
        ),
        CommitOutcome::Committed => panic!("empty index인데 Committed로 판정됐다"),
    }
    assert_eq!(
        head(&r, wt).await,
        before,
        "실패한 커밋은 HEAD를 전진시키면 안 된다"
    );
}

// --- crux: identity override 없음 (mutant: -c user.* 주입 → author 불일치로 FAIL) ---
// commit_changes가 어떤 identity도 주입하지 않으므로 author는 fixture의 repo-local
// 정체(`test <t@example.com>`)여야 한다. `-c user.name=...`을 넣으면 이 단언이 깨진다.
#[tokio::test]
async fn commit_uses_fixture_repo_local_identity_not_injected() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("g.txt"), "x\n").unwrap();
    stage(&r, wt, "g.txt").await.unwrap();

    assert_eq!(
        commit_changes(&r, wt, "id check").await.unwrap(),
        CommitOutcome::Committed
    );

    let author = r
        .run(wt, &["log", "-1", "--format=%an <%ae>"])
        .await
        .unwrap()
        .stdout;
    assert_eq!(
        author.trim(),
        "test <t@example.com>",
        "커밋은 fixture의 repo-local identity로 저작되어야 한다(주입 금지)"
    );
}

// --- crux: 특수문자 메시지는 리터럴로 커밋 (mutant: shell 보간 시 메시지 변형 → FAIL) ---
// 선행 대시 + 셸 메타문자가 담긴 메시지가 별개 argv로 넘어가 그대로 기록된다.
// argv 전달이라 shell이 없어 `$(...)`/backtick/`;`가 실행되지 않는다.
#[tokio::test]
async fn commit_message_with_special_chars_is_literal() {
    let (repo, r) = setup();
    let wt = repo.path();
    std::fs::write(wt.join("h.txt"), "x\n").unwrap();
    stage(&r, wt, "h.txt").await.unwrap();

    let weird = "-x weird $(whoami) `id` ; rm -rf /";
    assert_eq!(
        commit_changes(&r, wt, weird).await.unwrap(),
        CommitOutcome::Committed
    );

    let msg = r
        .run(wt, &["log", "-1", "--format=%B"])
        .await
        .unwrap()
        .stdout;
    assert_eq!(msg.trim(), weird, "메시지는 리터럴로 기록되어야 한다");
}
