mod platform;

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use suaegi_term::pty::{PtyReader, PtySession, PtySpawn};

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
    session.kill().unwrap();
    // 죽었으면 슬레이브가 닫히며 리더가 EOF에 도달한다
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
