//! M3 `push` 드라이버의 real-git AV 테스트 — **로컬 bare remote**로 왕복(네트워크 없음).
//! bare `git init --bare`가 origin, clone A/B가 두 협업자를 흉내낸다. 순수 헬퍼(outcome
//! 분류, argv, --force 금지)는 `src/remote.rs` unit이 mutation 검증한다.
//!
//! M3 crux(대죄 방지): 원격이 non-fast-forward로 **거부**한 push는 절대 성공(Ok/UpToDate)으로
//! 읽히지 않고, **bare remote의 main도 미변경**이어야 한다 — B의 커밋이 원격에 안 올랐는데
//! 워크플로가 "올랐다"로 착각해 stale ref에 PR을 만드는 사태를 막는다.

mod fixture;

use std::path::{Path, PathBuf};
use suaegi_git::remote::{push, push_args, PushOutcome};
use suaegi_git::runner::GitRunner;

/// bare origin + clone A + clone B. A가 init 커밋을 push해두고, B는 그 상태의 clone이다.
/// `TempDir`는 살려서 반환(drop되면 지워진다).
struct Remotes {
    _root: tempfile::TempDir,
    bare: PathBuf,
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

    // B: 그 상태를 clone(origin/main == A의 init 커밋, main은 upstream tracking 있음).
    fixture::clone_from(&bare, &b);

    Remotes {
        _root: root,
        bare,
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

/// bare remote(`--git-dir`)에서 ref를 읽는다 — origin이 실제로 어디를 가리키는지 확인용.
fn bare_rev_parse(bare: &Path, refname: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["--git-dir", bare.to_str().unwrap(), "rev-parse", refname])
        .env("LC_ALL", "C")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "bare rev-parse {refname}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `wt`에서 README를 `content`로 바꾼 새 커밋을 만든다(push는 하지 않는다).
fn commit_local(wt: &Path, content: &str, msg: &str) {
    std::fs::write(wt.join("README.md"), content).unwrap();
    fixture::run(wt, &["add", "-A"]);
    fixture::run(wt, &["commit", "-m", msg]);
}

// ── push 해피패스: 원격 ref 전진 + Ok(crux) ──────────────────────────────────
//
// B가 로컬 커밋을 만들고 `push` → 원격(bare) main이 그 커밋으로 전진하고 PushOutcome::Ok.
// Mutation: `push_args`가 `HEAD:<branch>` 대신 엉뚱한 refspec을 내거나 classify가 Ok를
// 다른 값으로 바꾸면 FAIL.
#[tokio::test]
async fn push_advances_remote_and_returns_ok() {
    let env = setup();
    let (bare, b, r) = (&env.bare, &env.b, &env.r);

    commit_local(b, "B v2\n", "b-c2");
    let b_head = rev_parse(r, b, "HEAD").await;

    // main은 clone 시 upstream tracking이 이미 있으므로 set_upstream=false.
    let outcome = push(r, b, "main", false)
        .await
        .expect("push는 성공해야 한다");
    assert_eq!(outcome, PushOutcome::Ok, "새 커밋 push는 Ok여야 한다");

    // 원격 ref가 B의 커밋으로 실제 전진했는지 bare에서 직접 확인.
    assert_eq!(
        bare_rev_parse(bare, "main"),
        b_head,
        "push 후 bare remote의 main이 B의 HEAD로 전진해야 한다"
    );
}

// ── push --set-upstream: 새 브랜치 최초 publish가 tracking을 세운다 ────────────
//
// B가 upstream 없는 새 브랜치를 만들고 push(set_upstream=true) → 원격에 브랜치 생성 +
// 로컬 브랜치의 upstream이 origin/<branch>로 설정. Mutation: `push_args`가 `--set-upstream`을
// 빼면 upstream이 안 잡혀 `@{upstream}` 조회가 실패 → FAIL.
#[tokio::test]
async fn push_set_upstream_sets_tracking() {
    let env = setup();
    let (bare, b, r) = (&env.bare, &env.b, &env.r);

    // upstream 없는 새 브랜치.
    fixture::run(b, &["checkout", "-b", "feature"]);
    commit_local(b, "feature work\n", "feat-c1");
    let b_head = rev_parse(r, b, "HEAD").await;

    let outcome = push(r, b, "feature", true)
        .await
        .expect("최초 publish push는 성공해야 한다");
    assert_eq!(
        outcome,
        PushOutcome::Ok,
        "새 브랜치 최초 push는 Ok여야 한다"
    );

    // 원격에 브랜치가 생겼고 B의 커밋을 가리킨다.
    assert_eq!(
        bare_rev_parse(bare, "feature"),
        b_head,
        "push가 원격에 feature 브랜치를 만들어야 한다"
    );
    // 로컬 feature의 upstream이 origin/feature로 설정됐다.
    let upstream = rev_parse(r, b, "feature@{upstream}").await;
    let origin_feature = rev_parse(r, b, "origin/feature").await;
    assert_eq!(
        upstream, origin_feature,
        "--set-upstream이 feature@{{upstream}}을 origin/feature로 세워야 한다"
    );
}

// ── push up-to-date ──────────────────────────────────────────────────────────
//
// B에 새 커밋이 없다(fresh clone) → push → UpToDate("Everything up-to-date"). 원격 미변경.
#[tokio::test]
async fn push_up_to_date_when_nothing_new() {
    let env = setup();
    let (bare, b, r) = (&env.bare, &env.b, &env.r);

    let bare_before = bare_rev_parse(bare, "main");

    let outcome = push(r, b, "main", false)
        .await
        .expect("up-to-date push는 성공해야 한다");
    assert_eq!(
        outcome,
        PushOutcome::UpToDate,
        "보낼 게 없으면 UpToDate여야 한다"
    );
    assert_eq!(
        bare_rev_parse(bare, "main"),
        bare_before,
        "up-to-date push는 원격 ref를 바꾸면 안 된다"
    );
}

// ── push NON-FAST-FORWARD REJECTED: M3 대죄-방지 CRUX ─────────────────────────
//
// 원격(bare)에 B가 갖지 못한 커밋이 있다(A가 divergent 커밋을 push해둠). B는 로컬로 **다른**
// 커밋을 만든다 → B `push` → 원격이 non-fast-forward로 **거부**. `push`는
// `PushOutcome::NonFastForwardRejected`를 돌려야 하고, **절대 Ok/UpToDate가 아니다**. 그리고
// **bare remote의 main은 A의 커밋에 그대로**여야 한다(B의 커밋이 안 올랐음 = 거부가 진짜였음).
//
// 죽이는 mutation:
//  - `classify_push_outcome`에서 NonFastForwardRejected → Ok(또는 UpToDate)로 매핑하면
//    outcome 단언 FAIL(+ `assert_ne!` Ok/UpToDate).
//  - `push_args`에 `--force`를 추가하는 mutation은 push를 **강제 성공**시켜 원격 main을 B의
//    커밋으로 덮어쓴다 → "bare main이 A의 커밋에 미변경" 단언 FAIL(force 금지의 행위적 증명).
#[tokio::test]
async fn push_non_fast_forward_rejected_never_success_remote_unchanged() {
    let env = setup();
    let (bare, a, b, r) = (&env.bare, &env.a, &env.b, &env.r);

    // A가 divergent 커밋을 원격에 push → bare main = A의 c2. B는 이 커밋을 모른다.
    commit_local(a, "A-side change\n", "a-c2");
    fixture::run(a, &["push", "origin", "main"]);
    let a_c2 = rev_parse(r, a, "HEAD").await;
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_c2,
        "사전조건: bare main이 A의 divergent 커밋을 가리켜야 한다"
    );

    // B가 로컬로 **다른** 커밋을 만든다(diverged, non-ff 유발).
    commit_local(b, "B-side change\n", "b-c2");

    let outcome = push(r, b, "main", false)
        .await
        .expect("non-ff 거부는 clean 값이어야(Err 아님)");
    assert_eq!(
        outcome,
        PushOutcome::NonFastForwardRejected,
        "diverged push는 NonFastForwardRejected여야 한다"
    );
    // 대죄 방지: 절대 성공으로 읽히면 안 된다.
    assert_ne!(
        outcome,
        PushOutcome::Ok,
        "non-ff 거부가 Ok로 오분류됐다 — PR이 stale ref를 가리킨다"
    );
    assert_ne!(outcome, PushOutcome::UpToDate);

    // 원격 ref 미변경: B의 커밋은 올라가지 **않았다**(거부가 진짜였음, force도 없었음).
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_c2,
        "거부된 push 후 bare main이 A의 커밋에 미변경이어야 한다(B 커밋 미착지)"
    );
    assert_ne!(
        bare_rev_parse(bare, "main"),
        rev_parse(r, b, "HEAD").await,
        "원격 main이 B의 HEAD로 옮겨가면 안 된다(거부됐으므로)"
    );
}

// ── no --force: argv에 강제 플래그 없음(M4까지) ──────────────────────────────
//
// `push_args`가 `--force`/`--force-with-lease`를 절대 내지 않는다. src/remote.rs unit에도
// 있지만, 실제 드라이버가 부르는 argv 빌더를 여기서도 못 박는다. Mutation: 어느 강제 플래그를
// 넣어도 FAIL.
#[test]
fn push_args_never_contains_force_flags() {
    for set_upstream in [false, true] {
        let args = push_args("main", set_upstream);
        assert!(
            !args.iter().any(|a| a == "--force"),
            "--force 금지(force-with-lease는 M4): {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "--force-with-lease"),
            "--force-with-lease 금지(M4): {args:?}"
        );
    }
}

// ── transient: 원격 없음 → Err(false success 아님) ───────────────────────────
//
// 원격이 아예 없는 repo → push는 Err. Mutation: 실패를 Ok/UpToDate로 삼키면
// (transient=false-negative) 이 단언이 FAIL.
#[tokio::test]
async fn push_errors_when_no_remote() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("solo");
    std::fs::create_dir_all(&repo).unwrap();
    fixture::init_repo(&repo);
    let r = GitRunner::new();

    let pushed = push(&r, &repo, "main", false).await;
    assert!(
        pushed.is_err(),
        "원격 없는 push는 Err여야 한다(조용한 false 'up to date' 금지): {pushed:?}"
    );
}
