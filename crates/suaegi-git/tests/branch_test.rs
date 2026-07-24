//! `branch.rs`(M3) 회귀 테스트 — 실 git tempdir. 모든 crux는 mutation 검증 대상
//! (저장소 하드룰: 공허한 테스트 금지). fixture로 격리된 config를 쓴다.

mod fixture;

use suaegi_git::branch::{current_branch, list_branches};
use suaegi_git::runner::GitRunner;

fn runner() -> GitRunner {
    GitRunner::new()
}

/// main(현재) + feature + release/1 세 브랜치. 반환: repo.
fn build_three_branch_repo() -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p); // main 위 커밋 1개
    fixture::run(p, &["branch", "feature"]);
    fixture::run(p, &["branch", "release/1"]);
    // main에 체크아웃된 채로 둔다(init_repo가 -b main).
    repo
}

// ── crux: list + current 마커 ────────────────────────────────────────────────
// 세 브랜치 모두 나오고, main만 is_current=true.
// mutation: `marker == "*"`를 `marker == " "`/항상 false/항상 true로 바꾸면
// current 개수 단언이 깨진다. 정렬 mutation(current-first 제거)도 [0]==main 단언이 잡는다.
#[tokio::test]
async fn lists_all_branches_with_current_marker() {
    let repo = build_three_branch_repo();
    let out = list_branches(&runner(), repo.path()).await.unwrap();

    let names: Vec<&str> = out.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names.len(), 3, "세 브랜치 모두: {names:?}");
    assert!(names.contains(&"main"));
    assert!(names.contains(&"feature"));
    assert!(names.contains(&"release/1"));

    // 정확히 하나만 current, 그게 main.
    let current: Vec<&str> = out
        .iter()
        .filter(|b| b.is_current)
        .map(|b| b.name.as_str())
        .collect();
    assert_eq!(current, vec!["main"], "current는 main 하나뿐");

    // current-first 정렬: 맨 앞이 main.
    assert_eq!(out[0].name, "main", "현재 브랜치가 맨 앞");
    assert!(out[0].is_current);
}

// ── crux: current_branch 편의 함수 ───────────────────────────────────────────
#[tokio::test]
async fn current_branch_returns_checked_out_name() {
    let repo = build_three_branch_repo();
    let cur = current_branch(&runner(), repo.path()).await.unwrap();
    assert_eq!(cur, Some("main".to_string()));
}

// ── crux: TAB 분할이 이름을 온전히 보존 ──────────────────────────────────────
// 슬래시가 든 이름(`release/1`)과 하이픈 든 이름을 정확히 파싱.
// mutation: split을 TAB이 아닌 공백(' ')으로 바꾸면, marker가 공백이라
// `feat-x`류 이름은 여전히 통과하지만 **current 브랜치**(marker=`*`)는
// `* main`이 아니라 `*\tmain`이라 공백 split 시 name이 통째로 사라진다 →
// lists_all_branches의 3개 단언과 current 단언이 FAIL. 여기선 이름 보존만 확인.
#[tokio::test]
async fn preserves_full_branch_names() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    fixture::init_repo(p);
    fixture::run(p, &["branch", "feat-x"]);
    fixture::run(p, &["branch", "team/sub/deep"]);
    let out = list_branches(&runner(), p).await.unwrap();
    let names: Vec<&str> = out.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"feat-x"), "하이픈 이름 보존: {names:?}");
    assert!(
        names.contains(&"team/sub/deep"),
        "슬래시 이름 보존: {names:?}"
    );
    // 이름이 marker와 안 섞였는지: 어떤 name도 `*`/TAB/공백을 포함 안 함.
    for b in &out {
        assert!(!b.name.contains('*'), "이름에 marker 누출: {:?}", b.name);
        assert!(!b.name.contains('\t'));
        assert!(!b.name.starts_with(' '));
    }
}

// ── crux: 빈 repo → 빈 Vec, 에러 아님 ────────────────────────────────────────
// mutation: "빈 출력 → Err"로 바꾸면 FAIL. transient≠false-negative.
#[tokio::test]
async fn empty_repo_yields_empty_vec_not_error() {
    let repo = tempfile::tempdir().unwrap();
    let p = repo.path();
    // 커밋도 브랜치도 없는 fresh init (fixture::init_repo은 커밋을 만들어서 수동 init).
    std::fs::write(p.join(".test-gitconfig"), "").unwrap();
    fixture::run(p, &["init", "-b", "main"]);

    let out = list_branches(&runner(), p).await.expect("빈 repo는 Ok");
    assert!(out.is_empty(), "브랜치 없음 → 빈 Vec: {out:?}");

    let cur = current_branch(&runner(), p).await.expect("빈 repo는 Ok");
    assert_eq!(cur, None, "unborn HEAD → current 없음");
}

// ── crux: detached HEAD → current 없음 ───────────────────────────────────────
// 커밋에 detached 체크아웃하면 어떤 브랜치도 `*`를 못 받는다.
// mutation: detached에서 아무 브랜치나 current로 잡으면(예: 첫 줄 강제 current)
// current_branch가 Some을 내 FAIL.
#[tokio::test]
async fn detached_head_has_no_current() {
    let repo = build_three_branch_repo();
    let p = repo.path();
    // HEAD를 커밋 oid로 detach.
    fixture::run(p, &["checkout", "--detach", "HEAD"]);

    let out = list_branches(&runner(), p).await.unwrap();
    assert_eq!(out.len(), 3, "브랜치는 여전히 셋 다 보인다");
    assert!(
        out.iter().all(|b| !b.is_current),
        "detached면 어떤 브랜치도 current 아님: {out:?}"
    );

    let cur = current_branch(&runner(), p).await.unwrap();
    assert_eq!(cur, None, "detached HEAD → current None");
}
