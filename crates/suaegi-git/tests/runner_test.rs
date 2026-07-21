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

/// 상한 초과의 두 가지를 함께 본다:
///
/// 1. **`OutputTooLarge`가 나오는가** — `Timeout`이 나오면 파이프 교착이다. 리더가
///    한쪽을 그만 읽었는데 자식이 그쪽에 계속 쓰면 자식이 블록되고, `child.wait()`가
///    영영 끝나지 않아 타임아웃까지 간다. 그래서 자식이 **stdout과 stderr 양쪽에
///    동시에** 대량으로 쓴다.
/// 2. **프로세스가 남지 않는가** — `kill_on_drop(true)`은 종료를 *요청*할 뿐 수확이
///    아니다. 자식은 `wait`로 자기 손자를 기다리므로 스스로 끝나지 않는다:
///    우리가 그룹을 죽이지 않으면 `sleep`이 살아남는다.
#[cfg(unix)]
#[tokio::test]
async fn output_over_the_cap_aborts_and_leaves_no_process_behind() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let marker = format!("suaegi-cap-{}", std::process::id());
    // 5MB + 5MB = 10MB > MAX_DIFF_BYTES(6MB). 양쪽을 동시에 쓴다.
    let alias = format!(
        "alias.zzz=!sleep 300 & echo $! > {marker}.pid; \
         (yes aaaaaaaa | head -c 5000000) & \
         (yes bbbbbbbb | head -c 5000000 >&2) & wait"
    );
    // 교착이면 기본 30초를 다 쓰고 Timeout이 난다. 20초로 줄여 실패를 빨리 본다.
    let started = std::time::Instant::now();
    let err = r
        .run_with_timeout(dir.path(), &["-c", &alias, "zzz"], Duration::from_secs(20))
        .await
        .unwrap_err();
    let elapsed = started.elapsed();
    match err {
        GitError::OutputTooLarge { limit } => {
            assert_eq!(limit, suaegi_git::runner::MAX_DIFF_BYTES);
        }
        other => panic!("expected OutputTooLarge, got {other:?}"),
    }

    // **손자 생존 확인만으로는 부족하다.** 킬을 지워도 `sleep 300`은 300초 뒤에
    // 스스로 죽고, 그러면 아래 단언이 통과해버린다 — 죽인 것과 기다린 것을
    // 구별하지 못한다. 중단은 **즉시**여야 한다.
    assert!(
        elapsed < Duration::from_secs(10),
        "abort waited for the child instead of killing it: {elapsed:?}"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let pid_text = std::fs::read_to_string(dir.path().join(format!("{marker}.pid")))
        .expect("alias never ran — the test proved nothing");
    let pid: i32 = pid_text.trim().parse().unwrap();
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    assert!(!alive, "descendant sleep survived the OutputTooLarge abort");
}

/// 대조군: 상한 아래의 출력은 온전히 돌아온다. 위 테스트가 "무조건 거절"을
/// 잡아내지 못하는 것을 막는다.
#[cfg(unix)]
#[tokio::test]
async fn output_under_the_cap_is_returned_whole() {
    let r = GitRunner::new();
    let dir = tempfile::tempdir().unwrap();
    let alias = "alias.zzz=!yes aaaaaaaa | head -c 100000";
    let out = r
        .run_with_timeout(dir.path(), &["-c", alias, "zzz"], Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(out.stdout.len(), 100_000);
}
