//! remote M1의 real-git 테스트: 전역 env-guard(F1/F2)가 실제 git 프로세스에 도달하는지.
//! 순수 헬퍼(정제/분류/argv)는 `src/remote.rs`의 unit 테스트가 mutation 검증한다.

use std::time::Duration;
use suaegi_git::runner::GitRunner;

/// 전역 gitconfig에 오염되지 않은 최소 repo. system/global config를 끊어 env-config가
/// **유일한** credential.* 출처가 되게 한다.
fn clean_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    };
    git(&["init", "-b", "main"]);
    dir
}

/// **F2 real-git 증명**: `git config credential.interactive`가 `false`를 돌려준다 —
/// 이 값은 오직 `run_full`이 주입한 GIT_CONFIG env 프로토콜에서만 온다(repo/global엔
/// 없음). GIT_CONFIG_COUNT나 KEY/VALUE 세팅을 지우는 mutation은 git이 config를 못 봐서
/// `git config --get`이 exit 1 → `GitError::Failed` → 이 테스트가 FAIL.
#[tokio::test]
async fn credential_prompt_guard_reaches_git() {
    let dir = clean_repo();
    let r = GitRunner::new();
    let out = r
        .run(dir.path(), &["config", "--get", "credential.interactive"])
        .await
        .expect("credential.interactive는 env-config로 세팅돼 있어야 한다");
    assert_eq!(
        out.stdout.trim(),
        "false",
        "env GIT_CONFIG 프로토콜이 git에 도달하지 않았다"
    );

    // guiPrompt도 동일 경로로 도달하는지 확인.
    let gui = r
        .run(dir.path(), &["config", "--get", "credential.guiPrompt"])
        .await
        .expect("credential.guiPrompt도 세팅돼 있어야 한다");
    assert_eq!(gui.stdout.trim(), "false");
}

/// **F1**: credential이 필요한(존재하지 않는) 원격에 대한 push가 **행 걸리지 않고**
/// 빠르게 실패한다. 비대화형 가드가 없으면 credential 프롬프트로 매달릴 수 있다.
/// 로컬 file:// 원격이라 네트워크에 의존하지 않는다.
#[tokio::test]
async fn push_to_missing_remote_fails_fast_not_hangs() {
    let dir = clean_repo();
    let r = GitRunner::new();

    // 커밋 하나 만든다(push할 게 있어야 함).
    std::fs::write(dir.path().join("f.txt"), "hi\n").unwrap();
    for args in [
        &["config", "user.email", "t@t"][..],
        &["config", "user.name", "t"][..],
        &["add", "-A"][..],
        &["commit", "-m", "init"][..],
    ] {
        r.run(dir.path(), args).await.expect("setup git failed");
    }

    // 존재하지 않는 bare 원격으로 push → fatal, 빠르게. 프롬프트로 매달리면 타임아웃.
    let missing = dir.path().join("no-such-remote.git");
    let url = format!("file://{}", missing.display());
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        r.run(dir.path(), &["push", &url, "HEAD:refs/heads/main"]),
    )
    .await
    .expect("push가 프롬프트로 매달렸다(타임아웃)");
    assert!(
        result.is_err(),
        "존재하지 않는 원격 push는 실패해야 한다: {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "push가 즉시 실패하지 않고 지연됐다: {:?}",
        started.elapsed()
    );
}
