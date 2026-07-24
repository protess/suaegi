//! M4 force-with-lease의 real-git AV 테스트 — **로컬 bare remote**로 왕복(네트워크 없음).
//! **SAFETY 마일스톤**: force-push는 원격을 덮어쓴다. 게이트는 원격의 behind 커밋이 전부
//! 에이전트 **자신의** 로컬 히스토리를 rebase-rewrite한 patch-equivalent일 때만 열려야 하고,
//! 남이 push한 진짜 divergent 기여는 **절대** 덮어쓰면 안 된다. 순수 헬퍼(cherry-mark 파스,
//! 게이트 조건, argv의 bare-`--force` 금지)는 `src/remote.rs` unit이 mutation 검증한다.
//!
//! THE 안전 crux(`unsafe_someone_else_pushed_is_not_clobbered`): 남이 push한 커밋 Y가 원격에
//! 있으면 게이트가 닫히고 force하지 않아 **Y가 원격에 그대로** 남는다. `behind_commits_are_
//! patch_equivalent`를 무조건 true로 만드는 mutation은 게이트를 열어 Y를 클로버하고 이 테스트를
//! 죽인다 — load-bearing 안전 가드다.

mod fixture;

use std::path::{Path, PathBuf};
use suaegi_git::remote::{
    behind_commits_are_patch_equivalent, push, push_with_lease_args, push_with_lease_if_safe,
    should_force_push_with_lease, LeasePushOutcome, PushOutcome,
};
use suaegi_git::runner::GitRunner;

/// bare origin + clone A + clone B. A가 init 커밋을 push해두고, B는 그 상태의 clone이다
/// (main이 origin/main을 tracking). `TempDir`는 살려서 반환(drop되면 지워진다).
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

// ── SAFE force: patch-equivalent rebase-then-push(정당한 케이스) ──────────────
//
// B가 커밋 X를 push(원격=X, B의 origin/main tracking=X). B가 X를 amend → X'(같은 트리, 새 해시)
// 이라 원격의 X는 이제 "behind"이고 B의 로컬(X')과 patch-equivalent다. 게이트가 열려
// force-with-lease로 원격을 X'까지 전진시킨다. 이게 에이전트가 자기 작업을 rebase-then-push하는
// 합법 케이스다.
//
// 죽이는 mutation: 게이트 조건을 닫으면(예: patch-equivalent 검사를 항상 false로) 게이트가 안 열려
// NotSafeToForce가 되고 원격이 X'로 전진하지 않아 FAIL.
#[tokio::test]
async fn safe_force_patch_equivalent_advances_remote() {
    let env = setup();
    let (bare, b, r) = (&env.bare, &env.b, &env.r);

    // B가 커밋 X를 push → 원격=X, B의 origin/main tracking=X.
    commit_local(b, "X content\n", "commit-X");
    assert_eq!(
        push(r, b, "main", false).await.expect("X push"),
        PushOutcome::Ok
    );

    // B가 X를 amend → X'(같은 트리 = patch-equivalent, 새 해시). 원격은 여전히 X.
    fixture::run(b, &["commit", "--amend", "-m", "commit-X-prime-same-tree"]);
    let x_prime = rev_parse(r, b, "HEAD").await;
    assert_ne!(
        bare_rev_parse(bare, "main"),
        x_prime,
        "사전조건: 원격은 아직 X(amend 전), B는 X'"
    );

    // 게이트가 열려야 한다: has_upstream && ahead==1(X') && behind==1(X) && patch-equivalent.
    assert!(
        should_force_push_with_lease(r, b)
            .await
            .expect("게이트 판정"),
        "자기 작업의 rebase-rewrite는 force-with-lease 게이트를 열어야 한다"
    );

    // force-with-lease가 실행돼 원격이 X'로 전진한다.
    let outcome = push_with_lease_if_safe(r, b, "main")
        .await
        .expect("force-with-lease push");
    assert_eq!(
        outcome,
        LeasePushOutcome::ForcedWithLease(PushOutcome::Ok),
        "게이트가 열렸으니 force-with-lease가 성공해야 한다"
    );
    assert_eq!(
        bare_rev_parse(bare, "main"),
        x_prime,
        "force-with-lease 후 원격 main이 X'로 전진해야 한다"
    );
}

// ── UNSAFE: 남이 push한 커밋은 절대 클로버되지 않는다(THE 안전 crux) ──────────
//
// B의 upstream이 X에 있다가, **다른 사람**(clone A)이 원격에 진짜 새 커밋 Y를 push한다(Y는
// upstream-only 진짜 기여 — cherry-mark `+`, `=` 아님). B는 자기 로컬 커밋 Z를 갖고 fetch로
// origin/main=Y를 본다. 게이트는 **닫혀야** 하고(Y가 patch-equivalent가 아니므로) force하지 않아
// **원격의 Y가 그대로** 남는다 — A의 작업이 클로버되지 않는다.
//
// 죽이는 mutation(load-bearing): `behind_commits_are_patch_equivalent`를 무조건 true로 만들면
// 게이트가 열려 force-with-lease가 실행되고(B가 fetch해 lease가 만족되므로 성공) 원격 main이
// Z로 덮여 Y가 사라진다 → "원격이 여전히 Y" 단언 FAIL.
#[tokio::test]
async fn unsafe_someone_else_pushed_is_not_clobbered() {
    let env = setup();
    let (bare, a, b, r) = (&env.bare, &env.a, &env.b, &env.r);

    // 다른 사람(A)이 원격에 진짜 새 커밋 Y를 push.
    commit_local(a, "A real work Y\n", "A-real-Y");
    fixture::run(a, &["push", "origin", "main"]);
    let a_y = rev_parse(r, a, "HEAD").await;
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_y,
        "사전조건: 원격 main이 A의 Y를 가리켜야 한다"
    );

    // B는 자기 로컬 커밋 Z를 만들고 fetch로 origin/main=Y를 본다(하지만 Y를 병합하지 않음).
    commit_local(b, "B own work Z\n", "B-own-Z");
    fixture::run(b, &["fetch", "origin"]);

    // 게이트는 닫혀야 한다: behind 커밋 Y가 patch-equivalent가 아니다(남의 진짜 기여).
    assert!(
        !should_force_push_with_lease(r, b)
            .await
            .expect("게이트 판정"),
        "남이 push한 커밋이 behind에 있으면 게이트는 닫혀야 한다"
    );

    // force를 시도하지 않는다 → 원격의 Y는 그대로.
    let outcome = push_with_lease_if_safe(r, b, "main")
        .await
        .expect("게이트 닫힘은 clean 값");
    assert_eq!(
        outcome,
        LeasePushOutcome::NotSafeToForce,
        "게이트가 닫히면 force하지 않고 NotSafeToForce여야 한다"
    );

    // **THE 단언**: A의 Y가 원격에 그대로 남아 있다(클로버되지 않음).
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_y,
        "게이트가 닫혔으니 A의 Y가 원격 main에 그대로 남아야 한다(클로버 금지)"
    );
    assert_ne!(
        bare_rev_parse(bare, "main"),
        rev_parse(r, b, "HEAD").await,
        "원격 main이 B의 Z로 덮이면 안 된다(A의 작업이 사라진다)"
    );
}

// ── Layer 2: lease backstop — 게이트가 stale tracking ref에 열려도 lease가 막는다 ─
//
// **TOCTOU 방어**(보안리뷰 nit 1). 게이트(Layer 1)는 fetch된 남의 커밋만 막는다 — B가
// **re-fetch하지 않으면** origin/main이 낡아(=X) 게이트가 잘못 열릴 수 있다. 이때 유일한
// 방어선이 bare `--force-with-lease`(Layer 2): 원격이 우리 마지막 fetch 이후 움직였으면
// (origin/main=X인데 실제 원격=Y) "stale info"로 **거부**해 남의 Y를 보존한다.
//
// 시나리오: B가 X push→amend X'(same tree). A가 fetch로 X를 받아 그 위에 진짜 커밋 Y를
// push(원격=Y). **B는 re-fetch 안 함** → B의 origin/main은 여전히 X → 게이트 열림
// (ahead=1 X', behind=1 X, patch-equivalent). force-with-lease가 실행되지만 원격=Y≠X라
// lease가 거부 → 원격은 여전히 Y.
//
// 죽이는 mutation: `--force-with-lease`→bare `--force`면 lease 검사가 사라져 원격이 X'로
// 클로버되고 Y가 사라진다 → "원격이 여전히 Y" 단언 FAIL. (argv 문자열 테스트와 달리
// 이건 lease의 **런타임 행위**를 못 박는다.)
#[tokio::test]
async fn lease_backstop_rejects_when_remote_moved_after_stale_fetch() {
    let env = setup();
    let (bare, a, b, r) = (&env.bare, &env.a, &env.b, &env.r);

    // B가 X를 push → 원격=X, B의 origin/main tracking=X.
    commit_local(b, "X content\n", "commit-X");
    assert_eq!(
        push(r, b, "main", false).await.expect("X push"),
        PushOutcome::Ok
    );
    // B가 X를 amend → X'(같은 트리 = patch-equivalent). B의 origin/main은 아직 X.
    fixture::run(b, &["commit", "--amend", "-m", "commit-X-prime-same-tree"]);
    let x_prime = rev_parse(r, b, "HEAD").await;

    // A가 fetch로 X를 받아 그 위에 진짜 커밋 Y를 push → 원격=Y.
    fixture::run(a, &["fetch", "origin"]);
    fixture::run(a, &["reset", "--hard", "origin/main"]);
    commit_local(a, "A real work Y on top of X\n", "A-real-Y");
    fixture::run(a, &["push", "origin", "main"]);
    let a_y = rev_parse(r, a, "HEAD").await;
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_y,
        "사전조건: 원격 main이 A의 Y를 가리켜야 한다"
    );

    // **B는 re-fetch하지 않는다** → B의 origin/main은 낡은 X → 게이트가 (stale하게) 열린다.
    assert!(
        should_force_push_with_lease(r, b)
            .await
            .expect("게이트 판정"),
        "낡은 tracking ref(origin/main=X) 위에서 게이트는 열린다 — Layer 2가 유일한 방어선"
    );

    // force-with-lease가 실행되지만 원격=Y≠origin/main=X라 lease가 거부한다.
    let outcome = push_with_lease_if_safe(r, b, "main")
        .await
        .expect("lease 거부는 clean 값(NonFastForwardRejected)");
    assert_eq!(
        outcome,
        LeasePushOutcome::ForcedWithLease(PushOutcome::NonFastForwardRejected),
        "lease가 stale info로 거부해야 한다(원격이 fetch 이후 움직임)"
    );

    // **THE 단언**: A의 Y가 원격에 그대로 — bare `--force`였다면 X'로 클로버됐을 것이다.
    assert_eq!(
        bare_rev_parse(bare, "main"),
        a_y,
        "lease 거부 후 원격 main이 A의 Y로 남아야 한다(클로버 금지)"
    );
    assert_ne!(
        bare_rev_parse(bare, "main"),
        x_prime,
        "원격이 B의 X'로 덮이면 안 된다(lease가 막았어야 한다)"
    );
}

// ── 프로브 실패 → 보수적 false ───────────────────────────────────────────────
//
// upstream 이름이 해석되지 않으면(없는 브랜치) `git log ...`가 에러 → 프로브는 **false**를
// 돌린다(Orca `catch { return false }`). 절대 불확실성 위에서 force하지 않는다.
//
// 죽이는 mutation: `behind_commits_are_patch_equivalent`의 `Err(_) => false`를 `=> true`로
// 바꾸면 프로브가 true를 돌려 이 단언 FAIL.
#[tokio::test]
async fn probe_failure_is_conservative_false() {
    let env = setup();
    let (b, r) = (&env.b, &env.r);

    // 존재하지 않는 upstream → git log가 fatal로 실패 → 보수적 false.
    let equivalent = behind_commits_are_patch_equivalent(r, b, "origin/does-not-exist").await;
    assert!(
        !equivalent,
        "프로브가 에러나면 보수적으로 false여야 한다(불확실성 위에서 force 금지)"
    );
}

// ── ahead/behind 게이트: behind==0 / ahead==0이면 force 불필요 ────────────────
//
// behind==0(원격에 덮어쓸 게 없음): B가 로컬 커밋만 있고 원격은 미변경 → 게이트 닫힘(force
// 불필요, 일반 push로 충분). ahead==0(push할 게 없음): B가 fetch로 뒤처졌지만 로컬 고유 커밋
// 없음 → 게이트 닫힘. 순수 게이트 조건의 mutation은 `src/remote.rs` unit이 죽인다; 여기선
// 드라이버가 실제 ahead/behind로 게이트를 닫는지 행위로 확인한다.
#[tokio::test]
async fn gate_closed_when_behind_zero() {
    let env = setup();
    let (b, r) = (&env.b, &env.r);

    // B는 로컬 커밋만(ahead=1), 원격 미변경(behind=0).
    commit_local(b, "ahead only\n", "b-ahead");
    assert!(
        !should_force_push_with_lease(r, b)
            .await
            .expect("게이트 판정"),
        "behind==0이면 덮어쓸 게 없어 게이트가 닫혀야 한다(force 불필요)"
    );
    assert_eq!(
        push_with_lease_if_safe(r, b, "main")
            .await
            .expect("게이트 닫힘"),
        LeasePushOutcome::NotSafeToForce
    );
}

#[tokio::test]
async fn gate_closed_when_ahead_zero() {
    let env = setup();
    let (a, b, r) = (&env.a, &env.b, &env.r);

    // A가 Y를 push, B는 fetch로 뒤처지지만(behind=1) 로컬 고유 커밋 없음(ahead=0).
    commit_local(a, "A work\n", "a-work");
    fixture::run(a, &["push", "origin", "main"]);
    fixture::run(b, &["fetch", "origin"]);

    assert!(
        !should_force_push_with_lease(r, b)
            .await
            .expect("게이트 판정"),
        "ahead==0이면 push할 게 없어 게이트가 닫혀야 한다"
    );
}

// ── argv: force-with-lease를 쓰고 bare --force는 절대 없다(F3) ────────────────
//
// 드라이버가 부르는 argv 빌더를 여기서도 못 박는다. Mutation: `--force-with-lease`를 bare
// `--force`로 바꾸면 두 단언이 FAIL(있어야 할 force-with-lease가 사라지고 금지된 --force가 나타남).
#[test]
fn lease_push_argv_never_bare_force() {
    let args = push_with_lease_args("main");
    assert!(
        args.iter().any(|a| a == "--force-with-lease"),
        "force-with-lease를 써야 한다: {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "--force"),
        "bare --force 절대 금지(원격 무조건 클로버): {args:?}"
    );
}
