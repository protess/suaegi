//! M2 `fetch`/`pull` 드라이버의 real-git AV 테스트 — **로컬 bare remote**로 왕복(네트워크
//! 없음). bare `git init --bare`가 origin, clone A/B가 두 협업자를 흉내낸다. 순수 헬퍼
//! (ff-only 거부 판정, up-to-date 파싱, argv)는 `src/remote.rs` unit이 mutation 검증한다.
//!
//! F4 crux: `pull --ff-only`이 divergent에서 **clean 실패**(NotFastForward)하고 워크트리를
//! stuck시키지 않는다 — half-merge·MERGE_HEAD·conflict marker 없이 미변경으로 남는다.

mod fixture;

use std::path::{Path, PathBuf};
use suaegi_git::remote::{fetch, pull, PullOutcome};
use suaegi_git::runner::GitRunner;

/// bare origin + clone A + clone B. A는 초기 커밋을 push해두고, B는 그 상태의 clone이다.
/// `TempDir`는 살려서 반환(drop되면 지워진다).
struct Remotes {
    _root: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    r: GitRunner,
}

fn setup() -> Remotes {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("remote.git");
    let a = root.path().join("a");
    let b = root.path().join("b");

    std::fs::create_dir_all(&bare).unwrap();
    fixture::init_bare_remote(&bare);

    // A: 완전한 repo(README init 커밋) → origin 등록 → 초기 push.
    std::fs::create_dir_all(&a).unwrap();
    fixture::init_repo(&a);
    fixture::run(&a, &["remote", "add", "origin", bare.to_str().unwrap()]);
    fixture::run(&a, &["push", "-u", "origin", "main"]);

    // B: 그 상태를 clone(origin/main == A의 init 커밋).
    fixture::clone_from(&bare, &b);

    Remotes {
        _root: root,
        a,
        b,
        r: GitRunner::new(),
    }
}

/// `git rev-parse <refname>`의 트림된 stdout. 실제 드라이버가 쓰는 `GitRunner`로 읽는다.
async fn rev_parse(r: &GitRunner, wt: &Path, refname: &str) -> String {
    r.run(wt, &["rev-parse", refname])
        .await
        .unwrap_or_else(|e| panic!("rev-parse {refname} failed: {e}"))
        .stdout
        .trim()
        .to_string()
}

/// A가 README를 `content`로 바꾼 새 커밋을 만들고 origin/main으로 push한다.
fn a_push_new_commit(a: &Path, content: &str, msg: &str) {
    std::fs::write(a.join("README.md"), content).unwrap();
    fixture::run(a, &["add", "-A"]);
    fixture::run(a, &["commit", "-m", msg]);
    fixture::run(a, &["push", "origin", "main"]);
}

// ── fetch 왕복(crux: fetch_args wrong-remote mutation → FAIL) ─────────────────
//
// A가 새 커밋을 push → B가 `fetch` → B의 origin/main이 그 커밋으로 전진하고, B의 HEAD와
// 워크트리는 **미변경**. fetch는 remote-tracking ref만 갱신하는 안전한 read op다.
// Mutation: `fetch_args`를 `["fetch","wrong"]`로 바꾸면 origin이 없어 fetch가 Err →
// `.unwrap()` 패닉 → FAIL.
#[tokio::test]
async fn fetch_advances_remote_tracking_ref_only() {
    let env = setup();
    let (a, b, r) = (&env.a, &env.b, &env.r);

    a_push_new_commit(a, "v2 from A\n", "c2");
    let remote_head = rev_parse(r, a, "HEAD").await;

    let b_head_before = rev_parse(r, b, "HEAD").await;
    let b_origin_before = rev_parse(r, b, "origin/main").await;
    assert_eq!(
        b_origin_before, b_head_before,
        "사전조건: fetch 전 B의 origin/main == HEAD(init 커밋)"
    );

    fetch(r, b).await.expect("fetch는 성공해야 한다");

    let b_origin_after = rev_parse(r, b, "origin/main").await;
    assert_eq!(
        b_origin_after, remote_head,
        "fetch 후 B의 origin/main이 A가 push한 커밋으로 전진해야 한다"
    );
    // HEAD·워크트리는 미변경(fetch는 tracking ref만 건드린다).
    assert_eq!(
        rev_parse(r, b, "HEAD").await,
        b_head_before,
        "fetch가 HEAD를 건드리면 안 된다"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("README.md")).unwrap(),
        "hello\n",
        "fetch가 워크트리를 건드리면 안 된다"
    );
}

// ── pull fast-forward(crux) ──────────────────────────────────────────────────
//
// B는 로컬 커밋이 없고 원격이 앞서 있다 → `pull` → fast-forward → HEAD가 원격까지 전진,
// PullOutcome::Ok, 파일 내용 갱신.
#[tokio::test]
async fn pull_fast_forwards_when_behind() {
    let env = setup();
    let (a, b, r) = (&env.a, &env.b, &env.r);

    a_push_new_commit(a, "ff content\n", "c2");
    let remote_head = rev_parse(r, a, "HEAD").await;

    let outcome = pull(r, b).await.expect("ff pull은 성공해야 한다");
    assert_eq!(
        outcome,
        PullOutcome::Ok,
        "behind면 fast-forward(Ok)여야 한다"
    );
    assert_eq!(
        rev_parse(r, b, "HEAD").await,
        remote_head,
        "pull 후 B의 HEAD가 원격까지 전진해야 한다"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("README.md")).unwrap(),
        "ff content\n",
        "pull이 워크트리 파일을 갱신해야 한다"
    );
}

// ── pull already-up-to-date ──────────────────────────────────────────────────
#[tokio::test]
async fn pull_up_to_date_is_noop() {
    let env = setup();
    let (b, r) = (&env.b, &env.r);

    let head_before = rev_parse(r, b, "HEAD").await;
    let outcome = pull(r, b).await.expect("up-to-date pull은 성공해야 한다");
    assert_eq!(
        outcome,
        PullOutcome::UpToDate,
        "원격과 같으면 UpToDate여야 한다"
    );
    assert_eq!(
        rev_parse(r, b, "HEAD").await,
        head_before,
        "up-to-date pull은 HEAD를 바꾸면 안 된다"
    );
}

// ── pull NON-fast-forward: F4 CRUX ───────────────────────────────────────────
//
// B가 로컬 커밋을 갖고 원격은 **다른**(충돌하는) 커밋을 가진다(diverged) → `pull --ff-only`
// → NotFastForward(clean 실패). **B의 HEAD·워크트리는 미변경** — half-merge도, MERGE_HEAD도,
// conflict marker도 없다.
//
// 죽이는 mutation 둘:
//  (1) `pull_args`에서 `--ff-only`를 빼면 → `pull.rebase=false` 위에서 plain merge를 시도해
//      충돌 → MERGE_HEAD 생성 + conflict marker + 다른 stderr("Not possible to fast-forward"
//      아님) → `is_ff_only_rejected`가 false → pull이 `Err` → outcome 단언 FAIL, 그리고
//      MERGE_HEAD/marker 단언도 FAIL.
//  (2) `is_ff_only_rejected`를 `Ok`로(또는 상수 false로) 매핑 → outcome != NotFastForward → FAIL.
#[tokio::test]
async fn pull_ff_only_diverged_fails_clean_worktree_unchanged() {
    let env = setup();
    let (a, b, r) = (&env.a, &env.b, &env.r);

    // B: 로컬 커밋(README를 B쪽으로).
    std::fs::write(b.join("README.md"), "B-side change\n").unwrap();
    fixture::run(b, &["add", "-A"]);
    fixture::run(b, &["commit", "-m", "b-local"]);
    let b_head_before = rev_parse(r, b, "HEAD").await;

    // A: 같은 파일을 **다르게** 바꾼 커밋을 push → diverged + 충돌 소지.
    a_push_new_commit(a, "A-side change\n", "a-remote");

    let outcome = pull(r, b)
        .await
        .expect("ff-only 거부는 clean 값이어야(Err 아님)");
    assert_eq!(
        outcome,
        PullOutcome::NotFastForward,
        "divergent pull은 NotFastForward(clean 실패)여야 한다"
    );

    // 워크트리·HEAD 미변경 — stuck 상태 없음.
    assert_eq!(
        rev_parse(r, b, "HEAD").await,
        b_head_before,
        "ff-only 실패 후 HEAD가 미변경이어야 한다"
    );
    let content = std::fs::read_to_string(b.join("README.md")).unwrap();
    assert_eq!(
        content, "B-side change\n",
        "ff-only 실패 후 워크트리 파일이 미변경이어야 한다"
    );
    assert!(
        !content.contains("<<<<<<<") && !content.contains(">>>>>>>"),
        "conflict marker가 워크트리에 새면 안 된다(half-merge): {content}"
    );
    assert!(
        !b.join(".git").join("MERGE_HEAD").exists(),
        "MERGE_HEAD가 남으면 stuck merge 상태다 — ff-only는 merge를 시작조차 하면 안 된다"
    );
}

// ── transient: no remote → Err(false success 아님) ───────────────────────────
//
// 원격이 아예 없는 repo → fetch/pull 모두 Err. Mutation: 실패를 Ok/UpToDate로 삼키면
// (transient=false-negative) 이 단언들이 FAIL.
#[tokio::test]
async fn fetch_and_pull_error_when_no_remote() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("solo");
    std::fs::create_dir_all(&repo).unwrap();
    fixture::init_repo(&repo);
    let r = GitRunner::new();

    assert!(
        fetch(&r, &repo).await.is_err(),
        "원격 없는 fetch는 Err여야 한다(조용한 no-op 금지)"
    );
    let pulled = pull(&r, &repo).await;
    assert!(
        pulled.is_err(),
        "원격 없는 pull은 Err여야 한다(false 'up to date' 금지): {pulled:?}"
    );
}

// ── pull FF가 uncommitted 편집을 덮을 상황: DATA-SAFETY 회귀 ───────────────────
//
// B는 원격보다 뒤처져 있고(FF 가능) FF 대상 파일에 **커밋 안 된 편집**을 갖고 있다 →
// `git pull --ff-only`은 그 편집을 덮어쓰지 않으려고 **거부**한다("Your local changes ...
// would be overwritten by merge"). 이건 `is_ff_only_rejected`("Not possible to
// fast-forward")에 **안 걸린다** → `pull()`이 `Err`(NotFastForward도 Ok도 아님).
// git이 아예 손대지 않으므로 uncommitted 편집·HEAD 미변경, MERGE_HEAD 없음(데이터손실 0).
//
// 죽이는 mutation: `is_ff_only_rejected`가 이 문구("would be overwritten")까지 매칭하도록
// 확장하면 → pull이 이걸 NotFastForward(Ok 값)로 잘못 분류 → `is_err()` 단언 FAIL.
#[tokio::test]
async fn pull_ff_only_preserves_uncommitted_edit_and_errors() {
    let env = setup();
    let (a, b, r) = (&env.a, &env.b, &env.r);

    // 원격이 README를 FF로 갱신하게 만든다(B는 이 커밋을 아직 안 가짐).
    a_push_new_commit(a, "remote FF content\n", "c2");

    // B: README에 **커밋 안 된** 편집(FF가 이 파일을 건드리므로 덮어쓰기 대상).
    std::fs::write(b.join("README.md"), "B uncommitted edit\n").unwrap();
    let b_head_before = rev_parse(r, b, "HEAD").await;

    let pulled = pull(r, b).await;
    assert!(
        pulled.is_err(),
        "uncommitted 편집을 덮을 FF pull은 Err여야 한다(NotFastForward도 false success도 아님): {pulled:?}"
    );

    // 데이터-안전: uncommitted 편집 보존 + HEAD 미변경 + stuck merge 상태 없음.
    assert_eq!(
        std::fs::read_to_string(b.join("README.md")).unwrap(),
        "B uncommitted edit\n",
        "uncommitted 편집이 클로버되면 안 된다(데이터손실)"
    );
    assert_eq!(
        rev_parse(r, b, "HEAD").await,
        b_head_before,
        "거부된 FF는 HEAD를 건드리면 안 된다"
    );
    assert!(
        !b.join(".git").join("MERGE_HEAD").exists(),
        "MERGE_HEAD가 남으면 stuck 상태다 — FF 거부는 merge를 시작조차 하지 않는다"
    );
}
