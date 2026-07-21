mod platform;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use suaegi_term::pty::{KillOutcome, PtyReader, PtySession, PtySpawn};

fn spec(cmd: (String, Vec<String>)) -> PtySpawn {
    PtySpawn {
        program: cmd.0,
        args: cmd.1,
        cwd: None,
        env: Vec::new(),
        rows: 24,
        cols: 80,
    }
}

/// 리더를 별도 스레드에서 EOF까지 읽는다. 타임아웃 시 실패시키되, 남은 스레드가
/// 프로세스를 붙들지 않도록 세션 kill은 호출자가 책임진다.
fn read_to_end_with_timeout(mut reader: PtyReader, timeout: Duration) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = reader.read_to_end(&mut out);
        let _ = tx.send(String::from_utf8_lossy(&out).into_owned());
    });
    rx.recv_timeout(timeout)
        .expect("reader did not reach EOF (slave not dropped, or child still alive)")
}

#[test]
fn spawns_command_and_reads_output() {
    let (session, reader) = PtySession::spawn(spec(platform::echo("hello-suaegi"))).unwrap();
    let out = read_to_end_with_timeout(reader, Duration::from_secs(10));
    assert!(out.contains("hello-suaegi"), "got: {out:?}");
    drop(session);
}

#[test]
fn injects_terminal_env_vars() {
    let (session, reader) = PtySession::spawn(spec(platform::print_env("TERM_PROGRAM"))).unwrap();
    let out = read_to_end_with_timeout(reader, Duration::from_secs(10));
    assert!(out.contains("Suaegi"), "got: {out:?}");
    drop(session);
}

#[test]
fn caller_env_overrides_defaults() {
    let mut s = spec(platform::print_env("TERM"));
    s.env.push(("TERM".into(), "dumb".into()));
    let (session, reader) = PtySession::spawn(s).unwrap();
    let out = read_to_end_with_timeout(reader, Duration::from_secs(10));
    assert!(out.contains("dumb"), "got: {out:?}");
    drop(session);
}

#[test]
fn cwd_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = spec(platform::print_cwd());
    s.cwd = Some(dir.path().to_path_buf());
    let (session, reader) = PtySession::spawn(s).unwrap();
    let out = read_to_end_with_timeout(reader, Duration::from_secs(10));
    let expected: PathBuf = dir.path().canonicalize().unwrap();
    let expected_tail = expected.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        out.contains(&expected_tail),
        "expected {expected_tail:?} in {out:?}"
    );
    drop(session);
}

#[test]
fn write_reaches_the_child() {
    let (session, mut reader) = PtySession::spawn(spec(platform::echo_stdin())).unwrap();
    session.write(b"ping\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = String::new();
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline && !seen.contains("ping") {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
    }
    assert!(seen.contains("ping"), "got: {seen:?}");
    session.kill().unwrap();
}

#[test]
fn wait_reports_exit_code() {
    let (session, reader) = PtySession::spawn(spec(platform::exit_with(3))).unwrap();
    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
    assert_eq!(session.wait().unwrap(), 3);
}

#[test]
fn try_wait_is_none_while_running_then_some() {
    let (session, reader) = PtySession::spawn(spec(platform::exit_with(0))).unwrap();
    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut code = None;
    while Instant::now() < deadline {
        if let Some(c) = session.try_wait().unwrap() {
            code = Some(c);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(code, Some(0));
}

#[test]
fn kill_terminates_a_long_running_child() {
    let (session, reader) = PtySession::spawn(spec(platform::sleep_seconds(60))).unwrap();
    assert_eq!(
        session.kill().unwrap(),
        KillOutcome::Signalled,
        "kill() on a live child that hasn't started reaping must report that it \
         actually sent the signal"
    );
    // 죽었으면 슬레이브가 닫히며 리더가 EOF에 도달한다
    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
}

/// 이미 수확된(reaped) 자식에 대한 두 번째 `kill()` 호출은 아무것도 하지
/// 않지만, 그 사실을 반환값으로 정직하게 알려야 한다 — 첫 kill과 똑같이
/// `Signalled`를 돌려주면 호출자가 시그널이 다시 나갔다고 오해할 수 있다.
#[test]
fn kill_after_natural_exit_reports_suppressed_not_signalled() {
    let (session, reader) = PtySession::spawn(spec(platform::exit_with(0))).unwrap();
    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
    session.wait().unwrap();
    assert_eq!(
        session.kill().unwrap(),
        KillOutcome::SuppressedAfterReap,
        "kill() after the child has already been reaped must not claim it signalled"
    );
}

/// 회귀 테스트: `wait()`가 자식 프로세스를 기다리며 파킹된 동안 다른 스레드가
/// `try_wait()`를 부르고 이어서 `kill()`을 불러도 락 역전으로 인해 어느 쪽도
/// 멈춰서는 안 된다 (`kill()`은 언제나 즉시 반환한다는 모듈 계약).
///
/// 예전 구현에서는 `wait()`가 `child` 락을 쥔 채로(스코프가 없어 함수 끝까지
/// 유지됨) 꼬리에서 `lifecycle`을 다시 잡았고, `try_wait()`는 `lifecycle` →
/// `child` 순서로 잡았다. 두 순서가 어긋나 있어 A가 `wait()`로 `child`를 쥐고
/// `lifecycle`을 기다리는 동안, B의 `try_wait()`가 `lifecycle`을 쥔 채 `child`를
/// 기다리면 서로 영원히 막힌다 — 이어서 부른 `kill()`도 `lifecycle`을 기다리다
/// 함께 멈춘다(3자 데드락). 이 테스트는 그 시나리오를 그대로 재현한다.
///
/// 감시 스레드에서 프로브(=`try_wait` + `kill`)를 돌리고 `is_finished()`를
/// 데드라인까지 폴링한다 — 회귀가 나면 테스트 스위트 전체가 멈추는 대신 이
/// 테스트가 분명한 메시지와 함께 실패한다.
#[test]
fn try_wait_then_kill_do_not_deadlock_while_wait_is_parked() {
    let (session, reader) = PtySession::spawn(spec(platform::sleep_seconds(3))).unwrap();
    let session = Arc::new(session);

    // 스레드 A: 자식이 끝날 때까지(또는 kill될 때까지) wait()에서 블로킹한다.
    let waiter_session = Arc::clone(&session);
    let waiter = std::thread::spawn(move || waiter_session.wait());

    // wait()가 child 락을 잡고 파킹할 시간을 준다.
    std::thread::sleep(Duration::from_millis(300));

    // 스레드 B(감시 대상 프로브): try_wait() 다음 kill()을 호출한다. 역전이
    // 살아있다면 이 스레드는 영원히 리턴하지 않는다.
    let probe_session = Arc::clone(&session);
    let probe = std::thread::spawn(move || {
        let _ = probe_session.try_wait();
        probe_session.kill()
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while !probe.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        probe.is_finished(),
        "try_wait()/kill() did not return within the deadline — \
         likely a lock-ordering deadlock between wait() and try_wait()/kill()"
    );
    // 이 시점에는 스레드 A의 wait()가 이미 reaping을 세워둔 뒤라 kill()이
    // 시그널을 보내지 않는다 — 그런데도 자식은 sleep 3s를 다 채울 때까지
    // 진짜로 살아 있다(아래 waiter 대기가 그걸 증명한다). kill()의 반환값이
    // 이 억제를 정직하게 알려야 한다: 무조건 `Ok(())`였다면 호출자가 "kill이
    // 성공했으니 자식이 곧 죽는다"고 잘못 믿을 수 있었다.
    assert_eq!(
        probe.join().expect("probe thread panicked").unwrap(),
        KillOutcome::SuppressedAfterReap,
        "kill() must report that it suppressed the signal once wait() had already \
         started reaping, not claim an unconditional success"
    );

    // 정리: wait()는 자식이 자연 종료될 때까지(수확이 이미 시작된 뒤의 kill은
    // 시그널을 보내지 않는 설계이므로) 계속 진행 중일 수 있다 — 데드락이 아닌
    // 정상적인 완료를 기다린다.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !waiter.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        waiter.is_finished(),
        "wait() never returned after try_wait()/kill() completed"
    );
    waiter.join().expect("waiter thread panicked").unwrap();

    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
}

#[cfg(unix)]
#[test]
fn resize_is_visible_to_the_child() {
    let mut s = spec(platform::shell_command("sleep 0.3; stty size"));
    s.rows = 24;
    s.cols = 80;
    let (session, reader) = PtySession::spawn(s).unwrap();
    session.resize(30, 100).unwrap();
    let out = read_to_end_with_timeout(reader, Duration::from_secs(10));
    assert!(out.contains("30 100"), "got: {out:?}");
}

#[cfg(unix)]
#[test]
fn foreground_pgid_is_available_while_running() {
    let (session, reader) = PtySession::spawn(spec(platform::sleep_seconds(30))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut pgid = None;
    while Instant::now() < deadline && pgid.is_none() {
        pgid = session.foreground_pgid();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(pgid.is_some_and(|p| p > 0), "expected a foreground pgid");
    session.kill().unwrap();
    let _ = read_to_end_with_timeout(reader, Duration::from_secs(10));
}
