use std::time::Duration;
use suaegi_git::runner::{GitError, GitRunner};

#[tokio::test]
async fn run_version_succeeds() {
    let r = GitRunner::new();
    let out = r
        .run(std::env::temp_dir().as_path(), &["--version"])
        .await
        .unwrap();
    assert!(out.stdout.starts_with("git version"));
}

#[tokio::test]
async fn failed_command_returns_structured_error() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let err = r.run(dir.path(), &["worktree", "list"]).await.unwrap_err();
    match err {
        GitError::Failed { code, stderr, .. } => {
            assert_ne!(code, Some(0));
            assert!(stderr.to_lowercase().contains("not a git repository"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn run_expecting_accepts_listed_codes() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    // --no-index는 차이가 있으면 exit 1 — extra_ok로 수용
    let out = r
        .run_expecting(
            dir.path(),
            &[
                "diff",
                "--no-index",
                "--",
                a.to_str().unwrap(),
                b.to_str().unwrap(),
            ],
            &[1],
        )
        .await
        .unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("-1"));
    assert!(out.stdout.contains("+2"));
}

// `sleep`은 POSIX 전용이므로 Unix에서만 실행. Windows CI에는 별도 타임아웃
// 테스트를 추가할 때까지 이 케이스를 건너뛴다.
#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_process_group_including_descendants() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    // 셸 alias가 자식(sh)과 손자(sleep)를 만든다. 타임아웃 후 손자까지 죽어야 한다.
    let marker = format!("suaegi-test-{}", std::process::id());
    let alias = format!("alias.zzz=!sleep 300 & echo $! > {marker}.pid; wait");
    let err = r
        .run_with_timeout(
            dir.path(),
            &["-c", &alias, "zzz"],
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GitError::Timeout { .. }));
    // 프로세스 그룹 킬이 전파될 시간을 잠깐 주고 손자 생존 여부 확인
    tokio::time::sleep(Duration::from_millis(200)).await;
    if let Ok(pid_text) = std::fs::read_to_string(dir.path().join(format!("{marker}.pid"))) {
        let pid: i32 = pid_text.trim().parse().unwrap();
        // kill(pid, 0) == -1 (ESRCH) 이어야 함 = 이미 죽음
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "descendant sleep survived the timeout kill");
    }
}
