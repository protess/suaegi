mod platform;

use std::time::{Duration, Instant};
use suaegi_term::grid::TitleChange;
use suaegi_term::pty::PtySpawn;
use suaegi_term::session::{SessionSpec, TerminalSession};

fn spec(cmd: (String, Vec<String>)) -> SessionSpec {
    SessionSpec {
        pty: PtySpawn {
            program: cmd.0,
            args: cmd.1,
            cwd: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        },
        scrollback: 500,
    }
}

fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut cond: F) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn snapshot_contains(session: &TerminalSession, needle: &str) -> bool {
    let snap = session.snapshot();
    (0..snap.size.rows).any(|r| snap.row_text(r).contains(needle))
}

#[test]
fn child_output_reaches_the_grid() {
    let session = TerminalSession::start(spec(platform::echo("from-session"))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || snapshot_contains(
        &session,
        "from-session"
    )));
}

#[test]
fn generation_increases_when_output_arrives() {
    let session = TerminalSession::start(spec(platform::echo("gen"))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session.generation() > 0));
}

#[test]
fn exit_code_is_reported_after_child_finishes() {
    let session = TerminalSession::start(spec(platform::exit_with(7))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session.exit_code()
        == Some(7)));
    assert!(!session.is_running());
}

#[test]
fn write_is_echoed_into_the_grid() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    assert!(session.write(b"typed-text\n".to_vec()), "queued write");
    assert!(wait_until(Duration::from_secs(10), || snapshot_contains(
        &session,
        "typed-text"
    )));
}

#[test]
fn resize_updates_both_pty_and_grid() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    session.resize(30, 100).unwrap();
    let snap = session.snapshot();
    assert_eq!(snap.size.rows, 30);
    assert_eq!(snap.size.cols, 100);
}

#[test]
fn zero_size_resize_is_ignored() {
    let session = TerminalSession::start(spec(platform::echo_stdin())).unwrap();
    session.resize(0, 0).unwrap();
    assert_eq!(
        session.snapshot().size.rows,
        24,
        "degenerate size must not reach the grid"
    );
}

#[test]
fn title_escape_is_surfaced() {
    let script = "printf '\\033]0;session-title\\007'";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();
    let mut seen = Vec::new();
    assert!(
        wait_until(Duration::from_secs(10), || {
            seen.extend(session.take_title_changes());
            seen.contains(&TitleChange::Set("session-title".to_string()))
        }),
        "title change was not surfaced"
    );
}

#[test]
fn dropping_the_session_does_not_block() {
    // Drop이 자식을 죽이지 않으면 이 프로세스는 60초간 살아남는다
    let session = TerminalSession::start(spec(platform::sleep_seconds(60))).unwrap();
    let start = Instant::now();
    drop(session);
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "drop must not block on a long-running child"
    );
}

/// unix에서는 그룹 SIGKILL로 자손까지 확실히 죽는다는 보장이 있다.
/// (Windows는 자손 종료 수단이 없어 이 보장을 하지 않는다 — Global Constraints 참고)
#[cfg(unix)]
#[test]
fn dropping_the_session_kills_the_process_group() {
    let session = TerminalSession::start(spec(platform::sleep_seconds(60))).unwrap();
    assert!(wait_until(Duration::from_secs(10), || session
        .foreground_pgid()
        .is_some()));
    let pgid = session.foreground_pgid().unwrap();
    drop(session);
    assert!(
        wait_until(Duration::from_secs(5), || {
            // kill(pgid, 0)이 실패하면(ESRCH) 그룹이 사라진 것
            (unsafe { libc::killpg(pgid as libc::pid_t, 0) }) != 0
        }),
        "process group survived session drop"
    );
}

/// 장치 질의(DA1) 응답이 PTY로 되돌아가야 한다. 응답에는 개행이 없으므로
/// **먼저 라인 디시플린을 비정규 모드로 바꿔야** 한다 — 그러지 않으면 `dd`든
/// `read`든 개행이 올 때까지 커널에서 블로킹된다.
#[cfg(unix)]
#[test]
fn device_query_is_answered_back_to_the_pty() {
    let script = "stty -icanon min 1 time 0 -echo; printf '\\033[c'; \
                  dd bs=1 count=3 >/dev/null 2>&1; printf 'ANSWERED'";
    let session = TerminalSession::start(spec(platform::shell_command(script))).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || snapshot_contains(
            &session,
            "ANSWERED"
        )),
        "device query reply never reached the child"
    );
}
